//! The builds of `build_external_docs` of the default project's packages for another target than
//! the project builds for (requirement R-WS-10, decided in M2e's E4a).
//!
//! A package is planned as the query tools plan it for the project's targets
//! (`local::member_for` and `local::dependency_for`, with [`Built::Elsewhere`]), built in the
//! project's workspace, in a target directory of its own for the target (decided in M2e's E4a,
//! 2), by work that the calls for it share under the lock of its record's key ([`Key::Target`]),
//! and recorded. A call again builds again what it is made from changed, as the query tools'
//! builds do (requirement R-WS-10); cargo metadata is not recorded: the query tools resolve the
//! project themselves (decided in M2e's E4a, 1).

use std::sync::Arc;

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use tokio::{task::JoinSet, time::Instant};

use super::{
    ToolState,
    error_result,
    external_build::{
        self,
        DocsRs,
        SharedExternal,
        external_body,
        failed_so,
        record_made,
        remember_unmade,
        stopped_body,
    },
    get_item::{self, building_response},
    local::{self, Asked, Built, Member, targets_built},
    project,
};
use crate::{
    data::{
        crate_name::CrateName,
        external::{Key, Recorded, Told},
        flight::{Doing, Joined, Stage},
        metadata::{Metadata, Resolution},
        packages::{PackageGraph, ProjectPackage, Source},
        project::{ProjectFrom, Workspace},
        target::TargetTriple,
        version::VersionSpec,
    },
    query::local::{LocalError, LocalWanted, RECORDED_BY, local_archive_telling},
};

/// The key of the record of the build that `wanted` plans in `workspace`.
pub(super) fn key_of(workspace: &Workspace, wanted: &LocalWanted) -> Key {
    Key::Target {
        workspace: workspace.root.display().to_string().into(),
        package: wanted.plan.package.0.clone(),
        name: wanted.plan.crate_name.clone(),
        target: wanted.target.as_str().into(),
        hidden: false,
    }
}

/// The package of the default project that `build_external_docs` is asked for, by `name` and
/// `spec`, in the workspace `project` or else the project's: a member, or a dependency that is not
/// from crates.io, which docs.rs does not have; documented for `target`, another target than the
/// project builds for (decided in M2e's E4a). For the project's targets, or without one, the query
/// tools build it themselves.
///
/// `None` when it is none of these, as a release of crates.io, which `external_build::release`
/// documents; when `latest` is asked for, which is the release of crates.io; and, without
/// `project`, when the project cannot tell.
pub(super) async fn project_crate(
    state: &Arc<ToolState>,
    (name, spec): (&CrateName, Option<&VersionSpec>),
    target: Option<&TargetTriple>,
    project: Option<&std::path::Path>,
) -> Option<CallToolResult> {
    if matches!(spec, Some(VersionSpec::Latest)) {
        return None;
    }
    let until = state.until();
    let (workspace, project_from) = match project::workspace(state.project.as_ref(), project) {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(result) if project.is_none() => {
            drop(result);
            return None;
        },
        // Outside the default project (M2e's E4b), or not a workspace.
        // Outside the default project: of all that it records of it, only the crate (decided in
        // M2e's E4b).
        Err(result) if reason_of(&result) == Some("no_record") => {
            let path = project::project_path(state.project.as_ref(), project?);
            let Ok(workspace) = crate::data::project::workspace_of(&path) else {
                return Some(*result);
            };
            let outside = super::external_workspace::outside(
                Arc::clone(state),
                (workspace, path),
                target.cloned(),
                Some((name.clone(), spec.cloned())),
            );
            return Some(outside.await);
        },
        Err(result) => return Some(*result),
    };
    let resolution = match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return project
                .is_some()
                .then(|| project::resolve_error(&workspace, &error));
        },
        Joined::Ended => return Some(get_item::stopped()),
        Joined::Going(doing) => {
            return Some(planning((&workspace, project_from), target, doing));
        },
    };
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return None;
    };
    let package = match of_project(&PackageGraph::new(&resolution), (name, spec))? {
        Ok(package) => package,
        Err(result) => return Some(*result),
    };
    let targets = match targets_built(state, &workspace.root, &package.what()).await {
        Ok(targets) => targets,
        Err(result) => return Some(*result),
    };
    let found = (&workspace, project_from);
    let Some(target) = target.filter(|target| !targets.contains(target)) else {
        let Some(built) = target.or_else(|| targets.first()) else {
            return Some(super::build_external_docs::not_yet(
                "a project that builds for no target",
                "look its crates up with get_item",
            ));
        };
        return Some(for_built_target(state, (found, metadata), &package, built, until).await);
    };
    let planned = match &package {
        Found::Member(package) => {
            let asked = Asked::Package {
                name: &package.name,
                version: &package.version,
            };
            local::member_for(
                state,
                &workspace,
                (name, asked),
                (target, Built::Elsewhere),
                until,
            )
            .await
        },
        Found::Dependency(package) => {
            let elsewhere = (target, Built::Elsewhere);
            local::dependency_for(state, &workspace, package, elsewhere, until).await
        },
    };
    let planned = (planned, Vec::new());
    Some(built_planned(state, found, (package.package(), target), planned).await)
}

