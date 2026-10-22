//! Searching several crates together: the project's direct dependencies when no crate is given,
//! or std with alloc and core (requirements R-SR-2 and R-SR-5).
//!
//! The crates are loaded at once, up to a time limit (requirement R-ARCH-6). Those still being
//! fetched or indexed then are left out of this answer, which says so (`partial`); their loading
//! goes on, for the next call.

#[cfg(feature = "local")]
use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::CallToolResult;
use semver::Version;
use serde_json::{Map, Value, json};
#[cfg(feature = "local")]
use tokio::time::Instant;

use super::{
    ErrorChain,
    ToolState,
    get_item::{self, Searching, StdUnready},
};
use crate::{
    data::{
        archive::Mapped,
        crate_name::CrateName,
        dist::StdCrate,
        flight::Doing,
        std_json::Chosen,
        target::TargetTriple,
    },
    query::{
        across::Indexed,
        get_item::{Detail, ItemRequest, Origin, ProjectFeatures, ResolvedFrom},
        load,
        search::{SearchOptions, Searched, search_across},
        view::CrateView,
    },
};
#[cfg(feature = "local")]
use crate::{
    data::{
        features::FeatureSets,
        flight::Joined,
        packages::{PackageGraph, Source},
    },
    query::load::ArchiveError,
};

/// When to ask again for the crates still being fetched or indexed.
const RETRY_AFTER_SECS: u64 = get_item::RETRY_AFTER_SECS;

/// A crate of the scope, and how it fared.
struct Member {
    name: CrateName,
    version: Version,
    outcome: Outcome,
}

/// How loading a crate of the scope fared.
enum Outcome {
    /// Its archive, to search.
    Loaded(Arc<Mapped<Indexed>>),
    /// It could not be loaded: the code, the reason and whether trying again may help (§8).
    Failed(Value),
    /// It was still being fetched or indexed, as this says, when the search stopped waiting.
    #[cfg_attr(
        not(feature = "local"),
        expect(
            dead_code,
            reason = "only the dependencies of a project are fetched at once"
        )
    )]
    Going(Doing),
}

