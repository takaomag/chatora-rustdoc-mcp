//! The MCP tools: which tools there are, how a `tools/call` reaches one, and what they share.
//!
//! Each tool is a module with `NAME`, `definition()`, `parse_arguments()` when its arguments need
//! checking, and `execute()`.

#[cfg(feature = "local")]
mod build_external_docs;
mod call_log;
#[cfg(test)]
mod checks;
mod crate_overview;
#[cfg(feature = "local")]
mod external_build;
#[cfg(feature = "local")]
mod external_git;
#[cfg(feature = "local")]
mod external_target;
#[cfg(feature = "local")]
mod external_workspace;
mod get_item;
#[cfg(feature = "local")]
mod list_crates;
mod list_implementors;
mod list_impls;
mod list_methods;
mod list_module;
#[cfg(feature = "local")]
mod local;
mod project;
#[cfg(feature = "local")]
mod recorded;
#[cfg(feature = "local")]
mod recorded_git;
#[cfg(feature = "local")]
mod recorded_workspace;
mod resolve_version;
mod search_across;
mod search_items;
#[cfg(debug_assertions)]
mod sleep;

use std::{borrow::Cow, sync::Arc, time::Duration};

use rmcp::{
    model::{
        CallToolRequestParams,
        CallToolResponse,
        CallToolResult,
        ContentBlock,
        ErrorData,
        JsonObject,
        ProgressNotificationParam,
        ProgressToken,
        Tool,
        ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
};
use serde_json::{Value, json};
use tokio::{
    sync::{OnceCell, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::Instant,
};
use tracing::Instrument as _;

#[cfg(feature = "local")]
use crate::data::{metadata::Resolver, project::DefaultProject};
use crate::{
    config::Settings,
    data::{
        Network,
        cache::Cache,
        crate_file::StaticCrates,
        dist::Dist,
        docs_rs::DocsRs,
        fetches::Fetches,
        flight::Flights,
        sparse_index::SparseIndex,
        target::{self, HostTarget},
        waits::Waits,
    },
    query::load::{SharedArchive, SharedStd},
};

/// What the tools share for the life of the server.
#[derive(Debug)]
pub(super) struct ToolState {
    /// Where rustdoc JSON is cached.
    pub(super) cache: Cache,
    /// The docs.rs client.
    pub(super) docs_rs: DocsRs,
    /// The archives being made from rustdoc JSON on docs.rs, shared by the calls that want them
    /// at once (requirements R-NW-2 and R-WS-6).
    pub(super) archives: Flights<SharedArchive>,
    /// The JSON of std being chosen and fetched, shared by the calls that want it at once
    /// (requirements R-STD-6 and R-WS-6).
    pub(super) stds: Flights<SharedStd>,
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
    /// The builds of workspace members, shared by the calls that want them at once (requirement
    /// R-WS-6, decided in M2's D2). A build that went on past a deadline is found fresh by the
    /// next call, or handed to it when it has no key (decided in M2's D4).
    #[cfg(feature = "local")]
    pub(super) locals: Flights<crate::query::local::SharedLocal>,
    /// The builds of `build_external_docs`, shared by the calls that want them at once, by the
    /// key of their record (requirement R-WS-10, decided in M2e's E2a).
    #[cfg(feature = "local")]
    pub(super) externals: Flights<external_build::SharedExternal>,
    /// The builds of `build_external_docs` of crates of git repositories, by the key of the ref
    /// asked for (decided in M2e's E3).
    #[cfg(feature = "local")]
    pub(super) git_externals: Flights<external_git::SharedGit>,
    /// The records of `build_external_docs` of workspaces outside the default project, and the
    /// plans of their builds, by workspace and target (decided in M2e's E4b): what went on past a
    /// deadline is handed to the next call, which would else record it again.
    #[cfg(feature = "local")]
    pub(super) workspaces: Flights<external_workspace::SharedPlanned>,
    /// What the query tools read of the records of workspaces outside the default project, until
    /// they are recorded again (decided in M2e's E4b-3).
    #[cfg(feature = "local")]
    pub(super) recorded_workspaces: recorded_workspace::Kept,
    /// The calls being handled, at most `limits.concurrent_calls` (requirement R-SEC-7).
    pub(super) calls: CallLimit,
    /// How long a call waits for work that goes on without it, such as fetching and indexing a
    /// crate (requirement R-ARCH-6). See [`wait_for_work`].
    pub(super) wait: Duration,
    /// How long a call waits for a local build, from when it starts to wait for it (requirement
    /// R-WS-6). See [`wait_for_builds`].
    #[cfg(feature = "local")]
    pub(super) build_wait: Duration,
}

impl ToolState {
    /// The host target, from `rustc -vV` in the cache directory, found once. The first call waits
    /// for it, which `serve` starts at startup.
    pub(super) async fn host(&self) -> &HostTarget {
        self.host
            .get_or_init(|| target::host_target(self.cache.dir()))
            .await
    }

    /// Until when a call that starts now waits for work that goes on without it.
    pub(super) fn until(&self) -> Instant {
        let now = Instant::now();
        now.checked_add(self.wait).unwrap_or(now)
    }
}

/// `limits.request_timeout_secs` unless set (requirement R-ARCH-6).
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// How long a call waits for work that goes on without it: two thirds of the time a call may take,
/// the setting `limits.request_timeout_secs` (requirements R-ARCH-6 and R-DIST-3), which leaves the
/// rest for what the call does before and after the work. The call then returns `status:
/// building`, or `partial` across crates (requirement R-OUT-9).
pub(super) fn wait_for_work(settings: &Settings) -> Duration {
    let secs = settings
        .get("limits.request_timeout_secs", |value| {
            value
                .whole_number()
                .filter(|secs| *secs > 0)
                .ok_or("a whole number of seconds from 1")
        })
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    Duration::from_secs(secs)
        .saturating_mul(2)
        .checked_div(3)
        .unwrap_or_default()
}

/// `build.wait_secs` unless set (requirement R-WS-6, initial value).
#[cfg(feature = "local")]
const DEFAULT_BUILD_WAIT_SECS: u64 = 10;

/// How long a call waits for a local build, from when it starts to wait for it: the setting
/// `build.wait_secs` (requirement R-WS-6, decided in M2's D3), but no longer than a call waits for
/// work, `wait`, which is logged. The call then returns `status: building`.
#[cfg(feature = "local")]
pub(super) fn wait_for_builds(settings: &Settings, wait: Duration) -> Duration {
    let set = settings.get("build.wait_secs", |value| {
        value
            .whole_number()
            .filter(|secs| *secs > 0)
            .ok_or("a whole number of seconds from 1")
    });
    let secs = set.unwrap_or(DEFAULT_BUILD_WAIT_SECS);
    let asked = Duration::from_secs(secs);
    // Only what the user set is worth a warning: a short `limits.request_timeout_secs` cuts the
    // default too.
    if asked > wait && set.is_some() {
        tracing::warn!(
            build_wait_secs = secs,
            wait_secs = wait.as_secs(),
            "build.wait_secs is longer than a call waits for work (two thirds of \
             limits.request_timeout_secs), so a call waits for a build as long as that"
        );
    }
    asked.min(wait)
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

    /// The limit of the setting `limits.concurrent_calls` (requirement R-DIST-3), or the default.
    pub(super) fn from_settings(settings: &Settings) -> Self {
        let max = settings
            .get("limits.concurrent_calls", |value| {
                value
                    .whole_number()
                    .and_then(|max| usize::try_from(max).ok())
                    .filter(|max| *max > 0)
                    .ok_or("a whole number from 1")
            })
            .unwrap_or(Self::DEFAULT);
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

/// The environment variable that adds the tool for testing (`sleep`) to a debug build. The
/// integration tests set it to `1`; a debug build installed for use (`cargo install --debug`)
/// does not list the tool. It is not a setting (requirement R-DIST-3), so it is not under the
/// settings' prefix.
#[cfg(debug_assertions)]
const TEST_TOOLS_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_TOOLS";

/// Whether the tool for testing is served: a debug build with [`TEST_TOOLS_ENV_VAR`] set to `1`.
#[cfg(debug_assertions)]
fn test_tools() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os(TEST_TOOLS_ENV_VAR).is_some_and(|value| value == "1"))
}

/// The environment variable that points the docs.rs client of a debug build with the tool for
/// testing at a server of the tests, for the tests of work that takes long (`tests/stdio.rs`). Like
/// [`TEST_TOOLS_ENV_VAR`], it is not a setting.
#[cfg(debug_assertions)]
const TEST_DOCS_RS_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_DOCS_RS_URL";

/// The docs.rs client: of docs.rs, or of the server of the tests that [`TEST_DOCS_RS_ENV_VAR`]
/// names in a debug build with the tool for testing.
///
/// # Errors
///
/// Returns an error when the HTTP client cannot be set up.
pub(super) fn docs_rs(network: &Network) -> Result<DocsRs, reqwest::Error> {
    #[cfg(debug_assertions)]
    if let Some(base_url) = test_url(TEST_DOCS_RS_ENV_VAR) {
        return DocsRs::with_base_url(&base_url, network);
    }
    DocsRs::new(network)
}

/// The environment variables that point the clients of the crates.io index and of
/// static.crates.io of a debug build with the tool for testing at a fake crates.io of the tests,
/// for the builds of `build_external_docs` (`tests/stdio.rs`). Like [`TEST_DOCS_RS_ENV_VAR`],
/// they are not settings.
#[cfg(debug_assertions)]
const TEST_INDEX_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_INDEX_URL";
#[cfg(debug_assertions)]
const TEST_STATIC_ENV_VAR: &str = "CHATORA_RUSTDOC_TEST_STATIC_URL";

/// The URL that the environment variable `name` gives a debug build with the tool for testing.
#[cfg(debug_assertions)]
fn test_url(name: &str) -> Option<String> {
    test_tools().then(|| std::env::var(name).ok()).flatten()
}

/// The client of the crates.io index, caching in `cache`: of index.crates.io, or of the server of
/// the tests that [`TEST_INDEX_ENV_VAR`] names in a debug build with the tool for testing.
///
/// # Errors
///
/// Returns an error when the HTTP client cannot be set up.
pub(super) fn sparse_index(cache: Cache, network: &Network) -> Result<SparseIndex, reqwest::Error> {
    #[cfg(debug_assertions)]
    if let Some(base_url) = test_url(TEST_INDEX_ENV_VAR) {
        return SparseIndex::with_base_url(cache, &base_url, network);
    }
    SparseIndex::new(cache, network)
}

/// The client of static.crates.io: of it, or of the server of the tests that
/// [`TEST_STATIC_ENV_VAR`] names in a debug build with the tool for testing.
///
/// # Errors
///
/// Returns an error when the HTTP client cannot be set up.
pub(super) fn static_crates(network: &Network) -> Result<StaticCrates, reqwest::Error> {
    #[cfg(debug_assertions)]
    if let Some(base_url) = test_url(TEST_STATIC_ENV_VAR) {
        return StaticCrates::with_base_url(&base_url, network);
    }
    StaticCrates::new(network)
}

/// Definitions of every tool, in the order `tools/list` returns them.
pub(super) fn definitions() -> Vec<Tool> {
    let tools = vec![
        // It builds what is outside the default project (requirement R-ARCH-4).
        #[cfg(feature = "local")]
        build_external_docs::definition(),
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
    ];
    #[cfg_attr(
        not(debug_assertions),
        expect(unused_mut, reason = "only a debug build adds the tool for testing")
    )]
    let mut tools: Vec<Tool> = if cfg!(feature = "local") {
        tools.into_iter().map(with_refresh).map(with_git).collect()
    } else {
        tools.into_iter().map(without_project).collect()
    };
    // A tool for testing progress and cancellation.
    #[cfg(debug_assertions)]
    if test_tools() {
        tools.push(sleep::definition());
    }
    tools
}

