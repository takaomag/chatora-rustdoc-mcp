#!/usr/bin/env bash
# B7-3 and B7-4 (doc/m0-plan.md §4.2, Q-30): how cargo reports failures that the server has to
# tell apart. Everything runs with isolate_home (empty HOME, git config and CARGO_HOME under
# work/), so the user's settings and credentials are not used. It reaches the network: crates.io,
# github.com, and a host that does not resolve.
#
# B7-3: `cargo metadata` when Cargo.toml is wrong, when the registry or a git host is
#   unreachable, offline without the package in the cache, and for a version that does not exist.
# B7-4: git dependencies whose rev, branch or repository does not exist, with libgit2 and with the
#   git CLI (`net.git-fetch-with-cli`), for `cargo fetch`, `cargo metadata`, `cargo tree` and
#   `cargo update`.
# Each case is a generated package under work/b7f/; the log goes to work/b7f/log.txt.
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

OUT="${WORK}/b7f"
rm -rf "${OUT}"
mkdir -p "${OUT}"
LOG="${OUT}/log.txt"
isolate_home
export LC_ALL=C

log() { printf '%s\n' "$*" | tee -a "${LOG}" >&2; }

# Writes a package in work/b7f/<case> whose [dependencies] table is <dependencies>, and whose
# Cargo.toml gets <extra> appended.
package() {
  local dir="${OUT}/$1"
  mkdir -p "${dir}/src"
  printf '[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\n%s\n%s\n' \
    "$2" "${3:-}" >"${dir}/Cargo.toml"
  printf '//! A probe.\n' >"${dir}/src/lib.rs"
}

# Runs `cargo +stable <args>` in work/b7f/<case> and logs the exit status, the first error line,
# and the first lines after `Caused by:`: `run <case> <label> <args>...`.
run() {
  local case="$1" label="$2"
  shift 2
  local status=0
  (cd "${OUT}/${case}" && cargo +stable "$@") >"${OUT}/${case}.${label}.out" 2>&1 || status=$?
  local error caused
  error="$(grep -m1 -E '^error' "${OUT}/${case}.${label}.out" | cut -c1-160 || true)"
  caused="$(awk '/^Caused by:/ { c = 1; next } c && NF { print; n++ } n == 3 { exit }' \
    "${OUT}/${case}.${label}.out" | sed 's/^ *//' | cut -c1-140 | paste -sd '|' || true)"
  log "  ${case} ${label}: exit ${status}; ${error:-no error}${caused:+; caused by: ${caused}}"
}

log "# B7-3, B7-4 $(date -Iseconds)"
log "cargo: $(cargo +stable -V); git: $(git --version); LC_ALL=${LC_ALL}"

# B7-3: cargo metadata.
log "## B7-3: cargo metadata --format-version 1"
package syntax-error 'itoa = "1"
this is not toml'
package unknown-key 'itoa = { version = "1", no-such-key = true }'
package no-such-version 'itoa = "=999.0.0"'
package git-unreachable 'x = { git = "https://unreachable.invalid/x.git" }'
package registry-ok 'itoa = "1"'
for case in syntax-error unknown-key no-such-version git-unreachable registry-ok; do
  run "${case}" metadata metadata --format-version 1
done
log "  offline, with an empty cache (a CARGO_HOME of its own, since the cases above fill the shared one):"
package offline-uncached 'itoa = "1"'
rm -rf "${WORK}/cargo-home-offline"
CARGO_HOME="${WORK}/cargo-home-offline" run offline-uncached metadata metadata --format-version 1 \
  --offline
log "  the registry unreachable (crates.io replaced by a host that does not resolve):"
package registry-unreachable 'itoa = "1"'
mkdir -p "${OUT}/registry-unreachable/.cargo"
printf '[source.crates-io]\nreplace-with = "unreachable"\n\n[source.unreachable]\nregistry = "sparse+https://unreachable.invalid/index/"\n' \
  >"${OUT}/registry-unreachable/.cargo/config.toml"
run registry-unreachable metadata metadata --format-version 1

# B7-4: git dependencies that cannot be fetched, with libgit2 and with the git CLI.
REPO_URL="https://github.com/dtolnay/itoa"
package git-no-rev "itoa = { git = \"${REPO_URL}\", rev = \"0123456789abcdef0123456789abcdef01234567\" }"
package git-no-branch "itoa = { git = \"${REPO_URL}\", branch = \"no-such-branch-for-b7\" }"
package git-no-repo 'itoa = { git = "https://github.com/dtolnay/no-such-repository-for-b7" }'
package git-no-local-repo 'itoa = { git = "file:///nonexistent/repository" }'
for backend in libgit2 cli; do
  if [[ ${backend} == cli ]]; then
    export CARGO_NET_GIT_FETCH_WITH_CLI=true
  else
    unset CARGO_NET_GIT_FETCH_WITH_CLI
  fi
  log "## B7-4: ${backend}"
  for case in git-no-rev git-no-branch git-no-repo git-no-local-repo; do
    rm -rf "${CARGO_HOME}/git"
    run "${case}" "${backend}.fetch" fetch
    run "${case}" "${backend}.metadata" metadata --format-version 1
    run "${case}" "${backend}.tree" tree
    run "${case}" "${backend}.update" update
  done
done

log "# done $(date -Iseconds)"