/// A package of the project that this module documents.
enum Found {
    /// A workspace member.
    Member(ProjectPackage),
    /// A dependency that is not from crates.io.
    Dependency(ProjectPackage),
}

impl Found {
    /// What it is, for the messages.
    fn what(&self) -> String {
        let package = self.package();
        format!("{} {}", package.name, package.version)
    }

    /// Its package.
    const fn package(&self) -> &ProjectPackage {
        let (Self::Member(package) | Self::Dependency(package)) = self;
        package
    }
}

/// The project's package named `name`, at a version that `spec` matches, if given: a member, or a
/// dependency that is not from crates.io; `None` for none of these; the error when several
/// packages of the project are so named, as a member and a release of crates.io, which this tool
/// cannot tell apart (requirement R-WS-10).
fn of_project(
    graph: &PackageGraph,
    (name, spec): (&CrateName, Option<&VersionSpec>),
) -> Option<Result<Found, Box<CallToolResult>>> {
    let named: Vec<&ProjectPackage> = graph
        .packages()
        .iter()
        .filter(|package| {
            package
                .name
                .parse::<CrateName>()
                .is_ok_and(|named| named.cache_key() == name.cache_key())
                && match spec {
                    Some(VersionSpec::Exact(version)) => package.version == *version,
                    Some(VersionSpec::Requirement(requirement)) => {
                        requirement.matches(&package.version)
                    },
                    Some(VersionSpec::Latest) | None => true,
                }
        })
        .collect();
    let ours =
        |package: &&&ProjectPackage| graph.is_member(package) || package.source != Source::CratesIo;
    if !named.iter().any(|package| ours(&package)) {
        return None;
    }
    match named.as_slice() {
        [one] if graph.is_member(one) => Some(Ok(Found::Member((*one).clone()))),
        [one] => Some(Ok(Found::Dependency((*one).clone()))),
        several => {
            let candidates: Vec<String> = several
                .iter()
                .map(|package| {
                    let of = if graph.is_member(package) {
                        "member"
                    } else {
                        source_name(&package.source)
                    };
                    format!("{} {} ({of})", package.name, package.version)
                })
                .collect();
            Some(Err(Box::new(error_result(&json!({
                "code": "invalid_argument",
                "message": format!(
                    "the project has several packages named {name}, which this tool does not \
                     tell apart: give the version, as crate@version"
                ),
                "retryable": false,
                "suggestions": ["give the version of one of the candidates"],
                "candidates": candidates,
            })))))
        },
    }
}

/// How the responses name where a package of the project comes from.
const fn source_name(source: &Source) -> &'static str {
    match source {
        Source::Local => "path",
        Source::CratesIo => "crates.io",
        Source::Registry(_) => "registry",
        Source::Git(_) => "git",
    }
}

