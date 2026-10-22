//! The builds of `build_external_docs` of a release of crates.io that the project does not use:
//! not a dependency, or a version that its Cargo.lock does not have (requirements R-WS-5 and
//! R-WS-10, decided in M2e's E2a).
//!
//! The release is documented in a temporary project of its own, by work that the calls for it
//! share, which holds the lock of its record's key from before the project is written until the
//! record is (decided in M2e, 13). A call waits for it as the query tools wait for a build, and
//! then returns `status: building` while it goes on; the query tools find the record.

use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    ErrorChain,
    ToolState,
    error_result,
    get_item::{self, building_response},
    local::{self, Member},
    project,
};
use crate::{
    data::{
        LoadError,
        build::{BuildError, Built, Failed},
        cargo::Failure,
        crate_file,
        crate_name::CrateName,
        docs_rs::FetchError,
        external::{self, Key, Making, Recorded, Told},
        flight::{Doing, Joined, Stage},
        metadata::{CargoError, Resolution},
        packages::{DocsRsCheck, PackageGraph, ProjectPackage},
        project::{ProjectFrom, Workspace, workspace_of},
        rustdoc_json::ParseError,
        sparse_index::IndexEntry,
        target::TargetTriple,
        temp_project::{self, Origin, Release, Set},
        version::VersionSpec,
    },
    query::{
        load::{self, ArchiveError},
        local::{
            LocalBuilt,
            LocalError,
            LocalWanted,
            RECORDED_BY,
            local_archive_telling,
            recorded,
        },
    },
};

/// The build of a release for the calls that wanted it at once, or the body of the error that it
/// ended with, which its record keeps.
pub(super) type SharedExternal = Result<Arc<LocalBuilt>, Arc<Value>>;

/// How long the work waits for another server that makes the same record: about as long as a
/// build may take (requirement R-WS-7).
const OTHER_MAKES_FOR: Duration = Duration::from_mins(30);

/// How often the work tries the lock of the key again while another holds it.
const TRY_AGAIN_EVERY: Duration = Duration::from_millis(200);

/// Documents the release of `name` that `spec` names, or the latest, for `target`, or the host's
/// (decided in M2e, 11), in a temporary project, unless the project uses it or docs.rs has it. The
/// project is the workspace `project` of the default project, if given.
pub(super) async fn release(
    state: Arc<ToolState>,
    (name, spec): (CrateName, Option<VersionSpec>),
    target: Option<TargetTriple>,
    project: Option<&Path>,
) -> CallToolResult {
    let until = state.until();
    let entry = match get_item::index_entry(&state, &name).await {
        Ok(entry) => entry,
        Err(result) => return *result,
    };
    // A dependency's, as its Cargo.lock has it (requirement R-WS-10), or else the latest.
    let spec = match spec {
        Some(spec) => spec,
        None => match project_version(&state, (&name, project), until).await {
            Ok(version) => version.map_or(VersionSpec::Latest, VersionSpec::Exact),
            Err(result) => return *result,
        },
    };
    let resolved = match spec.resolve(&entry.versions) {
        Ok(resolved) => resolved,
        Err(no_version) => {
            return get_item::version_not_found(&state, &entry, &spec, &no_version);
        },
    };
    let version = resolved.version.version.clone();
    // cargo does not resolve a yanked version anew (decided in M2e's E2a).
    if resolved.version.yanked {
        return yanked(&entry.name, &version, &entry.versions);
    }
    let mut warnings: Vec<Box<str>> = entry.stale.iter().cloned().collect();
    warnings.extend(
        resolved
            .warnings
            .iter()
            .map(|warning| Box::from(warning.as_ref())),
    );
    let published: CrateName = entry.name.parse().unwrap_or(name);
    let used = match uses(&state, (&entry, &version), project, until).await {
        Uses::No => None,
        Uses::Unknown(why) => {
            warnings.push(why.into());
            None
        },
        Uses::Yes(used) => Some(used),
        Uses::Response(result) => return *result,
    };
    // A dependency's for the project's build target; else the host's (decided in M2e, 11).
    let asked = target.is_some();
    let mut target = match (target, &used) {
        (Some(target), _) => target,
        (None, Some(used)) => build_target(&state, &used.0).await,
        (None, None) => state.host().await.triple.clone(),
    };
    // A dependency that the query tools build is not built here: one that `cargo rustdoc -p`
    // cannot name is, as the release alone, for the target it is built for (decided in M2e's
    // E2b).
    if let Some(used) = &used {
        match unnameable(&state, used, (&target, asked), until).await {
            Ok(Some(documented)) => {
                target = documented;
                warnings.push(FOR_THE_PROJECT.into());
            },
            Ok(None) => {},
            Err(result) => return *result,
        }
    }
    let key = Key::Release {
        name: published.cache_key().into(),
        version: version.to_string().into(),
        target: target.as_str().into(),
        hidden: false,
    };
    // A record is not made again: a call again only forgets a failure (requirement R-WS-10).
    if let Some(built) = whole_record(&state, &key).await {
        return built_response(&built, (&published, &target), &warnings);
    }
    match on_docs_rs(&state, (&published, &version, &target), until).await {
        DocsRs::Lacks => {},
        DocsRs::Unknown(why) => warnings.push(why.into()),
        DocsRs::Response(result) => return *result,
    }
    let release = Release {
        cfgs: Box::default(),
        features: vec!["default".into()],
        asks_features: false,
        name: published,
        origin: Origin::CratesIo(version.clone()),
    };
    let release = match with_docs_rs_features(
        &state,
        (release, &version),
        &resolved.version.checksum,
        &mut warnings,
    )
    .await
    {
        Ok(release) => release,
        Err(result) => return *result,
    };
    built(&state, key, (release, version), target, warnings, until).await
}

