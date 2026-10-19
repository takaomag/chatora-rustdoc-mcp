#!/usr/bin/env bash
# B7-1, B7-6, B7-7 and B7-8 (doc/m0-plan.md §4.2, Q-30) on ws/: the member `lib` has items that
# show whether `--cfg foo` from RUSTFLAGS reached the crate (`lib_saw_foo`), its build script
# (`build_saw_foo`) and its proc-macro `foomacro` (`macro_saw_foo`).
#
# B7-1: RUSTFLAGS with and without `--target <host>`.
# B7-6: a target whose rust-std is not installed (nightly has only the host here).
# B7-7: RUSTC or RUSTDOC from another toolchain, and a toolchain's cargo run without rustup.
# B7-8: a target JSON spec in `build.target`.
# The log goes to work/b7/log.txt.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b7"
rm -rf "${OUT}"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
WS="${M0B}/b7-toolchain/ws"
HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"
MISSING="aarch64-unknown-linux-gnu" # installed for stable, not for nightly
JSON_ARGS=(-Z unstable-options --output-format json)
export RUSTC_BOOTSTRAP="lib,foomacro"

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }
# The items of `lib` whose names end in `_saw_foo`, from its JSON.
saw() { jq -r '[.index[] | .name // empty | select(endswith("_saw_foo"))] | sort | join(" ")' "$1"; }
# Runs a command, and logs "ok" or its first error line: `attempt <label> <cmd>...`.
attempt() {
  local label="$1"
  shift
  if "$@" >"${OUT}/${label}.out" 2>&1; then
    log "  ${label}: ok"
  else
    log "  ${label}: $(grep -m1 -E '^error|panicked at|^fatal' "${OUT}/${label}.out" | cut -c1-200)"
  fi
}

log "# B7 $(date -Iseconds)"
log "stable: $(rustc +stable -V); nightly: $(rustc +nightly -V)"
cd "${WS}"

# B7-1: where RUSTFLAGS reaches, with and without --target.
log "## B7-1: RUSTFLAGS=--cfg foo"
for with_target in no yes; do
  clean_target_dir "b7-rustflags-${with_target}"
  use_target_dir "b7-rustflags-${with_target}"
  target_args=()
  json="${CARGO_TARGET_DIR}/doc/lib.json"
  if [[ ${with_target} == yes ]]; then
    target_args=(--target "${HOST}")
    json="${CARGO_TARGET_DIR}/${HOST}/doc/lib.json"
  fi
  RUSTFLAGS="--cfg foo" cargo +stable rustdoc -q -p lib --lib "${target_args[@]}" \
    -- "${JSON_ARGS[@]}"
  log "  --target ${with_target}: items $(saw "${json}")"
done
clean_target_dir b7-rustflags-none
use_target_dir b7-rustflags-none
cargo +stable rustdoc -q -p lib --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
log "  no RUSTFLAGS: items $(saw "${CARGO_TARGET_DIR}/${HOST}/doc/lib.json")"

# B7-6: a target without rust-std.
log "## B7-6: ${MISSING}"
for toolchain in stable nightly; do
  sysroot="$(rustc "+${toolchain}" --print sysroot)"
  log "  ${toolchain}: lib/rustlib/${MISSING} $([[ -d "${sysroot}/lib/rustlib/${MISSING}" ]] &&
    echo exists || echo 'does not exist')"
  clean_target_dir "b7-missing-${toolchain}"
  use_target_dir "b7-missing-${toolchain}"
  attempt "missing-${toolchain}" cargo "+${toolchain}" rustdoc -p lib --lib --target "${MISSING}" \
    -- "${JSON_ARGS[@]}"
done
log "  rustup's list for nightly: $(rustup +nightly target list --installed | tr '\n' ' ')"