/// The response for `package` of the project, asked for `built`, a target that the project builds
/// for: the query tools build it themselves, unless it is a dependency that the project's build
/// does not build for it, which tells what it is used for, as `metadata` tells; or the error.
async fn for_built_target(
    state: &Arc<ToolState>,
    ((workspace, project_from), metadata): ((&Workspace, ProjectFrom), &Metadata),
    package: &Found,
    built: &TargetTriple,
    until: Instant,
) -> CallToolResult {
    let found = (workspace, project_from);
    let Found::Dependency(dependency) = package else {
        return member_for_built_target(state, found, package.package(), built, until).await;
    };
    let what = package.what();
    match local::dependency(state, workspace, dependency, built, until).await {
        Ok(Member::Is { .. }) => query_tools_build(found, dependency, built),
        Ok(Member::Resolving(doing) | Member::Planning(doing)) => {
            planning(found, Some(built), doing)
        },
        Ok(Member::OtherTarget { .. }) => {
            external_build::not_used_for(dependency, built, &conditions(metadata, dependency))
        },
        Err(error) if reason_of(&error) == Some("build_unsupported") => {
            external_build::not_used_for(dependency, built, &conditions(metadata, dependency))
        },
        Ok(Member::Unnameable { .. }) => super::build_external_docs::not_yet(
            &format!("{what}, a dependency that cargo rustdoc -p cannot name"),
            "look it up with get_item, which tells what it can",
        ),
        Ok(Member::Not | Member::OtherVersion(_)) => no_crate(&what),
        Ok(Member::NotForTarget(error) | Member::Unknown(error)) | Err(error) => {
            external_build::external_body_of(&error)
        },
        // Of the query tools only.
        Ok(Member::Recorded(recorded)) => external_build::external_body_of(&recorded.error),
    }
}

/// The response for the member `member` of the project, asked for `built`, a target that the
/// project builds for: the query tools build it themselves, unless the features of its
/// `[package.metadata.docs.rs]` build beyond their scope, which are built with here, apart, and
/// recorded (decided in M2e's E4c, 1).
async fn member_for_built_target(
    state: &Arc<ToolState>,
    (workspace, project_from): (&Workspace, ProjectFrom),
    member: &ProjectPackage,
    built: &TargetTriple,
    until: Instant,
) -> CallToolResult {
    let found = (workspace, project_from);
    let Ok(name) = member.name.parse::<CrateName>() else {
        return query_tools_build(found, member, built);
    };
    let asked = || Asked::Package {
        name: &member.name,
        version: &member.version,
    };
    match local::member(state, workspace, (&name, asked()), built, until).await {
        Ok(Member::Is { wanted, .. }) if wanted.docs_rs_beyond_scope => {
            let apart = (built, Built::Elsewhere);
            let planned = local::member_for(state, workspace, (&name, asked()), apart, until).await;
            built_planned(state, found, (member, built), (planned, Vec::new())).await
        },
        Ok(Member::Resolving(doing) | Member::Planning(doing)) => {
            planning(found, Some(built), doing)
        },
        Ok(Member::Is { .. } | Member::OtherVersion(_)) => query_tools_build(found, member, built),
        Ok(Member::Not) => no_crate(&format!("{} {}", member.name, member.version)),
        Ok(
            Member::NotForTarget(error)
            | Member::Unknown(error)
            | Member::OtherTarget { error, .. }
            | Member::Unnameable { error, .. },
        )
        | Err(error) => external_build::external_body_of(&error),
        // Of the query tools only.
        Ok(Member::Recorded(recorded)) => external_build::external_body_of(&recorded.error),
    }
}

/// `docs_unavailable` for `what`, which has nothing to document.
fn no_crate(what: &str) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!("{what} has no lib nor bin to document"),
        "retryable": false,
        "suggestions": [],
        "reason": "build_unsupported",
    }))
}

