#!/usr/bin/env bash
# B1 (doc/m0-plan.md §4.2, Q-12): stable + `RUSTC_BOOTSTRAP=<crate names>` for local builds, on
# this repository as the workspace (requirement R-WS-8).
#
# 1. Which dependencies watch RUSTC_BOOTSTRAP: build scripts that print
#    `rerun-if-env-changed=RUSTC_BOOTSTRAP`, and build.rs sources that mention it.
# 2. What is rebuilt, and how long it takes, when the value changes or stays the same, and when a
#    plain `cargo check` shares the target dir.
# 3. Whether the documentation changes with the value: the member and thiserror (whose build.rs
#    turns on a nightly feature when RUSTC_BOOTSTRAP names it) across the target dirs of 2, and
#    thiserror against nightly and docs.rs (fetched into the server's cache).
# The log goes to work/b1/log.txt; the JSON messages of each step to work/b1/.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b1"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
: >"${LOG}"
cd "${REPO}"

HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"
MEMBER_PKG="chatora-rustdoc-mcp@0.1.0"
MEMBER="chatora_rustdoc_mcp"
lock_version() { sed -n "/^name = \"$1\"$/{n;s/^version = \"\\(.*\\)\"$/\\1/p}" "${REPO}/Cargo.lock"; }
SERDE="serde@$(lock_version serde)"
TOKIO="tokio@$(lock_version tokio)"
THISERROR="thiserror@$(lock_version thiserror)"

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }

