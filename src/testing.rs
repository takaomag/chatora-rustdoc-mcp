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

/// Whether the environment variable `name` is `1`.
fn is_one(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| value == "1")
}
