#!/usr/bin/env python3
"""Runs the eval: each case of cases.json as a headless Claude Code session in a copy of the fixture
project, with this server's tools alone, or with the built-in tools too, to see which the model
picks (doc/m1-plan.md, E1 and P5; doc/m2-plan.md, M2f's F4a; requirement R-QA-6).

Each run costs API usage. Usage: python3 -I eval/run.py [--model MODEL] [--budget-usd 1.0]
[--case ID ...] [--dry-run]. The raw stream of each case and a summary go to eval/results/<time>/.
With --dry-run, each case's project is made and set up, and the command is shown, but Claude Code
is not run.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import Server  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
EVAL = ROOT / "eval"
SERVER = "chatora-rustdoc-mcp"
PREFIX = f"mcp__{SERVER}__"
# Characters per token, as the server's log estimates them (requirement R-OBS-1).
CHARS_PER_TOKEN = 4
# The built-in tools of a case that sees whether the model picks this server (`builtin`): those that
# read code, and the search of deferred tools, as in everyday use; not Bash, which could run cargo.
BUILTIN = ["Read", "Grep", "Glob", "LSP", "ToolSearch"]
# Without the user's git config, nor the system's, for the copies of the fixture.
GIT_ENV = dict(
    os.environ,
    GIT_CONFIG_GLOBAL=os.devnull,
    GIT_CONFIG_NOSYSTEM="1",
    GIT_AUTHOR_NAME="eval",
    GIT_AUTHOR_EMAIL="eval@example.invalid",
    GIT_COMMITTER_NAME="eval",
    GIT_COMMITTER_EMAIL="eval@example.invalid",
)


def build_server() -> Path:
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    binary = ROOT / "target" / "release" / SERVER
    return binary.with_suffix(".exe") if os.name == "nt" else binary


def claude_args(case: dict, prompt: str, mcp_config: str, args: argparse.Namespace) -> list[str]:
    builtin = case.get("builtin", False)
    command = [
        "claude",
        "-p",
        prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        # Only this server of the MCP servers.
        "--strict-mcp-config",
        "--mcp-config",
        mcp_config,
        # None of the built-in tools, so that the answer comes from this server; or those that read
        # code, to see which the model picks.
        "--tools",
        ",".join(BUILTIN) if builtin else "",
        "--allowedTools",
        ",".join([f"{PREFIX}*", *BUILTIN]) if builtin else f"{PREFIX}*",
        # Not the user's settings (hooks, permissions, env), which would change the result; but for
        # the cases with the built-in tools, whose language server comes from the user's plugins.
        # The user's CLAUDE.md and rules are read either way: --bare, which leaves them out, leaves
        # out the language server and the user's login too (decided in M2f's F4a, 6).
        "--setting-sources",
        "user,project" if builtin else "project",
        "--no-session-persistence",
        "--max-budget-usd",
        str(args.budget_usd),
    ]
    if args.model:
        command += ["--model", args.model]
    return command


def git(arguments: list[str], cwd: Path) -> None:
    subprocess.run(["git", *arguments], cwd=cwd, env=GIT_ENV, check=True, capture_output=True)


def workspace_for(case: dict, place: Path) -> dict:
    """A copy of the fixture for `case` in `place`, a repository of its own with a commit, and the
    other directories that its prompt names: {fixture}, and as the case asks, {other} (a copy of
    tests/fixtures/other-workspace) and {worktree} (a git worktree of the fixture's copy)."""
    place.mkdir(parents=True)
    fixture = place / "fixture"
    shutil.copytree(EVAL / "fixture", fixture)
    git(["init", "--quiet"], fixture)
    git(["add", "--all"], fixture)
    git(["commit", "--quiet", "-m", "fixture"], fixture)
    paths = {"fixture": fixture}
    if "{other}" in case["prompt"]:
        paths["other"] = place / "other"
        shutil.copytree(ROOT / "tests" / "fixtures" / "other-workspace", paths["other"])
    if "{worktree}" in case["prompt"]:
        paths["worktree"] = place / "worktree"
        git(["worktree", "add", "--quiet", "--detach", str(paths["worktree"])], fixture)
    return paths


def server_env(case: dict, logs: Path) -> dict:
    """The environment of the server of `case`: its log directory, and the settings it sets."""
    return {"CHATORA_RUSTDOC_MCP_LOG__DIR": str(logs), **case.get("env", {})}


def set_up(case: dict, paths: dict, logs: Path) -> None:
    """What `case` does before its session: `build` builds the fixture's member, with a server of
    its own and the same cache, as an earlier session would have; `append` adds text to a file of
    the fixture's copy, `replace` replaces text in one, and `write` writes one, as edits since."""
    for step in case.get("setup", []):
        if step == "build":
            env = {
                **server_env(case, logs),
                "CHATORA_RUSTDOC_MCP_BUILD__WAIT_SECS": "600",
                "CHATORA_RUSTDOC_MCP_LIMITS__REQUEST_TIMEOUT_SECS": "900",
            }
            server = Server(logs, cwd=paths["fixture"], env=env)
            try:
                server.call("crate_overview", {"crate": "eval_fixture"})
            finally:
                server.close()
        elif isinstance(step, dict) and "append" in step:
            file, text = step["append"]
            with (paths["fixture"] / file).open("a") as appended:
                appended.write(text)
        elif isinstance(step, dict) and "write" in step:
            file, text = step["write"]
            (paths["fixture"] / file).write_text(text)
        elif isinstance(step, dict) and "replace" in step:
            file, old, new = step["replace"]
            path = paths["fixture"] / file
            text = path.read_text()
            if old not in text:
                raise ValueError(f"{case['id']}: {old!r} is not in {file}")
            path.write_text(text.replace(old, new, 1))
        else:
            raise ValueError(f"{case['id']}: no such step {step!r}")


def placed(text: str, paths: dict) -> str:
    """`text` with the directories of `paths` in place of their names in braces."""
    for name, path in paths.items():
        text = text.replace("{" + name + "}", str(path))
    return text


def text_of(content) -> str:
    """The text of a tool_result's content: a string, or text blocks."""
    if isinstance(content, str):
        return content
    return "\n".join(block.get("text", "") for block in content or [] if block.get("type") == "text")


def read_stream(lines: list[str]) -> dict:
    """The calls, results, answer and usage of a session's stream-json."""
    calls, results, first_usage = [], [], None
    final = {}
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        message = event.get("message") or {}
        if kind == "assistant":
            if first_usage is None and message.get("usage"):
                first_usage = message["usage"]
            for block in message.get("content") or []:
                if block.get("type") == "tool_use":
                    calls.append({"id": block["id"], "name": block["name"], "input": block.get("input")})
        elif kind == "user":
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    results.append(
                        {
                            "id": block.get("tool_use_id"),
                            "is_error": bool(block.get("is_error")),
                            "text": text_of(block.get("content")),
                        }
                    )
        elif kind == "result":
            final = event
    return {"calls": calls, "results": results, "first_usage": first_usage or {}, "final": final}


def input_tokens(usage: dict) -> int:
    return sum(usage.get(key, 0) for key in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))


