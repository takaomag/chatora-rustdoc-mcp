#!/usr/bin/env bash
# B4 (doc/m0-plan.md §4.2, Q-11): how often `--cfg docsrs` breaks the documentation of a
# dependency, so that it has to be built again without it. Every normal dependency that this
# repository builds for the host is documented on its own with `cargo rustdoc -p`, with
# `--cfg docsrs` given to its final rustdoc only (requirement R-WS-1), first on stable with
# RUSTC_BOOTSTRAP as B1 decided, then on nightly. A failure is retried without `--cfg docsrs`.
#
# Results go to work/b4/<toolchain>.tsv (crate, result, seconds of the last attempt, first error
# with --cfg docsrs) and a summary to work/b4/log.txt. Results: ok, docsrs-only (fails only with
# --cfg docsrs), fails (fails either way), cargo-panic (cargo panicked, as with -p on a dependency
# that only proc-macros use).
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b4"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
: >"${LOG}"
cd "${REPO}"
HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }

# The normal dependencies built for the host, as name@version, without the members.
PACKAGES="$(cargo +stable tree --workspace --target "${HOST}" -e normal --prefix none \
  --format '{p}' | grep -v ' (/' | sed -E 's/^([^ ]+) v([^ ]+).*/\1@\2/' | sort -u)"
ALL_NAMES="$(cargo +stable metadata --format-version 1 --locked | jq -r '
  [.packages[].targets[] | select(.kind | index("lib") or index("rlib") or index("proc-macro"))
    | .name | gsub("-"; "_")] | unique | join(",")')"

log "# B4 $(date -Iseconds)"
log "stable: $(rustc +stable -V); nightly: $(rustc +nightly -V)"
log "normal dependencies built for ${HOST}: $(wc -l <<<"${PACKAGES}")"

# Documents <spec> on <toolchain> with the extra rustdoc arguments <args>...; sets STATUS,
# ELAPSED and ERROR (the first error line, if any).
document() {
  local toolchain="$1" spec="$2"
  shift 2
  timed cargo "+${toolchain}" rustdoc -q -p "${spec}" --lib --target "${HOST}" \
    -- -Z unstable-options --output-format json "$@" 2>"${OUT}/last.stderr"
  ERROR="$(grep -m1 -E '^error|panicked at' "${OUT}/last.stderr" | cut -c1-200 || true)"
}

run_toolchain() {
  local toolchain="$1" tsv="${OUT}/$1.tsv" spec result
  clean_target_dir "b4-$1"
  use_target_dir "b4-$1"
  : >"${tsv}"
  while read -r spec; do
    document "${toolchain}" "${spec}" --cfg docsrs
    if [[ ${STATUS} -eq 0 ]]; then
      result=ok
    elif grep -q 'panicked at' "${OUT}/last.stderr"; then
      result=cargo-panic
    else
      local with_docsrs="${ERROR}"
      cp "${OUT}/last.stderr" "${OUT}/${toolchain}.${spec}.docsrs.stderr"
      document "${toolchain}" "${spec}"
      if [[ ${STATUS} -eq 0 ]]; then
        result=docsrs-only
      else
        result=fails
      fi
      ERROR="${with_docsrs}"
    fi
    printf '%s\t%s\t%s\t%s\n' "${spec}" "${result}" "${ELAPSED}" "${ERROR}" >>"${tsv}"
  done <<<"${PACKAGES}"
  log "## ${toolchain}"
  cut -f2 "${tsv}" | sort | uniq -c | sed 's/^/  /' | tee -a "${LOG}" >&2
  awk -F'\t' '$2 != "ok" { printf "  %s %s: %s\n", $2, $1, $4 }' "${tsv}" | tee -a "${LOG}" >&2
}

RUSTC_BOOTSTRAP="${ALL_NAMES}" run_toolchain stable
unset RUSTC_BOOTSTRAP
run_toolchain nightly

log "# done $(date -Iseconds)"