/// What the response warns about a dependency of the project built here as the release alone.
const FOR_THE_PROJECT: &str = "the project's build cannot document it, so it is built as the \
                               release of crates.io alone, with the features that docs.rs builds \
                               it with: the project's build of it may have other features";

/// Whether the project uses a release.
enum Uses {
    /// It does not, or there is no project to tell.
    No,
    /// It cannot be told, as this tells: as when it does not.
    Unknown(String),
    /// It does: its workspace, its package of the release, and where the project came from.
    Yes(Box<(Workspace, ProjectPackage, ProjectFrom)>),
    /// The response to return, while cargo resolves the project.
    Response(Box<CallToolResult>),
}

/// The version of the crate `name` that the default project resolves, if it has one from
/// crates.io; or the error when it has several, or several workspaces, of which one is not told
/// (requirement R-MCP-3), or the response while cargo resolves it.
async fn project_version(
    state: &ToolState,
    (name, project): (&CrateName, Option<&Path>),
    until: Instant,
) -> Result<Option<semver::Version>, Box<CallToolResult>> {
    let Some((workspace, project_from)) = project::workspace(state.project.as_ref(), project)?
    else {
        return Ok(None);
    };
    let resolution = match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(_)) => return Ok(None),
        Joined::Ended => return Err(Box::new(get_item::stopped())),
        Joined::Going(doing) => {
            return Err(Box::new(resolving(&workspace, project_from, doing)));
        },
    };
    let graph = PackageGraph::new(&resolution);
    let versions: Vec<&semver::Version> = graph
        .crates_io_named(name.as_str())
        .into_iter()
        .map(|package| &package.version)
        .collect();
    match versions.as_slice() {
        [] => Ok(None),
        [version] => Ok(Some((*version).clone())),
        several => Err(Box::new(error_result(&json!({
            "code": "invalid_argument",
            "message": format!(
                "the project has several versions of {name}: give the version, as crate@version"
            ),
            "retryable": false,
            "suggestions": ["give one of the candidates as the version"],
            "candidates": several.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })))),
    }
}

/// The response while cargo resolves the packages of `workspace`, as `doing` says.
fn resolving(workspace: &Workspace, project_from: ProjectFrom, doing: Doing) -> CallToolResult {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("project"));
    provenance.insert(
        "workspace".to_owned(),
        json!(workspace.root.display().to_string()),
    );
    provenance.insert("project_from".to_owned(), json!(project_from.as_str()));
    building_response(
        provenance,
        doing,
        "the project's dependencies are being resolved, to tell whether it uses the release",
        &[],
    )
}

/// The target that the project at `workspace` builds for, the first of several; or the host's.
async fn build_target(state: &ToolState, workspace: &Workspace) -> TargetTriple {
    let host = state.host().await.triple.clone();
    match project::build_targets(state, &workspace.root).await {
        project::BuildTargets::These(targets) => targets.into_iter().next().unwrap_or(host),
        project::BuildTargets::OwnTarget(_) => host,
    }
}

/// Whether the project's `package` of `workspace`, a dependency, is one that `cargo rustdoc -p`
/// cannot name for `target`, built here, for the target that it returns (decided in M2e's E2b);
/// `None` for one that is not the project's after all, or has no lib. Else the response: the query
/// tools build it, it is not built for the target, which is documented for another target if
/// `asked` (decided in M2e's E4a), or else to be given, or cargo plans it still.
async fn unnameable(
    state: &Arc<ToolState>,
    used: &(Workspace, ProjectPackage, ProjectFrom),
    (target, asked): (&TargetTriple, bool),
    until: Instant,
) -> Result<Option<TargetTriple>, Box<CallToolResult>> {
    let (workspace, package, project_from) = used;
    let planned = match local::dependency(state, workspace, package, target, until).await {
        Ok(planned) => planned,
        // Not built for the target (`not_built_for`).
        Err(error) if body_of(&error).get("reason") == Some(&json!("build_unsupported")) => {
            return other_target(state, used, (target, asked), until).await;
        },
        Err(error) => return Err(Box::new(external_body_of(&error))),
    };
    match planned {
        Member::Unnameable { target, .. } => Ok(Some(target)),
        Member::Not | Member::OtherVersion(_) => Ok(None),
        Member::Is { .. } => Err(Box::new(the_query_tools_build(
            (workspace, package, *project_from),
            target,
        ))),
        Member::OtherTarget { .. } => other_target(state, used, (target, asked), until).await,
        Member::NotForTarget(error) | Member::Unknown(error) => {
            Err(Box::new(external_body_of(&error)))
        },
        // Of the query tools only.
        Member::Recorded(recorded) => Err(Box::new(external_body_of(&recorded.error))),
        Member::Resolving(doing) | Member::Planning(doing) => {
            let mut provenance = Map::new();
            provenance.insert("source".to_owned(), json!("project"));
            provenance.insert(
                "workspace".to_owned(),
                json!(workspace.root.display().to_string()),
            );
            provenance.insert("target".to_owned(), json!(target.as_str()));
            Err(Box::new(building_response(
                provenance,
                doing,
                "the project's build is being planned, to tell whether it documents the release",
                &[],
            )))
        },
    }
}

