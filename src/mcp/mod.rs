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
        dist::DistSettings,
        flight::Flights,
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

/// The server's `instructions` (requirement R-MCP-6): what it documents, the order of calls, the
/// rules for versions and targets, what to do with an answer that is not ready, and where the
/// language server is the better tool (§9.9, decided in M2f's F4a). What each tool answers is in
/// its description. Only tools of this build are named (requirement R-QA-5).
pub(crate) const INSTRUCTIONS: &str = const_format::concatcp!(
    WHAT,
    " Use these tools instead of answering from memory.\n\n",
    "- Order of calls: to find an item, ",
    FIND,
    "`crate_overview`, `list_module` or `search_items`; for the item itself, `get_item`; for \
     what a type or trait has, `list_methods`, `list_impls` or `list_implementors`. None needs an \
     earlier call (but see below): give the path, such as `tokio::sync::Mutex`, directly.",
    EXTERNAL,
    "\n",
    VERSIONS,
    TARGETS,
    "- An answer with `status: building`, or `partial` with `retry_after_secs`, is not ready: \
     call again after it.",
    STALE,
    "\n",
    LANGUAGE_SERVER,
);

/// What the documentation is of.
#[cfg(feature = "local")]
const WHAT: &str = "Rust API documentation of this workspace's crates, built here with their \
                    private items, of their dependencies at the versions that the project uses, \
                    of the standard library and of crates on docs.rs.";
#[cfg(not(feature = "local"))]
const WHAT: &str = "Rust API documentation of crates from docs.rs and of the standard library, at \
                    the versions asked for.";

/// The tools that find an item, before those of every build.
#[cfg(feature = "local")]
const FIND: &str = "`list_crates` (the project's crates and dependencies), ";
#[cfg(not(feature = "local"))]
const FIND: &str = "";

/// When `build_external_docs` goes first.
#[cfg(feature = "local")]
const EXTERNAL: &str = " For a workspace outside the project or a crate of a git repository, call \
                        `build_external_docs` first; for a release or a target that docs.rs has \
                        no data of, when an answer says to. It runs build scripts: only for code \
                        you trust.";
#[cfg(not(feature = "local"))]
const EXTERNAL: &str = "";

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

/// The rule for targets.
#[cfg(feature = "local")]
const TARGETS: &str = "- Targets: the data is for the project's `build.target` for the crates it \
                       uses, else the host's, unless `target` names another, so items for other \
                       platforms may be missing. std, core and alloc are those of a nightly toolchain: check an \
                       item's `stability` before relying on it.\n";
#[cfg(not(feature = "local"))]
const TARGETS: &str = "- Targets: the data is that of the host's target unless `target` names \
                       another, so items for other platforms may be missing. std, core and alloc \
                       are those of a nightly toolchain: check an item's `stability` before \
                       relying on it.\n";

/// How to read a stale answer.
#[cfg(feature = "local")]
const STALE: &str = " One with `stale` is of an earlier build: `stale_reason` and the hints tell \
                     why and what to do; an item may have changed, and its lines moved.";
#[cfg(not(feature = "local"))]
const STALE: &str = "";

/// Where the language server is the better tool.
#[cfg(feature = "local")]
const LANGUAGE_SERVER: &str = "- Look up this workspace's own items here too, by path or name, as \
                               its dependencies'. Use the language server (rust-analyzer) for \
                               what is at a position (line and column), references, diagnostics, \
                               and code edited after an answer's `built_at`.";
