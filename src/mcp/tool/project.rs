//! The version of a crate in the project, when the caller gives none (requirements R-MCP-3 and
//! R-VR-2), and the target of a lookup that names none (requirement R-TG-1): the project is the
//! default one, or the one the `project` argument names.

use std::path::Path;
#[cfg(feature = "local")]
use std::sync::Arc;

#[cfg(feature = "local")]
use rmcp::model::CallToolResult;
#[cfg(feature = "local")]
use serde_json::json;
use tokio::time::Instant;

use super::ToolState;
#[cfg(feature = "local")]
use super::{ErrorChain, error_result, recorded_workspace};
#[cfg(feature = "local")]
use crate::data::flight::{Doing, Joined};
#[cfg(feature = "local")]
use crate::data::{
    cargo::{Failure, GitFetchFailure},
    cargo_config::{self, CargoConfig},
    metadata::{CargoError, Featured, Resolution, ResolveError, Resolved},
    packages::{Candidate, Lookup, PackageGraph, ProjectPackage},
    project::{self, DefaultProject, ProjectFrom, Workspace},
};
use crate::data::{crate_name::CrateName, target::TargetTriple};
#[cfg(feature = "local")]
use crate::query::get_item::ResolvedFrom;

/// What the project says about a crate.
#[derive(Debug)]
pub(super) enum InProject {
    /// The project resolves the crate to this package.
    #[cfg(feature = "local")]
    Found(Box<Found>),
    /// The project does not decide the version: there is no project, or the crate is not one of
    /// its packages. The latest version is used, with these warnings.
    Elsewhere(Vec<Box<str>>),
    /// The version cannot be decided: the tool error to return.
    #[cfg(feature = "local")]
    Error(Box<CallToolResult>),
    /// cargo is still resolving the packages, as this says, past the deadline of the call; it goes
    /// on (requirement R-WS-6). Where the project came from is given.
    #[cfg(feature = "local")]
    Going(Doing, &'static str),
}

/// A package of the project that a crate name means.
#[cfg(feature = "local")]
#[derive(Debug, Clone)]
pub(super) struct Found {
    /// The package, with its source and checksum.
    pub(super) package: ProjectPackage,
    /// `cargo_lock`, or `cargo_lock_raw` when `cargo metadata` failed.
    pub(super) resolved_from: ResolvedFrom,
    /// Where the project came from (§7.3).
    pub(super) project_from: &'static str,
    /// What the response warns about.
    pub(super) warnings: Vec<Box<str>>,
    /// The workspace, whose build decides `enabled_in_project` (requirement R-FT-2).
    pub(super) workspace: Workspace,
}

/// The package that `name` means in the project: the one `project` names (a `Cargo.toml` or a
/// directory, relative to the default project's directory), or else the default project.
#[cfg(feature = "local")]
pub(super) async fn version_in_project(
    state: &ToolState,
    name: &CrateName,
    project: Option<&Path>,
    until: Instant,
) -> InProject {
    let (workspace, project_from) = match located(state, project).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            return InProject::Elsewhere(vec![
                "there is no project here, so the latest release is used".into(),
            ]);
        },
        Err(error) => return InProject::Error(error),
    };
    let resolution = match resolve_until(state, &workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return InProject::Error(Box::new(resolve_error(&workspace, &error)));
        },
        Joined::Ended => return InProject::Error(Box::new(super::get_item::stopped())),
        Joined::Going(doing) => return InProject::Going(doing, project_from.as_str()),
    };
    let mut warnings = Vec::new();
    let resolved_from = match &*resolution {
        // As `build_external_docs` recorded it (decided in M2e's E4b-3).
        Resolution::Metadata { .. } if recorded_workspace::is_outside(state, &workspace) => {
            ResolvedFrom::External
        },
        Resolution::Metadata { .. } => ResolvedFrom::CargoLock,
        Resolution::CargoLockRaw { because, .. } => {
            warnings.extend(raw_warnings(because));
            ResolvedFrom::CargoLockRaw
        },
    };
    let graph = PackageGraph::new(&resolution);
    let package = match graph.look_up(name.as_str()) {
        Lookup::Found {
            package,
            other_versions,
        } => {
            if !other_versions.is_empty() {
                let others: Vec<String> = other_versions
                    .iter()
                    .map(|other| other.version.to_string())
                    .collect();
                warnings.push(
                    format!(
                        "the project also has {name} {}, which no member depends on directly; \
                         {} is shown",
                        others.join(", "),
                        package.version
                    )
                    .into(),
                );
            }
            package.clone()
        },
        Lookup::AmbiguousVersion(candidates) => {
            return InProject::Error(Box::new(ambiguous("ambiguous_version", name, &candidates)));
        },
        Lookup::AmbiguousPackage(candidates) => {
            return InProject::Error(Box::new(ambiguous("ambiguous_crate", name, &candidates)));
        },
        Lookup::NotFound => {
            return InProject::Elsewhere(vec![
                format!("{name} is not a dependency of the project, so the latest release is used")
                    .into(),
            ]);
        },
    };
    InProject::Found(Box::new(Found {
        package,
        resolved_from,
        project_from: project_from.as_str(),
        warnings,
        workspace,
    }))
}

