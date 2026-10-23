#!/usr/bin/env python3
"""Measures the local builds of a workspace's member (doc/m2-plan.md, M2f's F5).

It uses the release build of the server, copies of the workspaces under target/f5 (never /tmp,
which is small), an empty cache for each cold build, and the user's ~/.cargo offline (decided in
F5, 3), so that it needs no network; with --online, as in CI (.github/workflows/measure.yml), the
network. It runs on Linux, macOS and Windows.

  prepare   copies this repository (a clone of HEAD, without the user's git config), eval/fixture
            and tests/fixtures/local-build into target/f5/src
  run       for each workspace and each -j: a cold build of the member, timed by the one call that
            waits for it, with the size of what it made; then rebuilds after an edit of the
            member's lib, each once the dep-info of the build before is recorded, with how many
            units cargo built again (the fingerprints newer than the edit)
  deps      the dependencies of the eval fixture that the server builds locally, as on macOS and
            Windows, where docs.rs has no JSON for their target: the time of each first lookup
  diagnostics
            breaks a copy of a workspace in a few ways (F5b), and for each tells the errors of the
            compiler as cargo writes them (how many, how long each is as JSON escapes it) and what
            the server answers (build_failed, its diagnostics and how long its text is)

Each run is a line of JSON in target/f5/results.jsonl, or target/f5/diagnostics.jsonl.

Usage: python3 -I eval/build_bench.py prepare
       python3 -I eval/build_bench.py run --workspaces self,eval --jobs 1,6 --repeat 2 --rebuilds 5
       python3 -I eval/build_bench.py deps --online
       python3 -I eval/build_bench.py diagnostics
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import BINARY, ROOT, Server  # noqa: E402

F5 = ROOT / "target" / "f5"
SOURCES = F5 / "src"
RESULTS = F5 / "results.jsonl"

# The workspaces: their root, the member to build, and its lib, which an edit changes.
WORKSPACES = {
    "self": (SOURCES / "self", "chatora_rustdoc_mcp", Path("src/lib.rs")),
    "eval": (SOURCES / "eval", "eval_fixture", Path("src/lib.rs")),
    "local-build": (SOURCES / "local-build" / "ws", "one", Path("one/src/lib.rs")),
}

# Without the user's git config, nor the system's.
GIT_ENV = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")


def git(args: list[str], cwd: Path) -> None:
    subprocess.run(["git", *args], cwd=cwd, env=GIT_ENV, check=True, stdout=subprocess.DEVNULL)


def prepare() -> None:
    if SOURCES.exists():
        shutil.rmtree(SOURCES)
    SOURCES.mkdir(parents=True)
    git(["clone", "--quiet", "--no-hardlinks", str(ROOT), str(SOURCES / "self")], ROOT)
    shutil.copytree(ROOT / "eval" / "fixture", SOURCES / "eval")
    shutil.copytree(ROOT / "tests" / "fixtures" / "local-build", SOURCES / "local-build")
    subprocess.run(
        ["cargo", "generate-lockfile", "--offline"],
        cwd=SOURCES / "local-build" / "ws",
        check=True,
    )
    # A repository of its own each, as a user's workspace is.
    for copy in [SOURCES / "eval", SOURCES / "local-build"]:
        git(["init", "--quiet"], copy)
        git(["add", "--all"], copy)
    print(f"prepared {SOURCES}")


def size(path: Path) -> int:
    """The bytes of the files under `path`, not following links, a file that has hard links once,
    as `du -sb` counts them: cargo links what it builds into more than one place."""
    total = 0
    seen = set()
    for directory, _, files in os.walk(path):
        for name in files:
            try:
                status = os.lstat(os.path.join(directory, name))
            except OSError:
                continue
            if status.st_nlink > 1:
                if (status.st_dev, status.st_ino) in seen:
                    continue
                seen.add((status.st_dev, status.st_ino))
            total += status.st_size
    return total


def records(cache: Path) -> list[dict]:
    """The records of the local builds in `cache`."""
    found = []
    for path in (cache / "local" / "v1").glob("*/*/*/*/record.json"):
        try:
            found.append(json.loads(path.read_text()))
        except (OSError, json.JSONDecodeError):
            pass
    return found


def taken_at(record: dict) -> int:
    return (record.get("inputs") or {}).get("taken_at", 0)


def wait_for_dep_info(cache: Path, after: int, limit: float = 300) -> float:
    """Waits until a build whose inputs were taken after `after` has its dep-info recorded, as
    the server records it after it answers; returns how long it waited."""
    started = time.perf_counter()
    while time.perf_counter() - started < limit:
        if any(r.get("dep_info") and taken_at(r) > after for r in records(cache)):
            return time.perf_counter() - started
        time.sleep(0.5)
    raise TimeoutError(f"no dep-info recorded in {limit} s")


def rebuilt_units(cache: Path, since: float) -> int:
    """How many units cargo built since `since`: their fingerprint directories with a file
    written since, as cargo writes its files there again, which leaves the directory's time."""
    return sum(
        1
        for fingerprint in (cache / "t").glob("*/**/.fingerprint/*")
        if fingerprint.is_dir()
        and any(file.stat().st_mtime > since for file in fingerprint.iterdir() if file.is_file())
    )


