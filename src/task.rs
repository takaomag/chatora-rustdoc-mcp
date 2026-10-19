//! Helpers for Tokio tasks, shared by all layers.

use tokio::task::JoinError;

/// The stack size of the threads of the Tokio runtime, its workers and its blocking threads.
///
/// Rendering and freeing a parsed crate recurse as deep as its types nest, up to
/// [`crate::data::rustdoc_json::MAX_NESTING`] levels of JSON, which needs more than the default
/// 2 MiB (requirement R-PR-8).
pub const THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

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
