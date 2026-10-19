#!/usr/bin/env bash
# B2 (doc/m0-plan.md §4.2, Q-17): two versions of one crate (itoa 0.4 and 1, both normal
# dependencies of dup-ws/) with `cargo doc --output-format json` and with `cargo rustdoc`.
# Prints which JSON files each way leaves, and the version in each.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

cd "$(dirname "${BASH_SOURCE[0]}")/dup-ws"
HOST="$(rustc +stable -vV | sed -n 's/^host: //p')"

show() {
  local f
  for f in $({ find "${CARGO_TARGET_DIR}" "${CARGO_BUILD_BUILD_DIR}" -name 'itoa.json' 2>/dev/null || true; }); do
    echo "  ${f#"${WORK}/"}: $(jq -r .crate_version "${f}")"
  done
}

echo "## cargo doc, every dependency"
clean_target_dir b2-dup
use_target_dir b2-dup
cargo +nightly doc -Z unstable-options --output-format json --target "${HOST}" -q 2>&1 | tail -n3
show

echo "## cargo doc -p itoa@0.4.8 -p itoa@1.0.18 --no-deps"
clean_target_dir b2-dup
use_target_dir b2-dup
cargo +nightly doc -Z unstable-options --output-format json -p itoa@0.4.8 -p itoa@1.0.18 \
  --no-deps --target "${HOST}" -q 2>&1 | head -n2 || true
show

echo "## cargo rustdoc -p itoa@0.4.8, then -p itoa@1.0.18, in one target dir"
clean_target_dir b2-dup
use_target_dir b2-dup
for version in 0.4.8 1.0.18; do
  cargo +nightly rustdoc -p "itoa@${version}" --lib --target "${HOST}" -q \
    -- -Z unstable-options --output-format json
  echo "after itoa@${version}:"
  show
done