/// The response when the query tools build `package` of `workspace` for `target` themselves, as
/// the project builds for it, or as a proc macro, for the host: nothing is built (requirement
/// R-WS-10).
fn query_tools_build(
    (workspace, project_from): (&Workspace, ProjectFrom),
    package: &ProjectPackage,
    target: &TargetTriple,
) -> CallToolResult {
    CallToolResult::structured(json!({
        "provenance": {
            "source": "project",
            "workspace": workspace.root.display().to_string(),
            "project_from": project_from.as_str(),
            "crate": &*package.name,
            "version": package.version.to_string(),
            "target": target.as_str(),
        },
        "data": {
            "built": false,
            "reason": "query_tools_build",
            "crate": &*package.name,
            "version": package.version.to_string(),
            "target": target.as_str(),
        },
        "hints": [format!(
            "nothing was built: the other tools build this crate of the project for {target} \
             themselves; look it up with get_item or crate_overview, passing {}",
            json!({ "crate": &*package.name, "target": target.as_str() })
        )],
    }))
}

/// The response while cargo resolves or plans the project at `workspace`, as `doing` says.
fn planning(
    (workspace, project_from): (&Workspace, ProjectFrom),
    target: Option<&TargetTriple>,
    doing: Doing,
) -> CallToolResult {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("project"));
    provenance.insert(
        "workspace".to_owned(),
        json!(workspace.root.display().to_string()),
    );
    provenance.insert("project_from".to_owned(), json!(project_from.as_str()));
    if let Some(target) = target {
        provenance.insert("target".to_owned(), json!(target.as_str()));
    }
    building_response(
        provenance,
        doing,
        "the project's build is being planned, to tell what to build",
        &[],
    )
}

/// Whether the query tools build what `wanted` plans themselves (decided in M2e's E4a): a proc
/// macro, which they build for the host whatever the target; or a dependency that the project's
/// build for another target builds for the host, `package`, when the project's own build builds
/// it for the host too.
async fn built_by_query_tools(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    (wanted, package): (&LocalWanted, Option<&ProjectPackage>),
    target: &TargetTriple,
) -> bool {
    // Never of a workspace outside the default project, which they do not build (decided in
    // M2e's E4b-3).
    if super::recorded_workspace::is_outside(state, workspace) {
        return false;
    }
    if wanted.plan.is_proc_macro() {
        return true;
    }
    let Some(package) = package.filter(|_| wanted.target != *target) else {
        return false;
    };
    let host = &wanted.target;
    matches!(
        local::dependency(state, workspace, package, host, state.until()).await,
        Ok(Member::Is { .. })
    )
}

/// Builds and records what `planned` plans for `package` of `workspace` for `target`, with what the
/// response warns of, or returns why not (decided in M2e's E4a).
async fn built_planned(
    state: &Arc<ToolState>,
    (workspace, project_from): (&Workspace, ProjectFrom),
    (package, target): (&ProjectPackage, &TargetTriple),
    (planned, mut warnings): (Result<Member, Box<CallToolResult>>, Vec<Box<str>>),
) -> CallToolResult {
    let what = format!("{} {}", package.name, package.version);
    let found = (workspace, project_from);
    match planned {
        Ok(Member::Is { wanted, .. }) => {
            let dependency = (!wanted.plan.document_private).then_some(package);
            if built_by_query_tools(state, workspace, (&wanted, dependency), target).await {
                return query_tools_build(found, package, &wanted.target);
            }
            if wanted.target != *target {
                warnings.push(
                    format!(
                        "the project's build for {target} builds {what} for the host, {}: this is \
                         its documentation for the host",
                        wanted.target
                    )
                    .into(),
                );
            }
            let joined = joined(state, workspace, &wanted, state.until()).await;
            respond(found, &wanted, joined, &warnings)
        },
        Ok(Member::Resolving(doing) | Member::Planning(doing)) => {
            planning(found, Some(target), doing)
        },
        Ok(
            Member::NotForTarget(error)
            | Member::Unknown(error)
            | Member::OtherTarget { error, .. },
        )
        | Err(error) => external_build::external_body_of(&error),
        // Of the query tools only.
        Ok(Member::Recorded(recorded)) => external_build::external_body_of(&recorded.error),
        Ok(Member::Unnameable { .. }) => super::build_external_docs::not_yet(
            &format!("{what}, a dependency that cargo rustdoc -p cannot name, for {target}"),
            "look it up with get_item for a target that the project builds for",
        ),
        Ok(Member::Not | Member::OtherVersion(_)) => no_crate(&what),
    }
}