/// `tool` with the argument `refresh` (§6.4), which every query tool takes with local features, as
/// each may run cargo (requirement R-MCP-4). `build_external_docs` does not: calling it again with
/// the same arguments tries again (requirement R-WS-10).
fn with_refresh(mut tool: Tool) -> Tool {
    #[cfg(feature = "local")]
    if tool.name == build_external_docs::NAME {
        return tool;
    }
    let mut schema = (*tool.input_schema).clone();
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties.insert(
            "refresh".to_owned(),
            json!({
                "type": "boolean",
                "description": "Set true to try again what failed before, instead of the failure \
                                that is kept: a build of a workspace member (kept while its \
                                sources are the same), cargo metadata, or fetching from docs.rs. \
                                What worked is not redone.",
            }),
        );
    }
    tool.input_schema = Arc::new(schema);
    tool
}

/// `tool` with the arguments `git`, `rev`, `branch` and `tag` (§6.4), which every query tool that
/// takes `crate` takes with local features, to look up a crate of a git repository that
/// `build_external_docs` documented (requirement R-MCP-3); and that tool too. Not `list_crates`,
/// whose `crate` is the project's.
fn with_git(mut tool: Tool) -> Tool {
    let mut schema = (*tool.input_schema).clone();
    let Some(Value::Object(properties)) = schema.get_mut("properties") else {
        return tool;
    };
    if !properties.contains_key("crate") || tool.name == "list_crates" {
        return tool;
    }
    let arguments = if tool.name == "build_external_docs" {
        GIT_ARGUMENTS_TO_BUILD
    } else {
        GIT_ARGUMENTS
    };
    for &(name, description) in arguments {
        properties.insert(
            name.to_owned(),
            json!({ "type": "string", "description": description }),
        );
    }
    tool.input_schema = Arc::new(schema);
    tool
}

