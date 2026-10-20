//! The MCP tools: which tools there are, how a `tools/call` reaches one, and what they share.
//!
//! Each tool is a module with `NAME`, `definition()`, `parse_arguments()` when its arguments need
//! checking, and `execute()`.

mod call_log;
#[cfg(test)]
mod checks;
mod crate_overview;
mod get_item;
#[cfg(feature = "local")]
mod list_crates;
mod list_implementors;
mod list_impls;
mod list_methods;
mod list_module;
mod project;
mod resolve_version;
mod search_across;
mod search_items;
#[cfg(debug_assertions)]
mod sleep;

use std::{borrow::Cow, sync::Arc};

use rmcp::{
    model::{
        CallToolRequestParams,
        CallToolResponse,
        CallToolResult,
        ContentBlock,
        ErrorData,
        Tool,
        ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Value, json};
use tokio::{
    sync::{OnceCell, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use tracing::Instrument as _;

use crate::data::{
    cache::Cache,
    crate_file::StaticCrates,
    dist::Dist,
    docs_rs::DocsRs,
    fetches::Fetches,
    sparse_index::SparseIndex,
    target::{self, HostTarget},
};
#[cfg(feature = "local")]
use crate::data::{metadata::Resolver, project::DefaultProject};

/// What the tools share for the life of the server.
#[derive(Debug)]
pub(super) struct ToolState {
    /// Where rustdoc JSON is cached.
    pub(super) cache: Cache,
    /// The docs.rs client.
    pub(super) docs_rs: DocsRs,
    /// The client of the Rust distribution server, for std (requirement R-STD-6).
    pub(super) dist: Dist,
    /// The crates.io sparse index, for the versions of crates.
    pub(super) index: SparseIndex,
    /// static.crates.io, for the `.crate` of releases (requirement R-FT-1).
    pub(super) static_crates: StaticCrates,
    /// The host target, the default target of the documentation (requirement R-TG-1). It is
    /// found in the background from startup, as it runs rustc, so that the server answers at once
    /// (requirement R-MCP-11); see [`Self::host`].
    pub(super) host: OnceCell<HostTarget>,
    /// The default project, decided at startup (requirement R-MCP-3).
    #[cfg(feature = "local")]
    pub(super) project: Option<DefaultProject>,
    /// Resolves the packages of projects with `cargo metadata` (requirement R-VR-2).
    #[cfg(feature = "local")]
    pub(super) resolver: Resolver,
    /// The calls being handled, at most `limits.concurrent_calls` (requirement R-SEC-7).
    pub(super) calls: CallLimit,
}

impl ToolState {
    /// The host target, from `rustc -vV` in the cache directory, found once. The first call waits
    /// for it, which `serve` starts at startup.
    pub(super) async fn host(&self) -> &HostTarget {
        self.host
            .get_or_init(|| target::host_target(self.cache.dir()))
            .await
    }
}

/// The most tool calls handled at once (requirement R-SEC-7). Over it, a call gets `busy`.
#[derive(Debug)]
pub(super) struct CallLimit {
    permits: Arc<Semaphore>,
    max: usize,
}

impl CallLimit {
    /// `limits.concurrent_calls` unless set (requirement R-SEC-7, initial value).
    const DEFAULT: usize = 16;
    /// Environment variable of `limits.concurrent_calls` (requirement R-DIST-3).
    const ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_LIMITS__CONCURRENT_CALLS";

    /// The limit that [`Self::ENV_VAR`] sets, or the default. A value that is not a whole number
    /// from 1 is logged and replaced by the default.
    pub(super) fn from_env() -> Self {
        let max = std::env::var_os(Self::ENV_VAR).map_or(Self::DEFAULT, |value| {
            value
                .to_str()
                .and_then(|value| value.trim().parse().ok())
                .filter(|max| *max > 0)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        "{} is not valid ({}), so the default {} is used",
                        Self::ENV_VAR,
                        value.to_string_lossy(),
                        Self::DEFAULT
                    );
                    Self::DEFAULT
                })
        });
        Self::new(max)
    }

    fn new(max: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max.min(Semaphore::MAX_PERMITS))),
            max,
        }
    }

    /// A place among the calls being handled, or `None` when there is none.
    fn enter(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).try_acquire_owned().ok()
    }

    /// The error of a call that found no place (requirements R-SEC-7 and R-ERR-2).
    fn busy(&self) -> CallToolResult {
        error_result(&json!({
            "code": "busy",
            "message": format!(
                "the server is handling as many calls as it can at once ({})",
                self.max
            ),
            "retryable": true,
            "suggestions": ["call again in a moment, or make fewer calls at once"],
            "retry_after_secs": BUSY_RETRY_AFTER_SECS,
        }))
    }
}

