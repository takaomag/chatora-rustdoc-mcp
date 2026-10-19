//! Frontend layer: the MCP server. MCP types appear only in this module.

mod echo;
mod sleep;

use std::borrow::Cow;

use rmcp::{
    ServerHandler,
    ServiceExt as _,
    model::{
        CacheScope,
        CallToolRequestParams,
        CallToolResponse,
        CallToolResult,
        ContentBlock,
        DiscoverResult,
        ErrorCode,
        ErrorData,
        Implementation,
        InitializeRequestParams,
        InitializeResult,
        ListToolsResult,
        PaginatedRequestParams,
        ProtocolVersion,
        ServerCapabilities,
        ServerConfig,
        SubscriptionFilter,
    },
    service::{QuitReason, RequestContext, RoleServer, ServerInitializeError},
    transport::stdio,
};

/// Protocol versions this server implements (requirement R-MCP-2).
const SUPPORTED_VERSIONS: &[ProtocolVersion] = &[ProtocolVersion::V_2026_07_28];

/// Name shown to MCP clients in `serverInfo`. Keep it equal to the binary name (`src/bin/chatora-rustdoc-mcp.rs`):
/// `CARGO_BIN_NAME` is not set when compiling the library.
const SERVER_NAME: &str = "chatora-rustdoc-mcp";

/// Message of the error for `initialize`, the only hint an old client can show (requirement R-MCP-2).
const INITIALIZE_REJECTED_MESSAGE: &str = const_format::concatcp!(
    SERVER_NAME,
    " supports only MCP 2026-07-28, which has no initialize handshake. Use a client that speaks \
     it: Claude Code v2.1.292 or later, without MCP_PROTOCOL_NEGOTIATION=legacy or \
     MCP_SDK_GENERATION=v1.",
);

/// How long clients may cache `server/discover` and `tools/list` (requirement R-MCP-5).
const LIST_TTL_MS: u64 = 60 * 60 * 1000;

#[derive(Debug, Clone, Copy)]
struct Server;

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        config.server_info = Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));
        config
    }

    fn initialize(
        &self,
        request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<InitializeResult, ErrorData>> + Send + '_ {
        // No supported version has `initialize`, so every request is rejected.
        std::future::ready(Err(ErrorData::new(
            ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
            INITIALIZE_REJECTED_MESSAGE,
            Some(serde_json::json!({
                "requested": request.protocol_version,
                "supported": SUPPORTED_VERSIONS,
            })),
        )))
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(SUPPORTED_VERSIONS)
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(DiscoverResult::from_server_info(
            SUPPORTED_VERSIONS.to_vec(),
            self.get_info(),
        )
        .with_ttl_ms(LIST_TTL_MS)
        .with_cache_scope(CacheScope::Public)))
    }

    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        // Accept `subscriptions/listen` with no notifications: the server declares none
        // (requirement R-MCP-16). `None` would make rmcp answer -32601. The default `listen` holds
        // the subscription until it is cancelled.
        Some(SubscriptionFilter::default())
    }

    fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        // The list is not paginated, so any cursor is invalid (requirement R-MCP-5).
        let result = if request.is_some_and(|request| request.cursor.is_some()) {
            Err(ErrorData::invalid_params(
                "tools/list is not paginated; omit the cursor",
                None,
            ))
        } else {
            Ok(
                ListToolsResult::with_all_items(vec![echo::tool(), sleep::tool()])
                    .with_ttl_ms(LIST_TTL_MS)
                    .with_cache_scope(CacheScope::Public),
            )
        };
        std::future::ready(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Only an unknown tool is a JSON-RPC error; bad arguments are tool errors (requirement R-ARG-1).
        match request.name.as_ref() {
            echo::NAME => {
                let arguments = request.arguments;
                Ok(run_tool(async move { echo::call(arguments.as_ref()) })
                    .await
                    .into())
            },
            sleep::NAME => match sleep::parse(request.arguments.as_ref()) {
                Ok(seconds) => Ok(run_tool(sleep::call(seconds, context)).await.into()),
                Err(error) => Ok(error.into()),
            },
            name => Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
        }
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

/// Builds the `invalid_argument` tool error (requirement R-ARG-1).
fn invalid_argument(message: &str, suggestions: &[&str]) -> CallToolResult {
    tool_error("invalid_argument", message, false, suggestions)
}

/// Builds a tool error whose `content[0].text` is self-contained JSON (requirement R-ERR-2).
///
/// It has no `structuredContent`: Claude Code ignores it on errors (requirement R-OUT-1).
fn tool_error(code: &str, message: &str, retryable: bool, suggestions: &[&str]) -> CallToolResult {
    let body = serde_json::json!({
        "code": code,
        "message": message,
        "retryable": retryable,
        "suggestions": suggestions,
    });
    CallToolResult::error(vec![ContentBlock::text(body.to_string())])
}

/// Errors that stop the MCP server.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The MCP session could not start, for example because the client asked for an
    /// unsupported protocol version.
    // Boxed because `ServerInitializeError` is several hundred bytes (`clippy::result_large_err`).
    #[error("failed to start the MCP session")]
    Initialize(#[source] Box<ServerInitializeError>),
    /// A task of the MCP session was cancelled. Panics are not reported here; they are resumed.
    #[error("a task of the MCP session was cancelled")]
    Cancelled(#[source] tokio::task::JoinError),
}

/// Runs the MCP server over stdio until the client closes the connection.
///
/// It must run inside a Tokio runtime.
///
/// # Errors
///
/// Returns an error when the MCP session fails.
///
/// # Panics
///
/// Resumes the panic of a task of the MCP session.
pub async fn serve() -> Result<(), ServeError> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "starting the MCP server over stdio"
    );
    match Server.serve(stdio()).await {
        Ok(running) => match running.waiting().await {
            Ok(QuitReason::JoinError(error)) | Err(error) => match error.try_into_panic() {
                Ok(payload) => std::panic::resume_unwind(payload),

                Err(error) => Err(ServeError::Cancelled(error)),
            },

            Ok(_) => Ok(()),
        },

        // The client closed stdin after `server/discover` only.
        Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),

        Err(error) => Err(ServeError::Initialize(Box::new(error))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the JSON in `content[0].text` of a tool result.
    fn text_json(result: &CallToolResult) -> serde_json::Value {
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
        let expected = CallToolResult::structured(serde_json::json!({ "text": "hi" }));
        let result = run_tool(std::future::ready(expected.clone())).await;
        assert_eq!(result, expected);
    }
}