/// The arguments of a crate of a git repository to look up, and what each is (§6.4, decided in
/// M2e's E3b): what `build_external_docs` recorded.
const GIT_ARGUMENTS: &[(&str, &str)] = &[
    (
        "git",
        "The URL of a git repository (https://, ssh://, http:// or git://) that the crate is in, \
         which build_external_docs documented. Takes no version.",
    ),
    (
        "rev",
        "With `git`: the commit, as its hash (7 to 64 hex digits; a short one must begin only one \
         of the commits documented) or a ref under refs/, at the commit that build_external_docs \
         last resolved it to (stale after a day).",
    ),
    (
        "branch",
        "With `git`: the branch, at the commit that build_external_docs last resolved it to, \
         which it resolves anew when called again (stale after a day). Without rev, branch and \
         tag: the default branch.",
    ),
    (
        "tag",
        "With `git`: the tag, at the commit that build_external_docs last resolved it to (stale \
         after a day).",
    ),
];

/// The arguments of a crate of a git repository to document, for `build_external_docs`.
const GIT_ARGUMENTS_TO_BUILD: &[(&str, &str)] = &[
    (
        "git",
        "The URL of a git repository (https://, ssh://, http:// or git://) that the crate is in, \
         to document the crate at the commit that rev, branch or tag names. Takes no version.",
    ),
    (
        "rev",
        "With `git`: the commit, as its hash (7 to 64 hex digits) or a ref under refs/.",
    ),
    (
        "branch",
        "With `git`: the branch, resolved anew on each call. Without rev, branch and tag: the \
         default branch.",
    ),
    ("tag", "With `git`: the tag, resolved anew on each call."),
];

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

