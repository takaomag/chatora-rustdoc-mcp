//! Frontend layer: the MCP server. MCP types appear only in this module.

use std::borrow::Cow;

use rmcp::{
    ServerHandler,
    ServiceExt as _,
    model::{
        CacheScope,
        DiscoverResult,
        ErrorData,
        Implementation,
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

/// How long clients may cache `server/discover` (requirement R-MCP-5).
const DISCOVER_TTL_MS: u64 = 60 * 60 * 1000;

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
        .with_ttl_ms(DISCOVER_TTL_MS)
        .with_cache_scope(CacheScope::Public)))
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
