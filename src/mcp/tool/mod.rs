//! The MCP tools: which tools there are, how a `tools/call` reaches one, and what they share.
//!
//! Each tool is a module with `NAME`, `definition()`, `parse_arguments()` when its arguments need
//! checking, and `execute()`.

mod get_item;
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
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Value, json};

use crate::data::{cache::Cache, docs_rs::DocsRs};

/// What the tools share for the life of the server.
#[derive(Debug)]
pub(super) struct ToolState {
    /// Where rustdoc JSON is cached.
    pub(super) cache: Cache,
    /// The docs.rs client.
    pub(super) docs_rs: DocsRs,
}

/// Definitions of every tool, in the order `tools/list` returns them.
pub(super) fn definitions() -> Vec<Tool> {
    vec![
        get_item::definition(),
        // A tool for testing progress and cancellation, only in debug builds.
        #[cfg(debug_assertions)]
        sleep::definition(),
    ]
}

/// Runs the tool that `request` names.
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
    match request.name.as_ref() {
        get_item::NAME => match get_item::parse_arguments(request.arguments.as_ref()) {
            Ok(arguments) => Ok(run_tool(get_item::execute(arguments, Arc::clone(state)))
                .await
                .into()),
            Err(error) => Ok(error.into_result().into()),
        },
        #[cfg(debug_assertions)]
        sleep::NAME => match sleep::parse_arguments(request.arguments.as_ref()) {
            Ok(seconds) => Ok(run_tool(sleep::execute(seconds, context)).await.into()),
            Err(error) => Ok(error.into_result().into()),
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
    match tokio::spawn(future).await {
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
    async fn run_tool_returns_the_result_of_the_tool() {
        let expected = CallToolResult::structured(json!({ "text": "hi" }));
        let result = run_tool(std::future::ready(expected.clone())).await;
        assert_eq!(result, expected);
    }
}