/// The annotations of every query tool (requirement R-MCP-4). With local features, each may run
/// cargo or rustc on the project (`cargo metadata`, `rustc -vV`, and builds from M2), whose
/// settings can run any program, so none is read-only. Without them, none runs anything.
fn annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(!cfg!(feature = "local"))
        .destructive(false)
        .idempotent(true)
        .open_world(true)
}

/// The annotations of `build_external_docs` (requirement R-MCP-4): it builds, and calling it again
/// with the same arguments does it again (requirement R-WS-10), so it is not idempotent.
#[cfg(feature = "local")]
fn external_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(false)
        .idempotent(false)
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
    let mut progress = Progress::of(&request, &context);
    let waits = Waits::default();
    let running = call.fetches().scope(
        waits
            .clone()
            .scope(dispatch(request, context, state).instrument(span)),
    );
    // On the heap: the futures of the tools are large.
    let mut running = Box::pin(running);
    // The first tick one period from now.
    let mut ticks = tokio::time::interval(PROGRESS_EVERY);
    ticks.reset();
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // On `notifications/cancelled`, the call stops: its task is aborted, and with it what it was
    // fetching, unless another call waits for the same or a call has told that it goes on
    // (requirement R-WS-6). rmcp sends no response to a cancelled request. Progress is told only
    // in this loop, so that none follows the response or the cancellation (requirement R-WS-6).
    let called = loop {
        // The response and the cancellation before a tick that is due at the same time.
        tokio::select! {
            biased;
            called = &mut running => break called,
            () = cancelled.cancelled() => {
                let mut result = tool_error("cancelled", "the call was cancelled", false, &[]);
                call.finish(&mut result);
                return Ok(result.into());
            },
            _ = ticks.tick(), if progress.is_some() => {
                if let Some(progress) = &mut progress {
                    progress.tell(&waits).await;
                }
            },
        }
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

/// How often a call that takes long tells its progress (requirement R-WS-6: from 1 to 30 seconds).
const PROGRESS_EVERY: Duration = Duration::from_secs(3);

/// The progress notifications of a call whose request has a progress token (requirement R-WS-6).
struct Progress {
    peer: rmcp::Peer<RoleServer>,
    token: ProgressToken,
    /// What the last notification said, in seconds since the call started.
    told: u64,
}

impl Progress {
    /// The progress notifications of the call of `request`, when its request asked for them with
    /// a progress token. `sleep` tells its own.
    fn of(request: &CallToolRequestParams, context: &RequestContext<RoleServer>) -> Option<Self> {
        #[cfg(debug_assertions)]
        if request.name == sleep::NAME {
            return None;
        }
        #[cfg(not(debug_assertions))]
        let _ = request;
        Some(Self {
            peer: context.peer.clone(),
            token: context.meta.get_progress_token()?,
            told: 0,
        })
    }

    /// Tells how long the call has taken and what it waits for. The progress goes up with each
    /// notification (requirement R-WS-6), by [`PROGRESS_EVERY`].
    async fn tell(&mut self, waits: &Waits) {
        self.told = self.told.saturating_add(PROGRESS_EVERY.as_secs());
        let progress = self.told;
        let doing = waits
            .message()
            .unwrap_or_else(|| "looking it up".to_owned());
        #[expect(
            clippy::cast_precision_loss,
            reason = "seconds that a call takes are far below 2^52"
        )]
        let notification = ProgressNotificationParam::new(self.token.clone(), progress as f64)
            .with_message(format!("{doing} ({progress} s)"));
        if let Err(error) = self.peer.notify_progress(notification).await {
            tracing::debug!(%error, "failed to send a progress notification");
        }
    }
}