def load() -> float | None:
    """The load of the last minute, where the OS tells it (not Windows)."""
    return os.getloadavg()[0] if hasattr(os, "getloadavg") else None


def answered(server: Server, member: str) -> tuple[float, dict]:
    """The time of one call that waits for the member's build, and its result."""
    seconds, result = server.call("crate_overview", {"crate": member})
    if result.get("status") == "building":
        raise RuntimeError(f"still building after {seconds:.1f} s: {result}")
    return seconds, result


def record(line: dict) -> None:
    with RESULTS.open("a") as results:
        results.write(json.dumps(line) + "\n")
    shown = {k: v for k, v in line.items() if k not in ("toolchain",)}
    print(json.dumps(shown), flush=True)


def settings(place: Path, cache: Path, online: bool, jobs: int | None = None) -> dict:
    """The environment of a server whose cache is `cache`, with settings that are not the user's,
    in which one call waits for the whole build, whose time is that call's."""
    config = place / "config.toml"
    config.write_text("")
    env = {
        "CHATORA_RUSTDOC_MCP_CONFIG": str(config),
        "CHATORA_RUSTDOC_MCP_CACHE__DIR": str(cache),
        "CHATORA_RUSTDOC_MCP_BUILD__WAIT_SECS": "600",
        "CHATORA_RUSTDOC_MCP_LIMITS__REQUEST_TIMEOUT_SECS": "900",
        "CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE": "false" if online else "true",
    }
    if jobs is not None:
        env["CHATORA_RUSTDOC_MCP_BUILD__JOBS"] = str(jobs)
    return env


def run_one(name: str, jobs: int, repeat: int, rebuilds: int, online: bool) -> None:
    root, member, lib = WORKSPACES[name]
    place = F5 / "runs" / f"{name}-j{jobs}-{repeat}"
    if place.exists():
        shutil.rmtree(place)
    cache, logs = place / "cache", place / "logs"
    cache.mkdir(parents=True)
    logs.mkdir()
    env = settings(place, cache, online, jobs)
    source = root / lib
    original = source.read_text()
    server = Server(logs, cwd=root, env=env)
    try:
        before = load()
        seconds, result = answered(server, member)
        provenance = result.get("provenance", {})
        record({
            "workspace": name,
            "member": member,
            "jobs": jobs,
            "repeat": repeat,
            "kind": "cold",
            "seconds": round(seconds, 2),
            "target_dir_mb": round(size(cache / "t") / 1e6, 1),
            "local_mb": round(size(cache / "local") / 1e6, 1),
            "load_before": before,
            "toolchain": provenance.get("toolchain"),
        })
        last = max((taken_at(r) for r in records(cache)), default=0)
        for edit in range(rebuilds):
            waited = wait_for_dep_info(cache, last - 1)
            last = max(taken_at(r) for r in records(cache))
            since = time.time()
            with source.open("a") as lib_file:
                lib_file.write(f"\n/// Added by F5.\npub fn f5_probe_{edit}() {{}}\n")
            seconds, result = answered(server, member)
            stale = result.get("provenance", {}).get("stale")
            record({
                "workspace": name,
                "member": member,
                "jobs": jobs,
                "repeat": repeat,
                "kind": "rebuild",
                "edit": edit,
                "seconds": round(seconds, 2),
                "units": rebuilt_units(cache, since),
                "dep_info_waited": round(waited, 2),
                "stale": stale,
            })
    finally:
        server.close()
        source.write_text(original)
        shutil.rmtree(place, ignore_errors=True)


DIAGNOSTICS = F5 / "diagnostics.jsonl"

# Dependencies of the eval fixture, with an item each, whose docs.rs builds are for Linux alone.
DEPENDENCIES = [("tokio", "sync::Mutex"), ("axum", "Router"), ("serde", "Serialize")]


