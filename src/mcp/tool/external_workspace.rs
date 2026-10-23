//! The builds of `build_external_docs` of a workspace outside the default project (requirement
//! R-WS-10, decided in M2e's E4b).
//!
//! The query tools run no cargo there (requirement R-MCP-3), so this tool records what they need
//! of it first, under the lock of its record (decided in M2e, 13): what `cargo metadata` resolved,
//! its Cargo.lock, the files that it is made from and the targets that it builds for; then, for the
//! target, what `cargo tree` tells of its features and what becomes of each package that its build
//! builds. That is work that the calls for the workspace and the target share, which goes on past
//! the deadline of a call, and starts the builds of its members and of its dependencies that
//! docs.rs does not have (path, git, another registry), as the default project's crates are built
//! for another target (E4a), apart, with the features of their `[package.metadata.docs.rs]`. Its
//! dependencies of crates.io are left to docs.rs, which the query tools ask when they look one up,
//! and built in the workspace when asked for by `crate` (decided in M2e's E4b, 4). A call again
//! records it again, and builds again only what changed.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    ErrorChain,
    ToolState,
    error_result,
    external_build::{self, cargo_failure, failed_so},
    external_target::{self, Entry},
    get_item::{self, building_response},
    local::{self, Asked, Built, Member, targets_built},
};
use crate::{
    data::{
        build,
        build_scope::{BuildScope, Buildable},
        cargo::Failure,
        crate_name::CrateName,
        external::{Making, Read},
        external_workspace::{self, Outcome, RecordedPackage, TargetRecord, WorkspaceRecord},
        features,
        flight::{Doing, Joined, Stage},
        freshness,
        metadata::{CargoError, Metadata, Package},
        packages::{PackageGraph, ProjectPackage, Source},
        project::{ProjectFrom, Workspace},
        target::TargetTriple,
        version::VersionSpec,
    },
    query::local::{LocalWanted, RECORDED_BY},
};

/// What the work for a workspace outside the default project and a target recorded and planned,
/// for the calls that wanted it at once; or the error that it ended with.
pub(in crate::mcp) type SharedPlanned = Result<Arc<Recorded>, Arc<CallToolResult>>;

/// What the work for a workspace outside the default project recorded and planned for a target.
#[derive(Debug)]
pub(in crate::mcp) struct Recorded {
    /// The target.
    target: TargetTriple,
    /// What becomes of its packages.
    planned: Planned,
}

/// What becomes of the packages of a workspace that its build for a target builds.
#[derive(Debug, Clone, Default)]
struct Planned {
    /// The members to build.
    members: Vec<LocalWanted>,
    /// The dependencies to build: path, git and another registry's.
    dependencies: Vec<LocalWanted>,
    /// The dependencies of crates.io, left to docs.rs unless asked for.
    crates_io: Vec<ProjectPackage>,
    /// What is not built here, as `cargo rustdoc -p` cannot name it.
    later: Vec<Entry>,
}

/// How long the work waits for cargo to plan a build, which no call waits for: about as long as a
/// build may take (requirement R-WS-7).
const CARGO_WAITS_FOR: Duration = Duration::from_mins(30);

/// Documents the workspace `workspace`, outside the default project, named by `given`, for
/// `target`, or the first that it builds for: all its members and its dependencies that docs.rs
/// does not have, or the crate `only` of it, at the version that it asks for (requirement
/// R-WS-10). While it is recorded past the deadline of the call, the work goes on.
pub(super) async fn outside(
    state: Arc<ToolState>,
    (workspace, given): (Workspace, PathBuf),
    target: Option<TargetTriple>,
    only: Option<(CrateName, Option<VersionSpec>)>,
) -> CallToolResult {
    let until = state.until();
    let key = external_workspace::workspace_key(&workspace.root);
    let flight = format!(
        "{}:{}",
        key.lock_key(),
        target.as_ref().map_or("", TargetTriple::as_str)
    );
    let what = format!("the workspace at {}", workspace.root.display());
    let start = || {
        let (state, workspace, given, target) = (
            Arc::clone(&state),
            workspace.clone(),
            given.clone(),
            target.clone(),
        );
        async move |stage: Arc<Stage>| {
            record_and_plan(&state, (&workspace, &given), target, &stage).await
        }
    };
    match state.workspaces.join(&flight, &what, until, start()).await {
        Joined::Done(Ok(recorded)) => built(&state, &workspace, &recorded, only, until).await,
        Joined::Done(Err(result)) => (*result).clone(),
        Joined::Ended => get_item::stopped(),
        Joined::Going(doing) => recording(&workspace, target.as_ref(), doing),
    }
}

