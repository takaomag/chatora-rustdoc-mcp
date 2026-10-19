#!/usr/bin/env bash
# B2 (doc/m0-plan.md §4.2, Q-17): `cargo doc --output-format json` against `cargo rustdoc`, on
# this repository as the workspace. Each case runs in a target dir of its own, cold and then warm.
# The log goes to work/b2/log.txt; the JSON messages and stderr of each run to work/b2/.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b2"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
: >"${LOG}"
cd "${REPO}"

HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"
MEMBER="chatora-rustdoc-mcp@0.1.0"
# A dependency, at the version in Cargo.lock.
DEP="serde@$(sed -n '/^name = "serde"$/{n;s/^version = "\(.*\)"$/\1/p}' "${REPO}/Cargo.lock")"
JSON_ARGS=(-- -Z unstable-options --output-format json)

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }

# Runs `cargo +<toolchain> <subcommand> --message-format=json <args>...` in the current target dir
# and logs its time, exit status, and how many units, fresh units and JSON files its messages
# name: `run_once <label> <toolchain> <subcommand> <args>...`. RUSTC_BOOTSTRAP passes through.
run_once() {
  local label="$1" toolchain="$2" subcommand="$3"
  shift 3
  timed cargo "+${toolchain}" "${subcommand}" --message-format=json "$@" \
    >"${OUT}/${label}.jsonl" 2>"${OUT}/${label}.stderr"
  log "${label}: ${ELAPSED} s, exit ${STATUS}"
  if [[ ${STATUS} -ne 0 ]]; then
    log "  $(grep -m1 -E '^error' "${OUT}/${label}.stderr" || tail -n1 "${OUT}/${label}.stderr")"
  fi
  jq -rs '
    [.[] | select(.reason == "compiler-artifact")] as $a
    | "  units: \($a | length), fresh: \([$a[] | select(.fresh)] | length)",
      "  JSON files named in messages: \([$a[] | .filenames[] | select(endswith(".json"))] | length)"
  ' "${OUT}/${label}.jsonl" | tee -a "${LOG}" >&2
}

# Logs how many rustdoc JSON files are in the current target dir and build dir.
list_json() {
  { find "${CARGO_TARGET_DIR}" "${CARGO_BUILD_BUILD_DIR}" \
    \( -path '*/doc/*.json' -o -path '*/out/*.json' \) -not -path '*/fingerprint/*' \
    -not -name 'search-index*' 2>/dev/null || true; } | wc -l |
    sed 's/^/  JSON files in the target dir and the build dir: /' | tee -a "${LOG}" >&2
}

# A case in a target dir of its own, cold then warm: `run_case <case> <toolchain> <subcommand>
# <args>...`.
run_case() {
  local case="$1"
  shift
  clean_target_dir "b2-${case}"
  use_target_dir "b2-${case}"
  log "## ${case}: cargo +$1 $2 ${*:3}"
  run_once "${case}.cold" "$@"
  run_once "${case}.warm" "$@"
  list_json
}

log "# B2 $(date -Iseconds)"
log "stable: $(rustc +stable -V); $(cargo +stable -V)"
log "nightly: $(rustc +nightly -V); $(cargo +nightly -V)"
log "host: ${HOST}"

# 1. A member: one crate, with its dependencies checked.
run_case rustdoc-member nightly rustdoc -p "${MEMBER}" --lib --target "${HOST}" "${JSON_ARGS[@]}"
run_case doc-member nightly doc -Z unstable-options --output-format json -p "${MEMBER}" --lib \
  --no-deps --target "${HOST}"

# 2. A dependency.
run_case rustdoc-dep nightly rustdoc -p "${DEP}" --lib --target "${HOST}" "${JSON_ARGS[@]}"
run_case doc-dep nightly doc -Z unstable-options --output-format json -p "${DEP}" --no-deps \
  --target "${HOST}"

# 3. Every dependency of the workspace at once (only cargo doc can do this in one command).
run_case doc-all nightly doc -Z unstable-options --output-format json --target "${HOST}"

# 4. Two versions of one crate (syn 2 and 3 are both in Cargo.lock). Only proc-macros use them,
# so cargo panics when `-p` names them (B7-9); dup.sh covers two versions that `-p` can name.
run_case doc-syn-both nightly doc -Z unstable-options --output-format json -p syn@2.0.119 \
  -p syn@3.0.9 --no-deps --target "${HOST}"
for f in $(find "${CARGO_TARGET_DIR}" "${CARGO_BUILD_BUILD_DIR}" -name 'syn.json' 2>/dev/null || true); do
  log "  ${f#"${WORK}/"}: crate_version $(jq -r .crate_version "${f}")"
done
run_case rustdoc-syn2 nightly rustdoc -p syn@2.0.119 --lib --target "${HOST}" "${JSON_ARGS[@]}"
log "## rustdoc-syn3 in the target dir of rustdoc-syn2"
run_once rustdoc-syn3 nightly rustdoc -p syn@3.0.9 --lib --target "${HOST}" "${JSON_ARGS[@]}"
for f in $(find "${CARGO_TARGET_DIR}" "${CARGO_BUILD_BUILD_DIR}" -name 'syn.json' 2>/dev/null || true); do
  log "  ${f#"${WORK}/"}: crate_version $(jq -r .crate_version "${f}")"
done

# 5. Stable with RUSTC_BOOTSTRAP limited to one crate (requirement R-WS-8), and for the process.
RUSTC_BOOTSTRAP=chatora_rustdoc_mcp run_case stable-rustdoc-member stable rustdoc -p "${MEMBER}" \
  --lib --target "${HOST}" "${JSON_ARGS[@]}"
RUSTC_BOOTSTRAP=chatora_rustdoc_mcp run_case stable-doc-member stable doc -Z unstable-options \
  --output-format json -p "${MEMBER}" --lib --no-deps --target "${HOST}"
RUSTC_BOOTSTRAP=1 run_case stable-doc-member-bootstrap-all stable doc -Z unstable-options \
  --output-format json -p "${MEMBER}" --lib --no-deps --target "${HOST}"

# 6. Fingerprints across commands in one target dir: cargo doc after cargo rustdoc, cargo rustdoc
# after cargo doc, and cargo check after both.
use_target_dir b2-rustdoc-member
log "## other commands in the target dir of rustdoc-member"
run_once doc-after-rustdoc nightly doc -Z unstable-options --output-format json -p "${MEMBER}" \
  --lib --no-deps --target "${HOST}"
run_once rustdoc-after-doc nightly rustdoc -p "${MEMBER}" --lib --target "${HOST}" "${JSON_ARGS[@]}"
run_once check-after-rustdoc nightly check -p "${MEMBER}" --lib --target "${HOST}"

log "# done $(date -Iseconds)"
