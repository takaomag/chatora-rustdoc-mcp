#!/usr/bin/env bash
# Starts chatora-rustdoc-mcp for the A3 experiment (doc/m0-plan.md) and records what it exchanges
# with Claude Code: the lines it reads (stdin), the lines it writes (stdout) and its log (stderr).
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log_dir="${dir}/logs"
mkdir -p "${log_dir}"
prefix="${log_dir}/$(date +%Y%m%dT%H%M%S)-$$"

tee "${prefix}.in.jsonl" \
  | CHATORA_RUSTDOC_MCP_LOG__FILTER=debug "${dir}/../../target/debug/chatora-rustdoc-mcp" serve 2>"${prefix}.stderr.log" \
  | tee "${prefix}.out.jsonl"