/// The project's packages from crates.io named `name`, for a lookup that gives the version: if
/// one is at the version looked up, the project's build of it tells `enabled_in_project`
/// (requirements R-VR-1 and R-FT-2). The project is the one `project` names, or else the default
/// project. `None` when there is none, it is not resolved by `cargo metadata`, or it has no such
/// package: the lookup goes on without it, as before.
#[cfg(feature = "local")]
pub(super) async fn crates_io_in_project(
    state: &ToolState,
    name: &CrateName,
    project: Option<&Path>,
    until: Instant,
) -> Option<FromCratesIo> {
    let (workspace, project_from) = located(state, project).await.ok()??;
    // Past the deadline, the lookup goes on without it; the resolution goes on for later calls.
    let Joined::Done(Ok(resolution)) = resolve_until(state, &workspace, until).await else {
        return None;
    };
    // The build's features are known only from `cargo metadata` and `cargo tree`.
    let Resolution::Metadata { .. } = &*resolution else {
        return None;
    };
    let packages: Vec<ProjectPackage> = PackageGraph::new(&resolution)
        .crates_io_named(name.as_str())
        .into_iter()
        .cloned()
        .collect();
    (!packages.is_empty()).then(|| FromCratesIo {
        packages,
        project_from: project_from.as_str(),
        workspace,
    })
}

/// The project's packages from crates.io of one name.
#[cfg(feature = "local")]
#[derive(Debug)]
pub(super) struct FromCratesIo {
    /// The packages, one for each version.
    pub(super) packages: Vec<ProjectPackage>,
    /// Where the project came from (§7.3).
    pub(super) project_from: &'static str,
    /// The workspace, whose build decides `enabled_in_project`.
    pub(super) workspace: Workspace,
}

/// Without local features, there is no project context (requirement R-ARCH-3).
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs cargo"
)]
pub(super) async fn version_in_project(
    _state: &ToolState,
    _name: &CrateName,
    project: Option<&Path>,
    _until: Instant,
) -> InProject {
    let mut warnings: Vec<Box<str>> = vec![
        "this server is built without local features, so it does not read the project; the \
         latest release is used"
            .into(),
    ];
    if project.is_some() {
        warnings.push("`project` is ignored".into());
    }
    InProject::Elsewhere(warnings)
}

/// The target of a lookup of std that names none, with what the response warns about
/// (requirement R-TG-1): the `build.target` of the project (the one `project` names, or the
/// default one), the first of several; or else the host's, also for a project that builds for a
/// target of its own, whose std this server does not document.
pub(super) async fn default_target(
    state: &ToolState,
    project: Option<&Path>,
) -> (TargetTriple, Vec<Box<str>>) {
    let (target, mut warnings, own) = default_target_of(state, project).await;
    if let Some(own) = own {
        warnings.push(
            format!(
                "the project builds its own std, for a target of its own or with build-std \
                 ({own}), which this server does not document, so the host's target is used"
            )
            .into(),
        );
    }
    (target, warnings)
}

/// The target of a lookup, and of the rustdoc JSON of docs.rs for it (requirement R-TG-1).
#[derive(Debug, Clone)]
pub(super) struct LookupTarget {
    /// The target asked for, or else the project's, or the host's.
    pub(super) triple: TargetTriple,
    /// Whether docs.rs's JSON of each crate is of its default target there: none was asked for,
    /// and the project builds for a target of its own with its own std (decided in M2's D7).
    pub(super) docs_rs_default: bool,
}