/// The project's dependency `package` of `workspace`, which the project's build does not build for
/// `target`: documented for it if `asked` (decided in M2e's E4a), as [`unnameable`] returns; else
/// the error, which tells what the project uses it for, as far as `cargo metadata` tells
/// (requirement R-WS-10).
async fn other_target(
    state: &Arc<ToolState>,
    (workspace, package, project_from): &(Workspace, ProjectPackage, ProjectFrom),
    (target, asked): (&TargetTriple, bool),
    until: Instant,
) -> Result<Option<TargetTriple>, Box<CallToolResult>> {
    if asked {
        return super::external_target::dependency(
            state,
            (workspace, *project_from),
            package,
            target,
        )
        .await
        .map(Some);
    }
    let conditions = match state.resolver.resolve_until(workspace, until).await {
        Joined::Done(Ok(resolution)) => match &*resolution {
            Resolution::Metadata { metadata, .. } => {
                super::external_target::conditions(metadata, package)
            },
            Resolution::CargoLockRaw { .. } => Vec::new(),
        },
        Joined::Done(Err(_)) | Joined::Ended | Joined::Going(_) => Vec::new(),
    };
    Err(Box::new(not_used_for(package, target, &conditions)))
}

/// `invalid_argument` for the project's dependency `package`, which the project's build does not
/// build for `target`, the build target: it uses it for `conditions` (requirement R-WS-10).
pub(super) fn not_used_for(
    package: &ProjectPackage,
    target: &TargetTriple,
    conditions: &[Box<str>],
) -> CallToolResult {
    let used_for = if conditions.is_empty() {
        "it may use it only for another target".to_owned()
    } else {
        format!("the project uses it for {}", conditions.join(", "))
    };
    error_result(&json!({
        "code": "invalid_argument",
        "message": format!(
            "the project's build does not build {} {} for {target}: {used_for}",
            package.name, package.version
        ),
        "retryable": false,
        "suggestions": ["give a target that the project uses it for as `target`"],
        "conditions": conditions,
    }))
}

/// The error `result` as this tool tells it: with its suggestions ([`external_body`]).
pub(super) fn external_body_of(result: &CallToolResult) -> CallToolResult {
    error_result(&external_body(body_of(result)))
}

/// The response for the project's dependency `package` of `workspace`, which the query tools build
/// for `target` themselves: nothing is built (requirement R-WS-10).
fn the_query_tools_build(
    (workspace, package, project_from): (&Workspace, &ProjectPackage, ProjectFrom),
    target: &TargetTriple,
) -> CallToolResult {
    let release = format!("{}@{}", package.name, package.version);
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
            "nothing was built: the other tools build this dependency of the project themselves; \
             look it up with get_item or crate_overview, passing {}",
            json!({ "crate": release, "target": target.as_str() })
        )],
    }))
}

/// Whether the default project uses the release of `entry` at `version`, as the query tools tell
/// it (decided in M2's D7): a package of crates.io that `cargo metadata` resolves, whose checksum
/// is the index's. Another of the same name and version (a path, git or another registry's
/// package, or a copy) is not it, and a project that cannot be read does not tell: the release of
/// crates.io is documented apart then, which builds nothing of the project.
async fn uses(
    state: &ToolState,
    (entry, version): (&IndexEntry, &semver::Version),
    project: Option<&Path>,
    until: Instant,
) -> Uses {
    let (workspace, project_from) = match project::workspace(state.project.as_ref(), project) {
        Ok(Some(found)) => found,
        Ok(None) => return Uses::No,
        Err(_) => {
            return Uses::Unknown(
                "the project has several workspaces, so whether one uses the release is not told: \
                 it is built as the release of crates.io alone"
                    .to_owned(),
            );
        },
    };
    match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) if matches!(&*resolution, Resolution::Metadata { .. }) => {
            let used = PackageGraph::new(&resolution)
                .crates_io_named(&entry.name)
                .into_iter()
                .find(|package| {
                    package.version == *version
                        && package.docs_rs_check(entry) == DocsRsCheck::Allowed
                })
                .cloned();
            match used {
                Some(package) => Uses::Yes(Box::new((workspace, package, project_from))),
                // Told, as the workspace was asked for.
                None if project.is_some() => Uses::Unknown(format!(
                    "the workspace given does not use {} {version}, so it is built as the release \
                     of crates.io alone",
                    entry.name
                )),
                None => Uses::No,
            }
        },
        Joined::Done(Ok(_) | Err(_)) => Uses::No,
        Joined::Ended => Uses::Response(Box::new(get_item::stopped())),
        Joined::Going(doing) => {
            Uses::Response(Box::new(resolving(&workspace, project_from, doing)))
        },
    }
}

/// The build that the record of `key` records, archived, if it is whole.
async fn whole_record(state: &ToolState, key: &Key) -> Option<LocalBuilt> {
    let (cache, key) = (state.cache.clone(), key.clone());
    let read = crate::task::spawn_blocking(move || external::read(&cache, &key))
        .await
        .ok()?;
    let Recorded::Built(built, told) = read else {
        return None;
    };
    recorded(&state.cache, *built, &told).await.ok()
}

/// What docs.rs has of a release.
pub(super) enum DocsRs {
    /// No rustdoc JSON that this server reads: it is built here.
    Lacks,
    /// It could not be asked, as this tells: it is built here.
    Unknown(String),
    /// The response to return: docs.rs has it, its fetch failed, or goes on.
    Response(Box<CallToolResult>),
}