/// The work that records the workspace `workspace`, named by `given`, and what becomes of its
/// packages for `target`, or the first that it builds for, telling `progress` what it does; and that
/// starts the builds that it plans, which go on without it.
async fn record_and_plan(
    state: &Arc<ToolState>,
    (workspace, given): (&Workspace, &Path),
    target: Option<TargetTriple>,
    progress: &Stage,
) -> SharedPlanned {
    let what = format!("the workspace at {}", workspace.root.display());
    let failed = |why: &str, retryable| Arc::new(error_result(&failed_so(&what, why, retryable)));
    // The record of the workspace, under its lock throughout, as the query tools wait for it.
    let key = external_workspace::workspace_key(&workspace.root);
    let making = external_build::take_lock(state, &key, progress)
        .await
        .map_err(|why| failed(&why, true))?;
    progress.set(Doing::Fetching);
    let (metadata, record, making) = record_workspace(state, (workspace, given), making)
        .await
        .map_err(|result| Arc::new(*result))?;
    let first = record
        .targets
        .first()
        .and_then(|target| target.parse::<TargetTriple>().ok());
    let Some(target) = target.or(first) else {
        return Err(failed("it builds for no target", false));
    };
    let from = (&*metadata, &*record.recorded_at);
    let planned = planned_for(state, (workspace, from), &target, progress)
        .await
        .map_err(|result| Arc::new(*result))?;
    drop(making);
    // Started now, they go on without this work.
    let all: Vec<LocalWanted> = planned
        .members
        .iter()
        .chain(&planned.dependencies)
        .cloned()
        .collect();
    progress.set(Doing::Building);
    external_target::all_built(state, (workspace, false), all, Instant::now()).await;
    Ok(Arc::new(Recorded { target, planned }))
}

/// Records what `cargo metadata` resolves of `workspace`, named by `given`, under the lock of its
/// record, `making`, which it gives back: with its Cargo.lock, the files that it is made from and
/// the targets that it builds for; and the aliases of it (decided in M2e, 5). Returns the metadata
/// with the record, or the error.
async fn record_workspace(
    state: &ToolState,
    (workspace, given): (&Workspace, &Path),
    making: Making,
) -> Result<(Arc<Metadata>, WorkspaceRecord, Making), Box<CallToolResult>> {
    let root = &workspace.root;
    let what = format!("the workspace at {}", root.display());
    let failed = |why: &str| Box::new(error_result(&failed_so(&what, why, true)));
    let (metadata, text) = state
        .resolver
        .metadata_text(root)
        .await
        .map_err(|error| Box::new(error_result(&cargo_error(&what, &error))))?;
    // A workspace that builds its own std is not documented here (requirement R-TG-1).
    let targets = targets_built(state, root, &what).await?;
    let files = state.resolver.input_files(root, &metadata).await;
    let members: Vec<PathBuf> = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .filter_map(|package| package.manifest_path.parent().map(Path::to_path_buf))
        .collect();
    let (cache, root, given) = (state.cache.clone(), root.clone(), given.to_path_buf());
    let written = crate::task::spawn_blocking(move || {
        let written = (|| {
            let before = match external_workspace::read_workspace(&cache, &root, Some(&making)) {
                Read::Found(before) => Some(before),
                Read::Building | Read::Failed(_) | Read::Broken(_) | Read::Missing => None,
            };
            let inputs =
                freshness::collect(&files, &[], before.as_ref().map(|before| &before.inputs))?;
            let lock = std::fs::read_to_string(root.join("Cargo.lock"))?;
            let mut record = WorkspaceRecord {
                metadata: text,
                lock: lock.into(),
                inputs,
                members: external_workspace::members_of(&root),
                targets: targets
                    .iter()
                    .map(|target| target.as_str().into())
                    .collect(),
                recorded_at: crate::data::rfc3339(SystemTime::now()).into(),
            };
            // The same as before: the records for the targets planned from it stay of it.
            if let Some(before) = before
                && before.same_as(&record)
            {
                record.recorded_at = before.recorded_at;
            }
            external_workspace::record_workspace(&cache, &root, &record, &making)?;
            // The members under the path given, too, as it was written: the root and the members'
            // paths that cargo tells have symbolic links resolved, as `/private/var` on macOS.
            let given_dir = if given.file_name() == Some(std::ffi::OsStr::new("Cargo.toml")) {
                given.parent().unwrap_or(&given).to_path_buf()
            } else {
                given.clone()
            };
            let under_given: Vec<PathBuf> =
                if dunce::canonicalize(&given_dir).ok() == Some(root.clone()) {
                    members
                        .iter()
                        .filter_map(|member| member.strip_prefix(&root).ok())
                        .map(|relative| given_dir.join(relative))
                        .collect()
                } else {
                    Vec::new()
                };
            let mut aliases: Vec<&Path> = members
                .iter()
                .chain(&under_given)
                .map(PathBuf::as_path)
                .collect();
            aliases.push(&given);
            external_workspace::record_aliases(&cache, &aliases, &root)?;
            Ok::<_, std::io::Error>(record)
        })();
        (written, making)
    })
    .await
    .map_err(|error| failed(&error.to_string()))?;
    match written {
        (Ok(record), making) => Ok((Arc::new(metadata), record, making)),
        (Err(error), _) => Err(failed(&error.to_string())),
    }
}

