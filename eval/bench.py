#!/usr/bin/env python3
"""Measures the server's speed and memory (doc/m1-plan.md, E2; requirements §9.1 and R-MEM-1).

Linux only (memory is read from /proc). It uses the release build and the cache that
CHATORA_RUSTDOC_MCP_CACHE__DIR names, which must have the JSON of testdata/docs-rs-json.txt.

  1. normalize: removes the archives of some crates and of std, then times the first call to
     each in a new process (parsing, indexing and archiving; the JSON itself is cached)
  2. first call: a new process, the archives cached, the first call to each crate
  3. warm: one process, ROUNDS rounds of get_item and list_methods on representative items

Usage: python3 -I eval/bench.py [--rounds 20]
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "release" / ("chatora-rustdoc-mcp" + (".exe" if os.name == "nt" else ""))
META = {"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {}}
# The archives are `*.v<ARCHIVE_SCHEMA of src/query/across.rs>t<rustdoc_types::FORMAT_VERSION>.rkyv`,
# as `*.v8t61.rkyv`: those of any version are removed, so that this does not go stale.
SCHEMA = "v*t*"

# Crates whose archives are made again: (release, an item to ask for). Sizes of their JSON vary.
NORMALIZED = [
    ("serde_json@1.0.154", "Value"),
    ("axum@0.8.9", "Router"),
    ("tokio@1.53.3", "sync::Mutex"),
    ("syn@2.0.104", "ItemFn"),
    ("rmcp@3.5.1", "ServerHandler"),
]
# std::vec::Vec loads std and alloc, and core::option::Option core.
STD_ITEMS = [("core", "option::Option"), ("std", "vec::Vec")]

GET_ITEM = [
    ("tokio@1.53.3", "sync::Mutex"),
    ("std", "vec::Vec"),
    ("axum@0.8.9", "Router"),
    ("serde@1.0.228", "Serialize"),
    ("serde_json@1.0.154", "Value"),
    ("core", "option::Option"),
    ("std", "collections::HashMap"),
    ("tokio@1.53.3", "sync::Mutex::lock"),
]
LIST_METHODS = [
    ("tokio@1.53.3", "sync::Mutex"),
    ("std", "vec::Vec"),
    ("std", "string::String"),
    ("axum@0.8.9", "Router"),
]


class Server:
    """A server process asked one request at a time, with its anonymous memory sampled.

    It runs in `cwd`, `logs` unless given, which is its default project when it is one, with the
    environment variables `env` too (eval/build_bench.py).
    """

    def __init__(self, logs: Path, cwd: Path | None = None, env: dict | None = None):
        env = dict(os.environ, CHATORA_RUSTDOC_MCP_LOG__DIR=str(logs), **(env or {}))
        env.pop("CLAUDE_PROJECT_DIR", None)
        self.process = subprocess.Popen(
            [str(BINARY), "serve"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            env=env,
            cwd=cwd or logs,
        )
        self.next_id = 1
        self.peak_anon_kb = 0
        self.sampling = True
        self.sampler = threading.Thread(target=self._sample, daemon=True)
        self.sampler.start()

    def _status(self) -> dict:
        fields = {}
        try:
            for line in Path(f"/proc/{self.process.pid}/status").read_text().splitlines():
                key, _, value = line.partition(":")
                if value.strip().endswith("kB"):
                    fields[key] = int(value.split()[0])
        except (FileNotFoundError, ProcessLookupError):
            pass
        return fields

    def _sample(self):
        while self.sampling:
            self.peak_anon_kb = max(self.peak_anon_kb, self._status().get("RssAnon", 0))
            time.sleep(0.002)

    def memory(self) -> dict:
        status = self._status()
        return {"anon_mb": status.get("RssAnon", 0) // 1024, "hwm_mb": status.get("VmHWM", 0) // 1024}

    def call(self, tool: str, arguments: dict) -> tuple[float, dict]:
        request = {
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": arguments, "_meta": META},
        }
        self.next_id += 1
        started = time.perf_counter()
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        response = json.loads(self.process.stdout.readline())
        seconds = time.perf_counter() - started
        result = response.get("result", {})
        if result.get("isError"):
            raise RuntimeError(f"{tool} {arguments}: {result['content'][0]['text'][:300]}")
        return seconds, result.get("structuredContent", {})

    def close(self):
        self.process.stdin.close()
        self.process.wait()
        self.sampling = False


def cache_dir() -> Path:
    value = os.environ.get("CHATORA_RUSTDOC_MCP_CACHE__DIR")
    if not value:
        sys.exit("set CHATORA_RUSTDOC_MCP_CACHE__DIR to the cache with the test data")
    return Path(value)


def cache_key(name: str) -> str:
    """The directory of a crate in the cache, as `CrateName::cache_key` names it."""
    return name.lower().replace("_", "-")


def remove_archives(cache: Path, std_dir: Path | None) -> None:
    for release, _ in NORMALIZED:
        name, version = release.split("@")
        for archive in (cache / "docs.rs" / cache_key(name) / version).glob(f"*/*.{SCHEMA}.rkyv"):
            archive.unlink()
    if std_dir:
        for archive in std_dir.glob(f"*.{SCHEMA}.rkyv"):
            archive.unlink()


def std_dir_used(cache: Path) -> Path | None:
    """The directory of the std that the server uses: the one with the newest archive."""
    archives = sorted(cache.glob(f"std/*/*/std.*.{SCHEMA}.rkyv"), key=lambda path: path.stat().st_mtime)
    return archives[-1].parent if archives else None


def ms(seconds: float) -> float:
    return round(seconds * 1000, 1)


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, round(fraction * (len(ordered) - 1)))]


def normalize(cache: Path, logs: Path) -> list[str]:
    # Make sure that everything is fetched and archived once, so that only the archives go.
    server = Server(logs)
    for release, path in [*NORMALIZED, *STD_ITEMS]:
        server.call("get_item", {"crate": release, "path": path})
    server.close()
    remove_archives(cache, std_dir_used(cache))
    lines = ["| crate | JSON MB | first call ms | anon MB after | anon peak MB |", "| --- | --- | --- | --- | --- |"]
    for release, path in [*NORMALIZED, *STD_ITEMS]:
        server = Server(logs)
        seconds, result = server.call("get_item", {"crate": release, "path": path})
        memory = server.memory()
        peak = server.peak_anon_kb // 1024
        server.close()
        lines.append(f"| {release} | {json_mb(cache, result)} | {ms(seconds)} | {memory['anon_mb']} | {peak} |")
    return lines


def json_mb(cache: Path, result: dict) -> str:
    provenance = result.get("provenance", {})
    if provenance.get("source") == "std":
        # The crates of std that the call loaded: core for core, alloc and std for std.
        # The toolchain is "rustc 1.101.0-nightly (07e526db9 2026-10-19)", and the commit names the
        # directory.
        commit = provenance.get("toolchain", "").partition("(")[2].split(" ")[0]
        directory = next(iter(cache.glob(f"std/{commit}/*")), None) if commit else None
        names = ["core"] if provenance.get("crate") == "core" else ["alloc", "std"]
        files = [directory / f"{name}.json" for name in names] if directory else []
        total = sum(path.stat().st_size for path in files if path.is_file())
        return f"{total / 1e6:.0f} ({'+'.join(names)})"
    name, version, target = provenance.get("crate"), provenance.get("version"), provenance.get("target")
    files = [
        path
        for path in (cache / "docs.rs" / cache_key(name) / version / target).glob("*.json")
        if path.stem.isdigit()
    ]
    return f"{sum(path.stat().st_size for path in files) / 1e6:.1f}" if files else "?"


def first_calls(logs: Path) -> list[str]:
    server = Server(logs)
    lines = ["| crate | first call ms (archive cached) |", "| --- | --- |"]
    for release, path in [*NORMALIZED, *STD_ITEMS]:
        seconds, _ = server.call("get_item", {"crate": release, "path": path})
        lines.append(f"| {release} | {ms(seconds)} |")
    memory = server.memory()
    server.close()
    lines.append(f"\nanon after all: {memory['anon_mb']} MB, peak anon {server.peak_anon_kb // 1024} MB")
    return lines


def warm(logs: Path, rounds: int) -> list[str]:
    server = Server(logs)
    calls = [("get_item", release, path) for release, path in GET_ITEM] + [
        ("list_methods", release, path) for release, path in LIST_METHODS
    ]
    first = ["| first call | ms | anon MB before → after (peak) |", "| --- | --- | --- |"]
    for tool, release, path in calls:
        before = server.memory()["anon_mb"]
        server.peak_anon_kb = 0
        seconds, _ = server.call(tool, {"crate": release, "path": path})
        first.append(
            f"| {tool} {release} {path} | {ms(seconds)} | {before} → {server.memory()['anon_mb']} "
            f"({server.peak_anon_kb // 1024}) |"
        )
    times: dict[str, list[float]] = {"get_item": [], "list_methods": []}
    sizes = {}
    for _ in range(rounds):
        for tool, release, path in calls:
            seconds, result = server.call(tool, {"crate": release, "path": path})
            times[tool].append(seconds)
            sizes[f"{tool} {release} {path}"] = len(json.dumps(result, ensure_ascii=False, separators=(",", ":")))
    memory = server.memory()
    server.close()
    lines = [*first, "", "| tool | calls | p50 ms | p95 ms | max ms |", "| --- | --- | --- | --- | --- |"]
    for tool, values in times.items():
        lines.append(
            f"| {tool} | {len(values)} | {ms(statistics.median(values))} | {ms(percentile(values, 0.95))} "
            f"| {ms(max(values))} |"
        )
    lines.append(f"\nanon after: {memory['anon_mb']} MB, peak anon {server.peak_anon_kb // 1024} MB\n")
    lines += ["| call (detail normal) | characters |", "| --- | --- |"]
    lines += [f"| {call} | {size} |" for call, size in sizes.items()]
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rounds", type=int, default=20)
    args = parser.parse_args()
    cache = cache_dir()
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory() as logs:
        logs = Path(logs)
        report = ["## normalize (archives removed, JSON cached)", *normalize(cache, logs)]
        report += ["", "## first call in a new process (archives cached)", *first_calls(logs)]
        report += ["", f"## warm ({args.rounds} rounds)", *warm(logs, args.rounds)]
    print("\n".join(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