impl LookupTarget {
    /// The target of the JSON of docs.rs: `None` for the default target of each crate there.
    pub(super) fn docs_rs(&self) -> Option<&TargetTriple> {
        (!self.docs_rs_default).then_some(&self.triple)
    }
}

/// The target of a lookup that asks for `asked`, or for none in the project `project`
/// (requirement R-TG-1), with what the response warns about: for a project that builds for a
/// target of its own with its own std, docs.rs's default target of each crate, as its provenance
/// tells (decided in M2's D7).
pub(super) async fn lookup_target(
    state: &ToolState,
    asked: Option<&TargetTriple>,
    project: Option<&Path>,
) -> (LookupTarget, Vec<Box<str>>) {
    if let Some(triple) = asked {
        let target = LookupTarget {
            triple: triple.clone(),
            docs_rs_default: false,
        };
        return (target, Vec::new());
    }
    let (triple, mut warnings, own) = default_target_of(state, project).await;
    if let Some(own) = &own {
        warnings.push(
            format!(
                "the project builds its own std, for a target of its own or with build-std \
                 ({own}), which docs.rs does not build for: the rustdoc JSON of each crate on \
                 docs.rs is read for its default target there"
            )
            .into(),
        );
    }
    let target = LookupTarget {
        triple,
        docs_rs_default: own.is_some(),
    };
    (target, warnings)
}

/// The target of a lookup that names none: the project's `build.target`, the first of several,
/// or else the host's; with what the response warns about, and the target of its own, with its
/// own std, that the project builds for instead, if any.
async fn default_target_of(
    state: &ToolState,
    #[cfg_attr(
        not(feature = "local"),
        expect(
            unused_variables,
            reason = "without local features, no project is read"
        )
    )]
    project: Option<&Path>,
) -> (TargetTriple, Vec<Box<str>>, Option<String>) {
    let mut warnings = Vec::new();
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_mut, reason = "without local features, no project is read")
    )]
    let mut own = None;
    #[cfg(feature = "local")]
    if let Some(target) = project_target(state, project, &mut warnings, &mut own).await {
        return (target, warnings, None);
    }
    let host = state.host().await;
    if let Some(why) = &host.stand_in_because {
        warnings.push(
            format!(
                "the target {} is used, as the host target is not known: {why}",
                host.triple
            )
            .into(),
        );
    }
    (host.triple.clone(), warnings, own)
}

/// Whether the project at `root` sets `build.target` in cargo's settings (or `CARGO_BUILD_TARGET`):
/// a build then passes `--target` even for the host, which cargo would else take from the setting
/// (requirement R-WS-1). A project that builds for a target of its own is not built here
/// (decided in M2's D7).
#[cfg(feature = "local")]
pub(super) async fn sets_build_target(state: &ToolState, root: &Path) -> bool {
    read_cargo_config(state, root)
        .await
        .is_some_and(|config| config.is_ok_and(|config| config.build_target.is_some()))
}

/// The targets that the project at `root` builds for (requirement R-MCP-3): its `build.target`,
/// all of several, or else the host's; or the target of its own, with its own std, which this
/// server does not build for (requirement R-TG-1, decided in M2's D7). A setting that cannot be
/// read, or that has a value that is not a target, is taken for the host's, as for a lookup
/// without a target.
#[cfg(feature = "local")]
pub(super) async fn build_targets(state: &ToolState, root: &Path) -> BuildTargets {
    let host = state.host().await.triple.clone();
    match read_cargo_config(state, root).await {
        Some(Ok(config)) => match targets_of(&config, &host) {
            Ok(Some(targets)) => BuildTargets::These(targets),
            Err(TargetsError::OwnTarget(own)) => BuildTargets::OwnTarget(own),
            Ok(None) | Err(TargetsError::NotATarget(_)) => BuildTargets::These(vec![host]),
        },
        Some(Err(_)) | None => BuildTargets::These(vec![host]),
    }
}

/// The targets that a project builds for.
#[cfg(feature = "local")]
pub(super) enum BuildTargets {
    /// These, which this server builds for.
    These(Vec<TargetTriple>),
    /// A target of its own, with its own std, as its settings name it.
    OwnTarget(String),
}

/// Why the targets of `build.target` are not taken.
#[cfg(feature = "local")]
#[derive(Debug, PartialEq, Eq)]
enum TargetsError {
    /// The project builds for a target of its own, with its own std.
    OwnTarget(String),
    /// A value is not a target, as this tells.
    NotATarget(String),
}