/// The build that `wanted` plans in `workspace`, by the work that the calls for its record share,
/// waited for `build.wait_secs` from now, and while it is indexed, until `until`.
async fn joined(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    wanted: &LocalWanted,
    until: Instant,
) -> Joined<SharedExternal> {
    let key = key_of(workspace, wanted);
    let what = format!(
        "{} {} for {}",
        wanted.plan.crate_name, wanted.plan.version, wanted.target
    );
    let until_built = Instant::now()
        .checked_add(state.build_wait)
        .map_or(until, |until_built| until_built.min(until));
    let start = || {
        let (state, key, wanted) = (Arc::clone(state), key.clone(), wanted.clone());
        async move |stage: Arc<Stage>| make(&state, &key, wanted, &stage).await
    };
    let flight = key.lock_key();
    let joined = state
        .externals
        .join(&flight, &what, until_built, start())
        .await;
    if matches!(joined, Joined::Going(Doing::Indexing)) && until_built < until {
        return state.externals.join(&flight, &what, until, start()).await;
    }
    joined
}

/// The work that builds what `wanted` plans and records it as `key`, telling `progress` what it
/// does, under the lock of the key. What failed, or is broken, is built again, forgetting the
/// failure (requirement R-WS-10); else the build builds again only what changed.
async fn make(
    state: &ToolState,
    key: &Key,
    mut wanted: LocalWanted,
    progress: &Stage,
) -> SharedExternal {
    let what = format!(
        "{} {} for {}",
        wanted.plan.crate_name, wanted.plan.version, wanted.target
    );
    let making = external_build::take_lock(state, key, progress)
        .await
        .map_err(|why| Arc::new(failed_so(&what, &why, true)))?;
    let (now, making) = external_build::read_record(state, key, making)
        .await
        .ok_or_else(|| Arc::new(stopped_body()))?;
    if matches!(now, Recorded::Failed(_) | Recorded::Broken(_)) {
        wanted.refresh = true;
    }
    progress.set(Doing::Building);
    match local_archive_telling(&state.locals, &state.cache, wanted, progress).await {
        Some(Ok(built)) => {
            let told = Told {
                in_project: true,
                ..Told::default()
            };
            record_made(state, key, (&built, told), making, &what).await
        },
        Some(Err(error)) if matches!(*error, LocalError::Stopped) => Err(Arc::new(stopped_body())),
        Some(Err(error)) => {
            let body = external_body(body_of(&local::local_error(&what, &error)));
            Err(remember_unmade(state, key, body, making).await)
        },
        None => Err(Arc::new(stopped_body())),
    }
}

/// The `reason` of the tool error `result`, of those that tell a dependency not built for a
/// target, or a workspace outside the default project.
fn reason_of(result: &CallToolResult) -> Option<&'static str> {
    match body_of(result).get("reason").and_then(Value::as_str) {
        Some("build_unsupported") => Some("build_unsupported"),
        Some("no_json") => Some("no_json"),
        Some("no_record") => Some("no_record"),
        Some(_) | None => None,
    }
}

/// The JSON body of the tool error `result`.
fn body_of(result: &CallToolResult) -> Value {
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str(&text.text).ok())
        .unwrap_or_else(|| json!({ "code": "build_failed", "message": "the build failed" }))
}