def deps(online: bool) -> None:
    """Each dependency's first lookup in the eval fixture, for the host: the time that it takes, of
    docs.rs or of a local build when docs.rs has no JSON for the host (macOS and Windows), the
    builds after the first sharing the target directory."""
    root, _, _ = WORKSPACES["eval"]
    place = F5 / "runs" / "deps"
    if place.exists():
        shutil.rmtree(place)
    cache, logs = place / "cache", place / "logs"
    cache.mkdir(parents=True)
    logs.mkdir()
    server = Server(logs, cwd=root, env=settings(place, cache, online))
    try:
        for crate, path in DEPENDENCIES:
            seconds, result = server.call("get_item", {"crate": crate, "path": path})
            provenance = result.get("provenance", {})
            record({
                "workspace": "eval",
                "kind": "dependency",
                "crate": crate,
                "seconds": round(seconds, 2),
                "source": provenance.get("source"),
                "status": result.get("status"),
                "target": provenance.get("target"),
                "target_dir_mb": round(size(cache / "t") / 1e6, 1),
            })
    finally:
        server.close()
        shutil.rmtree(place, ignore_errors=True)

# A path dependency of the eval fixture whose body has a long error of a trait bound, as a handler
# of axum that is not one (cargo checks the dependencies' bodies, not rustdoc the documented
# crate's).
HELPER_MANIFEST = """[package]
name = "helper"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "=0.8.9"
"""
HELPER_LIB = """pub struct NotAResponse;

async fn handler() -> NotAResponse {
    NotAResponse
}

pub fn app() -> axum::Router {
    axum::Router::new().route("/", axum::routing::get(handler))
}
"""

# A build script that tells a few lines and then fails.
FAILING_BUILD = """fn main() {
    for line in 0..40 {
        eprintln!("checking the environment, step {line} of 40: nothing is wrong yet");
    }
    panic!("the build script of `one` failed on purpose (F5b)");
}
"""


def scenarios() -> list[tuple[str, str, str, dict]]:
    """The ways a workspace is broken: (name, workspace, package, {path: change}), where a change
    is the whole text, ("append", text) or ("replace", old, new)."""
    return [
        ("D-1 one wrong signature", "self", "chatora-rustdoc-mcp", {
            "src/lib.rs": ("append", "\n/// Broken on purpose (F5b).\npub fn f5() -> F5Missing {\n    todo!()\n}\n"),
        }),
        ("D-2 a type used widely renamed", "self", "chatora-rustdoc-mcp", {
            "src/data/crate_name.rs": ("replace", "pub struct CrateName(", "pub struct CrateNameF5("),
        }),
        ("D-3 a long trait bound in a dependency", "eval", "eval-fixture", {
            "helper/Cargo.toml": HELPER_MANIFEST,
            "helper/src/lib.rs": HELPER_LIB,
            "Cargo.toml": ("replace", "[dependencies]\n", "[dependencies]\nhelper = { path = \"helper\" }\n"),
            "src/lib.rs": ("append", "\npub use helper::app;\n"),
        }),
        ("D-4 an error in a body only", "eval", "eval-fixture", {
            "src/lib.rs": ("append", "\n/// Broken in its body only (F5b).\npub fn f5() -> u32 {\n    \"not a number\"\n}\n"),
        }),
        ("D-5 a build script that fails", "local-build", "one", {
            "one/build.rs": FAILING_BUILD,
        }),
    ]


def apply(root: Path, changes: dict) -> dict:
    """Makes `changes` in the workspace at `root`; returns what to put back."""
    saved = {}
    for relative, change in changes.items():
        path = root / relative
        saved[path] = path.read_text() if path.exists() else None
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(change, str):
            path.write_text(change)
        elif change[0] == "append":
            path.write_text(path.read_text() + change[1])
        else:
            _, old, new = change
            text = path.read_text()
            assert old in text, (relative, old)
            path.write_text(text.replace(old, new, 1))
    return saved


def put_back(saved: dict) -> None:
    for path, text in saved.items():
        if text is None:
            path.unlink(missing_ok=True)
        else:
            path.write_text(text)