/// What becomes of the packages of `workspace`, which `metadata` resolves as recorded at
/// `recorded_at`, that its build for `target` builds, recorded with what `cargo tree` tells of its
/// features for the target, under the lock of that record, telling `progress` that it waits for it;
/// or the error.
async fn planned_for(
    state: &Arc<ToolState>,
    (workspace, (metadata, recorded_at)): (&Workspace, (&Metadata, &str)),
    target: &TargetTriple,
    progress: &Stage,
) -> Result<Planned, Box<CallToolResult>> {
    let root = &workspace.root;
    let what = format!("the workspace at {} for {target}", root.display());
    let failed = |why: &str| Box::new(error_result(&failed_so(&what, why, true)));
    let key = external_workspace::target_key(root, target.as_str());
    let making = external_build::take_lock(state, &key, progress)
        .await
        .map_err(|why| failed(&why))?;
    progress.set(Doing::Fetching);
    let offline = state.resolver.offline_in(root).await;
    let trees = features::trees(root, offline, target)
        .await
        .map_err(|error| Box::new(error_result(&cargo_error(&what, &error))))?;
    let scope = state
        .resolver
        .scope(workspace, target)
        .await
        .map_err(|error| failed(&ErrorChain(&*error).to_string()))?;
    let (planned, table) = plan_packages(state, (workspace, metadata, &scope), target).await?;
    let record = TargetRecord {
        planned_from: recorded_at.into(),
        trees,
        packages: table,
    };
    let (cache, root) = (state.cache.clone(), root.clone());
    let target_name = target.as_str().to_owned();
    let written = crate::task::spawn_blocking(move || {
        external_workspace::record_target(&cache, (&root, &target_name), &record, &making)
    })
    .await;
    if let Ok(Err(error)) | Err(error) = written.map_err(std::io::Error::other) {
        return Err(failed(&error.to_string()));
    }
    Ok(planned)
}