/// The response for what `wanted` planned in `workspace`, as `joined` ended: the build, the
/// error, or that it goes on; with `warnings`.
fn respond(
    (workspace, project_from): (&Workspace, ProjectFrom),
    wanted: &LocalWanted,
    joined: Joined<SharedExternal>,
    warnings: &[Box<str>],
) -> CallToolResult {
    let (name, version) = package_of(wanted);
    let target = &wanted.target;
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    provenance.insert(
        "workspace".to_owned(),
        json!(workspace.root.display().to_string()),
    );
    provenance.insert("project_from".to_owned(), json!(project_from.as_str()));
    provenance.insert("crate".to_owned(), json!(name));
    provenance.insert("version".to_owned(), json!(version));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    let lookup = json!({ "crate": name, "target": target.as_str() });
    match joined {
        Joined::Done(Ok(built)) => {
            external_build::built_provenance(&mut provenance, &built);
            let mut response = Map::new();
            response.insert("provenance".to_owned(), Value::Object(provenance));
            let mut warnings = warnings.to_vec();
            warnings.extend(built.archived.warnings.iter().cloned());
            if !warnings.is_empty() {
                response.insert("warnings".to_owned(), json!(warnings));
            }
            response.insert(
                "data".to_owned(),
                json!({
                    "built": true,
                    "crate": name,
                    "version": version,
                    "target": target.as_str(),
                }),
            );
            response.insert(
                "hints".to_owned(),
                json!([format!(
                    "look it up with get_item, crate_overview or search_items, passing {lookup}"
                )]),
            );
            CallToolResult::structured(Value::Object(response))
        },
        Joined::Done(Err(body)) => error_result(&body),
        Joined::Ended => get_item::stopped(),
        Joined::Going(doing) => {
            provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
            let being = get_item::being(
                &format!("{name} {version} for {target}"),
                doing,
                "the project",
            );
            let mut result = building_response(provenance, doing, &being, warnings);
            if let Some(Value::Object(response)) = &mut result.structured_content {
                response.insert(
                    "hints".to_owned(),
                    json!([format!(
                        "{being}, which goes on without this call: look it up with get_item or \
                         crate_overview, passing {lookup}, which answer once it is built; do not \
                         call build_external_docs again for it"
                    )]),
                );
            }
            result
        },
    }
}

/// The package name and the version that `wanted` builds.
pub(super) fn package_of(wanted: &LocalWanted) -> (String, String) {
    let version = wanted.plan.version.to_string();
    let name = wanted.plan.package.name_and_version().map_or_else(
        || wanted.plan.crate_name.to_string(),
        |(name, _)| name.to_owned(),
    );
    (name, version)
}

/// Documents every member of the default project's `workspace` for `target`, another target than
/// it builds for: each by work of its own, started at once and waited for together for
/// `build.wait_secs` (decided in M2e's E4a, 4). The response lists each, built, being built, or
/// failed, which does not fail the call; a member that the query tools build for one of the
/// project's targets, as a proc macro for the host, is built by them.
pub(super) async fn members(
    state: Arc<ToolState>,
    (workspace, project_from): (Workspace, ProjectFrom),
    target: TargetTriple,
) -> CallToolResult {
    let until = state.until();
    let resolution = match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => return project::resolve_error(&workspace, &error),
        Joined::Ended => return get_item::stopped(),
        Joined::Going(doing) => {
            return planning((&workspace, project_from), Some(&target), doing);
        },
    };
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return error_result(&json!({
            "code": "docs_unavailable",
            "message": "cargo metadata did not resolve the project, so its members are not known",
            "retryable": true,
            "suggestions": ["run `cargo metadata` in the project's terminal to see why it fails"],
            "reason": "build_unsupported",
        }));
    };
    // A project that builds its own std is not documented here (requirement R-TG-1).
    if let Err(result) =
        targets_built(state.as_ref(), &workspace.root, "the project's crates").await
    {
        return *result;
    }
    let mut planned = Vec::new();
    for id in &metadata.workspace_members {
        let Some(package) = metadata.packages.iter().find(|package| package.id == *id) else {
            continue;
        };
        let Ok(name) = package.name.parse::<CrateName>() else {
            continue;
        };
        let asked = Asked::Package {
            name: &package.name,
            version: &package.version,
        };
        let elsewhere = (&target, Built::Elsewhere);
        match local::member_for(&state, &workspace, (&name, asked), elsewhere, until).await {
            Ok(Member::Is { wanted, .. }) => planned.push(*wanted),
            // No lib nor bin to document.
            Ok(Member::Not | Member::OtherVersion(_)) => {},
            Ok(Member::Resolving(doing) | Member::Planning(doing)) => {
                return planning((&workspace, project_from), Some(&target), doing);
            },
            Ok(
                Member::NotForTarget(error)
                | Member::Unknown(error)
                | Member::OtherTarget { error, .. },
            )
            | Err(error) => {
                return external_build::external_body_of(&error);
            },
            // Of the query tools only.
            Ok(Member::Recorded(recorded)) => {
                return external_build::external_body_of(&recorded.error);
            },
            Ok(Member::Unnameable { error, .. }) => return *error,
        }
    }
    let entries = all_built(&state, (&workspace, true), planned, until).await;
    members_response((&workspace, project_from), &target, &entries)
}

