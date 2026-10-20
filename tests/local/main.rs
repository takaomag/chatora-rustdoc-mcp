//! Tests that start cargo, rustup and git, each in an [`isolated::Isolated`] environment, so that
//! they neither use the user's settings and credentials nor reach the network
//! (`doc/m2-plan.md` §2). The projects they build are made in temporary directories, from the
//! test crates under `tests/fixtures/` or in the test itself.

#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

#[cfg(feature = "local")]
mod build_scope;
#[path = "../common/git.rs"]
mod git;
#[cfg(feature = "local")]
mod git_failures;
#[path = "../common/isolated.rs"]
mod isolated;
#[path = "../common/registry.rs"]
mod registry;
mod tools;
#[cfg(feature = "local")]
mod worktree;