/// What becomes of the packages of `workspace`, which `metadata` resolves, that its build for
/// `target` builds, as `scope` tells: planned to build, or not built here; with the rows of the
/// record of what becomes of them. Or the error.
async fn plan_packages(
    state: &Arc<ToolState>,
    (workspace, metadata, scope): (&Workspace, &Metadata, &BuildScope),
    target: &TargetTriple,
) -> Result<(Planned, Vec<RecordedPackage>), Box<CallToolResult>> {
    let mut planned = Planned::default();
    let mut table = Vec::new();
    let host = state.host().await.triple.clone();
    let until = Instant::now()
        .checked_add(CARGO_WAITS_FOR)
        .unwrap_or_else(Instant::now);
    for package in &metadata.packages {
        let member = metadata.workspace_members.contains(&package.id);
        let lib = build::lib_of(package).map_or_else(
            || package.name.replace('-', "_"),
            |(name, _)| name.to_string(),
        );
        if !member {
            match scope.buildable(&package.id) {
                Buildable::NotBuilt => continue,
                // docs.rs has it, whether `cargo rustdoc -p` names it or not.
                buildable if Source::parse(package.source.as_deref()) == Source::CratesIo => {
                    // Where this tool records it, when docs.rs has none (decided in M2e's E4b, 4).
                    let target = match buildable {
                        Buildable::ForTarget => Some(target.as_str().into()),
                        Buildable::ForHost => Some(host.as_str().into()),
                        Buildable::Unnameable | Buildable::NotBuilt => None,
                    };
                    table.push(recorded(&package.id.0, &lib, Outcome::DocsRs { target }));
                    let found = project_package(state, workspace, package, until).await?;
                    planned.crates_io.push(found);
                    continue;
                },
                Buildable::Unnameable => {
                    table.push(recorded(&package.id.0, &lib, Outcome::Unnameable));
                    planned.later.push(not_built(package));
                    continue;
                },
                Buildable::ForTarget | Buildable::ForHost => {},
            }
        }
        let Ok(name) = package.name.parse::<CrateName>() else {
            continue;
        };
        let planning = if member {
            let asked = Asked::Package {
                name: &package.name,
                version: &package.version,
            };
            // A proc macro is built for the host whatever the target, as a dependency that is one
            // is.
            let target = if is_proc_macro(package) {
                &host
            } else {
                target
            };
            let elsewhere = (target, Built::Elsewhere);
            local::member_for(state, workspace, (&name, asked), elsewhere, until).await
        } else {
            let found = project_package(state, workspace, package, until).await?;
            let elsewhere = (target, Built::Elsewhere);
            local::dependency_for(state, workspace, &found, elsewhere, until).await
        };
        match planning? {
            Member::Is { wanted, .. } => {
                let outcome = Outcome::Built {
                    target: wanted.target.as_str().into(),
                };
                table.push(recorded(&package.id.0, &wanted.plan.crate_name, outcome));
                if member {
                    planned.members.push(*wanted);
                } else {
                    planned.dependencies.push(*wanted);
                }
            },
            Member::Unnameable { .. } => {
                table.push(recorded(&package.id.0, &lib, Outcome::Unnameable));
                planned.later.push(not_built(package));
            },
            Member::Resolving(_) | Member::Planning(_) => {
                let what = format!("{} {}", package.name, package.version);
                let why = "cargo did not plan its build in time";
                return Err(Box::new(error_result(&failed_so(&what, why, true))));
            },
            Member::NotForTarget(error)
            | Member::Unknown(error)
            | Member::OtherTarget { error, .. } => {
                return Err(Box::new(external_build::external_body_of(&error)));
            },
            // Of the query tools only.
            Member::Recorded(recorded) => {
                return Err(Box::new(external_build::external_body_of(&recorded.error)));
            },
            // No lib nor bin to document.
            Member::Not | Member::OtherVersion(_) => {},
        }
    }
    Ok((planned, table))
}

/// Whether `package` is a proc macro.
fn is_proc_macro(package: &Package) -> bool {
    package
        .targets
        .iter()
        .any(|target| target.kind.iter().any(|kind| &**kind == "proc-macro"))
}