/// How long a `busy` call is told to wait: lookups take well under a second (requirement R-ERR-2).
const BUSY_RETRY_AFTER_SECS: u64 = 1;

/// An error and its sources, joined with `: `.
pub(super) struct ErrorChain<'a>(&'a dyn std::error::Error);

impl std::fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        for source in std::iter::successors(self.0.source(), |&source| source.source()) {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

/// Definitions of every tool, in the order `tools/list` returns them.
pub(super) fn definitions() -> Vec<Tool> {
    let tools = vec![
        crate_overview::definition(),
        get_item::definition(),
        // A project is needed, which only local features read (requirement R-ARCH-4).
        #[cfg(feature = "local")]
        list_crates::definition(),
        list_implementors::definition(),
        list_impls::definition(),
        list_methods::definition(),
        list_module::definition(),
        resolve_version::definition(),
        search_items::definition(),
        // A tool for testing progress and cancellation, only in debug builds.
        #[cfg(debug_assertions)]
        sleep::definition(),
    ];
    if cfg!(feature = "local") {
        tools
    } else {
        tools.into_iter().map(without_project).collect()
    }
}

/// The end of every tool's description of `project` (doc/m2-plan.md §5, P5): the server sees only
/// the directory that Claude Code was opened in, not the worktree of a subagent, so one that works
/// in a worktree has to pass it.
const PROJECT_IN_A_WORKTREE: &str = " In another git worktree of the project, such as a \
                                     subagent's, pass the worktree's directory.";

/// `tool` without the `project` argument, which does nothing without local features: no argument
/// is declared that is not used (requirements R-ARCH-3 and R-QA-5).
fn without_project(mut tool: Tool) -> Tool {
    let mut schema = (*tool.input_schema).clone();
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        // `remove` would swap the last argument into its place.
        properties.shift_remove("project");
    }
    tool.input_schema = Arc::new(schema);
    tool
}

/// The annotations of every tool (requirement R-MCP-4). With local features, each may run cargo
/// or rustc on the project (`cargo metadata`, `rustc -vV`, and builds from M2), whose settings
/// can run any program, so none is read-only. Without them, none runs anything.
fn annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(!cfg!(feature = "local"))
        .destructive(false)
        .idempotent(true)
        .open_world(true)
}

/// Runs the tool that `request` names, and logs the call with a new `request_id`, which an error
/// carries too (requirements R-OBS-1 and R-ERR-2).
///
/// # Errors
///
/// Returns a JSON-RPC error only for an unknown tool. Bad arguments are tool errors in the
/// result (requirement R-ARG-1).
pub(super) async fn call(
    request: CallToolRequestParams,
    context: RequestContext<RoleServer>,
    state: &Arc<ToolState>,
) -> Result<CallToolResponse, ErrorData> {
    let call = call_log::Call::start(&request.name, request.arguments.as_ref());
    let Some(entered) = state.calls.enter() else {
        let mut busy = state.calls.busy();
        call.finish(&mut busy);
        return Ok(busy.into());
    };
    let cancelled = context.ct.clone();
    let span = call.span();
    let running = call
        .fetches()
        .scope(dispatch(request, context, state).instrument(span));
    // On `notifications/cancelled`, the call stops: its task is aborted, and with it what it was
    // fetching, unless another call waits for the same (requirement R-WS-6). rmcp sends no
    // response to a cancelled request.
    let called = tokio::select! {
        called = running => called,
        () = cancelled.cancelled() => {
            let mut result = tool_error("cancelled", "the call was cancelled", false, &[]);
            call.finish(&mut result);
            return Ok(result.into());
        },
    };
    drop(entered);
    match called {
        Ok(mut result) => {
            call.finish(&mut result);
            Ok(result.into())
        },
        Err(error) => {
            call.finish_rejected(&error);
            Err(error)
        },
    }
}