/// What became of the build of a member.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    /// Its package name.
    pub(super) name: String,
    /// Its version.
    pub(super) version: String,
    /// `built`, `building`, `failed` or `query_tools_build`.
    pub(super) status: &'static str,
    /// The error, when it failed.
    pub(super) error: Option<Arc<Value>>,
}

impl Entry {
    /// It as the responses list it.
    pub(super) fn to_json(&self) -> Value {
        let mut entry = json!({
            "crate": self.name,
            "version": self.version,
            "status": self.status,
        });
        if let (Some(error), Some(fields)) = (&self.error, entry.as_object_mut()) {
            fields.insert("error".to_owned(), (**error).clone());
        }
        entry
    }
}

/// The builds of `planned` in `workspace`, started at once and waited for together until
/// `build.wait_secs` from now, within `until`: a proc macro, which the query tools build for the
/// host whatever the target, is not built in the default project, `of_project`.
pub(super) async fn all_built(
    state: &Arc<ToolState>,
    (workspace, of_project): (&Workspace, bool),
    planned: Vec<LocalWanted>,
    until: Instant,
) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut builds = JoinSet::new();
    for (order, wanted) in planned.into_iter().enumerate() {
        let (name, version) = package_of(&wanted);
        // A proc macro, which they build for the host whatever the target.
        if of_project && wanted.plan.is_proc_macro() {
            entries.push((order, Entry {
                name,
                version,
                status: "query_tools_build",
                error: None,
            }));
            continue;
        }
        let (state, workspace) = (Arc::clone(state), workspace.clone());
        builds.spawn(async move {
            let joined = joined(&state, &workspace, &wanted, until).await;
            let (status, error) = match joined {
                Joined::Done(Ok(_)) => ("built", None),
                Joined::Done(Err(body)) => ("failed", Some(body)),
                Joined::Ended => ("failed", Some(Arc::new(stopped_body()))),
                Joined::Going(_) => ("building", None),
            };
            (order, Entry {
                name,
                version,
                status,
                error,
            })
        });
    }
    while let Some(done) = builds.join_next().await {
        match done {
            Ok(entry) => entries.push(entry),
            Err(error) => tracing::error!(%error, "a build of a member did not finish"),
        }
    }
    entries.sort_by_key(|(order, _)| *order);
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// The response for the members of `workspace` documented for `target`, as `entries` tell.
fn members_response(
    (workspace, project_from): (&Workspace, ProjectFrom),
    target: &TargetTriple,
    entries: &[Entry],
) -> CallToolResult {
    let root = workspace.root.display().to_string();
    let all_built = entries
        .iter()
        .all(|entry| matches!(entry.status, "built" | "query_tools_build"));
    let building = entries.iter().any(|entry| entry.status == "building");
    let members: Vec<Value> = entries.iter().map(Entry::to_json).collect();
    let mut hints = vec![format!(
        "look a member up with get_item, crate_overview or search_items, passing its crate and \
         {}",
        json!({ "target": target.as_str() })
    )];
    if building {
        hints.push(
            "the members being built go on without this call: the other tools answer once they \
             are built; do not call build_external_docs again for them"
                .to_owned(),
        );
    }
    CallToolResult::structured(json!({
        "provenance": {
            "source": "local",
            "workspace": root,
            "project_from": project_from.as_str(),
            "target": target.as_str(),
            "recorded_by": RECORDED_BY,
        },
        "data": {
            "built": all_built,
            "target": target.as_str(),
            "members": members,
        },
        "hints": hints,
    }))
}