/// The features of the packages in the build of `workspace` for `target`, waited for until
/// `until`, or `None` with a warning: without them, all direct dependencies are searched.
#[cfg(feature = "local")]
async fn build_sets(
    state: &ToolState,
    workspace: &crate::data::project::Workspace,
    target: &TargetTriple,
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Option<Arc<FeatureSets>> {
    match state
        .resolver
        .features_until(workspace, target, until)
        .await
    {
        Joined::Done(Ok(sets)) => Some(sets),
        Joined::Done(Err(error)) => {
            warnings.push(
                format!(
                    "which dependencies the project builds for {target} is not known, so all \
                     direct ones are searched: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
        Joined::Ended => {
            warnings.push(
                format!(
                    "which dependencies the project builds for {target} is not known, so all \
                     direct ones are searched: {}",
                    get_item::NOT_READ
                )
                .into(),
            );
            None
        },
        Joined::Going(doing) => {
            warnings.push(
                format!(
                    "which dependencies the project builds for {target} is not known yet, so all \
                     direct ones are searched: {}",
                    get_item::still_reading(doing)
                )
                .into(),
            );
            None
        },
    }
}

/// Searches the project's direct dependencies that its normal build or its build with the
/// dev-dependencies has for `target` (decided in M1 C4b; the workspace's own crates are M2).
#[cfg(feature = "local")]
pub(super) async fn dependencies(
    state: Arc<ToolState>,
    project: Option<PathBuf>,
    target: super::project::LookupTarget,
    detail: Detail,
    searching: Searching,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let until = state.until();
    // docs.rs's default target of each crate, for a project that builds for a target of its own
    // (decided in M2's D7).
    let docs_rs = target.docs_rs().cloned();
    let super::project::LookupTarget {
        triple: target,
        docs_rs_default,
    } = target;
    let (workspace, project_from) =
        match super::project::workspace(state.project.as_ref(), project.as_deref()) {
            Ok(Some(found)) => found,
            Ok(None) => {
                return super::tool_error(
                    "invalid_argument",
                    "there is no project here whose dependencies to search, so `crate` is \
                     required",
                    false,
                    &[r#"pass the crate to search, for example {"crate": "tokio"}"#],
                );
            },
            Err(error) => return *error,
        };
    let resolution = match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => return super::project::resolve_error(&workspace, &error),
        Joined::Ended => return get_item::stopped(),
        Joined::Going(doing) => {
            let mut provenance = Map::new();
            if let Some(target) = &docs_rs {
                provenance.insert("target".to_owned(), json!(target.as_str()));
            }
            provenance.insert("project_from".to_owned(), json!(project_from.as_str()));
            return super::project::resolving(provenance, doing, &warnings);
        },
    };
    let sets = build_sets(&state, &workspace, &target, until, &mut warnings).await;
    let graph = PackageGraph::new(&resolution);
    let mut members = Vec::new();
    let mut loads = Vec::new();
    for package in graph.direct_packages() {
        let built = sets.as_ref().is_none_or(|sets| {
            sets.normal(&package.name, &package.version).is_some()
                || sets.with_dev(&package.name, &package.version).is_some()
        });
        let Ok(name) = package.name.parse::<CrateName>() else {
            continue;
        };
        if !built {
            continue;
        }
        if package.source != Source::CratesIo {
            members.push(Member {
                name,
                version: package.version.clone(),
                outcome: Outcome::Failed(json!({
                    "code": "docs_unavailable",
                    "reason": "local_build_required",
                    "retryable": false,
                })),
            });
            continue;
        }
        let version = package.version.clone();
        loads.push((name.clone(), version.clone()));
        members.push(Member {
            name,
            version,
            outcome: Outcome::Going(Doing::Fetching),
        });
    }
    let loaded = load_all(&state, loads, docs_rs.as_ref(), until).await;
    for member in &mut members {
        if let Some(outcome) = loaded
            .iter()
            .position(|(name, version, _)| *name == member.name && *version == member.version)
            .and_then(|at| loaded.get(at))
        {
            member.outcome = match &outcome.2 {
                Outcome::Loaded(mapped) => Outcome::Loaded(Arc::clone(mapped)),
                Outcome::Failed(failure) => Outcome::Failed(failure.clone()),
                Outcome::Going(doing) => Outcome::Going(*doing),
            };
        }
    }
    let context = Context {
        target,
        docs_rs_default,
        project_from: Some(project_from.as_str()),
        resolved_from: resolution_from(&resolution),
        sets,
        std: None,
        detail,
        searching,
    };
    respond(members, context, warnings).await
}

/// How the project's versions were resolved.
#[cfg(feature = "local")]
const fn resolution_from(resolution: &crate::data::metadata::Resolution) -> ResolvedFrom {
    match resolution {
        crate::data::metadata::Resolution::Metadata { .. } => ResolvedFrom::CargoLock,
        crate::data::metadata::Resolution::CargoLockRaw { .. } => ResolvedFrom::CargoLockRaw,
    }
}

/// Searches std together with alloc and core, whose items std re-exports, as `get_item` reads
/// them: from a nightly toolchain's documentation (requirement R-STD-1).
pub(super) async fn std(
    state: Arc<ToolState>,
    target: TargetTriple,
    detail: Detail,
    searching: Searching,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let until = state.until();
    let at_least = get_item::project_toolchain(&state).await;
    let host = &state.host().await.triple;
    let chosen = match get_item::shared_std(&state, &target, host, at_least.as_ref(), until).await {
        Ok(chosen) => chosen,
        Err(StdUnready::Failed(error)) => {
            return get_item::dist_error(StdCrate::Std, &error);
        },
        Err(StdUnready::Stopped) => return get_item::stopped(),
        Err(StdUnready::Going(doing)) => {
            return get_item::std_building("std", &target, doing, &warnings);
        },
    };
    warnings.extend(chosen.warnings.iter().cloned());
    let version = chosen
        .json
        .release()
        .unwrap_or_else(|| Version::new(0, 0, 0));
    // Archived by the shared work already, unless it failed there, or another process makes them
    // now: until the call's deadline (requirement R-ARCH-6). What is being made goes on.
    let archiving = async {
        let mut archived = Vec::new();
        for krate in [StdCrate::Std, StdCrate::Alloc, StdCrate::Core] {
            archived.push((
                krate,
                load::archive_std_crate(&state.cache, &chosen, krate).await?,
            ));
        }
        Some(archived)
    };
    let archived = match tokio::time::timeout_at(until, archiving).await {
        Ok(Some(archived)) => archived,
        Ok(None) => return get_item::shutting_down(),
        Err(_) => return get_item::std_building("std", &target, Doing::Indexing, &warnings),
    };
    let members = archived
        .into_iter()
        .filter_map(|(krate, archived)| {
            let name: CrateName = krate.as_str().parse().ok()?;
            let outcome = match archived {
                Ok(mapped) => Outcome::Loaded(mapped),
                Err(error) => {
                    tracing::warn!(krate = krate.as_str(), error = %ErrorChain(&error), "the JSON of std could not be loaded");
                    Outcome::Failed(json!({
                        "code": "docs_unavailable",
                        "reason": "unparsable",
                        "retryable": false,
                    }))
                },
            };
            Some(Member {
                name,
                version: version.clone(),
                outcome,
            })
        })
        .collect();
    let context = Context {
        target,
        docs_rs_default: false,
        project_from: None,
        resolved_from: ResolvedFrom::Latest,
        #[cfg(feature = "local")]
        sets: None,
        std: Some(StdContext {
            chosen,
            project_release: at_least,
        }),
        detail,
        searching,
    };
    respond(members, context, warnings).await
}

/// The JSON of std that was chosen, for the provenance.
struct StdContext {
    chosen: Arc<Chosen>,
    project_release: Option<Version>,
}

/// What the search of the scope shares.
struct Context {
    target: TargetTriple,
    /// Whether docs.rs's JSON of each crate is of its default target there, not of `target`
    /// (decided in M2's D7).
    docs_rs_default: bool,
    project_from: Option<&'static str>,
    resolved_from: ResolvedFrom,
    /// The project's builds, for `enabled_in_project`.
    #[cfg(feature = "local")]
    sets: Option<Arc<FeatureSets>>,
    std: Option<StdContext>,
    detail: Detail,
    searching: Searching,
}

/// Loads the archives of `releases` at once, waiting for them until `until`. Loading what has not
/// finished then goes on, so that the next search has it.
#[cfg(feature = "local")]
async fn load_all(
    state: &Arc<ToolState>,
    releases: Vec<(CrateName, Version)>,
    target: Option<&TargetTriple>,
    until: Instant,
) -> Vec<(CrateName, Version, Outcome)> {
    let handles: Vec<_> = releases
        .into_iter()
        .map(|(name, version)| {
            let (state, target) = (Arc::clone(state), target.cloned());
            let (task_name, task_version) = (name.clone(), version.clone());
            // Aborted if the call is cancelled while it waits (requirement R-WS-6).
            let handle = super::AbortOnDrop::new(super::spawn(async move {
                load_release(&state, &task_name, &task_version, target.as_ref(), until).await
            }));
            (name, version, handle)
        })
        .collect();
    let mut outcomes = Vec::new();
    for (name, version, handle) in handles {
        let outcome = handle.await.unwrap_or_else(|error| {
            tracing::error!(%error, "loading a crate to search did not finish");
            Outcome::Failed(json!({ "code": "internal_error", "retryable": true }))
        });
        outcomes.push((name, version, outcome));
    }
    outcomes
}

/// The archive of `name` at `version` for `target`: from the cache, or made from its JSON on
/// docs.rs by work that the calls for it share, waiting for it until `until`.
#[cfg(feature = "local")]
async fn load_release(
    state: &ToolState,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
    until: Instant,
) -> Outcome {
    let dir = state.cache.docs_rs_dir(name, version, target);
    let cache = state.cache.clone();
    let opened = crate::task::spawn_blocking(move || load::open_archived(&cache, &dir))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    if let Some(mapped) = opened {
        return Outcome::Loaded(mapped);
    }
    let archived = load::docs_rs_archive(
        &state.archives,
        &state.cache,
        &state.docs_rs,
        name,
        version,
        target,
        until,
    )
    .await;
    match archived {
        Joined::Done(Ok(archived)) => Outcome::Loaded(Arc::clone(&archived.mapped)),
        Joined::Done(Err(error)) => match &*error {
            ArchiveError::Load(error) => {
                let result = get_item::load_error_of(
                    &state.cache,
                    name,
                    &format!("{name}@{version}"),
                    error,
                );
                Outcome::Failed(failure_of(&result))
            },
            ArchiveError::Make(error) => {
                tracing::warn!(%name, %version, error = %ErrorChain(error), "a crate to search could not be archived");
                Outcome::Failed(json!({
                    "code": "docs_unavailable",
                    "reason": "unparsable",
                    "retryable": false,
                }))
            },
            ArchiveError::Stopped => {
                Outcome::Failed(json!({ "code": "internal_error", "retryable": true }))
            },
        },
        Joined::Ended => Outcome::Failed(json!({ "code": "internal_error", "retryable": true })),
        Joined::Going(doing) => Outcome::Going(doing),
    }
}

/// The code, the reason and whether trying again may help, of the tool error `result`.
#[cfg(feature = "local")]
fn failure_of(result: &CallToolResult) -> Value {
    let body: Option<Value> = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str(&text.text).ok());
    let mut failure = Map::new();
    for key in ["code", "reason", "retryable"] {
        if let Some(value) = body.as_ref().and_then(|body| body.get(key)) {
            failure.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(failure)
}

/// Searches the crates that loaded, and puts what the others are in `provenance.sources`
/// (requirements R-OUT-9 and R-SR-5).
async fn respond(
    members: Vec<Member>,
    context: Context,
    warnings: Vec<Box<str>>,
) -> CallToolResult {
    let task = crate::task::spawn_blocking(move || search_members(&members, &context, warnings));
    task.await.unwrap_or_else(|error| {
        let error = crate::task::resume_if_panic(error);
        tracing::error!(%error, "a search across crates did not finish");
        get_item::shutting_down()
    })
}

/// The response of the search of `members`.
fn search_members(
    members: &[Member],
    context: &Context,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let loaded: Vec<(&Member, CrateView<'_>)> = members
        .iter()
        .filter_map(|member| match &member.outcome {
            Outcome::Loaded(mapped) => Some((member, CrateView::new(mapped.get()))),
            Outcome::Failed(_) | Outcome::Going(_) => None,
        })
        .collect();
    let toolchain = context.std.as_ref().map(|std| std.chosen.json.toolchain());
    let searched: Vec<Searched<'_>> = loaded
        .iter()
        .map(|(member, view)| Searched {
            view,
            request: request_of(member, context, toolchain.as_deref()),
        })
        .collect();
    let searching = &context.searching;
    let kinds: Vec<&str> = searching.kinds.to_vec();
    let found = match search_across(&searched, SearchOptions {
        query: &searching.query,
        fields: searching.fields,
        kinds: &kinds,
        limit: searching.limit,
        offset: searching.offset,
        cursor_request: &searching.request_digest,
        cursor_data: searching.cursor_data.as_deref(),
        entry: entry_crate(context, &searched),
    }) {
        Ok(found) => found,
        Err(error) => {
            return super::tool_error("invalid_argument", &error.to_string(), false, &[
                "call search_items again without cursor",
            ]);
        },
    };
    let (sources, failed, fetching) = sources(&found.provenances, members, context);
    if failed > 0 || fetching > 0 {
        warnings.insert(
            0,
            format!(
                "{} of {} crates were searched: {fetching} still being loaded, {failed} not \
                 available (see provenance.sources)",
                loaded.len(),
                members.len()
            )
            .into(),
        );
    }
    let mut response = Map::new();
    response.insert("provenance".to_owned(), provenance(context, sources));
    // Nothing found while crates are still being loaded: the answer is still to come
    // (requirement R-OUT-9).
    let nothing_yet = fetching > 0 && found.page.total == 0;
    if nothing_yet {
        response.insert("status".to_owned(), json!("building"));
        response.insert(
            "building_reason".to_owned(),
            json!(building_reason(members)),
        );
    }
    if failed > 0 || fetching > 0 {
        response.insert("partial".to_owned(), json!(true));
    }
    if fetching > 0 {
        response.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
    }
    let mut all_warnings: Vec<Value> = warnings.iter().map(|warning| json!(warning)).collect();
    all_warnings.extend(found.warnings.iter().map(|warning| json!(warning)));
    if !all_warnings.is_empty() {
        response.insert("warnings".to_owned(), Value::Array(all_warnings));
    }
    response.insert(
        "data".to_owned(),
        serde_json::to_value(&found.data).unwrap_or_default(),
    );
    response.insert(
        "page".to_owned(),
        serde_json::to_value(&found.page).unwrap_or_default(),
    );
    let hints = hints(
        &found.hints,
        nothing_yet,
        fetching > 0 && found.page.has_more,
    );
    if !hints.is_empty() {
        response.insert("hints".to_owned(), Value::Array(hints));
    }
    CallToolResult::structured(Value::Object(response))
}

/// The request of the search of `member`, whose provenance is that of std (of `toolchain`) when
/// std is searched.
fn request_of<'a>(
    member: &'a Member,
    context: &'a Context,
    toolchain: Option<&'a str>,
) -> ItemRequest<'a> {
    ItemRequest {
        name: &member.name,
        version: &member.version,
        path: member.name.as_str(),
        origin: match (&context.std, toolchain) {
            (Some(std), Some(toolchain)) => Origin::Std {
                toolchain,
                std_from: std.chosen.json.source(),
                why: &std.chosen.why,
                project_release: std.project_release.as_ref(),
            },
            (Some(_) | None, _) => Origin::DocsRs {
                resolved_from: context.resolved_from,
                project_from: context.project_from,
                build: None,
            },
        },
        detail: context.detail,
        project_features: project_features(context, member),
        follow: false,
        target: (!context.docs_rs_default).then(|| context.target.as_str()),
    }
}

/// The provenance of the response: what the crates searched share, and each one's `sources`
/// (requirement R-OUT-9).
fn provenance(context: &Context, sources: Vec<Value>) -> Value {
    let mut provenance = Map::new();
    if !context.docs_rs_default {
        provenance.insert("target".to_owned(), json!(context.target.as_str()));
    }
    if let Some(project_from) = context.project_from {
        provenance.insert("project_from".to_owned(), json!(project_from));
    }
    provenance.insert("sources".to_owned(), Value::Array(sources));
    Value::Object(provenance)
}

/// The hints of the response: those of the search, and what to do when crates are still being
/// loaded and nothing was found (`nothing_yet`) or there is a next page (`cursor_while_loading`).
fn hints(found: &[Box<str>], nothing_yet: bool, cursor_while_loading: bool) -> Vec<Value> {
    let mut hints: Vec<Value> = found.iter().map(|hint| json!(hint)).collect();
    if nothing_yet {
        hints.push(json!(format!(
            "no item matched in the crates searched so far; call again in about \
             {RETRY_AFTER_SECS} seconds to search the crates still being loaded too"
        )));
    } else if cursor_while_loading {
        // The cursor is bound to the crates searched (requirement R-OUT-3).
        hints.push(json!(
            "once the crates still being loaded are in, this cursor is refused as the data \
             changed; then search again without the cursor"
        ));
    }
    hints
}

/// Why the crates of `members` that are still being loaded are not in yet: `indexing` when any is
/// being indexed, as that is the last step (requirement R-OUT-9).
fn building_reason(members: &[Member]) -> &'static str {
    let indexing = members
        .iter()
        .any(|member| matches!(member.outcome, Outcome::Going(Doing::Indexing)));
    if indexing {
        Doing::Indexing.as_str()
    } else {
        Doing::Fetching.as_str()
    }
}

/// Which crate of `searched` the others' items are shown through: std, which re-exports the
/// items of alloc and core, when they are searched together (requirement R-PR-2).
fn entry_crate(context: &Context, searched: &[Searched<'_>]) -> Option<usize> {
    context.std.as_ref()?;
    searched
        .iter()
        .position(|source| source.request.name.as_str() == "std")
}

/// The sources of the response (requirement R-OUT-9): the provenance of each crate searched,
/// then each crate that was not, with why. Also how many failed and how many are being fetched.
fn sources(
    provenances: &[crate::query::get_item::Provenance<'_>],
    members: &[Member],
    context: &Context,
) -> (Vec<Value>, usize, usize) {
    let mut sources: Vec<Value> = Vec::new();
    for provenance in provenances {
        let mut source = serde_json::to_value(provenance).unwrap_or_default();
        if let Some(fields) = source.as_object_mut() {
            fields.insert("status".to_owned(), json!("ok"));
        }
        sources.push(source);
    }
    let (mut failed, mut fetching) = (0_usize, 0_usize);
    for member in members {
        let mut source = Map::new();
        source.insert("crate".to_owned(), json!(member.name.as_str()));
        source.insert("version".to_owned(), json!(member.version.to_string()));
        if !context.docs_rs_default {
            source.insert("target".to_owned(), json!(context.target.as_str()));
        }
        match &member.outcome {
            Outcome::Loaded(_) => continue,
            Outcome::Failed(failure) => {
                failed = failed.saturating_add(1);
                let status = if failure.get("reason") == Some(&json!("local_build_required")) {
                    "unavailable"
                } else {
                    "failed"
                };
                source.insert("status".to_owned(), json!(status));
                if let Some(failure) = failure.as_object() {
                    source.extend(failure.clone());
                }
            },
            Outcome::Going(doing) => {
                fetching = fetching.saturating_add(1);
                source.insert("status".to_owned(), json!("building"));
                source.insert("building_reason".to_owned(), json!(doing.as_str()));
                source.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
            },
        }
        sources.push(Value::Object(source));
    }
    (sources, failed, fetching)
}

/// The features that the project builds `member` with, for `enabled_in_project`.
#[cfg(feature = "local")]
fn project_features<'c>(context: &'c Context, member: &Member) -> Option<ProjectFeatures<'c>> {
    let sets = context.sets.as_ref()?;
    Some(ProjectFeatures {
        normal: sets.normal(member.name.as_str(), &member.version),
        with_dev: sets.with_dev(member.name.as_str(), &member.version),
    })
}

/// Without local features, there is no project.
#[cfg(not(feature = "local"))]
const fn project_features<'c>(
    _context: &'c Context,
    _member: &Member,
) -> Option<ProjectFeatures<'c>> {
    None
}
