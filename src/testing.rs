//! The hooks of a debug build for the integration tests, which a release build does not have.
//!
//! They are not settings (requirement R-DIST-3), so their environment variables are not under the
//! settings' prefix.

use std::sync::OnceLock;

/// The environment variable that turns the hooks on: the tool for testing (`sleep`) and the
/// others below. The integration tests set it to `1`; a debug build installed for use (`cargo
/// install --debug`), where it is not set, has them off.
const TOOLS_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_TOOLS";

/// The environment variable that, set to `1` with [`TOOLS_ENV_VAR`], makes every format of the
/// cache one version newer, as a server of a later release would write it, for the tests of
/// servers of different versions on one cache (decided in M3a, A4-1 and A4-2).
const FORMAT_BUMP_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_FORMAT_BUMP";

/// The environment variable that, with [`TOOLS_ENV_VAR`], sets the most crates that a lookup
/// across the dependency graph reads, in place of 300, for the tests of those past it (decided
/// in M4b).
#[cfg(feature = "local")]
const MAX_CRATES_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_MAX_CRATES";

/// Whether the hooks are on: `CHATORA_RUSTDOC_TEST_TOOLS` is `1`.
#[must_use]
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| is_one(TOOLS_ENV_VAR))
}

/// Whether the formats of the cache are one version newer: `CHATORA_RUSTDOC_TEST_FORMAT_BUMP` is
/// `1` with the hooks on.
#[must_use]
pub fn format_bump() -> bool {
    static BUMP: OnceLock<bool> = OnceLock::new();
    *BUMP.get_or_init(|| on() && is_one(FORMAT_BUMP_ENV_VAR))
}

/// The most crates that a lookup across the dependency graph reads, if
/// `CHATORA_RUSTDOC_TEST_MAX_CRATES` sets it with the hooks on.
#[cfg(feature = "local")]
#[must_use]
pub fn max_crates() -> Option<usize> {
    static MAX: OnceLock<Option<usize>> = OnceLock::new();
    *MAX.get_or_init(|| {
        on().then(|| std::env::var(MAX_CRATES_ENV_VAR).ok()?.parse().ok())
            .flatten()
    })
}

/// Whether the environment variable `name` is `1`.
fn is_one(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| value == "1")
}
