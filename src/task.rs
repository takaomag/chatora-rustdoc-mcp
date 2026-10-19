//! Helpers for Tokio tasks, shared by all layers.

use tokio::task::JoinError;

/// Resumes the panic of a task that panicked, so that a bug is not reported as an ordinary
/// error, and returns the error of a task that was cancelled.
///
/// Inside a tool call, the resumed panic is caught by the tool's own task and becomes an
/// `internal_error` result (requirement R-ERR-1).
pub(crate) fn resume_if_panic(error: JoinError) -> JoinError {
    match error.try_into_panic() {
        Ok(payload) => std::panic::resume_unwind(payload),
        Err(error) => error,
    }
}