# B7-7: executables from different toolchains, and no rustup.
log "## B7-7"
NIGHTLY_BIN="$(rustc +nightly --print sysroot)/bin"
STABLE_BIN="$(rustc +stable --print sysroot)/bin"
clean_target_dir b7-mixed
use_target_dir b7-mixed
attempt "stable-cargo-and-rustc-nightly-rustdoc" env RUSTDOC="${NIGHTLY_BIN}/rustdoc" \
  cargo +stable rustdoc -p lib --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
clean_target_dir b7-mixed
attempt "stable-cargo-and-rustdoc-nightly-rustc" env RUSTC="${NIGHTLY_BIN}/rustc" \
  RUSTDOC="${STABLE_BIN}/rustdoc" cargo +stable rustdoc -p lib --lib --target "${HOST}" \
  -- "${JSON_ARGS[@]}"
clean_target_dir b7-mixed
attempt "nightly-cargo-with-stable-rustc-and-rustdoc" env RUSTC="${STABLE_BIN}/rustc" \
  RUSTDOC="${STABLE_BIN}/rustdoc" cargo +nightly rustdoc -p lib --lib --target "${HOST}" \
  -- "${JSON_ARGS[@]}"
# No rustup: only the toolchain's bin directory and the system's on PATH, and its cargo by path.
clean_target_dir b7-no-rustup
use_target_dir b7-no-rustup
log "  without rustup on PATH:"
log "    rustc on PATH: $(env -u RUSTUP_TOOLCHAIN PATH="${STABLE_BIN}:/usr/bin:/bin" bash -c \
  'command -v rustc; rustc -V' | tr '\n' ' ')"
attempt "no-rustup" env -u RUSTUP_TOOLCHAIN PATH="${STABLE_BIN}:/usr/bin:/bin" \
  "${STABLE_BIN}/cargo" rustdoc -p lib --lib --target "${HOST}" -- "${JSON_ARGS[@]}"
log "    format_version: $(jq .format_version "${CARGO_TARGET_DIR}/${HOST}/doc/lib.json" 2>/dev/null ||
  echo none)"

# B7-8: a target JSON spec in build.target, in a copy of ws.
log "## B7-8: build.target = <a target JSON spec>"
custom="${OUT}/ws-custom"
cp -r "${WS}" "${custom}"
rustc +nightly -Z unstable-options --print target-spec-json --target "${HOST}" \
  >"${OUT}/custom.json"
mkdir -p "${custom}/.cargo"
printf '[build]\ntarget = "%s"\n' "${OUT}/custom.json" >"${custom}/.cargo/config.toml"
cd "${custom}"
for toolchain in stable nightly; do
  clean_target_dir "b7-custom-${toolchain}"
  use_target_dir "b7-custom-${toolchain}"
  attempt "custom-tree-${toolchain}" cargo "+${toolchain}" tree --workspace -e normal
  if [[ -s "${OUT}/custom-tree-${toolchain}.out" ]]; then
    log "    $(head -n1 "${OUT}/custom-tree-${toolchain}.out")"
  fi
  attempt "custom-rustdoc-${toolchain}" cargo "+${toolchain}" rustdoc -p lib --lib \
    -- "${JSON_ARGS[@]}"
done

log "  with [unstable] json-target-spec = true in the config:"
printf '\n[unstable]\njson-target-spec = true\n' >>"${custom}/.cargo/config.toml"
for toolchain in stable nightly; do
  use_target_dir "b7-custom-${toolchain}"
  attempt "custom-unstable-tree-${toolchain}" cargo "+${toolchain}" tree --workspace -e normal
  attempt "custom-unstable-rustdoc-${toolchain}" cargo "+${toolchain}" rustdoc -p lib --lib \
    -- "${JSON_ARGS[@]}"
done
use_target_dir b7-custom-stable
attempt "custom-unstable-tree-stable-bootstrap-1" env RUSTC_BOOTSTRAP=1 cargo +stable tree \
  --workspace -e normal

log "# done $(date -Iseconds)"