/// The cargo settings of the project at `root`, with `CARGO_BUILD_TARGET` of the server's
/// environment, which cargo that the server starts sees. `None` when the read did not end.
#[cfg(feature = "local")]
async fn read_cargo_config(
    state: &ToolState,
    root: &Path,
) -> Option<Result<CargoConfig, cargo_config::ConfigError>> {
    let (root, home) = (
        root.to_path_buf(),
        state.resolver.cargo_home().map(Path::to_path_buf),
    );
    let env = std::env::var("CARGO_BUILD_TARGET").ok();
    crate::task::spawn_blocking(move || cargo_config::read(&root, home.as_deref(), env.as_deref()))
        .await
        .ok()
}

/// The targets of `build.target` of `config`, with `host-tuple` for `host`, as cargo 1.91 and
/// later take it; `None` when it is not set. The error tells why the host's target is used
/// instead: a target of its own, which this server does not document yet, or a value that is not
/// a target.
#[cfg(feature = "local")]
fn targets_of(
    config: &CargoConfig,
    host: &TargetTriple,
) -> Result<Option<Vec<TargetTriple>>, TargetsError> {
    if let Some(own) = config.own_target() {
        return Err(TargetsError::OwnTarget(own));
    }
    let Some(build_target) = &config.build_target else {
        return Ok(None);
    };
    build_target
        .all()
        .iter()
        .map(|target| {
            let target = target.trim();
            if target == "host-tuple" {
                return Ok(host.clone());
            }
            target.parse().map_err(|error| {
                TargetsError::NotATarget(format!(
                    "build.target {target:?} of the project is not a target ({error}), so the \
                     host's is used"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|targets| (!targets.is_empty()).then_some(targets))
}

/// The project's `build.target`, as cargo's settings give it (Q-31), when it is a target this
/// server documents: the first of several. A target of its own (a target JSON, or with
/// `build-std`) is not: it is put in `own`, for the caller to tell (decided in M2's D7).
#[cfg(feature = "local")]
async fn project_target(
    state: &ToolState,
    project: Option<&Path>,
    warnings: &mut Vec<Box<str>>,
    own: &mut Option<String>,
) -> Option<TargetTriple> {
    let workspace = match place(state.project.as_ref(), project) {
        Ok(Place::Here(workspace, _)) => workspace,
        // As `build_external_docs` recorded it, the first of the targets that it builds for
        // (decided in M2e's E4b-3).
        Ok(Place::Outside(workspace)) => {
            let recorded = recorded_workspace::read(state, &workspace).await.ok()?;
            let targets: Vec<TargetTriple> = recorded
                .targets
                .iter()
                .filter_map(|target| target.parse().ok())
                .collect();
            return first_of(targets, warnings);
        },
        Ok(Place::Nowhere) | Err(_) => return None,
    };
    let config = match read_cargo_config(state, &workspace.root).await? {
        Ok(config) => config,
        Err(error) => {
            warnings.push(
                format!(
                    "the cargo settings of the project cannot be read ({}), so the host's target \
                     is used",
                    ErrorChain(&error)
                )
                .into(),
            );
            return None;
        },
    };
    let host = state.host().await.triple.clone();
    let targets = match targets_of(&config, &host) {
        Ok(targets) => targets?,
        Err(TargetsError::OwnTarget(target)) => {
            *own = Some(target);
            return None;
        },
        Err(TargetsError::NotATarget(why)) => {
            warnings.push(why.into());
            return None;
        },
    };
    first_of(targets, warnings)
}

/// The first of `targets` that a project builds for, with a warning when there are several.
#[cfg(feature = "local")]
fn first_of(targets: Vec<TargetTriple>, warnings: &mut Vec<Box<str>>) -> Option<TargetTriple> {
    if let [first, others @ ..] = targets.as_slice()
        && !others.is_empty()
    {
        let all: Vec<&str> = targets.iter().map(TargetTriple::as_str).collect();
        warnings.push(
            format!(
                "the project builds for several targets ({}); {first} is used: pass `target` for \
                 another",
                all.join(", ")
            )
            .into(),
        );
    }
    targets.into_iter().next()
}

/// The workspace to read and where it came from: the one `project` names, or the default
/// project's only one. `None` when there is no project. A project outside the default one is not
/// read with cargo (requirement R-MCP-3), so it is an error: the query tools read what
/// `build_external_docs` recorded of it instead ([`located`]).
#[cfg(feature = "local")]
pub(super) fn workspace(
    default: Option<&DefaultProject>,
    project: Option<&Path>,
) -> Result<Option<(Workspace, ProjectFrom)>, Box<CallToolResult>> {
    match place(default, project)? {
        Place::Here(workspace, project_from) => Ok(Some((workspace, project_from))),
        Place::Outside(workspace) => Err(Box::new(outside_error(&workspace))),
        Place::Nowhere => Ok(None),
    }
}

/// [`workspace`], for the query tools: a workspace outside the default project, which they read
/// as `build_external_docs` recorded it (decided in M2e's E4b-3); or the response while it is
/// recorded, when its record is broken, or `no_record`.
#[cfg(feature = "local")]
pub(super) async fn located(
    state: &ToolState,
    project: Option<&Path>,
) -> Result<Option<(Workspace, ProjectFrom)>, Box<CallToolResult>> {
    match place(state.project.as_ref(), project)? {
        Place::Here(workspace, project_from) => Ok(Some((workspace, project_from))),
        Place::Outside(workspace) => {
            recorded_workspace::read(state, &workspace).await?;
            Ok(Some((workspace, ProjectFrom::Argument)))
        },
        Place::Nowhere => Ok(None),
    }
}

/// Where a project is.
#[cfg(feature = "local")]
pub(super) enum Place {
    /// In the default project: the workspace, and where the project came from.
    Here(Workspace, ProjectFrom),
    /// Outside the default project, as the argument `project` names it.
    Outside(Workspace),
    /// There is no project.
    Nowhere,
}

/// Where the project is that `project` names, or else the default project, whose only workspace
/// it is.
#[cfg(feature = "local")]
pub(super) fn place(
    default: Option<&DefaultProject>,
    project: Option<&Path>,
) -> Result<Place, Box<CallToolResult>> {
    if let Some(project) = project {
        let path = project_path(default, project);
        let workspace = project::workspace_of(&path).map_err(|error| {
            Box::new(project_not_found(
                &format!("{}: {}", path.display(), ErrorChain(&error)),
                &[],
            ))
        })?;
        if default.is_some_and(|default| default.contains(&workspace)) {
            return Ok(Place::Here(workspace, ProjectFrom::Argument));
        }
        return Ok(Place::Outside(workspace));
    }
    let Some(default) = default else {
        return Ok(Place::Nowhere);
    };
    match &*default.workspaces {
        [] => Ok(Place::Nowhere),
        [workspace] => Ok(Place::Here(workspace.clone(), default.from)),
        workspaces => {
            let candidates: Vec<String> = workspaces
                .iter()
                .map(|workspace| workspace.root.display().to_string())
                .collect();
            Err(Box::new(project_not_found(
                &format!(
                    "{} has several workspaces directly inside it",
                    default.dir.display()
                ),
                &candidates,
            )))
        },
    }
}

/// `docs_unavailable` with `no_record` for `workspace`, outside the default project, which
/// `build_external_docs` documents, and which the external tool tells apart by it.
#[cfg(feature = "local")]
fn outside_error(workspace: &Workspace) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!(
            "{} is outside the default project, whose dependencies are not resolved with cargo; \
             give the version, or document it with build_external_docs",
            workspace.root.display()
        ),
        "retryable": false,
        "suggestions": ["give the version, as crate@version"],
        "reason": "no_record",
    }))
}

/// The path that the argument `project` names: relative to the default project's directory unless
/// absolute (§6.4).
#[cfg(feature = "local")]
pub(super) fn project_path(default: Option<&DefaultProject>, project: &Path) -> std::path::PathBuf {
    match default {
        Some(default) if project.is_relative() => default.dir.join(project),
        Some(_) | None => project.to_path_buf(),
    }
}

/// Whether the packages of the project that `project` names, or else of the default project, are
/// known now, as `cargo metadata` resolved them, without waiting: a release that a lookup did not
/// find among them is then not the project's (decided in M2e, 11). Without a project, none uses
/// it.
#[cfg(feature = "local")]
pub(super) async fn known_now(state: &ToolState, project: Option<&Path>) -> bool {
    match located(state, project).await {
        Ok(None) => true,
        // As kept, too: its inputs are read again before it is given, which a loaded machine may
        // not do at once.
        Ok(Some((workspace, _))) => {
            matches!(
                resolve_until(state, &workspace, Instant::now()).await,
                Joined::Done(Ok(resolution)) if matches!(&*resolution, Resolution::Metadata { .. })
            ) || state.resolver.resolved_before(&workspace).await
        },
        Err(_) => false,
    }
}

/// `project_not_found` (§8), with the workspaces to choose from.
#[cfg(feature = "local")]
fn project_not_found(why: &str, candidates: &[String]) -> CallToolResult {
    error_result(&json!({
        "code": "project_not_found",
        "message": format!("the project to read the version from is not known: {why}"),
        "retryable": false,
        "suggestions": [
            "pass `project` with one of the candidates, or give the version, as crate@version",
        ],
        "candidates": candidates,
    }))
}

/// The response while cargo resolves the packages of the project, past the deadline of the call, as
/// `doing` says (requirement R-OUT-9). The version is not known yet, so `provenance` has none.
#[cfg(feature = "local")]
pub(super) fn resolving(
    provenance: serde_json::Map<String, serde_json::Value>,
    doing: Doing,
    warnings: &[Box<str>],
) -> CallToolResult {
    let being = match doing {
        Doing::WaitingForLock => {
            "cargo waits for a lock that another cargo holds, to resolve the project's packages"
        },
        // Resolving builds nothing; the other stages are not reported for it.
        Doing::Fetching | Doing::Indexing | Doing::Building | Doing::WaitingForSlot => {
            "cargo is fetching the project's dependencies, to resolve the project's packages"
        },
    };
    super::get_item::building_response(provenance, doing, being, warnings)
}

/// [`resolving`] for a lookup of the crate `name` for `target` in the project from
/// `project_from`.
#[cfg(feature = "local")]
pub(super) fn resolving_crate(
    name: &CrateName,
    target: &TargetTriple,
    (doing, project_from): (Doing, &str),
    warnings: &[Box<str>],
) -> CallToolResult {
    let mut provenance = serde_json::Map::new();
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("project_from".to_owned(), json!(project_from));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    resolving(provenance, doing, warnings)
}

/// The response while cargo reads the build of the project from `project_from`, its features or
/// which packages it builds, past the deadline of the call, as `doing` says, to plan the build of
/// its member's or dependency's crate `name` for `target` (requirement R-OUT-9).
#[cfg(feature = "local")]
pub(super) fn planning_crate(
    name: &CrateName,
    target: &TargetTriple,
    (doing, project_from): (Doing, &str),
    warnings: &[Box<str>],
) -> CallToolResult {
    let mut provenance = serde_json::Map::new();
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("project_from".to_owned(), json!(project_from));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    provenance.insert("source".to_owned(), json!("local"));
    let being = match doing {
        Doing::WaitingForLock => {
            "cargo waits for a lock that another cargo holds, to read the project's build, which \
             the build of the crate is planned by"
        },
        Doing::Fetching | Doing::Indexing | Doing::Building | Doing::WaitingForSlot => {
            "cargo is reading the project's build, which the build of the crate is planned by"
        },
    };
    super::get_item::building_response(provenance, doing, being, warnings)
}

/// The tool error for a workspace whose packages cannot be resolved.
#[cfg(feature = "local")]
pub(super) fn resolve_error(workspace: &Workspace, error: &ResolveError) -> CallToolResult {
    let root = workspace.root.display();
    match error {
        ResolveError::NoCargoLock => error_result(&json!({
            "code": "lockfile_outdated",
            "message": format!("{root} has no Cargo.lock, which this server does not write"),
            "retryable": false,
            "suggestions": [
                "run `cargo fetch` in the workspace, which writes Cargo.lock without building",
                "or give the version, as crate@version",
            ],
        })),
        ResolveError::Unreadable(_) | ResolveError::Malformed(_) => error_result(&json!({
            "code": "lockfile_outdated",
            "message": format!("the Cargo.lock of {root} cannot be read: {}", ErrorChain(error)),
            "retryable": false,
            "suggestions": ["check Cargo.lock with `cargo fetch`", "or give the version, as crate@version"],
        })),
        ResolveError::Stopped => super::get_item::stopped(),
        ResolveError::NotRecorded => recorded_workspace::no_record(&workspace.root, None),
    }
}

/// The packages of `workspace` as `cargo metadata` resolves them, waiting for it until `until`;
/// for a workspace outside the default project, as `build_external_docs` recorded them, as the
/// query tools run no cargo there (requirement R-MCP-3, decided in M2e's E4b-3).
#[cfg(feature = "local")]
pub(super) async fn resolve_until(
    state: &ToolState,
    workspace: &Workspace,
    until: Instant,
) -> Joined<Resolved> {
    if recorded_workspace::is_outside(state, workspace) {
        // What the call returns otherwise is told when the workspace is read for it.
        return Joined::Done(
            recorded_workspace::read(state, workspace)
                .await
                .ok()
                .map(|recorded| Arc::clone(&recorded.resolution))
                .ok_or_else(|| Arc::new(ResolveError::NotRecorded)),
        );
    }
    state.resolver.resolve_until(workspace, until).await
}

/// The feature sets of the build of `workspace` for `target`, as `cargo tree` tells them, waiting
/// for it until `until`; for a workspace outside the default project, as `build_external_docs`
/// recorded them (decided in M2e's E4b-3).
#[cfg(feature = "local")]
pub(super) async fn features_until(
    state: &ToolState,
    workspace: &Workspace,
    target: &TargetTriple,
    until: Instant,
) -> Joined<Featured> {
    if recorded_workspace::is_outside(state, workspace) {
        return Joined::Done(recorded_workspace::feature_sets(state, workspace, target).await);
    }
    state
        .resolver
        .features_until(workspace, target, until)
        .await
}

/// The warnings when the versions come from Cargo.lock alone, because `cargo metadata` failed
/// (requirement R-VR-2).
#[cfg(feature = "local")]
fn raw_warnings(because: &CargoError) -> Vec<Box<str>> {
    let failure = match because {
        CargoError::Failed { failure, .. } => *failure,
        CargoError::Run(_) | CargoError::Unreadable(_) => Failure::Other,
    };
    let mut warnings: Vec<Box<str>> = vec![
        format!(
            "cargo metadata failed ({}), so the version is read from Cargo.lock directly",
            failure.code()
        )
        .into(),
    ];
    match failure {
        Failure::LockfileOutdated => warnings.push(
            "Cargo.lock does not match Cargo.toml, so the version may be out of date; run \
             `cargo fetch` to update Cargo.lock"
                .into(),
        ),
        // §8 `fetch_failed` with `auth`. GitHub asks for credentials for a missing repository,
        // too, and cargo uses git's credential helpers, which gh can be (`gh auth setup-git`).
        Failure::GitFetch(GitFetchFailure::Auth) => warnings.push(
            "a git dependency could not be fetched without credentials (or its URL is wrong); \
             run `cargo fetch` in the project's terminal to pass them. With the GitHub CLI, \
             `gh auth setup-git` lets git and cargo use its login"
                .into(),
        ),
        Failure::GitFetch(GitFetchFailure::NoRepository | GitFetchFailure::NoRev)
        | Failure::OfflineMiss
        | Failure::Killed
        | Failure::TooOld
        | Failure::Other => {},
    }
    warnings
}

/// `ambiguous_version` or `ambiguous_crate` (§8), with each candidate and what uses it.
#[cfg(feature = "local")]
fn ambiguous(code: &str, name: &CrateName, candidates: &[Candidate<'_>]) -> CallToolResult {
    let (message, suggestion) = if code == "ambiguous_crate" {
        (
            format!("{name} is a dependency's name in code and another package's name"),
            "ask with the package name of one of the candidates",
        )
    } else {
        (
            format!(
                "the project has several versions of {name}, and no single one is a direct dependency"
            ),
            "give the version of one of the candidates, as crate@version",
        )
    };
    error_result(&json!({
        "code": code,
        "message": message,
        "retryable": false,
        "suggestions": [suggestion],
        "candidates": candidates
            .iter()
            .map(|candidate| json!({
                "crate": format!("{}@{}", candidate.package.name, candidate.package.version),
                "used_by": candidate
                    .used_by
                    .iter()
                    .map(|user| format!("{}@{}", user.name, user.version))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    }))
}

#[cfg(feature = "local")]
#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[cfg(feature = "local")]
    #[test]
    fn the_targets_of_build_target_are_its_values_with_host_tuple_for_the_host() {
        use crate::data::cargo_config::BuildTarget;
        let host: TargetTriple = "x86_64-unknown-linux-gnu".parse().expect("a target");
        let with = |build_target: Option<BuildTarget>, build_std: bool| CargoConfig {
            build_target,
            build_std,
            ..CargoConfig::default()
        };
        let several = |targets: &[&str]| {
            Some(BuildTarget::Several(
                targets.iter().map(|&target| target.to_owned()).collect(),
            ))
        };
        let names = |targets: Vec<TargetTriple>| -> Vec<String> {
            targets
                .iter()
                .map(|target| target.as_str().to_owned())
                .collect()
        };
        assert_eq!(targets_of(&with(None, false), &host), Ok(None));
        assert_eq!(targets_of(&with(several(&[]), false), &host), Ok(None));
        let found = targets_of(
            &with(several(&["host-tuple", "wasm32-unknown-unknown"]), false),
            &host,
        )
        .map(|targets| targets.map(names));
        assert_eq!(
            found,
            Ok(Some(vec![
                "x86_64-unknown-linux-gnu".to_owned(),
                "wasm32-unknown-unknown".to_owned(),
            ]))
        );
        let one = Some(BuildTarget::One("wasm32-unknown-unknown".to_owned()));
        assert_eq!(
            targets_of(&with(one.clone(), false), &host).map(|targets| targets.map(names)),
            Ok(Some(vec!["wasm32-unknown-unknown".to_owned()]))
        );
        // A target of its own, with its own std (requirement R-TG-1).
        for (config, own) in [
            (with(one, true), "wasm32-unknown-unknown"),
            (with(None, true), "build-std"),
            (
                with(several(&["wasm32-unknown-unknown", "my.JSON"]), false),
                "my.JSON",
            ),
            // Before a value that is not a target, as cargo would not read the settings.
            (with(several(&["no target", " x.json "]), false), " x.json "),
        ] {
            assert_eq!(
                targets_of(&config, &host),
                Err(TargetsError::OwnTarget(own.to_owned()))
            );
        }
        let not_a_target = targets_of(
            &with(several(&["wasm32-unknown-unknown", "no target"]), false),
            &host,
        );
        assert!(
            matches!(&not_a_target, Err(TargetsError::NotATarget(why)) if why.contains("no target")),
            "{not_a_target:?}"
        );
    }

    /// A directory with a package `name` in it.
    fn package(dir: &Path, name: &str) {
        fs::create_dir_all(dir).expect("made");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .expect("written");
    }

    fn code(error: &CallToolResult) -> serde_json::Value {
        let text = error
            .content
            .first()
            .and_then(|content| content.as_text())
            .map(|text| text.text.clone())
            .expect("a text block");
        serde_json::from_str(&text).expect("JSON")
    }

    #[test]
    fn the_project_is_the_default_one_or_the_one_named_inside_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        package(&dir.path().join("a"), "a");
        package(&dir.path().join("b"), "b");
        let default =
            DefaultProject::find(None, Some(dir.path().to_path_buf())).expect("a directory");

        // Two workspaces directly inside: one must be named.
        let error = workspace(Some(&default), None).expect_err("ambiguous");
        assert_eq!(code(&error)["code"], "project_not_found");
        assert_eq!(code(&error)["candidates"].as_array().map(Vec::len), Some(2));

        let (named, from) = workspace(Some(&default), Some(Path::new("b")))
            .expect("inside")
            .expect("found");
        assert!(named.root.ends_with("b"));
        assert_eq!(from, ProjectFrom::Argument);

        // A workspace outside the default project is not read with cargo.
        let outside = tempfile::tempdir().expect("a temporary directory");
        package(outside.path(), "c");
        let error = workspace(Some(&default), Some(outside.path())).expect_err("outside");
        assert_eq!(code(&error)["reason"], "no_record");

        assert!(workspace(None, None).expect("no error").is_none());
    }

    #[test]
    fn a_git_dependency_without_credentials_is_explained() {
        let failed = |failure| CargoError::Failed {
            command: "metadata",
            failure,
            stderr: "".into(),
        };
        let auth = raw_warnings(&failed(Failure::GitFetch(GitFetchFailure::Auth)));
        assert_eq!(auth.len(), 2, "{auth:?}");
        assert!(auth[0].contains("fetch_failed"), "{auth:?}");
        assert!(auth[1].contains("gh auth setup-git"), "{auth:?}");
        assert_eq!(
            raw_warnings(&failed(Failure::GitFetch(GitFetchFailure::NoRev))).len(),
            1
        );
    }
}
