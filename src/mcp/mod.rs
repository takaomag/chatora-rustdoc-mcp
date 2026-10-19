//! Frontend layer: the MCP server. MCP types appear only in this module.

mod echo;

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
        ErrorData,
        Implementation,
        ListToolsResult,
        PaginatedRequestParams,
        ProtocolVersion,
        ServerCapabilities,
        ServerConfig,
    },
    service::{QuitReason, RequestContext, RoleServer, ServerInitializeError},
    transport::stdio,
};

/// Protocol versions this server implements (requirement R-MCP-2).
const SUPPORTED_VERSIONS: &[ProtocolVersion] = &[ProtocolVersion::V_2026_07_28];

/// Name shown to MCP clients in `serverInfo`. Keep it equal to the binary name (`src/bin/chatora-rustdoc-mcp.rs`):
/// `CARGO_BIN_NAME` is not set when compiling the library.
const SERVER_NAME: &str = "chatora-rustdoc-mcp";

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
            Ok(ListToolsResult::with_all_items(vec![echo::tool()])
                .with_ttl_ms(LIST_TTL_MS)
                .with_cache_scope(CacheScope::Public))
        };
        std::future::ready(result)
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, ErrorData>> + Send + '_ {
        // Only an unknown tool is a JSON-RPC error; bad arguments are tool errors (requirement R-ARG-1).
        let result = match request.name.as_ref() {
            echo::NAME => Ok(echo::call(request.arguments.as_ref()).into()),
            name => Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
        };
        std::future::ready(result)
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