/// Runs the tool that `request` names. See [`call`].
async fn dispatch(
    request: CallToolRequestParams,
    context: RequestContext<RoleServer>,
    state: &Arc<ToolState>,
) -> Result<CallToolResult, ErrorData> {
    match request.name.as_ref() {
        #[cfg(feature = "local")]
        build_external_docs::NAME => {
            match build_external_docs::parse_arguments(request.arguments.as_ref()) {
                Ok(arguments) => {
                    Ok(run_tool(build_external_docs::execute(arguments, Arc::clone(state))).await)
                },
                Err(error) => Ok(error.into_result()),
            }
        },
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
        sleep::NAME if test_tools() => match sleep::parse_arguments(request.arguments.as_ref()) {
            Ok(seconds) => Ok(run_tool(sleep::execute(seconds, context)).await),
            Err(error) => Ok(error.into_result()),
        },
        name => {
            // Only `sleep` uses the context, which only the tests have.
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

/// Spawns `future` as a task of the current tool call: in its span, noting its fetches
/// (requirement R-OBS-1) and what it waits for (requirement R-WS-6). Tokio's own `spawn` drops
/// them.
fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let span = tracing::Span::current();
    let (fetches, waits) = (Fetches::current(), Waits::current());
    tokio::spawn(
        async move {
            match (fetches, waits) {
                (Some(fetches), Some(waits)) => fetches.scope(waits.scope(future)).await,
                (Some(fetches), None) => fetches.scope(future).await,
                (None, Some(waits)) => waits.scope(future).await,
                (None, None) => future.await,
            }
        }
        .instrument(span),
    )
}

/// A task that is aborted when this is dropped before it finishes. Tokio's `JoinHandle` lets the
/// task go on.
pub(super) struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> AbortOnDrop<T> {
    pub(super) const fn new(handle: JoinHandle<T>) -> Self {
        Self(handle)
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(context)
    }
}

/// The schema of `building_reason`: why a crate is not ready yet (requirement R-OUT-9).
pub(super) fn building_reason_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["building", "waiting_for_lock", "waiting_for_slot", "fetching", "indexing"],
    })
}