def check_shape(shape: str, results: list[dict]) -> str | None:
    """What is wrong with the first tool_result, for the cases that check its form (R-OUT-1, R-ERR-2)."""
    if not results:
        return "no tool_result"
    first = results[0]
    try:
        body = json.loads(first["text"])
    except json.JSONDecodeError:
        return f"the tool_result is not JSON: {first['text'][:200]}"
    if shape == "structured":
        if first["is_error"] or not {"provenance", "data"} <= body.keys():
            return f"not the structured content: {first['text'][:200]}"
    elif shape == "error":
        if not first["is_error"] or not {"code", "request_id"} <= body.keys():
            return f"not the error text: is_error={first['is_error']} {first['text'][:200]}"
    return None


def retried_after_building(stream: dict) -> bool:
    """Whether a call of this server was made again after an answer of it that was not ready."""
    ids = [call["id"] for call in stream["calls"] if call["name"].startswith(PREFIX)]
    for result in stream["results"]:
        if result["id"] in ids and '"status":"building"' in result["text"].replace(" ", ""):
            return ids.index(result["id"]) < len(ids) - 1
    return False


def run_case(case: dict, binary: Path, args: argparse.Namespace, out: Path, run: str) -> dict:
    place = out / "cases" / run
    paths = workspace_for(case, place)
    logs = out / "logs"
    set_up(case, paths, logs)
    mcp_config = json.dumps(
        {"mcpServers": {SERVER: {"command": str(binary), "args": ["serve"], "env": server_env(case, logs)}}}
    )
    prompt = placed(case["prompt"], paths)
    command = claude_args(case, prompt, mcp_config, args)
    if args.dry_run:
        print(json.dumps({"case": run, "cwd": str(paths["fixture"]), "command": command}), flush=True)
        return {"id": run, "question": case["question"], "ok": True, "problems": [], "tools": [], "calls": 0,
                "errors": 0, "result_chars": [], "start_tokens": 0, "input_tokens": 0, "output_tokens": 0,
                "cost_usd": 0.0, "seconds": 0, "answer": "", "first_tool": None, "builtin_calls": 0,
                "retried": False}
    started = time.monotonic()
    process = subprocess.run(command, cwd=paths["fixture"], capture_output=True, text=True, check=False)
    seconds = time.monotonic() - started
    (out / f"{run}.jsonl").write_text(process.stdout)
    stream = read_stream(process.stdout.splitlines())
    final = stream["final"]
    answer = final.get("result", "")
    ours = [call["name"].removeprefix(PREFIX) for call in stream["calls"] if call["name"].startswith(PREFIX)]
    first = stream["calls"][0]["name"] if stream["calls"] else None
    builtin_calls = sum(1 for call in stream["calls"] if not call["name"].startswith(PREFIX))
    retried = retried_after_building(stream)
    problems = []
    if process.returncode != 0 and not final:
        problems.append(f"claude exited with {process.returncode}: {process.stderr[-500:]}")
    if not set(ours) & set(case["tools"]):
        problems.append(f"none of {case['tools']} was called")
    if case.get("retry") and not retried:
        problems.append("not called again after `status: building`")
    for name, value in case.get("arguments", {}).items():
        wanted = placed(value, paths)
        given = [str((call["input"] or {}).get(name, "")) for call in stream["calls"] if call["name"].startswith(PREFIX)]
        if not any(Path(given_value).resolve() == Path(wanted).resolve() for given_value in given if given_value):
            problems.append(f"no call with {name} = {wanted}")
    for pattern in case["answer"]:
        if not re.search(pattern, answer, re.IGNORECASE):
            problems.append(f"the answer lacks /{pattern}/")
    if case.get("shape"):
        problem = check_shape(case["shape"], stream["results"])
        if problem:
            problems.append(problem)
    usage = final.get("usage") or {}
    return {
        "id": run,
        "question": case["question"],
        "ok": not problems,
        "problems": problems,
        "tools": ours,
        "calls": len(stream["calls"]),
        "errors": sum(result["is_error"] for result in stream["results"]),
        "result_chars": [len(result["text"]) for result in stream["results"]],
        "start_tokens": input_tokens(stream["first_usage"]),
        "input_tokens": input_tokens(usage),
        "output_tokens": usage.get("output_tokens", 0),
        "cost_usd": final.get("total_cost_usd", 0.0),
        "seconds": round(seconds, 1),
        "answer": answer,
        "first_tool": first.removeprefix(PREFIX) if first else None,
        "builtin_calls": builtin_calls,
        "retried": retried,
    }


