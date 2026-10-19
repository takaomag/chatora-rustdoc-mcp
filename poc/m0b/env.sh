# shellcheck shell=bash
# shellcheck disable=SC2034 # REPO, ELAPSED and STATUS are for the scripts that source this file.
# Shared settings for the M0b experiments (doc/m0-plan.md §4). Source it from an experiment
# script: `. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"`.
#
# Nothing here changes the host: toolchains are never installed, and build output goes under
# poc/m0b/work/, not the user's target/.

set -euo pipefail

M0B="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "${M0B}/../.." && pwd)"
WORK="${M0B}/work"
mkdir -p "${WORK}"

# Never let `cargo +toolchain` or rust-toolchain.toml install a toolchain.
export RUSTUP_AUTO_INSTALL=0
export CARGO_TERM_COLOR=never

# Points the build output of the next cargo commands at work/<name>/ (requirement R-WS-2).
use_target_dir() {
  export CARGO_TARGET_DIR="${WORK}/$1/target"
  export CARGO_BUILD_BUILD_DIR="${WORK}/$1/build"
}

# Removes the build output of work/<name>/, for a cold build.
clean_target_dir() {
  rm -rf "${WORK:?}/$1"
}

# Keeps the user's git and cargo settings and credentials out of the next commands (for the
# experiments that fetch from git or a registry). The cargo home starts empty.
isolate_home() {
  # rustup finds its toolchains through HOME unless RUSTUP_HOME is set, so keep it pointing at
  # the real one before HOME moves.
  RUSTUP_HOME="${RUSTUP_HOME:-$(rustup show home)}"
  export RUSTUP_HOME
  export HOME="${WORK}/home"
  export XDG_CONFIG_HOME="${HOME}/.config"
  export CARGO_HOME="${WORK}/cargo-home"
  export GIT_CONFIG_NOSYSTEM=1
  # Never ask for credentials: not on the terminal, and not through a GUI askpass program on the
  # user's desktop (git runs SSH_ASKPASS when GitHub asks for credentials, even for a repository
  # that does not exist).
  export GIT_TERMINAL_PROMPT=0
  unset SSH_ASKPASS GIT_ASKPASS
  export SSH_ASKPASS_REQUIRE=never
  export GCM_INTERACTIVE=never
  mkdir -p "${HOME}" "${XDG_CONFIG_HOME}" "${CARGO_HOME}"
}

# Runs a command and sets ELAPSED to how long it took, in seconds, and STATUS to its exit status:
# `timed <cmd>...`. It always succeeds, so that `set -e` does not stop at a failed command.
timed() {
  local start end
  STATUS=0
  start="$(date +%s.%N)"
  "$@" || STATUS=$?
  end="$(date +%s.%N)"
  ELAPSED="$(awk -v s="${start}" -v e="${end}" 'BEGIN { printf "%.2f", e - s }')"
}