/// Runs the tool that `request` names. See [`call`].
async fn dispatch(
    request: CallToolRequestParams,
    context: RequestContext<RoleServer>,
    state: &Arc<ToolState>,
) -> Result<CallToolResult, ErrorData> {
    match request.name.as_ref() {
        get_item::NAME => match get_item::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => Ok(run_tool(get_item::execute(arguments, Arc::clone(state))).await),
            Err(error) => Ok(error.into_result()),
        },
        crate_overview::NAME => match crate_overview::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => {
                Ok(run_tool(crate_overview::execute(arguments, Arc::clone(state))).await)
            },
            Err(error) => Ok(error.into_result()),
        },
        #[cfg(feature = "local")]
        list_crates::NAME => match list_crates::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => Ok(run_tool(list_crates::execute(arguments, Arc::clone(state))).await),
            Err(error) => Ok(error.into_result()),
        },
        list_implementors::NAME => {
            match list_implementors::parse_arguments(request.arguments.as_ref()) {
                Ok(arguments) => {
                    Ok(run_tool(list_implementors::execute(arguments, Arc::clone(state))).await)
                },
                Err(error) => Ok(error.into_result()),
            }
        },
        list_impls::NAME => match list_impls::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => Ok(run_tool(list_impls::execute(arguments, Arc::clone(state))).await),
            Err(error) => Ok(error.into_result()),
        },
        list_methods::NAME => match list_methods::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => {
                Ok(run_tool(list_methods::execute(arguments, Arc::clone(state))).await)
            },
            Err(error) => Ok(error.into_result()),
        },
        list_module::NAME => match list_module::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => Ok(run_tool(list_module::execute(arguments, Arc::clone(state))).await),
            Err(error) => Ok(error.into_result()),
        },
        search_items::NAME => match search_items::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => {
                Ok(run_tool(search_items::execute(arguments, Arc::clone(state))).await)
            },
            Err(error) => Ok(error.into_result()),
        },
        resolve_version::NAME => match resolve_version::parse_arguments(request.arguments.as_ref())
        {
            Ok(arguments) => {
                Ok(run_tool(resolve_version::execute(arguments, Arc::clone(state))).await)
            },
            Err(error) => Ok(error.into_result()),
        },
        #[cfg(debug_assertions)]
        sleep::NAME => match sleep::parse_arguments(request.arguments.as_ref()) {
            Ok(seconds) => Ok(run_tool(sleep::execute(seconds, context)).await),
            Err(error) => Ok(error.into_result()),
        },
        name => {
            // Only `sleep` uses the context, and release builds have no `sleep`.
            drop(context);
            Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            ))
        },
    }
}

/// Runs a tool in a task of its own, so that a panic becomes an `internal_error` result
/// (requirement R-ERR-1).
///
/// rmcp runs each handler in a detached task and sends no response if it panics, which would
/// leave the client waiting until it times out.
async fn run_tool<F>(future: F) -> CallToolResult
where
    F: Future<Output = CallToolResult> + Send + 'static,
{
    // Dropping this future, as a cancelled call does, aborts the task.
    match AbortOnDrop::new(spawn(future)).await {
        Ok(result) => result,
        Err(error) => {
            // The panic hook has already written the panic message to stderr.
            tracing::error!(%error, "a tool task failed");
            tool_error(
                "internal_error",
                "the server hit an internal error (a bug), so this call did not complete",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            )
        },
    }
}

/// Spawns `future` as a task of the current tool call: in its span, and noting its fetches
/// (requirement R-OBS-1). Tokio's own `spawn` drops both.
fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let span = tracing::Span::current();
    match Fetches::current() {
        Some(fetches) => tokio::spawn(fetches.scope(future).instrument(span)),
        None => tokio::spawn(future.instrument(span)),
    }
}

/// A task that is aborted when this is dropped before it finishes. Tokio's `JoinHandle` lets the
/// task go on.
pub(super) struct AbortOnDrop<T>(Option<JoinHandle<T>>);

impl<T> AbortOnDrop<T> {
    pub(super) const fn new(handle: JoinHandle<T>) -> Self {
        Self(Some(handle))
    }

    /// Lets the task go on after this is dropped: it fills the cache for later calls.
    #[cfg(feature = "local")]
    pub(super) fn detach(mut self) {
        drop(self.0.take());
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // Only `detach` takes the handle, and it consumes `self`.
        self.0.as_mut().map_or(std::task::Poll::Pending, |handle| {
            std::pin::Pin::new(handle).poll(context)
        })
    }
}

