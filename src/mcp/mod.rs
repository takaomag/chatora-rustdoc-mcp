//! Frontend layer: the MCP server. MCP types appear only in this module.

mod tool;

use std::borrow::Cow;

use rmcp::{
    ServerHandler,
    ServiceExt as _,
    model::{
        CacheScope,
        CallToolRequestParams,
        CallToolResponse,
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
            Ok(ListToolsResult::with_all_items(tool::definitions())
                .with_ttl_ms(LIST_TTL_MS)
                .with_cache_scope(CacheScope::Public))
        };
        std::future::ready(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        tool::call(request, context).await
    }
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