/// What docs.rs has of the release of `name` at `version` for `target`, read as the query tools
/// read it (decided in M2e's E2a): if it has JSON that this server reads, nothing is built.
pub(super) async fn on_docs_rs(
    state: &ToolState,
    (name, version, target): (&CrateName, &semver::Version, &TargetTriple),
    until: Instant,
) -> DocsRs {
    let joined = load::docs_rs_archive(
        &state.archives,
        &state.cache,
        &state.docs_rs,
        name,
        version,
        Some(target),
        until,
    )
    .await;
    let release = format!("{name}@{version}");
    let error = match joined {
        Joined::Done(Ok(_)) => {
            return DocsRs::Response(Box::new(on_docs_rs_response(name, version, target)));
        },
        Joined::Done(Err(error)) => error,
        Joined::Ended => return DocsRs::Response(Box::new(get_item::stopped())),
        Joined::Going(doing) => {
            let mut provenance = Map::new();
            provenance.insert("source".to_owned(), json!("docs.rs"));
            provenance.insert("crate".to_owned(), json!(name.as_str()));
            provenance.insert("version".to_owned(), json!(version.to_string()));
            provenance.insert("target".to_owned(), json!(target.as_str()));
            let being = get_item::being(&release, doing, "docs.rs");
            return DocsRs::Response(Box::new(building_response(provenance, doing, &being, &[])));
        },
    };
    match &*error {
        ArchiveError::Load(
            LoadError::NoTargetJson { .. }
            | LoadError::Fetch(FetchError::NotFound | FetchError::BadRequest),
        )
        | ArchiveError::Make(load::LoadError::Parse(ParseError::UnsupportedFormat { .. })) => {
            DocsRs::Lacks
        },
        ArchiveError::Load(LoadError::Fetch(FetchError::Offline)) => DocsRs::Unknown(format!(
            "whether docs.rs has rustdoc JSON of {release} is not known offline, so it is built \
             here"
        )),
        error @ (ArchiveError::Load(_) | ArchiveError::Make(_) | ArchiveError::Stopped) => {
            DocsRs::Response(Box::new(get_item::archive_error_of(
                &state.cache,
                name,
                &release,
                error,
            )))
        },
    }
}

/// `release` with the features and the `--cfg` that docs.rs builds it with, as its `.crate`, of
/// `checksum`, tells (requirement R-FT-1, decided in M2e's E2a); or the error when the `.crate`
/// cannot be had, rather than a record of other features, which would last. A manifest that
/// cannot be read leaves the default features, with a warning.
async fn with_docs_rs_features(
    state: &ToolState,
    (mut release, version): (Release, &semver::Version),
    checksum: &str,
    warnings: &mut Vec<Box<str>>,
) -> Result<Release, Box<CallToolResult>> {
    let read = state
        .static_crates
        .crate_file(&state.cache, &release.name, version, Some(checksum))
        .await;
    let what = release.what();
    let contents = match read {
        Ok(Some(contents)) => contents,
        Ok(None) => return Err(Box::new(crate_failed(state, &what, "it is not there"))),
        Err(error) => {
            return Err(Box::new(crate_failed(
                state,
                &what,
                &ErrorChain(&error).to_string(),
            )));
        },
    };
    match crate_file::docs_rs_build(&contents, &release.name, version) {
        Ok(build) => as_docs_rs(&mut release, &build, warnings),
        Err(error) => warnings.push(
            format!(
                "the manifest in the .crate of {what} could not be read ({}), so it is built \
                 with its default features, not those of [package.metadata.docs.rs]",
                ErrorChain(&error)
            )
            .into(),
        ),
    }
    Ok(release)
}

/// Sets on `release` the features and the `--cfg` that docs.rs builds it with, as `build`, from
/// its manifest, tells (requirement R-FT-1): only the features that it declares, which cargo takes
/// for a dependency, warning of the others.
pub(super) fn as_docs_rs(
    release: &mut Release,
    build: &crate_file::DocsRsBuild,
    warnings: &mut Vec<Box<str>>,
) {
    let (features, undeclared): (Vec<_>, Vec<_>) = build
        .features
        .iter()
        .cloned()
        .partition(|feature| build.declared.contains(feature));
    if !undeclared.is_empty() {
        warnings.push(
            format!(
                "the features {} of the [package.metadata.docs.rs] of {} are not features that \
                 it declares, so they are left out",
                undeclared.join(", "),
                release.what()
            )
            .into(),
        );
    }
    release.features = features;
    release.cfgs.clone_from(&build.rustdoc_cfg);
    release.asks_features = build.asks_features;
}

/// The error when the `.crate` of `what` could not be had from static.crates.io, as `why` tells:
/// `offline_miss` offline, else `network_error`.
fn crate_failed(state: &ToolState, what: &str, why: &str) -> CallToolResult {
    let message = format!("the .crate of {what} could not be had from static.crates.io: {why}");
    if state.index.is_offline() {
        error_result(&json!({
            "code": "offline_miss",
            "message": message,
            "retryable": false,
            "suggestions": ["turn network.offline off in the server's settings to fetch it"],
        }))
    } else {
        error_result(&json!({
            "code": "network_error",
            "message": message,
            "retryable": true,
            "suggestions": ["try again in a while"],
        }))
    }
}