/// The project's package of `workspace` that `package` of its metadata is, as the planning of a
/// dependency takes it; or the error, as when cargo does not tell it.
async fn project_package(
    state: &ToolState,
    workspace: &Workspace,
    package: &Package,
    until: Instant,
) -> Result<ProjectPackage, Box<CallToolResult>> {
    let what = format!("{} {}", package.name, package.version);
    let failed = |why: &str| Box::new(error_result(&failed_so(&what, why, true)));
    let resolution = match state.resolver.resolve_until(workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return Err(Box::new(super::project::resolve_error(workspace, &error)));
        },
        Joined::Ended => return Err(Box::new(get_item::stopped())),
        Joined::Going(_) => return Err(failed("cargo did not resolve the workspace in time")),
    };
    let source = Source::parse(package.source.as_deref());
    PackageGraph::new(&resolution)
        .packages()
        .iter()
        .find(|found| {
            found.name == package.name && found.version == package.version && found.source == source
        })
        .cloned()
        .ok_or_else(|| failed("cargo did not tell it among the workspace's packages"))
}

/// A row of the record of what became of the package `package`, whose crate is `crate_name`.
fn recorded(package: &str, crate_name: &str, outcome: Outcome) -> RecordedPackage {
    RecordedPackage {
        package: package.into(),
        crate_name: crate_name.into(),
        outcome,
    }
}

/// The entry of the response for `package`, which is not built here, as `cargo rustdoc -p` cannot
/// name it.
fn not_built(package: &Package) -> Entry {
    Entry {
        name: package.name.to_string(),
        version: package.version.to_string(),
        status: "later",
        error: None,
    }
}

/// The builds of what `recorded` planned of `workspace`, or of the crate `only` of it at the
/// version that it asks for, which the work started, waited for until `until`; with the response.
/// A crate of crates.io asked for is read from docs.rs, or else built in the workspace (decided in
/// M2e's E4b, 4).
async fn built(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    recorded: &Recorded,
    only: Option<(CrateName, Option<VersionSpec>)>,
    until: Instant,
) -> CallToolResult {
    let Recorded { target, planned } = recorded;
    let mut planned = planned.clone();
    if let Some((only, spec)) = &only {
        let is = |names: [&str; 2], version: &semver::Version| {
            names
                .iter()
                .any(|name| name.replace('_', "-").to_ascii_lowercase() == only.cache_key())
                && match spec {
                    Some(VersionSpec::Exact(exact)) => version == exact,
                    Some(VersionSpec::Requirement(requirement)) => requirement.matches(version),
                    Some(VersionSpec::Latest) | None => true,
                }
        };
        let wanted = |wanted: &LocalWanted| {
            let (name, _) = external_target::package_of(wanted);
            is([&name, &wanted.plan.crate_name], &wanted.plan.version)
        };
        planned.members.retain(wanted);
        planned.dependencies.retain(wanted);
        planned
            .crates_io
            .retain(|package| is([&package.name, &package.lib_name], &package.version));
        planned.later.retain(|entry| {
            entry
                .version
                .parse()
                .is_ok_and(|version| is([&entry.name, &entry.name], &version))
        });
        if let ([], [], [package]) = (
            planned.members.as_slice(),
            planned.dependencies.as_slice(),
            planned.crates_io.as_slice(),
        ) {
            return crate_of_crates_io(state, workspace, package, target).await;
        }
        if planned.members.is_empty()
            && planned.dependencies.is_empty()
            && planned.crates_io.is_empty()
            && planned.later.is_empty()
        {
            return no_such_crate(workspace, only, target);
        }
    }
    let members =
        external_target::all_built(state, (workspace, false), planned.members, until).await;
    let mut dependencies =
        external_target::all_built(state, (workspace, false), planned.dependencies, until).await;
    dependencies.extend(planned.crates_io.iter().map(|package| Entry {
        name: package.name.to_string(),
        version: package.version.to_string(),
        status: "docs_rs",
        error: None,
    }));
    dependencies.extend(planned.later);
    outside_response((workspace, target), (&members, &dependencies))
}

/// The dependency `package` of crates.io of `workspace`, asked for: what docs.rs has of it for
/// `target`, or else its build in the workspace (decided in M2e's E4b, 4).
async fn crate_of_crates_io(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    package: &ProjectPackage,
    target: &TargetTriple,
) -> CallToolResult {
    let found = (workspace, ProjectFrom::Argument);
    match external_target::dependency(state, found, package, target).await {
        Err(result) => *result,
        // Its release alone is not built for a workspace outside the default project (decided in
        // M2e's E4b, 2).
        Ok(_) => super::build_external_docs::not_yet(
            &format!(
                "{} {}, which cargo rustdoc -p cannot name in the workspace",
                package.name, package.version
            ),
            "look it up on docs.rs with the query tools, without the project",
        ),
    }
}

