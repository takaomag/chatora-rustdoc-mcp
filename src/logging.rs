//! Logging to stderr. While serving, stdout carries only JSON-RPC messages (requirement R-SEC-1).

use tracing_subscriber::{
    filter::{EnvFilter, LevelFilter},
    util::{SubscriberInitExt as _, TryInitError},
};

/// Environment variable that filters the log, in the `EnvFilter` syntax (setting `log.filter`).
const FILTER_ENV_VAR: &str = "CHATORA_RUSTDOC_LOG__FILTER";

/// Installs the global subscriber that writes the log to stderr.
///
/// Invalid directives in the filter are ignored instead of stopping the server.
///
/// # Errors
///
/// Returns an error when a global subscriber is already installed.
pub fn init() -> Result<(), TryInitError> {
    let filter = EnvFilter::builder()
        .with_env_var(FILTER_ENV_VAR)
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    tracing_subscriber::fmt()
        // The default writer is stdout, which would corrupt JSON-RPC.
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .finish()
        .try_init()
}