def percentile(values: list[int], fraction: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, round(fraction * (len(ordered) - 1)))]


def summarize(rows: list[dict]) -> str:
    lines = [
        "| case | ok | tools | first | built-in calls | calls | errors | result chars (max) | input tokens "
        "| output tokens | USD | s |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for row in rows:
        lines.append(
            f"| {row['id']} | {'○' if row['ok'] else '×'} | {', '.join(row['tools'])} | {row['first_tool'] or ''} "
            f"| {row['builtin_calls']} | {row['calls']} "
            f"| {row['errors']} | {max(row['result_chars'], default=0)} | {row['input_tokens']} "
            f"| {row['output_tokens']} | {row['cost_usd']:.3f} | {row['seconds']} |"
        )
    sizes = [size for row in rows for size in row["result_chars"]]
    starts = [row["start_tokens"] for row in rows if row["start_tokens"]]
    lines += [
        "",
        f"- passed: {sum(row['ok'] for row in rows)} / {len(rows)}",
        f"- fixed cost at the start (input tokens of the first request, min): {min(starts, default=0)}",
        f"- tool_result: p50 {percentile(sizes, 0.5)} chars, p95 {percentile(sizes, 0.95)} chars "
        f"(about {percentile(sizes, 0.5) // CHARS_PER_TOKEN} / {percentile(sizes, 0.95) // CHARS_PER_TOKEN} tokens)",
        f"- calls: {sum(row['calls'] for row in rows)}, cost: {sum(row['cost_usd'] for row in rows):.2f} USD",
    ]
    for row in rows:
        if row["problems"]:
            lines.append(f"- {row['id']}: {'; '.join(row['problems'])}")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--model", help="the model of the sessions (default: Claude Code's)")
    parser.add_argument("--budget-usd", type=float, default=1.0, help="the most one case may cost")
    parser.add_argument("--case", action="append", help="run only this case (repeatable)")
    parser.add_argument("--dry-run", action="store_true", help="make and set up the projects, run no session")
    args = parser.parse_args()

    cases = json.loads((EVAL / "cases.json").read_text())
    if args.case:
        cases = [case for case in cases if case["id"] in args.case]
    out = EVAL / "results" / time.strftime("%Y%m%d-%H%M%S")
    (out / "logs").mkdir(parents=True)
    binary = build_server()
    claude_version = subprocess.run(["claude", "--version"], capture_output=True, text=True, check=False).stdout.strip()
    rows = []
    for case in cases:
        # A case that the model may answer either way is asked more than once (`repeat`).
        repeat = case.get("repeat", 1)
        for time_asked in range(repeat):
            run = case["id"] if repeat == 1 else f"{case['id']}-{time_asked + 1}"
            print(f"{run} ...", file=sys.stderr, flush=True)
            rows.append(run_case(case, binary, args, out, run))
    if args.dry_run:
        return 0
    summary = f"Claude Code {claude_version}, model {args.model or 'default'}\n\n" + summarize(rows)
    (out / "summary.json").write_text(json.dumps(rows, ensure_ascii=False, indent=2))
    (out / "summary.md").write_text(summary + "\n")
    print(summary)
    return 0 if all(row["ok"] for row in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