/// `crate_not_found` for `name`, which the build of `workspace` for `target` does not build.
fn no_such_crate(workspace: &Workspace, name: &CrateName, target: &TargetTriple) -> CallToolResult {
    error_result(&json!({
        "code": "crate_not_found",
        "message": format!(
            "the build of the workspace at {} for {target} builds no crate {name} at the version \
             asked for",
            workspace.root.display()
        ),
        "retryable": false,
        "suggestions": ["call build_external_docs with the project and without `crate`, to see its crates"],
    }))
}

/// The response while the work records `workspace` and plans its build for `target`, past the
/// deadline of the call, as `doing` says.
fn recording(workspace: &Workspace, target: Option<&TargetTriple>, doing: Doing) -> CallToolResult {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("project"));
    provenance.insert(
        "workspace".to_owned(),
        json!(workspace.root.display().to_string()),
    );
    provenance.insert("project_from".to_owned(), json!("argument"));
    if let Some(target) = target {
        provenance.insert("target".to_owned(), json!(target.as_str()));
    }
    building_response(
        provenance,
        doing,
        "the workspace is being recorded, and its build planned and started",
        &[],
    )
}

/// The response for `workspace` documented for `target`: what became of its members and of its
/// dependencies.
fn outside_response(
    (workspace, target): (&Workspace, &TargetTriple),
    (members, dependencies): (&[Entry], &[Entry]),
) -> CallToolResult {
    let root = workspace.root.display().to_string();
    let built = |entry: &Entry| matches!(entry.status, "built" | "docs_rs" | "later");
    let all_built =
        members.iter().all(|entry| entry.status == "built") && dependencies.iter().all(built);
    let building = members
        .iter()
        .chain(dependencies)
        .any(|entry| entry.status == "building");
    let lookup = json!({ "project": root, "target": target.as_str() });
    let mut hints = vec![format!(
        "look its crates up with get_item, crate_overview or list_crates, passing their crate and \
         {lookup}"
    )];
    if building {
        hints.push(
            "the crates being built go on without this call: the other tools answer once they are \
             built; do not call build_external_docs again for them"
                .to_owned(),
        );
    }
    if dependencies.iter().any(|entry| entry.status == "docs_rs") {
        hints.push(format!(
            "a dependency of crates.io (docs_rs) is read from docs.rs when it is looked up; when \
             docs.rs has none, build it in the workspace with build_external_docs, passing its \
             crate and {lookup}"
        ));
    }
    CallToolResult::structured(json!({
        "provenance": {
            "source": "local",
            "workspace": root,
            "project_from": "argument",
            "target": target.as_str(),
            "recorded_by": RECORDED_BY,
        },
        "data": {
            "built": all_built,
            "target": target.as_str(),
            "members": members.iter().map(Entry::to_json).collect::<Vec<Value>>(),
            "dependencies": dependencies.iter().map(Entry::to_json).collect::<Vec<Value>>(),
        },
        "hints": hints,
    }))
}

/// The body of the error of cargo, `error`, for `what`: with how to make a Cargo.lock that is
/// missing or out of date, which this server does not write (requirement R-VR-2).
fn cargo_error(what: &str, error: &CargoError) -> Value {
    let failure = match error {
        CargoError::Failed { failure, .. } => Some(*failure),
        CargoError::Run(_) | CargoError::Unreadable(_) => None,
    };
    let transient = !matches!(
        failure,
        Some(Failure::LockfileOutdated | Failure::OfflineMiss)
    );
    let mut body = cargo_failure(what, failure, transient, &ErrorChain(error).to_string());
    if failure == Some(Failure::LockfileOutdated)
        && let Some(Value::Array(suggestions)) = body.get_mut("suggestions")
    {
        suggestions.insert(
            0,
            json!(
                "make or update the workspace's Cargo.lock in its own terminal, as with `cargo \
                 generate-lockfile`: this server builds with --locked, and writes no Cargo.lock"
            ),
        );
    }
    body
}