/// The project's dependency `package` of crates.io, of `workspace`, documented for `target`,
/// another target than the project builds for, as `release` asked (decided in M2e's E4a): what
/// docs.rs has for the target, or else its build in the project. `Ok` with the target to build it
/// for as the release alone, when `cargo rustdoc -p` cannot name it (decided in M2e's E2b).
pub(super) async fn dependency(
    state: &Arc<ToolState>,
    (workspace, project_from): (&Workspace, ProjectFrom),
    package: &ProjectPackage,
    target: &TargetTriple,
) -> Result<TargetTriple, Box<CallToolResult>> {
    let until = state.until();
    let mut warnings = Vec::new();
    if let Ok(name) = package.name.parse::<CrateName>() {
        match external_build::on_docs_rs(state, (&name, &package.version, target), until).await {
            DocsRs::Lacks => {},
            DocsRs::Unknown(why) => warnings.push(why.into()),
            DocsRs::Response(result) => return Err(result),
        }
    }
    let elsewhere = (target, Built::Elsewhere);
    let planned = local::dependency_for(state, workspace, package, elsewhere, until).await;
    if let Ok(Member::Unnameable { target, .. }) = &planned {
        return Ok(target.clone());
    }
    let found = (workspace, project_from);
    Err(Box::new(
        built_planned(state, found, (package, target), (planned, warnings)).await,
    ))
}

/// The `cfg` or targets that the project uses its dependency `dependency` for, from `metadata`,
/// to tell when it is asked for without a target (requirement R-WS-10).
pub(super) fn conditions(metadata: &Metadata, dependency: &ProjectPackage) -> Vec<Box<str>> {
    metadata
        .packages
        .iter()
        .find(|package| {
            package.name == dependency.name
                && package.version == dependency.version
                && Source::parse(package.source.as_deref()) == dependency.source
                && !metadata.workspace_members.contains(&package.id)
        })
        .map(|package| metadata.platform_conditions(&package.id))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_members_built_tell_the_call_built_and_the_others_go_on() {
        let workspace = Workspace { root: "/p".into() };
        let target: TargetTriple = "wasm32-wasip1".parse().expect("a target");
        let entry = |name: &str, status| Entry {
            name: name.to_owned(),
            version: "0.1.0".to_owned(),
            status,
            error: (status == "failed").then(|| Arc::new(json!({ "code": "build_failed" }))),
        };
        let response = |entries: &[Entry]| {
            members_response((&workspace, ProjectFrom::Argument), &target, entries)
                .structured_content
                .expect("structured")
        };
        let all = response(&[entry("a", "built"), entry("m", "query_tools_build")]);
        assert_eq!(all["data"]["built"], true);
        assert_eq!(all["data"]["members"][1]["status"], "query_tools_build");
        assert_eq!(all["hints"].as_array().map(Vec::len), Some(1));
        let some = response(&[entry("a", "failed"), entry("b", "building")]);
        assert_eq!(some["data"]["built"], false);
        assert_eq!(some["data"]["members"][0]["error"]["code"], "build_failed");
        assert!(some["data"]["members"][1].get("error").is_none());
        assert_eq!(some["hints"].as_array().map(Vec::len), Some(2));
        assert_eq!(some["provenance"]["recorded_by"], RECORDED_BY);
    }
}