/// Builds `release` for `target` by the work that the calls for `key` share, waiting for it as
/// the query tools wait for a build (requirement R-WS-10): `build.wait_secs`, and while it is
/// indexed, until the deadline of the call `until`.
async fn built(
    state: &Arc<ToolState>,
    key: Key,
    (release, version): (Release, semver::Version),
    target: TargetTriple,
    warnings: Vec<Box<str>>,
    until: Instant,
) -> CallToolResult {
    let what = release.what();
    let until_built = Instant::now()
        .checked_add(state.build_wait)
        .map_or(until, |until_built| until_built.min(until));
    let flight = key.lock_key();
    let start = || {
        let (state, key, release, target) = (
            Arc::clone(state),
            key.clone(),
            release.clone(),
            target.clone(),
        );
        async move |stage: Arc<Stage>| make(&state, &key, release, target, &stage).await
    };
    let mut joined = state
        .externals
        .join(&flight, &what, until_built, start())
        .await;
    if matches!(joined, Joined::Going(Doing::Indexing)) && until_built < until {
        joined = state.externals.join(&flight, &what, until, start()).await;
    }
    let looked_up = (&release.name, &target);
    match joined {
        Joined::Done(Ok(built)) => built_response(&built, looked_up, &warnings),
        Joined::Done(Err(body)) => error_result(&body),
        Joined::Ended => get_item::stopped(),
        Joined::Going(doing) => going(looked_up, &version, doing, &warnings),
    }
}

/// The response while the build of `name` at `version` for `target` goes on, past the deadline:
/// the query tools wait for it, without the confirmation that this tool may ask for (requirement
/// R-WS-10).
fn going(
    (name, target): (&CrateName, &TargetTriple),
    version: &semver::Version,
    doing: Doing,
    warnings: &[Box<str>],
) -> CallToolResult {
    let what = format!("{name}@{version}");
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("version".to_owned(), json!(version.to_string()));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
    let being = get_item::being(&what, doing, "crates.io");
    let mut result = building_response(provenance, doing, &being, warnings);
    if let Some(Value::Object(response)) = &mut result.structured_content {
        response.insert(
            "hints".to_owned(),
            json!([format!(
                "{being}, which goes on without this call: look it up with get_item or \
                 crate_overview, passing {}, which answer once it is built; do not call \
                 build_external_docs again for it",
                json!({ "crate": what, "target": target.as_str() })
            )]),
        );
    }
    result
}

/// Why the work did not build a release.
enum Unmade {
    /// It stopped, as the server shuts down: not remembered.
    Stopped,
    /// It failed, as the body of the error tells, which is remembered; with the compiler's errors,
    /// which the features may cause, if `compiler`.
    Failed {
        /// The body of the error.
        body: Value,
        /// Whether the compiler's errors failed it ([`Failed::compiler_errors`]).
        compiler: bool,
    },
}

/// The work that documents `release` for `target` and records it as `key`, telling `progress` what
/// it does.
///
/// It holds the lock of the key throughout. A record made meanwhile by another server is taken as
/// it is; a failure or a broken record is forgotten, and so is the failure of the build in the
/// temporary project, which is made again (requirement R-WS-10).
pub(super) async fn make(
    state: &ToolState,
    key: &Key,
    release: Release,
    target: TargetTriple,
    progress: &Stage,
) -> SharedExternal {
    let what = release.what();
    let making = take_lock(state, key, progress)
        .await
        .map_err(|why| Arc::new(failed_so(&what, &why, true)))?;
    let (recorded_now, making) = read_record(state, key, making)
        .await
        .ok_or_else(|| Arc::new(stopped_body()))?;
    let refresh = match recorded_now {
        Recorded::Built(built, told) => match recorded(&state.cache, *built, &told).await {
            Ok(built) => return Ok(Arc::new(built)),
            Err(_) => true,
        },
        Recorded::Failed(_) | Recorded::Broken(_) => true,
        Recorded::Building | Recorded::Missing => false,
    };
    let (made, told) = with_a_set(state, (key, release, target), refresh, progress).await;
    match made {
        Ok(built) => record_made(state, key, (&built, told), making, &what).await,
        Err(Unmade::Stopped) => Err(Arc::new(stopped_body())),
        Err(Unmade::Failed { body, .. }) => Err(remember_unmade(state, key, body, making).await),
    }
}

/// Records `built`, of `what`, made under the lock of `key` (`making`), with what its record
/// tells, `told`, and returns it as the query tools read it from the record.
pub(super) async fn record_made(
    state: &ToolState,
    key: &Key,
    (built, told): (&LocalBuilt, Told),
    making: Making,
    what: &str,
) -> SharedExternal {
    let (cache, key) = (state.cache.clone(), key.clone());
    let to_record = Built {
        json: built.json.clone(),
        record: built.record.clone(),
        rebuilt: Vec::new(),
    };
    let recording = told.clone();
    let written = crate::task::spawn_blocking(move || {
        external::record(&cache, &key, (&to_record, &recording), &making)
    })
    .await;
    if let Ok(Err(error)) | Err(error) = written.map_err(std::io::Error::other) {
        tracing::warn!(%error, "an external record could not be written");
    }
    let built = Built {
        json: built.json.clone(),
        record: built.record.clone(),
        rebuilt: Vec::new(),
    };
    recorded(&state.cache, built, &told)
        .await
        .map(Arc::new)
        .map_err(|error| Arc::new(failed_so(what, &error.to_string(), true)))
}