/// Arguments that a tool cannot use (requirement R-ARG-1).
///
/// Parsing returns this instead of a `CallToolResult`, which is too large to return in an `Err`
/// (`clippy::result_large_err`); `call` turns it into the `invalid_argument` tool error.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InvalidArgument {
    /// Most messages are fixed text; a few include what was wrong.
    message: Cow<'static, str>,
    suggestions: &'static [&'static str],
}

impl InvalidArgument {
    fn new(message: impl Into<Cow<'static, str>>, suggestions: &'static [&'static str]) -> Self {
        Self {
            message: message.into(),
            suggestions,
        }
    }

    /// The `invalid_argument` tool error.
    fn into_result(self) -> CallToolResult {
        tool_error("invalid_argument", &self.message, false, self.suggestions)
    }
}

/// Builds a tool error whose `content[0].text` is self-contained JSON (requirement R-ERR-2).
fn tool_error(code: &str, message: &str, retryable: bool, suggestions: &[&str]) -> CallToolResult {
    error_result(&json!({
        "code": code,
        "message": message,
        "retryable": retryable,
        "suggestions": suggestions,
    }))
}

/// A tool error with `body` as the JSON of its text (requirement R-ERR-2). `body` has `code`,
/// `message`, `retryable` and `suggestions`, and may have more.
///
/// It has no `structuredContent`: Claude Code ignores it on errors (requirement R-OUT-1).
fn error_result(body: &Value) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(body.to_string())])
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Returns the JSON in `content[0].text` of a tool result.
    fn text_json(result: &CallToolResult) -> Value {
        let text = &result.content[0]
            .as_text()
            .expect("the first content block is text")
            .text;
        serde_json::from_str(text).expect("the text is JSON")
    }

    #[tokio::test]
    async fn run_tool_turns_a_panic_into_internal_error() {
        let result = run_tool(async { panic!("a tool panicked on purpose") }).await;
        assert_eq!(result.is_error, Some(true));
        assert!(result.structured_content.is_none());
        assert_eq!(text_json(&result)["code"], "internal_error");
    }

    #[tokio::test]
    async fn dropping_run_tool_aborts_the_task_of_the_tool() {
        /// Says when it is dropped, as the task's future is when the task is aborted.
        struct Dropped(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    sender.send(()).unwrap_or_default();
                }
            }
        }
        let (sender, dropped) = tokio::sync::oneshot::channel();
        let (started_sender, started) = tokio::sync::oneshot::channel::<()>();
        let running = tokio::spawn(run_tool(async move {
            let _dropped = Dropped(Some(sender));
            started_sender.send(()).unwrap_or_default();
            std::future::pending::<CallToolResult>().await
        }));
        started.await.expect("the tool started");
        running.abort();
        tokio::time::timeout(Duration::from_secs(10), dropped)
            .await
            .expect("the tool's task was aborted")
            .expect("dropped once");
    }

    #[test]
    fn calls_over_the_limit_are_busy_until_one_leaves() {
        let limit = CallLimit::new(2);
        let first = limit.enter().expect("a place");
        let _second = limit.enter().expect("a place");
        assert!(limit.enter().is_none());
        let busy = text_json(&limit.busy());
        assert_eq!(busy["code"], "busy");
        assert_eq!(busy["retryable"], true);
        assert_eq!(busy["retry_after_secs"], BUSY_RETRY_AFTER_SECS);
        drop(first);
        assert!(limit.enter().is_some());
    }

    #[test]
    fn without_project_keeps_the_order_of_the_other_arguments() {
        let names = |tool: &Tool| -> Vec<String> {
            tool.input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default()
        };
        // `project` is in the middle of its arguments.
        let tool = list_module::definition();
        let mut expected = names(&tool);
        expected.retain(|name| name != "project");
        assert_eq!(names(&without_project(tool)), expected);
    }

    #[tokio::test]
    async fn run_tool_returns_the_result_of_the_tool() {
        let expected = CallToolResult::structured(json!({ "text": "hi" }));
        let result = run_tool(std::future::ready(expected.clone())).await;
        assert_eq!(result, expected);
    }
}
