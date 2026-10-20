#!/usr/bin/env python3
"""Runs the eval: each case of cases.json as a headless Claude Code session that has only this
server's tools, in the fixture project (doc/m1-plan.md, E1 and P5; requirement R-QA-6).

Each run costs API usage. Usage: python3 -I eval/run.py [--model MODEL] [--budget-usd 1.0]
[--case ID ...]. The raw stream of each case and a summary go to eval/results/<time>/.
"""

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EVAL = ROOT / "eval"
SERVER = "chatora-rustdoc-mcp"
PREFIX = f"mcp__{SERVER}__"
# Characters per token, as the server's log estimates them (requirement R-OBS-1).
CHARS_PER_TOKEN = 4


def build_server() -> Path:
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    binary = ROOT / "target" / "release" / SERVER
    return binary.with_suffix(".exe") if os.name == "nt" else binary


def claude_args(case: dict, mcp_config: str, args: argparse.Namespace) -> list[str]:
    command = [
        "claude",
        "-p",
        case["prompt"],
        "--output-format",
        "stream-json",
        "--verbose",
        # Only this server, and none of the built-in tools, so that the answer comes from it.
        "--strict-mcp-config",
        "--mcp-config",
        mcp_config,
        "--tools",
        "",
        "--allowedTools",
        f"{PREFIX}*",
        # Not the user's settings (hooks, permissions, env), which would change the result.
        "--setting-sources",
        "project",
        "--no-session-persistence",
        "--max-budget-usd",
        str(args.budget_usd),
    ]
    if args.model:
        command += ["--model", args.model]
    return command


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


def run_case(case: dict, mcp_config: str, args: argparse.Namespace, out: Path) -> dict:
    started = time.monotonic()
    process = subprocess.run(
        claude_args(case, mcp_config, args),
        cwd=EVAL / "fixture",
        capture_output=True,
        text=True,
        check=False,
    )
    seconds = time.monotonic() - started
    (out / f"{case['id']}.jsonl").write_text(process.stdout)
    stream = read_stream(process.stdout.splitlines())
    final = stream["final"]
    answer = final.get("result", "")
    ours = [call["name"].removeprefix(PREFIX) for call in stream["calls"] if call["name"].startswith(PREFIX)]
    problems = []
    if process.returncode != 0 and not final:
        problems.append(f"claude exited with {process.returncode}: {process.stderr[-500:]}")
    if not set(ours) & set(case["tools"]):
        problems.append(f"none of {case['tools']} was called")
    for pattern in case["answer"]:
        if not re.search(pattern, answer, re.IGNORECASE):
            problems.append(f"the answer lacks /{pattern}/")
    if case.get("shape"):
        problem = check_shape(case["shape"], stream["results"])
        if problem:
            problems.append(problem)
    usage = final.get("usage") or {}
    return {
        "id": case["id"],
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
    }


def percentile(values: list[int], fraction: float) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, round(fraction * (len(ordered) - 1)))]


def summarize(rows: list[dict]) -> str:
    lines = [
        "| case | ok | tools | calls | errors | result chars (max) | input tokens | output tokens | USD | s |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for row in rows:
        lines.append(
            f"| {row['id']} | {'○' if row['ok'] else '×'} | {', '.join(row['tools'])} | {row['calls']} "
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
    args = parser.parse_args()

    cases = json.loads((EVAL / "cases.json").read_text())
    if args.case:
        cases = [case for case in cases if case["id"] in args.case]
    out = EVAL / "results" / time.strftime("%Y%m%d-%H%M%S")
    (out / "logs").mkdir(parents=True)
    binary = build_server()
    mcp_config = json.dumps(
        {
            "mcpServers": {
                SERVER: {
                    "command": str(binary),
                    "args": ["serve"],
                    "env": {"CHATORA_RUSTDOC_MCP_LOG__DIR": str(out / "logs")},
                }
            }
        }
    )
    claude_version = subprocess.run(["claude", "--version"], capture_output=True, text=True, check=False).stdout.strip()
    rows = []
    for case in cases:
        print(f"{case['id']} ...", file=sys.stderr, flush=True)
        rows.append(run_case(case, mcp_config, args, out))
    summary = f"Claude Code {claude_version}, model {args.model or 'default'}\n\n" + summarize(rows)
    (out / "summary.json").write_text(json.dumps(rows, ensure_ascii=False, indent=2))
    (out / "summary.md").write_text(summary + "\n")
    print(summary)
    return 0 if all(row["ok"] for row in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