# Runs one cargo step in the current target dir, on the toolchain in TOOLCHAIN (stable if unset),
# with RUSTC_BOOTSTRAP set to <value> (unset if empty), and logs its time and the units that were
# not fresh: `step <label> <value> <subcommand> <args>...`.
step() {
  local label="$1" value="$2" subcommand="$3"
  shift 3
  if [[ -n "${value}" ]]; then
    timed env RUSTC_BOOTSTRAP="${value}" cargo "+${TOOLCHAIN:-stable}" "${subcommand}" \
      --message-format=json \
      "$@" >"${OUT}/${label}.jsonl" 2>"${OUT}/${label}.stderr"
  else
    timed env -u RUSTC_BOOTSTRAP cargo "+${TOOLCHAIN:-stable}" "${subcommand}" \
      --message-format=json "$@" \
      >"${OUT}/${label}.jsonl" 2>"${OUT}/${label}.stderr"
  fi
  local rebuilt
  rebuilt="$(jq -rs '
    [.[] | select(.reason == "compiler-artifact" and (.fresh | not))
      | .target.name + (if .target.kind == ["custom-build"] then "(build.rs)" else "" end)]
    | "\(length): \(join(" "))"
  ' "${OUT}/${label}.jsonl")"
  local shown="${value:-unset}"
  if [[ ${#shown} -gt 60 ]]; then
    shown="<$(tr ',' '\n' <<<"${value}" | wc -l) names>"
  fi
  log "${label} [RUSTC_BOOTSTRAP=${shown}]: ${ELAPSED} s, exit ${STATUS}, rebuilt ${rebuilt}"
  if [[ ${STATUS} -ne 0 ]]; then
    log "  $(grep -m1 -E '^error' "${OUT}/${label}.stderr" || tail -n1 "${OUT}/${label}.stderr")"
  fi
}

doc_member() { step "$1" "$2" rustdoc -p "${MEMBER_PKG}" --lib --target "${HOST}" \
  -- -Z unstable-options --output-format json; }
doc_dep() { step "$1" "$2" rustdoc -p "$3" --lib --target "${HOST}" \
  -- -Z unstable-options --output-format json; }
check() { step "$1" "$2" check --workspace --all-targets --target "${HOST}"; }

log "# B1 $(date -Iseconds)"
log "stable: $(rustc +stable -V); $(cargo +stable -V)"
log "dependencies: ${SERDE} ${TOKIO} ${THISERROR}"

# 1. Dependencies that watch RUSTC_BOOTSTRAP.
log "## build.rs sources that mention RUSTC_BOOTSTRAP"
cargo +stable metadata --format-version 1 --locked |
  jq -r '.packages[] | select(.source != null) | .targets[]
    | select(.kind == ["custom-build"]) | .src_path' |
  while read -r build_rs; do
    if grep -q RUSTC_BOOTSTRAP "${build_rs}"; then
      log "  ${build_rs#"${CARGO_HOME:-${HOME}/.cargo}/registry/src/"}"
    fi
  done

# 2a. A value for each crate, in one target dir: member, serde, member.
clean_target_dir b1-per-crate
use_target_dir b1-per-crate
log "## a value for each crate (one target dir)"
doc_member per-crate.1-member "${MEMBER}"
doc_dep per-crate.2-serde serde "${SERDE}"
doc_member per-crate.3-member "${MEMBER}"
doc_dep per-crate.4-thiserror thiserror "${THISERROR}"
doc_member per-crate.5-member "${MEMBER}"
log "  build scripts that print rerun-if-env-changed=RUSTC_BOOTSTRAP:"
{ grep -rl --include=output 'rerun-if-env-changed=RUSTC_BOOTSTRAP' "${CARGO_BUILD_BUILD_DIR}" ||
  true; } | sed -E 's|.*/build/([^/]+)/[^/]+/output$|    \1|; s|.*/build/([^/]+-[0-9a-f]+)/output$|    \1|' |
  sort -u | tee -a "${LOG}" >&2

# 2b. One fixed value that names every crate to document, in one target dir.
FIXED="${MEMBER},serde,thiserror,tokio"
clean_target_dir b1-fixed
use_target_dir b1-fixed
log "## one fixed value ${FIXED} (one target dir)"
doc_member fixed.1-member "${FIXED}"
doc_dep fixed.2-serde "${FIXED}" "${SERDE}"
doc_dep fixed.3-tokio "${FIXED}" "${TOKIO}"
doc_dep fixed.4-thiserror "${FIXED}" "${THISERROR}"
doc_member fixed.5-member "${FIXED}"

# 2c. A list that grows as dependencies are documented, in one target dir.
clean_target_dir b1-growing
use_target_dir b1-growing
log "## a growing list (one target dir)"
doc_member growing.1-member "${MEMBER}"
doc_dep growing.2-serde "${MEMBER},serde" "${SERDE}"
doc_dep growing.3-thiserror "${MEMBER},serde,thiserror" "${THISERROR}"
doc_member growing.4-member "${MEMBER},serde,thiserror"

# 2d. Members only for the members' target dir, and a plain `cargo check` in the same target dir.
clean_target_dir b1-with-check
use_target_dir b1-with-check
log "## the members' value, sharing the target dir with plain cargo check"
check with-check.1-check ""
doc_member with-check.2-member "${MEMBER}"
check with-check.3-check ""
doc_member with-check.4-member "${MEMBER}"

# 2e. Every crate name in the resolve (members and the lib and proc-macro targets of all
# dependencies), fixed for the target dir: the value changes only with Cargo.lock.
ALL="$(cargo +stable metadata --format-version 1 --locked | jq -r '
  [.packages[].targets[] | select(.kind | index("lib") or index("rlib") or index("proc-macro"))
    | .name | gsub("-"; "_")] | unique | join(",")')"
clean_target_dir b1-all
use_target_dir b1-all
log "## every crate name in the resolve ($(tr ',' '\n' <<<"${ALL}" | wc -l) names, ${#ALL} characters)"
doc_member all.1-member "${ALL}"
doc_dep all.2-thiserror "${ALL}" "${THISERROR}"
doc_dep all.3-serde "${ALL}" "${SERDE}"
doc_member all.4-member "${ALL}"
log "  cfgs set by build scripts that watch RUSTC_BOOTSTRAP:"
{ grep -rh --include=output 'rustc-cfg' \
  $(grep -rl --include=output 'rerun-if-env-changed=RUSTC_BOOTSTRAP' "${CARGO_BUILD_BUILD_DIR}") ||
  true; } | sort -u | sed 's/^/    /' | tee -a "${LOG}" >&2

# 3. The documentation. The JSON is compared after sorting its keys and dropping the paths of
# external crates, which name the target dir.
normalized() { jq -S 'del(.external_crates[].path)' "$1" | sha256sum | cut -c1-16; }
# One line per item of the crate: name, kind, and the start of its attributes.
items() {
  jq -r '[.index[] | select(.crate_id == 0)
    | "\(.name // "-") \(.inner | keys[0]) \(.attrs | tostring | .[0:60])"] | sort | .[]' "$1"
}
log "## the documentation, normalized"
for name in "${MEMBER}" thiserror; do
  for dir in b1-per-crate b1-fixed b1-growing b1-with-check b1-all; do
    json="${WORK}/${dir}/target/${HOST}/doc/${name}.json"
    if [[ -f "${json}" ]]; then
      log "  ${name} in ${dir}: $(normalized "${json}")"
    fi
  done
done
clean_target_dir b1-nightly
use_target_dir b1-nightly
TOOLCHAIN=nightly step nightly-thiserror "" rustdoc -p "${THISERROR}" --lib --target "${HOST}" \
  -- -Z unstable-options --output-format json
log "  lines that differ in the member's JSON, the members' value against every name:"
{ diff <(jq -S 'del(.external_crates[].path)' "${WORK}/b1-fixed/target/${HOST}/doc/${MEMBER}.json") \
  <(jq -S 'del(.external_crates[].path)' "${WORK}/b1-all/target/${HOST}/doc/${MEMBER}.json") ||
  true; } | grep -c '^[<>]' | sed 's/^/    /' | tee -a "${LOG}" >&2
cargo +stable run -q --release --example fetch_docs_rs -- "${THISERROR}" >/dev/null
STABLE_JSON="${WORK}/b1-fixed/target/${HOST}/doc/thiserror.json"
NIGHTLY_JSON="${WORK}/b1-nightly/target/${HOST}/doc/thiserror.json"
DOCS_RS_JSON="${HOME}/.cache/chatora-rustdoc-mcp/docs.rs/thiserror/${THISERROR#thiserror@}/rustdoc.json"
for json in "${STABLE_JSON}" "${NIGHTLY_JSON}" "${DOCS_RS_JSON}"; do
  log "  $(jq .format_version "${json}") format, $(jq '.index | length' "${json}") items: ${json}"
done
log "  items that differ, stable + RUSTC_BOOTSTRAP against docs.rs:"
{ diff <(items "${STABLE_JSON}") <(items "${DOCS_RS_JSON}") || true; } | sed 's/^/    /' |
  tee -a "${LOG}" >&2
log "  items that differ, stable + RUSTC_BOOTSTRAP against nightly:"
{ diff <(items "${STABLE_JSON}") <(items "${NIGHTLY_JSON}") || true; } | cut -c1-120 |
  sed 's/^/    /' | tee -a "${LOG}" >&2

log "# done $(date -Iseconds)"