#[cfg(not(feature = "local"))]
const LANGUAGE_SERVER: &str = "- For the code of this workspace itself (its own items, the type at \
                               a position, references, diagnostics), use the language server \
                               (rust-analyzer). These tools document crates as published.";

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
    // Read before the problems of the settings are reported, so that theirs are too.
    crate::data::gc::set_limits(crate::data::layout::Limits::from_settings(settings));
    // After the first line of the log file, which starts what this run logs.
    settings.report();
    let cache = cache?;
    // Before the server puts anything in it, which would make a new cache look like another's.
    claim(&cache).await;
    #[cfg(feature = "local")]
    sweep_in_background(&cache);
    // When the server starts, in the background, for a while at most (decided in M2f's F1, 2).
    crate::data::gc::in_background(&cache);
    #[cfg(feature = "local")]
    crate::data::toolchain::Programs::set_this(settings);
    #[cfg(feature = "local")]
    crate::data::build::set_this(settings);
    let network = Network::from_settings(settings);
    let wait = tool::wait_for_work(settings);
    let server = Server {
        tools: Arc::new(tool::ToolState {
            docs_rs: tool::docs_rs(&network)?,
            archives: Flights::default(),
            crate_sources: Flights::default(),
            stds: Flights::default(),
            rust_srcs: Flights::default(),
            dist: tool::dist(&network, DistSettings::from_settings(settings))?,
            index: tool::sparse_index(cache.clone(), &network)?,
            // The `.crate` files that cargo has fetched are read before static.crates.io is asked.
            static_crates: tool::static_crates(&network)?
                .with_cargo_home(crate::data::cargo_home()),
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
            // cargo runs with `--offline` in offline mode (requirement R-CA-7).
            resolver: Resolver::new(network.offline),
            #[cfg(feature = "local")]
            locals: Flights::handing_over_only(crate::query::local::hands_over),
            #[cfg(feature = "local")]
            externals: Flights::default(),
            #[cfg(feature = "local")]
            git_externals: Flights::default(),
            #[cfg(feature = "local")]
            workspaces: Flights::handing_over(),
            #[cfg(feature = "local")]
            recorded_workspaces: parking_lot::Mutex::default(),
            #[cfg(feature = "local")]
            outsides: parking_lot::Mutex::default(),
            calls: tool::CallLimit::from_settings(settings),
            wait,
            #[cfg(feature = "local")]
            build_wait: tool::wait_for_builds(settings, wait),
        }),
    };
    settings.report();
    // The host target is found while the session starts, not before it: rustc may be slow to
    // start the first time (requirement R-MCP-11). The first tool call waits for it.
    let tools = Arc::clone(&server.tools);
    tokio::spawn(async move {
        tools.host().await;
    });
    // Told when stdin ends, as a signal is.
    #[cfg(feature = "local")]
    let stdin_ended = Arc::new(tokio::sync::Notify::new());
    #[cfg(feature = "local")]
    let transport = {
        let (stdin, stdout) = stdio();
        (StopChildrenAtEnd(stdin, Arc::clone(&stdin_ended)), stdout)
    };
    #[cfg(not(feature = "local"))]
    let transport = stdio();
    let outcome = match Box::pin(server.serve(transport)).await {
        #[cfg(feature = "local")]
        Ok(running) => stopped(running, stdin_ended).await,
        #[cfg(not(feature = "local"))]
        Ok(running) => stopped(running).await,

        // The client closed stdin before the session started, for example after only
        // `server/discover`.
        Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),

        Err(error) => Err(ServeError::Initialize(Box::new(error))),
    };
    // What this server leaves in the cache goes with it, however it ends (requirement R-CA-4).
    #[cfg(feature = "local")]
    if let Some(owner) = crate::data::owner::Owner::this() {
        owner.leave();
    }
    outcome
}

/// Waits for the session `running` to end: when the client closes stdin, or, with the local
/// features, on SIGINT or SIGTERM, which stops the children first (requirement R-WS-6). rmcp then
/// waits for the calls in progress and closes stdout, which may not end, as when the client reads
/// no more: on another signal, or [`STOP_LIMIT`] after the first signal or the end of stdin
/// (`stdin_ended`), it is not waited for. A later server sweeps what this one leaves (decided in
/// C4).
async fn stopped(
    running: rmcp::service::RunningService<RoleServer, Server>,
    #[cfg(feature = "local")] stdin_ended: Arc<tokio::sync::Notify>,
) -> Result<(), ServeError> {
    #[cfg(feature = "local")]
    let (end_now, ending_now) = tokio::sync::oneshot::channel::<()>();
    #[cfg(feature = "local")]
    let on_signal = {
        let token = running.cancellation_token();
        let mut signals = StopSignals::new();
        tokio::spawn(async move {
            // The client stops the server with a signal, or by closing stdin, which rmcp tells
            // the session by itself, and which stopped the children already.
            tokio::select! {
                () = signals.next() => {
                    tracing::info!("stopping on a signal");
                    crate::data::child::stop_all();
                    token.cancel();
                },
                () = stdin_ended.notified() => tracing::info!("stopping as stdin ended"),
            }
            tokio::select! {
                () = signals.next() => {},
                () = tokio::time::sleep(STOP_LIMIT) => {},
            }
            if end_now.send(()).is_err() {
                tracing::debug!("the session ended meanwhile");
            }
        })
    };
    #[cfg(feature = "local")]
    let waited = tokio::select! {
        waited = running.waiting() => Some(waited),
        Ok(()) = ending_now => None,
    };
    #[cfg(not(feature = "local"))]
    let waited = Some(running.waiting().await);
    #[cfg(feature = "local")]
    on_signal.abort();
    match waited {
        Some(Ok(QuitReason::JoinError(error)) | Err(error)) => {
            Err(ServeError::Cancelled(crate::task::resume_if_panic(error)))
        },
        Some(Ok(_)) => Ok(()),
        None => {
            tracing::warn!("the server did not stop in time; ending at once");
            Ok(())
        },
    }
}