def raw_errors(root: Path, package: str, target_dir: Path) -> dict:
    """The errors of the compiler that cargo writes for the documentation of `package`, as the
    server builds it (requirement R-WS-1), and whether it failed."""
    env = dict(os.environ, RUSTC_BOOTSTRAP="1", CARGO_TARGET_DIR=str(target_dir), CARGO_TERM_COLOR="never")
    output = subprocess.run(
        ["cargo", "rustdoc", "--offline", "-p", package, "--lib", "--message-format=json",
         "--", "-Z", "unstable-options", "--output-format", "json"],
        cwd=root, env=env, capture_output=True, text=True,
    )
    lengths = []
    for line in output.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-message" and message["message"].get("level") == "error":
            rendered = message["message"].get("rendered") or ""
            if rendered.startswith("error: aborting") or "could not compile" in rendered:
                continue
            # As JSON escapes it, without its quotes.
            lengths.append(len(json.dumps(rendered)) - 2)
    lengths.sort()
    return {
        "failed": output.returncode != 0,
        "errors": len(lengths),
        "lengths": lengths,
        "stderr_chars": len(json.dumps(output.stderr)) - 2,
    }


def answered_raw(server: Server, member: str) -> dict:
    """What the server answers for the member's build: its error's code, how long its text is,
    and the diagnostics it tells."""
    request = {
        "jsonrpc": "2.0",
        "id": server.next_id,
        "method": "tools/call",
        "params": {"name": "crate_overview", "arguments": {"crate": member}, "_meta": bench_meta()},
    }
    server.next_id += 1
    server.process.stdin.write(json.dumps(request) + "\n")
    server.process.stdin.flush()
    result = json.loads(server.process.stdout.readline()).get("result", {})
    if not result.get("isError"):
        return {"error": None, "status": result.get("structuredContent", {}).get("status")}
    text = result["content"][0]["text"]
    body = json.loads(text)
    return {
        "error": body.get("code"),
        "text_chars": len(text),
        "diagnostics": [len(json.dumps(d)) - 2 for d in body.get("diagnostics", [])],
        "diagnostics_omitted": body.get("diagnostics_omitted", 0),
        "stderr_chars": len(json.dumps(body["stderr"])) - 2 if "stderr" in body else None,
        "start": text[:300],
    }


def bench_meta() -> dict:
    from bench import META
    return META


def diagnostics() -> None:
    """Each scenario in a copy of its workspace, one copy for the scenarios of a workspace, put back
    after each, so that the builds of the dependencies are kept between them."""
    for name, workspace, package, changes in scenarios():
        root, member, _ = WORKSPACES[workspace]
        place = F5 / "diagnostics" / workspace
        # The directory that is copied, and the workspace's root in the copy.
        copied, inside = (root.parent, Path("ws")) if workspace == "local-build" else (root, Path("."))
        if not (place / "src").exists():
            shutil.copytree(copied, place / "src", symlinks=True, ignore=shutil.ignore_patterns("target"))
        copy = place / "src" / inside
        lock = copy / "Cargo.lock"
        locked = lock.read_text() if lock.exists() else None
        saved = apply(copy, changes)
        try:
            if "helper/Cargo.toml" in changes:
                # The new path dependency in Cargo.lock, offline.
                subprocess.run(["cargo", "update", "--offline", "--workspace"], cwd=copy, check=True,
                               capture_output=True)
            raw = raw_errors(copy, package, place / "raw-target")
            cache, logs = place / "cache", place / "logs"
            cache.mkdir(parents=True, exist_ok=True)
            logs.mkdir(exist_ok=True)
            server = Server(logs, cwd=copy, env=settings(place, cache, online=False))
            try:
                served = answered_raw(server, member)
            finally:
                server.close()
        finally:
            put_back(saved)
            if locked is not None:
                lock.write_text(locked)
            if "helper/Cargo.toml" in changes:
                shutil.rmtree(copy / "helper", ignore_errors=True)
        line = {"scenario": name, "raw": raw, "served": served}
        with DIAGNOSTICS.open("a") as results:
            results.write(json.dumps(line) + "\n")
        print(json.dumps(line)[:600], flush=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["prepare", "run", "deps", "diagnostics"])
    parser.add_argument("--online", action="store_true", help="let cargo and the server fetch")
    parser.add_argument("--workspaces", default="self,eval,local-build")
    parser.add_argument("--jobs", default="6")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--rebuilds", type=int, default=0)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare()
        return
    if args.command == "diagnostics":
        diagnostics()
        return
    if args.command == "deps":
        deps(args.online)
        return
    if not BINARY.exists():
        sys.exit(f"build the server first: cargo build --release --locked ({BINARY})")
    if not SOURCES.exists():
        sys.exit("prepare the workspaces first: python3 -I eval/build_bench.py prepare")
    for name in args.workspaces.split(","):
        for jobs in [int(jobs) for jobs in args.jobs.split(",")]:
            for repeat in range(args.repeat):
                run_one(name, jobs, repeat, args.rebuilds, args.online)


if __name__ == "__main__":
    main()
