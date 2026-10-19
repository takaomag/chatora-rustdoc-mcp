#!/usr/bin/env bash
# B3, B6, B7-2 and B7-9 (doc/m0-plan.md §4.2) on ws/, whose only member `app` uses deps/shared
# with a different feature as a normal (`normal`), Windows-only (`windows`), build (`build`) and
# dev (`dev`) dependency, and has dependencies of each kind (deps/).
#
# B3, B7-2 (Q-20): the features that `cargo tree --workspace --target <t> -e normal` shows,
#   against the features of the units that `cargo check` and `cargo check --all-targets` build.
# B6 (R-MCP-3): what `cargo tree` shows for each kind of edge, on stable and on nightly, and the
#   nightly `--unit-graph` (package ids, host or target).
# B7-9 (Q-30): `cargo rustdoc -p` on each kind of dependency, with resolver 1 and 2.
#
# ws/ is copied to work/b3/r1 and work/b3/r2 with `resolver = "1"` and `"2"` and absolute paths to
# deps/. The log goes to work/b3/log.txt, the full outputs next to it.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b3"
rm -rf "${OUT}"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"
OTHER="x86_64-pc-windows-gnu"
DEPS="${M0B}/b3-features/deps"
export RUSTC_BOOTSTRAP="app,shared,devonly,buildonly,winonly,pm,pmdep,outside"

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }
# Logs the output of a command, indented, and keeps it in work/b3/<name>.
show() {
  local name="$1"
  shift
  "$@" >"${OUT}/${name}" 2>&1 || true
  sed 's/^/    /' "${OUT}/${name}" | tee -a "${LOG}" >&2
}
# The features of the units of `shared` that a cargo build reports, one line per unit, with
# whether it is built for the host (no target directory in its output path) or the target.
shared_units() {
  jq -r --arg host "${HOST}" 'select(.reason == "compiler-artifact" and .target.name == "shared")
    | "\(if any(.filenames[]; contains("/" + $host + "/")) then "target" else "host" end): \(.features)"' |
    sort
}

log "# B3, B6, B7-2, B7-9 $(date -Iseconds)"
log "stable: $(cargo +stable -V); nightly: $(cargo +nightly -V)"

for resolver in 2 1; do
  ws="${OUT}/r${resolver}"
  cp -r "${M0B}/b3-features/ws" "${ws}"
  sed -i "s/^resolver = .*/resolver = \"${resolver}\"/" "${ws}/Cargo.toml"
  sed -i "s|\\.\\./\\.\\./deps/|${DEPS}/|" "${ws}/app/Cargo.toml"
  rm -f "${ws}/Cargo.lock"
  cd "${ws}"
  clean_target_dir "b3-r${resolver}"
  use_target_dir "b3-r${resolver}"
  log "## resolver ${resolver}"

  # B3, B7-2: features in cargo tree against the units that cargo builds.
  for target in "${HOST}" "${OTHER}"; do
    log "  cargo tree --workspace --target ${target} -e normal -f '{p} [{f}]':"
    show "r${resolver}.tree-normal.${target}" cargo +stable tree --workspace --target "${target}" \
      -e normal --prefix depth -f '{p} [{f}]'
  done
  log "  cargo tree --workspace --target ${HOST} -e normal,dev:"
  show "r${resolver}.tree-normal-dev" cargo +stable tree --workspace --target "${HOST}" \
    -e normal,dev --prefix depth -f '{p} [{f}]'
  log "  cargo tree --workspace --target ${HOST} -e build:"
  show "r${resolver}.tree-build" cargo +stable tree --workspace --target "${HOST}" -e build \
    --prefix depth -f '{p} [{f}]'
  log "  units of shared, cargo check --workspace --target ${HOST}:"
  cargo +stable check -q --workspace --target "${HOST}" --message-format=json 2>/dev/null |
    shared_units | sed 's/^/    /' | tee -a "${LOG}" >&2
  log "  units of shared, cargo check --workspace --all-targets --target ${HOST}:"
  cargo +stable check -q --workspace --all-targets --target "${HOST}" --message-format=json \
    2>/dev/null | shared_units | sed 's/^/    /' | tee -a "${LOG}" >&2

  log "  units of shared, cargo rustdoc -p app --lib --target ${HOST} (how the member is documented):"
  cargo +stable rustdoc -q -p app --lib --target "${HOST}" --message-format=json \
    -- -Z unstable-options --output-format json 2>/dev/null | shared_units | sed 's/^/    /' |
    tee -a "${LOG}" >&2

  # B6: the same tree on nightly, and nightly's unit graph.
  if diff <(cargo +stable tree --workspace --target "${HOST}" -e all --prefix depth -f '{p} [{f}]') \
    <(cargo +nightly tree --workspace --target "${HOST}" -e all --prefix depth -f '{p} [{f}]') \
    >"${OUT}/r${resolver}.tree-stable-vs-nightly.diff"; then
    log "  cargo tree -e all: the same on stable and nightly"
  else
    log "  cargo tree -e all: differs between stable and nightly (r${resolver}.tree-stable-vs-nightly.diff)"
  fi
  log "  {lib} and {r} placeholders, stable:"
  show "r${resolver}.tree-lib" cargo +stable tree --workspace --target "${HOST}" -e normal \
    --prefix none -f '{p} lib={lib} repo={r}'
  log "  nightly --unit-graph of cargo check --all-targets, units of shared:"
  cargo +nightly check -q --workspace --all-targets --target "${HOST}" --unit-graph \
    -Z unstable-options 2>/dev/null >"${OUT}/r${resolver}.unit-graph.json" || true
  jq -r '.units[] | select(.pkg_id | contains("shared"))
    | "    \(.pkg_id) mode=\(.mode) platform=\(.platform // "host") features=\(.features)"' \
    "${OUT}/r${resolver}.unit-graph.json" | sort | tee -a "${LOG}" >&2

  # B7-9: cargo rustdoc -p on each kind of dependency.
  log "  cargo rustdoc -p <dependency> --lib --target ${HOST}:"
  for dep in shared devonly buildonly winonly pm pmdep outside; do
    if cargo +stable rustdoc -q -p "${dep}" --lib --target "${HOST}" \
      -- -Z unstable-options --output-format json >"${OUT}/r${resolver}.rustdoc-${dep}.out" 2>&1; then
      log "    ${dep}: ok"
    else
      log "    ${dep}: $(grep -m1 -E '^error|panicked at|did not find' "${OUT}/r${resolver}.rustdoc-${dep}.out" |
        cut -c1-160)"
    fi
  done
  log "  cargo rustdoc -p winonly --lib --target ${OTHER}:"
  if cargo +stable rustdoc -q -p winonly --lib --target "${OTHER}" \
    -- -Z unstable-options --output-format json >"${OUT}/r${resolver}.rustdoc-winonly-other.out" 2>&1; then
    log "    ok"
  else
    log "    $(grep -m1 -E '^error|panicked at' "${OUT}/r${resolver}.rustdoc-winonly-other.out" | cut -c1-160)"
  fi
done

log "# done $(date -Iseconds)"