/// Remembers that making the record of `key` under its lock (`making`) failed, as `body` tells,
/// for the query tools; and returns `body`.
pub(super) async fn remember_unmade(
    state: &ToolState,
    key: &Key,
    body: Value,
    making: Making,
) -> Arc<Value> {
    let (cache, key) = (state.cache.clone(), key.clone());
    let transient = body.get("retryable") == Some(&Value::Bool(true));
    let failed = Failed::now(None, None, transient, body.clone());
    let remembered = crate::task::spawn_blocking(move || {
        external::remember_failure(&cache, &key, &failed, &making)
    })
    .await;
    if let Ok(Err(error)) | Err(error) = remembered.map_err(std::io::Error::other) {
        tracing::warn!(%error, "the failure of an external build could not be remembered");
    }
    Arc::new(body)
}

/// `release` built for `target` in the temporary project of `key` with the features that docs.rs
/// builds it with, or else, as the compiler's errors with those broke it, with its default
/// features (decided in M2e's E2b); with what its record tells. `refresh` forgets the failures of
/// the builds before.
async fn with_a_set(
    state: &ToolState,
    (key, release, target): (&Key, Release, TargetTriple),
    refresh: bool,
    progress: &Stage,
) -> (Result<Arc<LocalBuilt>, Unmade>, Told) {
    let requested = release.features.clone();
    let as_docs_rs = (key, release.clone(), target.clone());
    let made = build_in_temporary_project(state, as_docs_rs, Set::DocsRs, refresh, progress).await;
    let docs_rs: BTreeSet<&str> = requested.iter().map(|feature| &**feature).collect();
    // Not when it asks for none, or for the default ones, which would build the same.
    let fell_back = matches!(made, Err(Unmade::Failed { compiler: true, .. }))
        && release.asks_features
        && docs_rs != BTreeSet::from(["default"]);
    if !fell_back {
        let told = Told {
            features_requested: Some(requested),
            ..Told::default()
        };
        return (made, told);
    }
    let asked = requested.join(", ");
    tracing::info!(release = %release.name, "the features of [package.metadata.docs.rs] broke the build, so it is built with the default features");
    let default = Release {
        features: vec!["default".into()],
        ..release
    };
    let made = build_in_temporary_project(
        state,
        (key, default, target),
        Set::Default,
        refresh,
        progress,
    )
    .await;
    // What failed with its default features failed with those first.
    let made = made.map_err(|unmade| match unmade {
        Unmade::Failed { mut body, compiler } => {
            if let Some(Value::String(message)) = body.get_mut("message") {
                *message = format!(
                    "{message}; with the features of [package.metadata.docs.rs] ({asked}), the \
                     compiler failed first"
                );
            }
            Unmade::Failed { body, compiler }
        },
        Unmade::Stopped => Unmade::Stopped,
    });
    let told = Told {
        features_requested: Some(vec!["default".into()]),
        warnings: vec![
            format!(
                "built with its default features, as those of [package.metadata.docs.rs] \
                 ({asked}) broke the build: the documentation may differ from what docs.rs would \
                 show"
            )
            .into(),
        ],
        ..Told::default()
    };
    (made, told)
}

/// The record of `key`, as it is now, for the holder of its lock, `making`, which it gives back;
/// `None` when the task did not end (as the server shuts down, or a panic), which lets the lock go.
pub(super) async fn read_record(
    state: &ToolState,
    key: &Key,
    making: Making,
) -> Option<(Recorded, Making)> {
    let (cache, key) = (state.cache.clone(), key.clone());
    crate::task::spawn_blocking(move || (external::read_making(&cache, &key, &making), making))
        .await
        .ok()
}

/// The lock of `key`, waited for while another server makes its record (decided in M2e, 13), at
/// most [`OTHER_MAKES_FOR`]; or why it could not be taken.
pub(super) async fn take_lock(
    state: &ToolState,
    key: &Key,
    progress: &Stage,
) -> Result<Making, String> {
    let started = Instant::now();
    loop {
        let (cache, tried) = (state.cache.clone(), key.clone());
        let taken = crate::task::spawn_blocking(move || external::try_make(&cache, &tried))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("the lock of the record cannot be taken: {error}"))?;
        if let Some(making) = taken {
            return Ok(making);
        }
        // A query tool's try holds it for a moment; another server, for its build.
        progress.set(Doing::WaitingForLock);
        if started.elapsed() > OTHER_MAKES_FOR {
            return Err("another server has been making the record for too long".to_owned());
        }
        tokio::time::sleep(TRY_AGAIN_EVERY).await;
    }
}