/// `schema`, the `outputSchema` of a tool that looks a crate up, with the fields of the response
/// while the crate is being fetched or indexed, which has no `data` (requirement R-OUT-9):
/// `status`, `building_reason` and `retry_after_secs`, after `provenance`. Its root has no
/// `anyOf`, so what only some responses have is optional (requirement R-OUT-1).
pub(super) fn with_building(schema: JsonObject) -> JsonObject {
    let mut schema = schema;
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        let mut with = JsonObject::new();
        for (name, property) in std::mem::take(properties) {
            let after = name == "provenance";
            with.insert(name, property);
            if after {
                with.insert(
                    "status".to_owned(),
                    json!({ "type": "string", "enum": ["building"] }),
                );
                with.insert("building_reason".to_owned(), building_reason_schema());
                with.entry("retry_after_secs")
                    .or_insert_with(|| json!({ "type": "integer" }));
            }
        }
        *properties = with;
    }
    // The response tells when to call again.
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties
            .entry("hints")
            .or_insert_with(|| json!({ "type": "array", "items": { "type": "string" } }));
    }
    schema.insert("required".to_owned(), json!(["provenance"]));
    schema
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

    #[cfg(feature = "local")]
    #[test]
    fn a_call_waits_for_a_build_no_longer_than_for_work() {
        let wait = Duration::from_secs(20);
        let settings = |toml: &str| {
            let file = (std::path::PathBuf::from("config.toml"), toml.to_owned());
            Settings::from_sources(Some(&file), [])
        };
        assert_eq!(
            wait_for_builds(&settings(""), wait),
            Duration::from_secs(10)
        );
        assert_eq!(
            wait_for_builds(&settings("[build]\nwait_secs = 5\n"), wait),
            Duration::from_secs(5)
        );
        assert_eq!(
            wait_for_builds(&settings("[build]\nwait_secs = 60\n"), wait),
            wait
        );
    }

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

    /// With local features, every query tool takes `refresh`, as each may run cargo (§6.4);
    /// without them, none does (requirement R-ARCH-3). `build_external_docs` does not (requirement
    /// R-WS-10).
    #[test]
    fn every_query_tool_takes_refresh_with_local_features() {
        for tool in definitions() {
            let takes = tool
                .input_schema
                .get("properties")
                .and_then(|properties| properties.get("refresh"))
                .is_some();
            assert_eq!(
                takes,
                cfg!(feature = "local")
                    && tool.name != "sleep"
                    && tool.name != "build_external_docs",
                "{}",
                tool.name
            );
        }
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

    /// The tools that look one crate up, which share the shape of their responses while it is
    /// being fetched.
    const ONE_CRATE: [&str; 7] = [
        get_item::NAME,
        crate_overview::NAME,
        list_module::NAME,
        list_impls::NAME,
        list_methods::NAME,
        list_implementors::NAME,
        search_items::NAME,
    ];

    /// Checks that `result` has only what the `outputSchema` of each of `tools` declares, at its
    /// root and in its provenance, and what they require.
    #[track_caller]
    fn fits_the_schemas(result: CallToolResult, tools: &[&str]) {
        let response = result.structured_content.expect("structured");
        let mut checked = 0_usize;
        for tool in definitions()
            .into_iter()
            .filter(|tool| tools.contains(&tool.name.as_ref()))
        {
            checked = checked.saturating_add(1);
            let schema = Value::Object((*tool.output_schema.expect("a schema")).clone());
            for (object, schema) in [
                (&response, &schema),
                (&response["provenance"], &schema["properties"]["provenance"]),
            ] {
                let fields = object.as_object().expect("an object");
                for name in fields.keys() {
                    assert!(
                        schema["properties"].get(name).is_some(),
                        "{}: {name} is not declared",
                        tool.name
                    );
                }
                for required in schema["required"].as_array().into_iter().flatten() {
                    let required = required.as_str().expect("a name");
                    assert!(
                        fields.contains_key(required),
                        "{}: {required} is missing",
                        tool.name
                    );
                }
            }
        }
        assert_eq!(checked, tools.len(), "every tool is in this build");
    }

    /// The response while std is being fetched, whose provenance has no version yet, fits the
    /// tools that may return it (requirement R-OUT-9).
    #[test]
    fn a_response_while_std_is_fetched_fits_the_schemas() {
        let target = "x86_64-unknown-linux-gnu".parse().expect("a triple");
        let result =
            get_item::std_building("std", &target, crate::data::flight::Doing::Fetching, &[]);
        fits_the_schemas(result, &ONE_CRATE);
    }

    /// The responses while cargo resolves the project, before the version is known, fit the tools
    /// that may return them (requirement R-OUT-9).
    #[cfg(feature = "local")]
    #[test]
    fn responses_while_the_project_is_resolved_fit_the_schemas() {
        use crate::data::flight::Doing;
        let target = "x86_64-unknown-linux-gnu".parse().expect("a triple");
        let name = "serde".parse().expect("a crate name");
        let going = (Doing::WaitingForLock, "claude_project_dir");
        let mut tools = ONE_CRATE.to_vec();
        tools.push(resolve_version::NAME);
        fits_the_schemas(project::resolving_crate(&name, &target, going, &[]), &tools);
        let mut provenance = serde_json::Map::new();
        provenance.insert("source".to_owned(), json!("project"));
        provenance.insert("workspace".to_owned(), json!("/w"));
        provenance.insert("project_from".to_owned(), json!("claude_project_dir"));
        provenance.insert("target".to_owned(), json!("x86_64-unknown-linux-gnu"));
        fits_the_schemas(project::resolving(provenance, Doing::Fetching, &[]), &[
            list_crates::NAME,
        ]);
        let mut provenance = serde_json::Map::new();
        provenance.insert("target".to_owned(), json!("x86_64-unknown-linux-gnu"));
        provenance.insert("project_from".to_owned(), json!("claude_project_dir"));
        fits_the_schemas(project::resolving(provenance, Doing::Fetching, &[]), &[
            search_items::NAME,
        ]);
    }

    #[tokio::test]
    async fn run_tool_returns_the_result_of_the_tool() {
        let expected = CallToolResult::structured(json!({ "text": "hi" }));
        let result = run_tool(std::future::ready(expected.clone())).await;
        assert_eq!(result, expected);
    }
}
