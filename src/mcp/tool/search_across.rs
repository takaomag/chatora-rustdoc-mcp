//! Searching several crates together: the workspace's members and the project's direct
//! dependencies when no crate is given, or std with alloc and core (requirements R-SR-2 and
//! R-SR-5).
//!
//! The crates are loaded, or built here, at once, up to a time limit (requirement R-ARCH-6).
//! Those still being fetched, indexed or built then are left out of this answer, which says so
//! (`partial`); their loading goes on, for the next call.

#[cfg(feature = "local")]
use std::path::PathBuf;
use std::{path::Path, sync::Arc};

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
#[cfg(feature = "local")]
use super::{
    local::{self, Member as Planned},
    recorded_workspace,
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
        packages::{PackageGraph, ProjectPackage, Source},
        project::Workspace,
    },
    query::{load::ArchiveError, local::LocalBuilt},
};

/// When to ask again for the crates still being fetched or indexed.
const RETRY_AFTER_SECS: u64 = get_item::RETRY_AFTER_SECS;

/// A crate of the scope, and how it fared.
struct Member {
    name: CrateName,
    version: Version,
    outcome: Outcome,
}

impl Member {
    /// The root of its paths: the name of its crate built here, which a lib's name may make other
    /// than its package's, or else its name.
    fn root(&self) -> &str {
        #[cfg(feature = "local")]
        if let Outcome::Built(built) | Outcome::Recorded(built, _) = &self.outcome {
            return &built.record.crate_name;
        }
        self.name.as_str()
    }
}

/// How loading a crate of the scope fared.
enum Outcome {
    /// Its archive, to search.
    Loaded(Arc<Mapped<Indexed>>),
    /// It could not be loaded: the code, the reason and whether trying again may help (§8).
    Failed(Value),
    /// It was still being fetched, indexed or built, as this says, when the search stopped
    /// waiting.
    #[cfg_attr(
        not(feature = "local"),
        expect(
            dead_code,
            reason = "only the dependencies of a project are fetched at once"
        )
    )]
    Going(Doing),
    /// Built here, a workspace member's or a dependency's (decided in M2's D7).
    #[cfg(feature = "local")]
    Built(Arc<LocalBuilt>),
    /// Built by `build_external_docs`, a crate of a workspace outside the default project, with
    /// whether what it is made from changed since (decided in M2e's E4b-3).
    #[cfg(feature = "local")]
    Recorded(Arc<LocalBuilt>, bool),
}

impl Outcome {
    /// Whether it was built here (decided in M2's D7).
    #[cfg_attr(
        not(feature = "local"),
        expect(
            clippy::unused_self,
            reason = "without local features, nothing is built here"
        )
    )]
    const fn is_built(&self) -> bool {
        #[cfg(feature = "local")]
        if let Self::Built(_) | Self::Recorded(..) = self {
            return true;
        }
        false
    }
}