/// Writes the temporary project of `release` that builds with `set` in the directory of the record
/// of `key`, makes its Cargo.lock, and builds and archives the release in it for `target`,
/// forgetting a failure of the build if `refresh`; or why not.
async fn build_in_temporary_project(
    state: &ToolState,
    (key, release, target): (&Key, Release, TargetTriple),
    set: Set,
    refresh: bool,
    progress: &Stage,
) -> Result<Arc<LocalBuilt>, Unmade> {
    let what = release.what();
    let failed = |why: &str| Unmade::Failed {
        body: failed_so(&what, why, false),
        compiler: false,
    };
    let dir = temp_project::dir_in(&key.dir(state.cache.dir()), set);
    let (written, wanted) = (dir.clone(), release.clone());
    let workspace = crate::task::spawn_blocking(move || {
        temp_project::write(&written, &wanted)?;
        workspace_of(&written).map_err(std::io::Error::other)
    })
    .await
    .map_err(|error| failed(&error.to_string()))?
    .map_err(|error| failed(&error.to_string()))?;
    let root = workspace.root.clone();
    // The user's cargo settings apply to it too, which may build their own std, before cargo runs
    // (decided in M2's D7).
    let project_sets_target = match project::build_targets(state, &root).await {
        project::BuildTargets::These(_) => project::sets_build_target(state, &root).await,
        project::BuildTargets::OwnTarget(own) => {
            return Err(failed(&format!(
                "the cargo settings build their own std ({own}), which this server does not \
                 document"
            )));
        },
    };
    let offline = state.resolver.offline_in(&root).await;
    let resolution = resolved(state, &workspace, (&what, offline), progress)
        .await
        .map_err(|body| Unmade::Failed {
            body,
            compiler: false,
        })?;
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return Err(failed("cargo metadata did not resolve it"));
    };
    let plan = temp_project::plan(
        (&root, metadata),
        &release,
        None,
        (state.cache.dir(), offline),
    )
    .map_err(|error| failed(&error.to_string()))?;
    let wanted = LocalWanted {
        plan,
        target,
        project_sets_target,
        cargo_home: state.resolver.cargo_home().map(Path::to_path_buf),
        docs_rs_features: None,
        warnings: Vec::new(),
        docs_rs_beyond_scope: false,
        refresh,
        input_files: state.resolver.input_files(&root, metadata).await,
        input_packages: Vec::new(),
    };
    progress.set(Doing::Building);
    match local_archive_telling(&state.locals, &state.cache, wanted, progress).await {
        Some(Ok(built)) => Ok(built),
        Some(Err(error)) if matches!(*error, LocalError::Stopped) => Err(Unmade::Stopped),
        Some(Err(error)) => Err(Unmade::Failed {
            body: external_body(body_of(&local::local_error(&what, &error))),
            compiler: matches!(&*error, LocalError::Failed { failed, .. } if failed.compiler_errors),
        }),
        None => Err(Unmade::Stopped),
    }
}

/// The packages of the temporary project `workspace`, by `cargo metadata`, after its Cargo.lock is
/// made, offline if `offline`; or the body of the error, of `what`. A Cargo.lock that cargo
/// cannot read, as one cut short, is made again once, but not for what is missing offline.
async fn resolved(
    state: &ToolState,
    workspace: &Workspace,
    (what, offline): (&str, bool),
    progress: &Stage,
) -> Result<Arc<Resolution>, Value> {
    let root = &workspace.root;
    let mut body = Value::Null;
    for again in [false, true] {
        if again {
            remove_lock(root).await;
            state.resolver.forget_failures(root).await;
        }
        progress.set(Doing::Fetching);
        if let Err(error) = temp_project::lock(root, offline, |doing| progress.set(doing)).await {
            // Making a Cargo.lock only fetches.
            let (failure, stderr) = if let BuildError::Fetch { failure, stderr } = &error {
                (Some(*failure), stderr.as_ref())
            } else {
                (None, "")
            };
            return Err(cargo_failure(
                what,
                failure,
                error.is_transient(),
                &format!("{}{stderr}", ErrorChain(&error)),
            ));
        }
        match state.resolver.resolve(workspace).await {
            Ok(resolution) => match &*resolution {
                Resolution::Metadata { .. } => return Ok(resolution),
                Resolution::CargoLockRaw { because, .. } => {
                    let (failure, stderr) = if let CargoError::Failed {
                        failure, stderr, ..
                    } = because
                    {
                        (Some(*failure), stderr.as_ref())
                    } else {
                        (None, "")
                    };
                    body = cargo_failure(
                        what,
                        failure,
                        true,
                        &format!("{}: {stderr}", ErrorChain(because)),
                    );
                    if failure == Some(Failure::OfflineMiss) {
                        break;
                    }
                },
            },
            Err(error) => body = failed_so(what, &ErrorChain(&*error).to_string(), true),
        }
    }
    Err(body)
}

/// The body of the error of cargo for `what`, as `failure` classifies it, which may work when
/// tried again if `transient` (requirement R-WS-7), as `why` tells.
pub(super) fn cargo_failure(
    what: &str,
    failure: Option<Failure>,
    transient: bool,
    why: &str,
) -> Value {
    let mut body = failed_so(what, why, transient);
    if let Some(failure) = failure
        && let Some(fields) = body.as_object_mut()
    {
        fields.insert("code".to_owned(), json!(failure.code()));
        if let Failure::GitFetch(reason) = failure {
            fields.insert("reason".to_owned(), json!(reason.as_str()));
        }
        if matches!(failure, Failure::OfflineMiss | Failure::LockfileOutdated) {
            fields.insert("retryable".to_owned(), json!(false));
        }
    }
    external_body(body)
}

