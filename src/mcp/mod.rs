//! Frontend layer: the MCP server. MCP types appear only in this module.

mod tool;

use std::{borrow::Cow, sync::Arc};

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

#[cfg(feature = "local")]
use crate::data::{metadata::Resolver, project::DefaultProject};
use crate::{
    config::Settings,
    data::{
        Network,
        cache::{Cache, CacheError},
        crate_file::StaticCrates,
        dist::{Dist, DistSettings},
        docs_rs::DocsRs,
        sparse_index::SparseIndex,
    },
};

/// Protocol versions this server implements (requirement R-MCP-2).
const SUPPORTED_VERSIONS: &[ProtocolVersion] = &[ProtocolVersion::V_2026_07_28];

/// Name shown to MCP clients in `serverInfo`: the package name, which is also the binary name
/// (requirement R-DIST-1).
const SERVER_NAME: &str = env!("CARGO_PKG_NAME");

/// Message of the error for `initialize`, the only hint an old client can show (requirement
/// R-MCP-2).
const INITIALIZE_REJECTED_MESSAGE: &str = const_format::concatcp!(
    SERVER_NAME,
    " supports only MCP 2026-07-28, which has no initialize handshake. Use a client that speaks \
     it: Claude Code v2.1.292 or later, without MCP_PROTOCOL_NEGOTIATION=legacy or \
     MCP_SDK_GENERATION=v1.",
);

/// The server's `instructions` (requirement R-MCP-6): the order of calls, the rules for versions
/// and targets, and where the language server is the better tool. What each tool answers is in
/// its description. Only tools of this build are named (requirement R-QA-5).
pub(crate) const INSTRUCTIONS: &str = const_format::concatcp!(
    "Rust API documentation of crates from docs.rs and of the standard library, ",
    VERSIONS_OF,
    ". Use these tools instead of answering from memory.\n\n",
    "- Order of calls: to find an item, ",
    FIND,
    "`crate_overview`, `list_module` or `search_items`; for the item itself, `get_item`; for \
     what a type or trait has, `list_methods`, `list_impls` or `list_implementors`. None needs an \
     earlier call: give the path, such as `tokio::sync::Mutex`, directly.\n",
    VERSIONS,
    "- Targets: the data is that of the host's target unless `target` names another, so items \
     for other platforms may be missing. std, core and alloc are those of a nightly toolchain: \
     check an item's `stability` before relying on it.\n",
    "- For the code of this workspace itself (its own items, the type at a position, \
     references, diagnostics), use the language server (rust-analyzer). These tools document \
     crates as published.",
);

/// Whose versions the documentation is of.
#[cfg(feature = "local")]
const VERSIONS_OF: &str = "at the versions that the project uses";
#[cfg(not(feature = "local"))]
const VERSIONS_OF: &str = "at the versions asked for";

/// The tools that find an item, before those of every build.
#[cfg(feature = "local")]
const FIND: &str = "`list_crates` (the project's dependencies), ";
#[cfg(not(feature = "local"))]
const FIND: &str = "";

/// The rule for versions.
#[cfg(feature = "local")]
const VERSIONS: &str = "- Versions: without a version, a tool uses the one in the project's \
                        Cargo.lock (the project Claude Code was opened in, or `project`), or else \
                        the latest release; `provenance` tells which and why. `resolve_version` \
                        is not needed first. Working in another git worktree of the project, as a \
                        subagent may, pass the worktree's directory as `project`: the server sees \
                        only the directory Claude Code was opened in.\n";
#[cfg(not(feature = "local"))]
const VERSIONS: &str = "- Versions: without a version, a tool uses the latest release, as this \
                        server is built without reading projects: give the version that the \
                        project's Cargo.lock has. `provenance` tells which version was used.\n";

/// How long clients may cache `server/discover` and `tools/list` (requirement R-MCP-5).
const LIST_TTL_MS: u64 = 60 * 60 * 1000;

#[derive(Debug, Clone)]
struct Server {
    /// Shared by every tool call, which holds it with one reference count.
    tools: Arc<tool::ToolState>,
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        config.server_info = Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));
        config.instructions = Some(INSTRUCTIONS.to_owned());
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
        tool::call(request, context, &self.tools).await
    }
}

/// Errors that stop the MCP server.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The cache directory is not usable as configured.
    #[error("the cache directory cannot be used")]
    Cache(#[from] CacheError),
    /// The HTTP client for docs.rs or the crates.io index could not be set up.
    #[error("the HTTP client for docs.rs or the crates.io index could not be set up")]
    Http(#[from] reqwest::Error),
    /// The MCP session could not start, for example because the client asked for an
    /// unsupported protocol version.
    // Boxed because `ServerInitializeError` is several hundred bytes (`clippy::result_large_err`).
    #[error("failed to start the MCP session")]
    Initialize(#[source] Box<ServerInitializeError>),
    /// A task of the MCP session was cancelled. Panics are not reported here; they are resumed.
    #[error("a task of the MCP session was cancelled")]
    Cancelled(#[source] tokio::task::JoinError),
}

/// Runs the MCP server over stdio until the client closes the connection, with `settings`
/// (requirement R-DIST-3), which were read when it started.
///
/// It must run inside a Tokio runtime.
///
/// # Errors
///
/// Returns an error when the cache directory or the HTTP client cannot be set up, or when the MCP
/// session fails.
///
/// # Panics
///
/// Resumes the panic of a task of the MCP session.
pub async fn serve(settings: &Settings) -> Result<(), ServeError> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "starting the MCP server over stdio"
    );
    // The first line of the log file: the calls after it are this version's (requirement R-OBS-1).
    crate::logging::write_record(&serde_json::json!({
        "ts": crate::logging::now(),
        "kind": "start",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    }));
    let cache = Cache::open(settings);
    // After the first line of the log file, which starts what this run logs.
    settings.report();
    let cache = cache?;
    let network = Network::from_settings(settings);
    let server = Server {
        tools: Arc::new(tool::ToolState {
            docs_rs: DocsRs::new(&network)?,
            dist: Dist::new(&network, DistSettings::from_settings(settings))?,
            index: SparseIndex::new(cache.clone(), &network)?,
            static_crates: StaticCrates::new(&network)?,
            // Found in the background below: rustc may be slow to start (requirement R-MCP-11).
            host: tokio::sync::OnceCell::new(),
            cache,
            // Decided once, so that a repository cloned inside later does not join it
            // (requirement R-MCP-3).
            #[cfg(feature = "local")]
            project: DefaultProject::find(
                std::env::var_os("CLAUDE_PROJECT_DIR"),
                std::env::current_dir().ok(),
            ),
            #[cfg(feature = "local")]
            resolver: Resolver::new(false),
            calls: tool::CallLimit::from_settings(settings),
        }),
    };
    settings.report();
    // The host target is found while the session starts, not before it: rustc may be slow to
    // start the first time (requirement R-MCP-11). The first tool call waits for it.
    let tools = Arc::clone(&server.tools);
    tokio::spawn(async move {
        tools.host().await;
    });
    match Box::pin(server.serve(stdio())).await {
        Ok(running) => match running.waiting().await {
            Ok(QuitReason::JoinError(error)) | Err(error) => {
                Err(ServeError::Cancelled(crate::task::resume_if_panic(error)))
            },

            Ok(_) => Ok(()),
        },

        // The client closed stdin before the session started, for example after only
        // `server/discover`.
        Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),

        Err(error) => Err(ServeError::Initialize(Box::new(error))),
    }
}
