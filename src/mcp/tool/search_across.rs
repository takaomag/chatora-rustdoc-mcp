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
use super::gather::{Builds, Kind, build_sets, project_all};
use super::{
    ErrorChain,
    ToolState,
    gather::{Member, Outcome},
    get_item::{self, Searching, StdUnready},
};

/// When to ask again for the crates still being fetched or indexed.
const RETRY_AFTER_SECS: u64 = get_item::RETRY_AFTER_SECS;
#[cfg(feature = "local")]
use super::{local, recorded_workspace};
#[cfg(feature = "local")]
use crate::data::{
    features::FeatureSets,
    flight::Joined,
    packages::{PackageGraph, ProjectPackage, Source},
};
use crate::{
    data::{
        crate_name::CrateName,
        dist::StdCrate,
        flight::Doing,
        std_json::Chosen,
        target::TargetTriple,
    },
    query::{
        get_item::{Detail, ItemRequest, Origin, ProjectFeatures, ResolvedFrom},
        load,
        search::{SearchOptions, Searched, search_across},
        view::CrateView,
    },
};

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
    let at = (&target, docs_rs.as_ref());
    let scope = (scope_of(&graph, sets.as_deref()), Builds::Allow);
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

/// The packages of the search of a project: the workspace's members (decided in M2's D7), and its
/// direct dependencies that its normal build or its build with the dev-dependencies has, as `sets`
/// tells, or all of them without it.
#[cfg(feature = "local")]
fn scope_of(graph: &PackageGraph, sets: Option<&FeatureSets>) -> Vec<(ProjectPackage, Kind)> {
    let mut scope: Vec<(ProjectPackage, Kind)> = graph
        .packages()
        .iter()
        .filter(|package| graph.is_member(package))
        .map(|package| (package.clone(), Kind::Member))
        .collect();
    for package in graph.direct_packages() {
        let built = sets.is_none_or(|sets| {
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
    scope
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
            Outcome::Built(built) | Outcome::Cached(built) | Outcome::Recorded(built, _) => {
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
            _ if let Outcome::Built(built)
            | Outcome::Cached(built)
            | Outcome::Recorded(built, _) = &member.outcome =>
            {
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
        (true, None) if cfg!(feature = "local") => hints.push(json!(
            "for a crate whose status is failed, once the cause is fixed, call get_item with that \
             crate and \"refresh\": true to build it again"
        )),
        (true, None) => hints.push(json!(
            "for a crate whose status is failed, once the cause is fixed, call get_item with that \
             crate again"
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

/// Why the crates of `members` that are still being loaded are not in yet
/// ([`super::gather::building_reason`]).
fn building_reason(members: &[Member]) -> &'static str {
    super::gather::building_reason(members.iter().filter_map(|member| {
        if let Outcome::Going(doing) = &member.outcome {
            Some(doing)
        } else {
            None
        }
    }))
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
            // Built before and read as it is, not built again by this call: not to come (decided in
            // M4's plan, J2).
            #[cfg(feature = "local")]
            Outcome::Cached(_) => {
                if let Some(Value::Object(source)) = sources.get_mut(searched) {
                    source.insert("stale".to_owned(), json!(true));
                    source.insert("stale_reason".to_owned(), json!("unchecked"));
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
                                    | "too_many"
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
