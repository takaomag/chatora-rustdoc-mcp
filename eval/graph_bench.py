#!/usr/bin/env python3
"""Measures list_implementors across the dependency graph (doc/m4-plan.md, decision 8).

Linux only (memory is read from /proc). It uses the release build and a cache of its own under
target/graph-bench, which it fills from docs.rs (decision 9, (b)) and removes at the end unless
--keep is given. For each case, a trait in a project:

  C  cold: an empty cache, calls again while crates are still to come, until none is (reference)
  W  warm: the same process, ROUNDS calls after that
  F  first call of a new process, the archives on disk, ROUNDS processes
  R  the archives removed (the JSON kept), a new process, calls again until done (reference)

Usage: python3 -I eval/graph_bench.py [--rounds 20] [--keep]
"""

import argparse
import json
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import ROOT, Server, ms, percentile  # noqa: E402

CACHE = ROOT / "target" / "graph-bench" / "cache"
# (label, project, crate, path), decided in M4a, decision 8.
CASES = [
    ("fixture tower::Service", ROOT / "eval" / "fixture", "tower", "Service"),
    ("repo serde::Serialize", ROOT, "serde", "Serialize"),
    ("repo futures_core::Stream", ROOT, "futures-core", "Stream"),
]
# The most time to wait for a cold case, in seconds.
COLD_LIMIT = 900


def arguments(project: Path, krate: str, path: str) -> dict:
    return {"crate": krate, "path": path, "scope": "dependency_graph", "project": str(project)}


def summary(result: dict) -> str:
    provenance = result.get("provenance", {})
    sources = provenance.get("sources", [])
    reasons: dict[str, int] = {}
    for source in sources:
        key = source.get("reason") or source.get("stale_reason") or source.get("status") or "?"
        reasons[key] = reasons.get(key, 0) + 1
    told = ", ".join(f"{key} {count}" for key, count in sorted(reasons.items())) or "none"
    return (
        f"crates_read {provenance.get('crates_read')}, implementors {result.get('page', {}).get('total')}, "
        f"sources: {told}, partial {result.get('partial', False)}"
    )


def until_done(server: Server, args: dict) -> tuple[float, float, int, dict]:
    """Calls until no crate is to come: the first call's time, the whole time, the calls."""
    started = time.perf_counter()
    first, calls, result = None, 0, {}
    while True:
        seconds, result = server.call("list_implementors", args)
        calls += 1
        first = seconds if first is None else first
        retry = result.get("retry_after_secs")
        if result.get("status") != "building" and retry is None:
            break
        if time.perf_counter() - started > COLD_LIMIT:
            break
        time.sleep(retry or 1)
    return first, time.perf_counter() - started, calls, result


def remove_archives() -> int:
    removed = 0
    for archive in CACHE.rglob("*.rkyv"):
        archive.unlink()
        removed += 1
    return removed


def measure(logs: Path, rounds: int, label: str, project: Path, krate: str, path: str) -> list[str]:
    env = {"CHATORA_RUSTDOC_MCP_CACHE__DIR": str(CACHE)}
    args = arguments(project, krate, path)
    lines = [f"### {label}", ""]
    # C, then W in the same process.
    server = Server(logs, cwd=project, env=env)
    first, whole, calls, result = until_done(server, args)
    lines.append(f"- C: first call {ms(first)} ms, done after {ms(whole)} ms and {calls} calls; {summary(result)}")
    warm = [server.call("list_implementors", args)[0] for _ in range(rounds)]
    memory = server.memory()
    server.close()
    lines.append(
        f"- W: {rounds} calls, p50 {ms(statistics.median(warm))} ms, p95 {ms(percentile(warm, 0.95))} ms, "
        f"max {ms(max(warm))} ms; anon after {memory['anon_mb']} MB, peak {server.peak_anon_kb // 1024} MB"
    )
    # F: a new process each time.
    fresh, peaks = [], []
    for _ in range(rounds):
        server = Server(logs, cwd=project, env=env)
        seconds, result = server.call("list_implementors", args)
        server.close()
        fresh.append(seconds)
        peaks.append(server.peak_anon_kb // 1024)
    lines.append(
        f"- F: {rounds} processes, p50 {ms(statistics.median(fresh))} ms, p95 {ms(percentile(fresh, 0.95))} ms, "
        f"max {ms(max(fresh))} ms; peak anon p50 {statistics.median(peaks)} MB; {summary(result)}"
    )
    # R: the archives made again from the JSON.
    removed = remove_archives()
    server = Server(logs, cwd=project, env=env)
    first, whole, calls, result = until_done(server, args)
    server.close()
    lines.append(
        f"- R ({removed} archives removed): first call {ms(first)} ms, done after {ms(whole)} ms and {calls} "
        f"calls, peak anon {server.peak_anon_kb // 1024} MB; {summary(result)}"
    )
    return [*lines, ""]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--rounds", type=int, default=20)
    parser.add_argument("--keep", action="store_true", help="keep the cache of target/graph-bench")
    args = parser.parse_args()
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    if CACHE.exists():
        shutil.rmtree(CACHE)
    CACHE.mkdir(parents=True)
    report = [f"## list_implementors across the dependency graph ({args.rounds} rounds)", ""]
    try:
        with tempfile.TemporaryDirectory() as logs:
            for label, project, krate, path in CASES:
                report += measure(Path(logs), args.rounds, label, project, krate, path)
                print("\n".join(report[-6:]), file=sys.stderr, flush=True)
    finally:
        if not args.keep:
            shutil.rmtree(CACHE.parent, ignore_errors=True)
    print("\n".join(report))
    return 0


if __name__ == "__main__":
    sys.exit(main())
