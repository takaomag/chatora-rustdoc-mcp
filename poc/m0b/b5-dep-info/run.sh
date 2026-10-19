#!/usr/bin/env bash
# B5 (doc/m0-plan.md §4.2, R-WS-4): how to get the dep-info of the crate being documented.
# (a) rustdoc again with the same arguments and `--emit=dep-info`, or (b) the dep-info of
# `cargo check` for the crate.
#
# 1. probe/: which files each way records (include_str!, include_bytes!, #[path], code that
#    build.rs generates, and a module that only `--cfg docsrs` compiles).
# 2. This repository: how long each way takes, for the member and for a dependency (tokio), in
#    the target dir that documents them, with RUSTC_BOOTSTRAP fixed as B1 decided.
# The log goes to work/b5/log.txt.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b5"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
: >"${LOG}"
HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"
JSON_ARGS=(-Z unstable-options --output-format json --cfg docsrs)

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }

# The files a dep-info file records (its first rule), relative to the current directory and the
# work dir, one per line.
recorded() {
  sed -n '1s/^[^:]*: //p' "$1" | tr ' ' '\n' | grep . |
    sed -e "s|^$(pwd)/||" -e "s|^${WORK}/||" | sed 's/^/    /'
}

log "# B5 $(date -Iseconds)"
log "stable: $(rustc +stable -V); $(cargo +stable -V)"

# 1. Which files each way records.
cd "${M0B}/b5-dep-info/probe"
clean_target_dir b5-probe
use_target_dir b5-probe
export RUSTC_BOOTSTRAP=depinfo_probe
log "## probe: the JSON, then (a) rustdoc with --emit=dep-info"
cargo +stable rustdoc -q --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
json="${CARGO_TARGET_DIR}/${HOST}/doc/depinfo_probe.json"
before="$(stat -c %Y.%y "${json}")"
cargo +stable rustdoc -q --lib --target "${HOST}" -- "${JSON_ARGS[@]}" --emit=dep-info
log "  the JSON was written again by (a): $([[ "$(stat -c %Y.%y "${json}")" != "${before}" ]] &&
  echo yes || echo no)"
log "  (a) ${HOST}/doc/depinfo_probe.d records:"
recorded "${CARGO_TARGET_DIR}/${HOST}/doc/depinfo_probe.d" | tee -a "${LOG}" >&2
log "  (a) records env: $(grep '^# env-dep:' "${CARGO_TARGET_DIR}/${HOST}/doc/depinfo_probe.d" |
  sed "s|${WORK}/||" | tr '\n' ' ')"
log "## probe: (b) cargo check"
cargo +stable check -q --lib --target "${HOST}"
check_d="$(find "${CARGO_BUILD_BUILD_DIR}" -path '*/deps/depinfo_probe-*.d' | head -n1)"
log "  (b) ${check_d#"${WORK}/"} records:"
recorded "${check_d}" | tee -a "${LOG}" >&2
log "  build.rs is in the build script's own dep-info: $(find "${CARGO_BUILD_BUILD_DIR}" \
  -name 'build_script_build-*.d' -exec grep -l 'build.rs' {} + | wc -l) file(s)"

# 2. Time on this repository.
cd "${REPO}"
clean_target_dir b5-repo
use_target_dir b5-repo
RUSTC_BOOTSTRAP="$(cargo +stable metadata --format-version 1 --locked | jq -r '
  [.packages[].targets[] | select(.kind | index("lib") or index("rlib") or index("proc-macro"))
    | .name | gsub("-"; "_")] | unique | join(",")')"
export RUSTC_BOOTSTRAP
lock_version() { sed -n "/^name = \"$1\"$/{n;s/^version = \"\\(.*\\)\"$/\\1/p}" Cargo.lock; }

# Runs one cargo step, quietly, and logs its time and how many units were not fresh.
step() {
  local label="$1"
  shift
  # `--message-format` goes right after the subcommand, before any `--` for rustdoc.
  timed cargo +stable "$1" --message-format=json "${@:2}" >"${OUT}/${label}.jsonl" \
    2>"${OUT}/${label}.stderr"
  log "${label}: ${ELAPSED} s, exit ${STATUS}, rebuilt $(jq -s '[.[] | select(.reason ==
    "compiler-artifact" and (.fresh | not))] | length' "${OUT}/${label}.jsonl") unit(s)"
}

for package in "chatora-rustdoc-mcp@0.1.0:chatora_rustdoc_mcp" \
  "tokio@$(lock_version tokio):tokio"; do
  spec="${package%%:*}"
  crate="${package##*:}"
  log "## ${spec}"
  step "${crate}.json" rustdoc -p "${spec}" --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
  step "${crate}.json-again" rustdoc -p "${spec}" --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
  step "${crate}.a-emit" rustdoc -p "${spec}" --lib --target "${HOST}" -- "${JSON_ARGS[@]}" \
    --emit=dep-info
  log "  (a) records $(recorded "${CARGO_TARGET_DIR}/${HOST}/doc/${crate}.d" | wc -l) file(s)"
  step "${crate}.b-check" check -p "${spec}" --lib --target "${HOST}"
  check_d="$(ls -t "${CARGO_BUILD_BUILD_DIR}/${HOST}/debug/deps/${crate}"-*.d 2>/dev/null |
    head -n1 || true)"
  if [[ -n "${check_d}" ]]; then
    log "  (b) records $(recorded "${check_d}" | wc -l) file(s)"
  fi
  step "${crate}.json-after" rustdoc -p "${spec}" --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
done

log "# done $(date -Iseconds)"