/// The features of the packages in the build of `workspace` for `target`, waited for until
/// `until`, or `None` with a warning: without them, all direct dependencies are searched.
#[cfg(feature = "local")]
async fn build_sets(
    state: &ToolState,
    workspace: &Workspace,
    target: &TargetTriple,
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Option<Arc<FeatureSets>> {
    match super::project::features_until(state, workspace, target, until).await {
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

/// Searches the workspace's members and the project's direct dependencies that its normal build
/// or its build with the dev-dependencies has for `target` (decided in M1 C4b and M2's D7).
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
    let (workspace, project_from) = match super::project::located(&state, project.as_deref()).await
    {
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
    let resolution = match super::project::resolve_until(&state, &workspace, until).await {
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
    // Of a workspace outside the default project, what `build_external_docs` recorded for the
    // target, or else what tells to record it, or that it is being recorded (decided in M2e's
    // E4b-3b, 1).
    let outside = recorded_workspace::is_outside(&state, &workspace);
    if outside
        && let Err(result) = recorded_workspace::for_target_of(&state, &workspace, &target).await
    {
        return *result;
    }
    let sets = build_sets(&state, &workspace, &target, until, &mut warnings).await;
    let graph = PackageGraph::new(&resolution);
    // The workspace's members (decided in M2's D7), and its direct dependencies.
    let mut scope: Vec<(ProjectPackage, Kind)> = graph
        .packages()
        .iter()
        .filter(|package| graph.is_member(package))
        .map(|package| (package.clone(), Kind::Member))
        .collect();
    for package in graph.direct_packages() {
        let built = sets.as_ref().is_none_or(|sets| {
            sets.normal(&package.name, &package.version).is_some()
                || sets.with_dev(&package.name, &package.version).is_some()
        });
        if built {
            let kind = if package.source == Source::CratesIo {
                Kind::CratesIo
            } else {
                Kind::Dependency
            };
            scope.push((package.clone(), kind));
        }
    }
    let at = (&target, docs_rs.as_ref());
    let members = project_all(&state, &workspace, scope, at, until, &mut warnings).await;
    let resolved_from = match &*resolution {
        // As `build_external_docs` recorded it (decided in M2e's E4b-3).
        crate::data::metadata::Resolution::Metadata { .. } if outside => ResolvedFrom::External,
        crate::data::metadata::Resolution::Metadata { .. } => ResolvedFrom::CargoLock,
        crate::data::metadata::Resolution::CargoLockRaw { .. } => ResolvedFrom::CargoLockRaw,
    };
    // Not where it was, which is gone, and which the top-level tells (decided in M2e's E4b-3, 2).
    let gone = outside
        && recorded_workspace::since_recorded(&state, &workspace).await
            == Some(recorded_workspace::Since::Missing);
    let context = Context {
        recorded: (outside && !gone)
            .then(|| recorded_workspace::again(&workspace.root, Some(&target))),
        target: target.clone(),
        docs_rs_default,
        project_from: Some(project_from.as_str()),
        resolved_from,
        sets,
        dependencies: local::dependencies(&state),
        std: None,
        detail,
        searching,
    };
    let result = respond(members, context, warnings).await;
    // Of the record of a workspace that changed since (decided in M2e's E4b-3b, 3).
    let Some((recorded_at, again, since)) =
        recorded_workspace::changed_since(&state, &workspace, Some(&target)).await
    else {
        return result;
    };
    match &result.structured_content {
        Some(response) if result.is_error != Some(true) => {
            let mut response = response.clone();
            recorded_workspace::mark_changed(&mut response, &recorded_at, &again, since);
            CallToolResult::structured(response)
        },
        Some(_) | None => result,
    }
}

/// Searches std together with alloc and core, whose items std re-exports, as `get_item` reads
/// them: from a nightly toolchain's documentation (requirement R-STD-1).
pub(super) async fn std(
    state: Arc<ToolState>,
    target: TargetTriple,
    project: Option<&Path>,
    detail: Detail,
    searching: Searching,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let until = state.until();
    let at_least = get_item::project_toolchain(&state, project).await;
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
        recorded: None,
        target,
        docs_rs_default: false,
        project_from: None,
        resolved_from: ResolvedFrom::Latest,
        #[cfg(feature = "local")]
        sets: None,
        #[cfg(feature = "local")]
        dependencies: None,
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
    /// For a workspace outside the default project, the arguments of `build_external_docs` that
    /// record it for the target, which the hints tell (decided in M2e's E4b-3b).
    recorded: Option<String>,
    target: TargetTriple,
    /// Whether docs.rs's JSON of each crate is of its default target there, not of `target`
    /// (decided in M2's D7).
    docs_rs_default: bool,
    project_from: Option<&'static str>,
    resolved_from: ResolvedFrom,
    /// The project's builds, for `enabled_in_project`.
    #[cfg(feature = "local")]
    sets: Option<Arc<FeatureSets>>,
    /// Where cargo has the sources of the dependencies (decided in M3b, B5b).
    #[cfg(feature = "local")]
    dependencies: Option<crate::data::source_root::Dependencies>,
    std: Option<StdContext>,
    detail: Detail,
    searching: Searching,
}

/// What a package of the scope of the search is to the project.
#[cfg(feature = "local")]
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A workspace member, built here.
    Member,
    /// A dependency not from crates.io, built here.
    Dependency,
    /// A dependency from crates.io: its release on docs.rs, or built here if it is a copy, or if
    /// docs.rs has no JSON of it (decided in M2's D7).
    CratesIo,
}

/// Loads or builds the packages of `scope` of `workspace` at once, waiting for them until
/// `until`, for `target`, whose JSON on docs.rs is of `docs_rs` (requirements R-SR-2 and R-SR-5,
/// decided in M2's D7). What has not finished then goes on, so that the next search has it. A
/// member that has no crate to search is left out.
#[cfg(feature = "local")]
async fn project_all(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    scope: Vec<(ProjectPackage, Kind)>,
    (target, docs_rs): (&TargetTriple, Option<&TargetTriple>),
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Vec<Member> {
    // By the package's name, which the features of the project's build go by, as `get_item`
    // tells it; one that is not a crate's name is left out.
    let handles: Vec<_> = scope
        .into_iter()
        .filter_map(|(package, kind)| {
            let name = package.name.parse::<CrateName>().ok()?;
            let (state, workspace) = (Arc::clone(state), workspace.clone());
            let (target, docs_rs) = (target.clone(), docs_rs.cloned());
            let version = package.version.clone();
            // Aborted if the call is cancelled while it waits (requirement R-WS-6).
            let handle = super::AbortOnDrop::new(super::spawn(async move {
                let at = (&target, docs_rs.as_ref());
                project_outcome(&state, &workspace, (&package, kind), at, until).await
            }));
            Some((name, version, handle))
        })
        .collect();
    let mut members = Vec::new();
    let mut unchecked = Vec::new();
    for (name, version, handle) in handles {
        let (outcome, checked) = handle.await.unwrap_or_else(|error| {
            tracing::error!(%error, "loading a crate to search did not finish");
            let failure = json!({ "code": "internal_error", "retryable": true });
            (Some(Outcome::Failed(failure)), true)
        });
        if !checked {
            unchecked.push(format!("{name} {version}"));
        }
        if let Some(outcome) = outcome {
            members.push(Member {
                name,
                version,
                outcome,
            });
        }
    }
    // Once for all, as the index could not be read for them (decided in M2's D7).
    if !unchecked.is_empty() {
        warnings.push(
            format!(
                "whether the project's {} are the releases on crates.io is not known, as the \
                 index could not be read: their JSON on docs.rs is searched",
                unchecked.join(", ")
            )
            .into(),
        );
    }
    members
}

/// How the search finds `package`, of `kind`, of `workspace`, for `target`, whose JSON on docs.rs
/// is of `docs_rs`, waiting for it until `until`: `None` for a member that has no crate to search;
/// and whether a package of crates.io was checked against the index (decided in M2's D7).
#[cfg(feature = "local")]
async fn project_outcome(
    state: &ToolState,
    workspace: &Workspace,
    (package, kind): (&ProjectPackage, Kind),
    (target, docs_rs): (&TargetTriple, Option<&TargetTriple>),
    until: Instant,
) -> (Option<Outcome>, bool) {
    let dependency = async || {
        let planned = local::dependency(state, workspace, package, target, until).await;
        // Without a lib: none to search.
        let failure = json!({
            "code": "docs_unavailable",
            "reason": "local_build_required",
            "retryable": false,
        });
        Some(
            planned_outcome(state, planned, until)
                .await
                .unwrap_or(Outcome::Failed(failure)),
        )
    };
    match kind {
        Kind::Member => {
            let Ok(name) = package.lib_name.parse::<CrateName>() else {
                return (None, true);
            };
            let asked = local::Asked::Package {
                name: &package.name,
                version: &package.version,
            };
            let planned = local::member(state, workspace, (&name, asked), target, until).await;
            (planned_outcome(state, planned, until).await, true)
        },
        Kind::Dependency => (dependency().await, true),
        Kind::CratesIo => {
            let Ok(name) = package.name.parse::<CrateName>() else {
                return (None, true);
            };
            // Within the call's deadline, past which it is not checked (requirement R-ARCH-6).
            let checked =
                tokio::time::timeout_at(until, get_item::project_entry(state, package, &name))
                    .await
                    .ok()
                    .and_then(Result::ok);
            let unchecked = checked.is_none();
            match checked {
                Some(get_item::InIndex::Copy(_)) => (dependency().await, true),
                Some(get_item::InIndex::Unlisted(entry)) => {
                    let what = format!("{} {}", entry.name, package.version);
                    let error = get_item::stale_index_error(state, &what, &entry);
                    (Some(Outcome::Failed(failure_of(&error))), true)
                },
                Some(get_item::InIndex::Release(_)) | None => {
                    match load_release(state, &name, &package.version, docs_rs, until).await {
                        // docs.rs has no JSON of it: built here.
                        Outcome::Failed(failure)
                            if failure.get("reason").and_then(Value::as_str).is_some_and(
                                |reason| matches!(reason, "no_json" | "no_target_json"),
                            ) =>
                        {
                            (dependency().await, !unchecked)
                        },
                        outcome @ (Outcome::Loaded(_)
                        | Outcome::Failed(_)
                        | Outcome::Going(_)
                        | Outcome::Built(_)
                        | Outcome::Recorded(..)) => (Some(outcome), !unchecked),
                    }
                },
            }
        },
    }
}

/// The outcome of the build that `planned` plans, waited for until `until`, or why there is
/// none: `None` for a crate that the package does not have (decided in M2's D7).
#[cfg(feature = "local")]
async fn planned_outcome(
    state: &ToolState,
    planned: Result<Planned, Box<CallToolResult>>,
    until: Instant,
) -> Option<Outcome> {
    match planned {
        Ok(Planned::Is { wanted, .. }) => {
            // `build.wait_secs` from now, within the call's deadline (decided in M2's D3).
            let until_built = Instant::now()
                .checked_add(state.build_wait)
                .map_or(until, |until_built| until_built.min(until));
            let built = local::built(state, *wanted, (until_built, until), Map::new(), &[]).await;
            Some(match built {
                Ok(built) => Outcome::Built(built),
                Err(result) => outcome_of(&result),
            })
        },
        Err(error)
        | Ok(
            Planned::Unknown(error)
            | Planned::NotForTarget(error)
            | Planned::OtherTarget { error, .. }
            | Planned::Unnameable { error, .. },
        ) => Some(Outcome::Failed(failure_of(&error))),
        Ok(Planned::Recorded(recorded)) => Some(recorded_outcome(state, *recorded).await),
        Ok(Planned::Resolving(doing) | Planned::Planning(doing)) => Some(Outcome::Going(doing)),
        Ok(Planned::Not | Planned::OtherVersion(_)) => None,
    }
}

/// The outcome of what `build_external_docs` recorded of a crate of a workspace outside the default
/// project, as `recorded` tells: its build, with whether what it is made from changed since; while
/// it is made, as building; its failure; or else unavailable, with no record (decided in M2e's
/// E4b-3b).
#[cfg(feature = "local")]
async fn recorded_outcome(state: &ToolState, recorded: local::RecordedCrate) -> Outcome {
    let (name, version) = recorded
        .package
        .name_and_version()
        .unwrap_or((&recorded.package.0, ""));
    let what = format!("{name} {version} for {}", recorded.target);
    // Not where it was, which is gone (decided in M2e's E4b-3, 2).
    let call_again = if recorded.since == recorded_workspace::Since::Missing {
        recorded_workspace::MOVED.to_owned()
    } else {
        format!(
            "call build_external_docs with {} to build it again",
            recorded.again
        )
    };
    let provenance = Map::new;
    match super::recorded::recorded_as(state, (&recorded.key, &what, &call_again), provenance, &[])
        .await
    {
        // Of the crate itself: what the workspace became is told of the whole search (decided in
        // M2e's E4b-3b, 3).
        super::recorded::Found::Built(built) => {
            let changed = recorded.since != recorded_workspace::Since::Missing
                && get_item::inputs_changed(&built).await.unwrap_or(false);
            Outcome::Recorded(built, changed)
        },
        super::recorded::Found::Response(result) => outcome_of(&result),
        super::recorded::Found::Missing => Outcome::Failed(failure_of(&recorded.error)),
    }
}

/// The outcome that `result`, the response of a build here, tells: still going, as its
/// `building_reason` says, or failed.
#[cfg(feature = "local")]
fn outcome_of(result: &CallToolResult) -> Outcome {
    let reason = result
        .structured_content
        .as_ref()
        .filter(|response| response["status"] == "building")
        .and_then(|response| response["building_reason"].as_str());
    match reason {
        Some("fetching") => Outcome::Going(Doing::Fetching),
        Some("indexing") => Outcome::Going(Doing::Indexing),
        Some("waiting_for_lock") => Outcome::Going(Doing::WaitingForLock),
        Some("waiting_for_slot") => Outcome::Going(Doing::WaitingForSlot),
        Some(_) => Outcome::Going(Doing::Building),
        None => Outcome::Failed(failure_of(result)),
    }
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
    for key in ["code", "reason", "retryable", "retry_after_secs"] {
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
            #[cfg(feature = "local")]
            Outcome::Built(built) | Outcome::Recorded(built, _) => {
                Some((member, CrateView::new(built.archived.mapped.get())))
            },
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
    let (sources, failed, fetching, rebuilding) = sources(&found.provenances, members, context);
    let broke = failed_here(&sources);
    let no_record = sources.iter().any(|source| source["reason"] == "no_record");
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
    // Nothing found while crates are still being loaded or built again: the answer is still to
    // come (requirement R-OUT-9).
    let to_come = fetching.saturating_add(rebuilding);
    let nothing_yet = to_come > 0 && found.page.total == 0;
    if nothing_yet {
        response.insert("status".to_owned(), json!("building"));
        response.insert(
            "building_reason".to_owned(),
            json!(building_reason(members)),
        );
    }
    if failed > 0 || to_come > 0 {
        response.insert("partial".to_owned(), json!(true));
    }
    if to_come > 0 {
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
        (nothing_yet, to_come > 0 && found.page.has_more),
        (broke, rebuilding),
        (context.recorded.as_deref(), no_record),
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
        path: member.root(),
        origin: match (&context.std, toolchain) {
            // Built here, a member's with its private items (decided in M2's D7).
            #[cfg(feature = "local")]
            _ if let Outcome::Built(built) | Outcome::Recorded(built, _) = &member.outcome => {
                let record = &built.record;
                Origin::Local {
                    private: record.private,
                    package_source: (!record.private)
                        .then(|| local::package_source(&record.package)),
                    resolved_from: context.resolved_from,
                    project_from: context.project_from,
                    toolchain: record.toolchain.as_deref(),
                    built_at: &record.built_at,
                    features_requested: record.features_requested.as_deref(),
                    features_effective: record.features_effective.as_deref(),
                    recorded_by: built.recorded_by,
                    roots: record
                        .workspace_root
                        .as_deref()
                        .zip(record.package_root.as_deref()),
                    source_files: &record.source_files,
                    dependencies: context.dependencies.as_ref(),
                }
            },
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
        target: (!context.docs_rs_default || member.outcome.is_built())
            .then(|| recorded_target(member).unwrap_or(context.target.as_str())),
    }
}

/// The target of what `build_external_docs` recorded of `member`, which is the host's for a proc
/// macro (decided in M2e's E4b-3b).
#[cfg(feature = "local")]
fn recorded_target(member: &Member) -> Option<&str> {
    if let Outcome::Recorded(built, _) = &member.outcome {
        return built.record.target.as_deref();
    }
    None
}

/// Without local features, nothing is recorded.
#[cfg(not(feature = "local"))]
const fn recorded_target(_member: &Member) -> Option<&str> {
    None
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

/// Whether a build here failed, as `sources` tell, which is kept until its cause is fixed
/// (decided in M2's D5 and D7).
fn failed_here(sources: &[Value]) -> bool {
    sources.iter().any(|source| {
        source["status"] == "failed"
            && source["code"].as_str().is_some_and(|code| {
                matches!(
                    code,
                    "build_failed" | "fetch_failed" | "toolchain_missing" | "toolchain_too_old"
                )
            })
    })
}

/// The hints of the response: those of the search, and what to do when crates are still being
/// loaded and nothing was found (`nothing_yet`) or there is a next page (`cursor_while_loading`).
///
/// For a workspace outside the default project, which `build_external_docs` recorded as
/// `recorded` tells, what failed or has no record is made by that tool (decided in M2e's E4b-3b).
fn hints(
    found: &[Box<str>],
    (nothing_yet, cursor_while_loading): (bool, bool),
    (broke, rebuilding): (bool, usize),
    (recorded, no_record): (Option<&str>, bool),
) -> Vec<Value> {
    let mut hints: Vec<Value> = found.iter().map(|hint| json!(hint)).collect();
    if rebuilding > 0 {
        hints.push(json!(format!(
            "a crate whose source is stale is being built again, as what it is made from changed: \
             call again in about {RETRY_AFTER_SECS} seconds for the new build"
        )));
    }
    match (broke, recorded) {
        (true, Some(again)) => hints.push(json!(format!(
            "for a crate whose status is failed, once the cause is fixed, call build_external_docs \
             with {again} to build it again"
        ))),
        (true, None) => hints.push(json!(
            "for a crate whose status is failed, once the cause is fixed, call get_item with that \
             crate and \"refresh\": true to build it again"
        )),
        (false, _) => {},
    }
    if let (true, Some(again)) = (no_record, recorded) {
        hints.push(json!(format!(
            "for a crate whose reason is no_record, call build_external_docs with {again}, and for \
             a dependency of crates.io that docs.rs has no JSON of, with its crate too, as \
             name@version"
        )));
    }
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
    // What docs.rs is read for first, then what is built here (decided in M2's D7).
    [
        Doing::Indexing,
        Doing::Fetching,
        Doing::Building,
        Doing::WaitingForSlot,
        Doing::WaitingForLock,
    ]
    .into_iter()
    .find(|doing| {
        members
            .iter()
            .any(|member| matches!(&member.outcome, Outcome::Going(going) if going == doing))
    })
    .unwrap_or(Doing::Fetching)
    .as_str()
}

/// Marks `source` as of the build before, while the crate is built again, to ask for again
/// (requirements R-WS-9 and R-OUT-9, decided in M2's D4 and D7).
#[cfg(feature = "local")]
fn mark_stale(source: &mut Value) {
    if let Some(fields) = source.as_object_mut() {
        fields.insert("stale".to_owned(), json!(true));
        fields.insert("stale_reason".to_owned(), json!("rebuilding"));
        fields.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
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
) -> (Vec<Value>, usize, usize, usize) {
    let mut sources: Vec<Value> = Vec::new();
    for provenance in provenances {
        let mut source = serde_json::to_value(provenance).unwrap_or_default();
        if let Some(fields) = source.as_object_mut() {
            fields.insert("status".to_owned(), json!("ok"));
        }
        sources.push(source);
    }
    let (mut failed, mut fetching) = (0_usize, 0_usize);
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_mut, reason = "without local features, nothing is built again")
    )]
    let mut rebuilding = 0_usize;
    // The sources of the crates searched come first, in the order of `members`.
    let mut searched = 0_usize;
    for member in members {
        let mut source = Map::new();
        source.insert("crate".to_owned(), json!(member.name.as_str()));
        source.insert("version".to_owned(), json!(member.version.to_string()));
        if !context.docs_rs_default {
            source.insert("target".to_owned(), json!(context.target.as_str()));
        }
        match &member.outcome {
            Outcome::Loaded(_) => {
                searched = searched.saturating_add(1);
                continue;
            },
            // The build before, while it is made again: to come too (decided in M2's D7, as M2's
            // D4).
            #[cfg(feature = "local")]
            Outcome::Built(built) => {
                if built.stale
                    && let Some(source) = sources.get_mut(searched)
                {
                    mark_stale(source);
                    rebuilding = rebuilding.saturating_add(1);
                }
                searched = searched.saturating_add(1);
                continue;
            },
            // What it is made from changed since: not to come, as only that tool makes it again
            // (decided in M2e's E4b-3b, 3).
            #[cfg(feature = "local")]
            Outcome::Recorded(_, changed) => {
                if *changed && let Some(Value::Object(source)) = sources.get_mut(searched) {
                    source.insert("stale".to_owned(), json!(true));
                    source.insert("stale_reason".to_owned(), json!("inputs_changed"));
                }
                searched = searched.saturating_add(1);
                continue;
            },
            Outcome::Failed(failure) => {
                failed = failed.saturating_add(1);
                // What cannot be documented here, rather than what failed to be (decided in
                // M2's D7).
                let unavailable =
                    failure
                        .get("reason")
                        .and_then(Value::as_str)
                        .is_some_and(|reason| {
                            matches!(
                                reason,
                                "local_build_required"
                                    | "build_unsupported"
                                    | "target_unsupported"
                                    | "scope_unknown"
                                    | "no_json"
                                    | "no_target_json"
                                    | "no_record"
                            )
                        });
                let status = if unavailable { "unavailable" } else { "failed" };
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
    (sources, failed, fetching, rebuilding)
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