/// `body`, an error of a build, with what to do of this tool: not what the query tools tell, as
/// `refresh` or the project's terminal (requirement R-WS-10).
pub(super) fn external_body(mut body: Value) -> Value {
    let offline = body.get("code") == Some(&json!("offline_miss"));
    if let Some(fields) = body.as_object_mut() {
        let mut suggestions: Vec<Value> = match fields.remove("suggestions") {
            Some(Value::Array(suggestions)) => suggestions
                .into_iter()
                .filter(|suggestion| {
                    suggestion.as_str().is_some_and(|text| {
                        !text.contains("refresh")
                            && !text.contains("project's terminal")
                            && text != CALL_AGAIN
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        if offline {
            suggestions.push(json!(
                "turn network.offline off in the server's settings, so that cargo fetches it"
            ));
        }
        suggestions.push(json!(CALL_AGAIN));
        fields.insert("suggestions".to_owned(), Value::Array(suggestions));
    }
    body
}

/// What calls this tool again, which builds again what failed (requirement R-WS-10).
pub(super) const CALL_AGAIN: &str =
    "call build_external_docs again with the same arguments to try again";

/// Removes the Cargo.lock of the temporary project at `root`, if it has one: cargo could not read
/// it, or it is made anew.
pub(super) async fn remove_lock(root: &Path) {
    let lock = root.join("Cargo.lock");
    let removed = crate::task::spawn_blocking(move || std::fs::remove_file(lock)).await;
    if let Ok(Err(error)) = removed
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "the Cargo.lock of a temporary project could not be removed");
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

/// The body of the error when the work stopped without a result.
pub(super) fn stopped_body() -> Value {
    body_of(&get_item::stopped())
}

/// The body of `build_failed` for `what`, as `why` tells, which may work when tried again if
/// `retryable`.
pub(super) fn failed_so(what: &str, why: &str, retryable: bool) -> Value {
    json!({
        "code": "build_failed",
        "message": format!("the documentation of {what} could not be built: {why}"),
        "retryable": retryable,
        "suggestions": [CALL_AGAIN],
    })
}

/// The response for `built`, of `name` for `target` (its key, which the JSON of a proc macro,
/// built for the host, does not tell): the keys to look it up by (requirement R-WS-10).
fn built_response(
    built: &LocalBuilt,
    (name, target): (&CrateName, &TargetTriple),
    warnings: &[Box<str>],
) -> CallToolResult {
    let record = &built.record;
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    provenance.insert("package_source".to_owned(), json!("crates.io"));
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("version".to_owned(), json!(record.crate_version));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    built_provenance(&mut provenance, built);
    let mut warnings: Vec<Box<str>> = warnings.to_vec();
    warnings.extend(built.archived.warnings.iter().cloned());
    let release = format!("{name}@{}", record.crate_version);
    let mut response = Map::new();
    response.insert("provenance".to_owned(), Value::Object(provenance));
    if !warnings.is_empty() {
        response.insert("warnings".to_owned(), json!(warnings));
    }
    response.insert(
        "data".to_owned(),
        json!({
            "built": true,
            "crate": name.as_str(),
            "version": record.crate_version,
            "target": target.as_str(),
        }),
    );
    response.insert(
        "hints".to_owned(),
        json!([format!(
            "look it up with get_item, crate_overview or search_items, passing {}",
            json!({ "crate": release, "target": target.as_str() })
        )]),
    );
    CallToolResult::structured(Value::Object(response))
}

/// Adds to `provenance` what the record of `built` tells: the toolchain, when it was built, and
/// its features; and that this tool recorded it.
pub(super) fn built_provenance(provenance: &mut Map<String, Value>, built: &LocalBuilt) {
    let record = &built.record;
    if let Some(toolchain) = record.toolchain.as_deref().and_then(|t| t.lines().next()) {
        provenance.insert("toolchain".to_owned(), json!(toolchain));
    }
    provenance.insert("built_at".to_owned(), json!(record.built_at));
    if let Some(features) = &record.features_requested {
        provenance.insert("features_requested".to_owned(), json!(features));
    }
    if let Some(features) = &record.features_effective {
        provenance.insert("features_effective".to_owned(), json!(features));
    }
    provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
}

/// The response when docs.rs has rustdoc JSON of `name` at `version` for `target` that this
/// server reads: nothing is built (requirement R-WS-10).
fn on_docs_rs_response(
    name: &CrateName,
    version: &semver::Version,
    target: &TargetTriple,
) -> CallToolResult {
    let release = format!("{name}@{version}");
    CallToolResult::structured(json!({
        "provenance": {
            "source": "docs.rs",
            "crate": name.as_str(),
            "version": version.to_string(),
            "target": target.as_str(),
        },
        "data": {
            "built": false,
            "reason": "docs_rs",
            "crate": name.as_str(),
            "version": version.to_string(),
            "target": target.as_str(),
        },
        "hints": [format!(
            "nothing was built: docs.rs has it; look it up with get_item or crate_overview, \
             passing {}",
            json!({ "crate": release, "target": target.as_str() })
        )],
    }))
}

/// The error for the yanked `version` of `name`, which cargo does not resolve anew: with the
/// versions near it that are not yanked (decided in M2e's E2a).
fn yanked(
    name: &str,
    version: &semver::Version,
    versions: &[crate::data::sparse_index::IndexVersion],
) -> CallToolResult {
    let mut near: Vec<&semver::Version> = versions
        .iter()
        .filter(|other| !other.yanked && other.version.major == version.major)
        .map(|other| &other.version)
        .collect();
    near.sort_by_key(|other| std::cmp::Reverse(*other));
    let candidates: Vec<String> = near.iter().take(5).map(ToString::to_string).collect();
    error_result(&json!({
        "code": "invalid_argument",
        "message": format!(
            "{name} {version} is yanked, and cargo does not resolve a yanked version anew, so it \
             cannot be built here; docs.rs may have it: look it up with get_item"
        ),
        "retryable": false,
        "suggestions": ["ask for one of the candidates, which are not yanked"],
        "candidates": candidates,
    }))
}