/// How long the server may take to stop after a signal: more than rmcp waits for the calls in
/// progress (5 seconds) and for its tasks (2 seconds).
#[cfg(feature = "local")]
const STOP_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the sweep at startup may take (decided in C4).
#[cfg(feature = "local")]
const SWEEP_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Tells whether the cache directory is this server's, and makes it so when it may: made with its
/// marker, or adopted when an earlier version made it (decided in M2f's F1, 5, and F1a, 3). One that
/// is not is logged, and its garbage collection removes nothing. It reads a directory and writes
/// two small files, once, before the server puts anything in it.
async fn claim(cache: &Cache) {
    let root = cache.dir().to_path_buf();
    let claimed = crate::task::spawn_blocking(move || match crate::data::layout::claim(&root) {
        Ok(crate::data::layout::Ownership::Foreign(names)) => tracing::warn!(
            root = %root.display(),
            ?names,
            "the cache directory is not this server's: nothing in it is removed"
        ),
        Ok(ownership) => {
            tracing::debug!(root = %root.display(), ?ownership, "the cache directory is this server's");
        },
        Err(error) => {
            tracing::warn!(root = %root.display(), %error, "whether the cache directory is this server's could not be told");
        },
    });
    drop(claimed.await);
}

/// Starts this server as the owner of what it leaves in the cache, and removes, in the background
/// for at most [`SWEEP_LIMIT`], what the servers that have stopped left: the server answers at
/// once (requirement R-MCP-11), and the next one goes on where this one stops (requirements R-WS-6
/// and R-CA-4).
#[cfg(feature = "local")]
fn sweep_in_background(cache: &Cache) {
    let owner = crate::data::owner::Owner::start_this(cache);
    drop(crate::task::spawn_blocking(move || {
        let now = std::time::Instant::now();
        let swept = crate::data::owner::sweep(owner, now.checked_add(SWEEP_LIMIT).unwrap_or(now));
        if swept != crate::data::owner::Swept::default() {
            tracing::info!(
                ?swept,
                "removed what servers that have stopped left in the cache"
            );
        }
    }));
}

/// The server's stdin, which stops every child and what it started when it ends or fails, and
/// tells its [`tokio::sync::Notify`]: the client has gone, and rmcp still waits up to 5 seconds for
/// the calls in progress, and then for stdout to close, which a client that reads no more keeps
/// open (requirement R-WS-6).
#[cfg(feature = "local")]
struct StopChildrenAtEnd<R>(R, Arc<tokio::sync::Notify>);

#[cfg(feature = "local")]
impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for StopChildrenAtEnd<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let polled = std::pin::Pin::new(&mut self.0).poll_read(context, buffer);
        let ended = match &polled {
            std::task::Poll::Ready(Ok(())) => {
                buffer.filled().len() == before && buffer.remaining() > 0
            },
            std::task::Poll::Ready(Err(_)) => true,
            std::task::Poll::Pending => false,
        };
        if ended {
            crate::data::child::stop_all();
            self.1.notify_one();
        }
        polled
    }
}

/// SIGINT and SIGTERM on Unix, and Ctrl+C on Windows: how Claude Code stops the server
/// (requirement R-WS-6). Watched from when it is made, so that a signal that comes between two
/// waits is not missed. Never comes if the signals cannot be watched.
#[cfg(feature = "local")]
struct StopSignals {
    #[cfg(unix)]
    watched: Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>,
    #[cfg(windows)]
    watched: Option<tokio::signal::windows::CtrlC>,
}

#[cfg(feature = "local")]
impl StopSignals {
    fn new() -> Self {
        #[cfg(unix)]
        let watched = {
            use tokio::signal::unix::{SignalKind, signal};
            signal(SignalKind::interrupt())
                .ok()
                .zip(signal(SignalKind::terminate()).ok())
        };
        #[cfg(windows)]
        let watched = tokio::signal::windows::ctrl_c().ok();
        Self { watched }
    }

    /// The next signal.
    async fn next(&mut self) {
        #[cfg(unix)]
        if let Some((interrupt, terminate)) = &mut self.watched {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
            return;
        }
        #[cfg(windows)]
        if let Some(ctrl_c) = &mut self.watched {
            ctrl_c.recv().await;
            return;
        }
        std::future::pending::<()>().await;
    }
}
