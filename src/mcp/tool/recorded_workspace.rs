//! What `build_external_docs` recorded of a workspace outside the default project, as the query
//! tools read it: they run no cargo there (requirement R-MCP-3), so its packages, its targets, the
//! features of its build and what became of each package that it builds come from the records
//! (decided in M2e's E4b-3). A record made before the workspace changed is read, and told stale
//! (decided in M2e's E4b-3, 1).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use super::{
    ToolState,
    error_result,
    get_item::{building_response, shutting_down},
    local::{Asked, Member, RecordedCrate, not_built_for},
    recorded::broken,
};
use crate::{
    data::{
        build,
        crate_name::CrateName,
        external::{self, Key, Read},
        external_workspace::{self, Outcome, TargetRecord},
        flight::Doing,
        freshness::{self, Inputs},
        metadata::{Featured, FeaturesError, Metadata, Resolution},
        packages::{ProjectPackage, Source},
        project::Workspace,
        target::TargetTriple,
    },
    query::local::RECORDED_BY,
};

/// What the record of a workspace outside the default project tells, read once for the calls
/// until it is recorded again.
#[derive(Debug)]
pub(in crate::mcp) struct Recorded {
    /// Its packages, as `cargo metadata` resolved them.
    pub(super) resolution: Arc<Resolution>,
    /// The targets that it builds for, the first for a lookup that gives none.
    pub(super) targets: Vec<Box<str>>,
    /// When it was recorded, in RFC 3339.
    pub(super) at: Box<str>,
    /// The files that `cargo metadata` resolved it from, as they were.
    inputs: Inputs,
    /// The directories that its `members` patterns matched.
    members: Option<Vec<PathBuf>>,
    /// The file of the record as it was read, which tells that it was recorded again.
    stamp: Stamp,
    /// The records for its targets read, by target, with the stamps of their files, for the calls
    /// until they are recorded again: a search reads them for each crate.
    targets_read: TargetsRead,
    /// Whether what it is resolved from changed, as told last, with the lengths and times of its
    /// files then: a search asks for each crate at once, and their contents are hashed again only
    /// when these change.
    checked: parking_lot::Mutex<Option<(Vec<Option<FileStamp>>, Since)>>,
}

/// The length and the time of a file.
type FileStamp = (u64, Option<SystemTime>);

/// How recently a file is taken for written just now: more than the coarsest time that file
/// systems keep (two seconds on FAT), as the freshness of builds takes it.
const JUST_NOW: std::time::Duration = std::time::Duration::from_secs(3);

/// The records of a workspace for its targets read, by target, with the stamps of their files.
type TargetsRead = parking_lot::Mutex<HashMap<Box<str>, (Stamp, Arc<TargetRecord>)>>;

/// What became of a workspace outside the default project since `build_external_docs` recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Since {
    /// Nothing that it is resolved from changed.
    Same,
    /// Its manifests, Cargo.lock, cargo's settings or the directories that its `members` patterns
    /// match changed: `inputs_changed` (decided in M2e's E4b-3, 1).
    Changed,
    /// Its directory is gone, as its root has no Cargo.toml: `project_missing` (decided in M2e's
    /// E4b-3, 2).
    Missing,
}

impl Since {
    /// What the provenance tells of it, `stale_reason`; `None` when it is the same.
    pub(super) const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Same => None,
            Self::Changed => Some("inputs_changed"),
            Self::Missing => Some("project_missing"),
        }
    }

    /// This, or else `Changed` when `before`, as when the target's plan is of another record of
    /// the workspace.
    pub(super) const fn or_changed(self, before: bool) -> Self {
        match self {
            Self::Same if before => Self::Changed,
            Self::Same | Self::Changed | Self::Missing => self,
        }
    }
}

impl Recorded {
    /// The packages of the workspace, as `cargo metadata` resolved them.
    fn metadata(&self) -> Option<&Metadata> {
        match &*self.resolution {
            Resolution::Metadata { metadata, .. } => Some(metadata),
            Resolution::CargoLockRaw { .. } => None,
        }
    }

    /// What became of the workspace at `root` since it was recorded, as [`Since`] tells. `None`
    /// when the task did not end.
    pub(super) async fn since(self: &Arc<Self>, root: &Path) -> Option<Since> {
        let (recorded, root) = (Arc::clone(self), root.to_path_buf());
        crate::task::spawn_blocking(move || {
            if !root.join("Cargo.toml").is_file() {
                return Since::Missing;
            }
            if external_workspace::members_of(&root) != recorded.members {
                return Since::Changed;
            }
            let stamps: Vec<Option<FileStamp>> = recorded
                .inputs
                .files
                .iter()
                .map(|file| {
                    std::fs::metadata(&file.path)
                        .ok()
                        .map(|metadata| (metadata.len(), metadata.modified().ok()))
                })
                .collect();
            if let Some((checked, since)) = &*recorded.checked.lock()
                && *checked == stamps
            {
                return *since;
            }
            let since = if freshness::unchanged_as_recorded(&recorded.inputs) {
                Since::Same
            } else {
                Since::Changed
            };
            // Not of a file written just now, whose next write a coarse time may not tell.
            let settled = SystemTime::now()
                .checked_sub(JUST_NOW)
                .is_some_and(|before| {
                    stamps
                        .iter()
                        .flatten()
                        .all(|(_, modified)| modified.is_some_and(|modified| modified < before))
                });
            if settled {
                *recorded.checked.lock() = Some((stamps, since));
            }
            since
        })
        .await
        .ok()
    }
}

/// What tells that a file was written again: its length, its time and, on unix, its inode, as
/// the record is written anew and renamed into place, which a coarse time may not tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    /// Its length.
    len: u64,
    /// When it changed.
    modified: Option<SystemTime>,
    /// Its inode, or 0 where there is none.
    inode: u64,
}

impl Stamp {
    /// What no file is stamped with.
    const NONE: Self = Self {
        len: 0,
        modified: None,
        inode: 0,
    };

    /// The stamp of the file at `path`, if it is there.
    ///
    /// It reads the file system, so it is for a blocking thread.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(&metadata);
        #[cfg(not(unix))]
        let inode = 0;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            inode,
        })
    }
}

/// The records read, by the root of their workspace.
pub(in crate::mcp) type Kept = parking_lot::Mutex<HashMap<PathBuf, Arc<Recorded>>>;

/// Why the record of a workspace was not read.
enum Unread {
    /// `build_external_docs` is recording it.
    Building,
    /// There is none.
    Missing,
    /// It is broken, as this tells.
    Broken(String),
}

/// Whether `workspace` is outside the default project: the query tools read only what
/// `build_external_docs` recorded of it (requirement R-MCP-3).
///
/// It is kept for the process, as the default project is: telling it may look for the git
/// working tree of the workspace.
pub(super) fn is_outside(state: &ToolState, workspace: &Workspace) -> bool {
    if let Some(outside) = state.outsides.lock().get(&workspace.root) {
        return *outside;
    }
    let outside = !state
        .project
        .as_ref()
        .is_some_and(|default| default.contains(workspace));
    state
        .outsides
        .lock()
        .insert(workspace.root.clone(), outside);
    outside
}

/// Whether workspaces are outside the default project, as told, by root.
pub(in crate::mcp) type Outsides = parking_lot::Mutex<HashMap<PathBuf, bool>>;

/// What the responses tell to do once the directory of a workspace outside the default project is
/// gone, whose root `build_external_docs` cannot record again (decided in M2e's E4b-3, 2).
pub(super) const MOVED: &str = "if the workspace moved, call build_external_docs with its new \
                                path as project, and look it up there";

/// The workspace outside the default project that `path` is an alias of, as `build_external_docs`
/// recorded it, when it has a record: for a workspace whose directory is gone (decided in M2e's
/// E4b-3, 2). A path to a `Cargo.toml` is also taken for its directory; the workspace is not found
/// by a path that it moved to (decided in M2e's E4b-3b, 2).
pub(super) async fn aliased(state: &ToolState, path: &Path) -> Option<Workspace> {
    let (cache, path) = (state.cache.clone(), path.to_path_buf());
    crate::task::spawn_blocking(move || {
        let directory = (path.file_name() == Some(std::ffi::OsStr::new("Cargo.toml")))
            .then(|| path.parent())
            .flatten();
        // Only when its manifest is gone: one that cannot be read tells its own error.
        let manifest = directory.map_or_else(|| path.join("Cargo.toml"), |_| path.clone());
        if manifest.exists() {
            return None;
        }
        let root = std::iter::once(path.as_path())
            .chain(directory)
            .find_map(|path| external_workspace::aliased_root(&cache, path))?;
        // Not without its record (decided in M2e's E4b-3b, 6).
        external_workspace::workspace_file(&cache, &root)
            .is_file()
            .then_some(Workspace { root })
    })
    .await
    .ok()
    .flatten()
}

/// What became of `workspace`, outside the default project, since `build_external_docs` recorded
/// it; `None` when it did not, or that is not told.
pub(super) async fn since_recorded(state: &ToolState, workspace: &Workspace) -> Option<Since> {
    read(state, workspace)
        .await
        .ok()?
        .since(&workspace.root)
        .await
}

/// What `build_external_docs` recorded of `workspace`, outside the default project; or the
/// response while it is recorded, when it is broken, or `no_record`, which tells how to record it.
pub(super) async fn read(
    state: &ToolState,
    workspace: &Workspace,
) -> Result<Arc<Recorded>, Box<CallToolResult>> {
    let root = workspace.root.clone();
    let file = external_workspace::workspace_file(&state.cache, &root);
    let kept = state.recorded_workspaces.lock().get(&root).cloned();
    let cache = state.cache.clone();
    let read = crate::task::spawn_blocking(move || {
        let stamp = Stamp::of(&file);
        if let (Some(kept), Some(stamp)) = (&kept, stamp)
            && kept.stamp == stamp
        {
            external::note_read(&cache, &external_workspace::workspace_key(&root));
            return Ok(Arc::clone(kept));
        }
        match external_workspace::read_workspace(&cache, &root, None) {
            Read::Found(record) => {
                let resolution = record
                    .resolution()
                    .map_err(|error| Unread::Broken(error.to_string()))?;
                Ok(Arc::new(Recorded {
                    resolution: Arc::new(resolution),
                    targets: record.targets,
                    at: record.recorded_at,
                    inputs: record.inputs,
                    members: record.members,
                    targets_read: parking_lot::Mutex::default(),
                    checked: parking_lot::Mutex::default(),
                    stamp: stamp.unwrap_or(Stamp::NONE),
                }))
            },
            Read::Building => Err(Unread::Building),
            // It does not record a failure (decided in M2e's E4b-3).
            Read::Failed(_) | Read::Missing => Err(Unread::Missing),
            Read::Broken(why) => Err(Unread::Broken(why)),
        }
    })
    .await;
    let Ok(read) = read else {
        return Err(Box::new(shutting_down()));
    };
    let root = &workspace.root;
    match read {
        Ok(recorded) => {
            state
                .recorded_workspaces
                .lock()
                .insert(root.clone(), Arc::clone(&recorded));
            Ok(recorded)
        },
        Err(Unread::Building) => Err(Box::new(recording(root, None))),
        Err(Unread::Missing) => Err(Box::new(no_record(root, None))),
        Err(Unread::Broken(why)) => Err(Box::new(broken(
            &format!("the workspace at {}", root.display()),
            &why,
            &call_again(root, None),
        ))),
    }
}

/// When `build_external_docs` recorded `workspace`, outside the default project, with the
/// arguments that record it again for `target`, when what it is resolved from changed since: the
/// response is then of the record, and stale (decided in M2e's E4b-3, 1). `None` for a workspace of
/// the default project, or one that did not change.
pub(super) async fn changed_since(
    state: &ToolState,
    workspace: &Workspace,
    target: Option<&TargetTriple>,
) -> Option<(Box<str>, String, Since)> {
    if !is_outside(state, workspace) {
        return None;
    }
    let recorded = read(state, workspace).await.ok()?;
    // Planned from another record of the workspace, which changed since.
    let before = match target {
        Some(target) => for_target(state, workspace, &recorded, target)
            .await
            .is_ok_and(|(_, before)| before),
        None => false,
    };
    let since = recorded.since(&workspace.root).await?.or_changed(before);
    (since != Since::Same).then(|| (recorded.at.clone(), again(&workspace.root, target), since))
}

/// Marks `response`, read from the record of a workspace that `build_external_docs` made at
/// `recorded_at`, as stale, as `since` tells: `stale` and `stale_reason` in its provenance, a
/// warning, and a hint to record it again, passing `again`, or to record it where it moved
/// (decided in M2e's E4b-3, 1 and 2).
pub(super) fn mark_changed(response: &mut Value, recorded_at: &str, again: &str, since: Since) {
    let (Some(reason), Some(fields)) = (since.reason(), response.as_object_mut()) else {
        return;
    };
    if let Some(Value::Object(provenance)) = fields.get_mut("provenance") {
        provenance.insert("stale".to_owned(), json!(true));
        provenance.insert("stale_reason".to_owned(), json!(reason));
    }
    let (warning, hint) = if since == Since::Missing {
        (
            format!(
                "the workspace's directory is gone since build_external_docs recorded it at \
                 {recorded_at}: this is that record"
            ),
            MOVED.to_owned(),
        )
    } else {
        (
            format!(
                "the workspace changed since build_external_docs recorded it at {recorded_at}: \
                 its versions, features and crates are those of then"
            ),
            format!("to read it as it is now, call build_external_docs with {again}"),
        )
    };
    if let Value::Array(warnings) = fields
        .entry("warnings")
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        warnings.push(Value::from(warning));
    }
    if let Value::Array(hints) = fields
        .entry("hints")
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        hints.push(Value::from(hint));
    }
}

/// The arguments of `build_external_docs` that record the workspace at `root`, for `target` if
/// given, as JSON.
pub(super) fn again(root: &Path, target: Option<&TargetTriple>) -> String {
    let mut arguments = Map::new();
    arguments.insert("project".to_owned(), json!(root.display().to_string()));
    if let Some(target) = target {
        arguments.insert("target".to_owned(), json!(target.as_str()));
    }
    Value::Object(arguments).to_string()
}

/// What calls `build_external_docs` to record the workspace at `root` again.
fn call_again(root: &Path, target: Option<&TargetTriple>) -> String {
    format!(
        "call build_external_docs with {} to record it again",
        again(root, target)
    )
}

/// `docs_unavailable` with `no_record` for the workspace at `root`, outside the default project,
/// which `build_external_docs` has not recorded for `target`, or at all.
pub(super) fn no_record(root: &Path, target: Option<&TargetTriple>) -> CallToolResult {
    let what = target.map_or_else(String::new, |target| format!(" for {target}"));
    // Its directory is gone: it cannot be recorded there (decided in M2e's E4b-3, 2).
    if !root.join("Cargo.toml").is_file() {
        return error_result(&json!({
            "code": "docs_unavailable",
            "message": format!(
                "the directory of {} is gone, and build_external_docs has not recorded it{what}",
                root.display()
            ),
            "retryable": false,
            "suggestions": [MOVED],
            "reason": "no_record",
        }));
    }
    let mut suggestions = vec![format!(
        "call build_external_docs with {}, which runs its build scripts and proc macros, and then \
         look it up again",
        again(root, target)
    )];
    // Before the workspace tells what a crate of it is: a release of crates.io, maybe.
    if target.is_none() {
        suggestions.push(
            "or, for a dependency of crates.io, give the version, as crate@version, to read it \
             from docs.rs"
                .to_owned(),
        );
    }
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!(
            "{} is outside the default project, which the query tools read only as \
             build_external_docs recorded it, and it has not recorded it{what}",
            root.display()
        ),
        "retryable": false,
        "suggestions": suggestions,
        "reason": "no_record",
    }))
}

/// The response while `build_external_docs` records the workspace at `root`, for `target` if
/// given.
fn recording(root: &Path, target: Option<&TargetTriple>) -> CallToolResult {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("project"));
    provenance.insert("workspace".to_owned(), json!(root.display().to_string()));
    provenance.insert("project_from".to_owned(), json!("argument"));
    if let Some(target) = target {
        provenance.insert("target".to_owned(), json!(target.as_str()));
    }
    provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
    building_response(
        provenance,
        Doing::Building,
        "build_external_docs is recording the workspace",
        &[],
    )
}

/// Whether `build_external_docs` recorded `workspace`, outside the default project, for `target`;
/// or the response while it is recorded, when it is broken, or `no_record`, which tells how to
/// record it.
pub(super) async fn for_target_of(
    state: &ToolState,
    workspace: &Workspace,
    target: &TargetTriple,
) -> Result<(), Box<CallToolResult>> {
    let recorded = read(state, workspace).await?;
    for_target(state, workspace, &recorded, target).await?;
    Ok(())
}

/// What `build_external_docs` recorded of `workspace`, which `recorded` tells, for `target`, with
/// whether it was planned from another record of the workspace than `recorded`, as when the
/// workspace was recorded again and the target is not yet; or the response while it is recorded,
/// when it is broken, or `no_record`.
async fn for_target(
    state: &ToolState,
    workspace: &Workspace,
    recorded: &Recorded,
    target: &TargetTriple,
) -> Result<(Arc<TargetRecord>, bool), Box<CallToolResult>> {
    let (cache, root, name) = (
        state.cache.clone(),
        workspace.root.clone(),
        target.as_str().to_owned(),
    );
    let kept = recorded.targets_read.lock().get(target.as_str()).cloned();
    let read = crate::task::spawn_blocking(move || {
        let stamp = Stamp::of(&external_workspace::target_file(&cache, &root, &name));
        if let (Some((kept_stamp, kept)), Some(stamp)) = (kept, stamp)
            && kept_stamp == stamp
        {
            external::note_read(&cache, &external_workspace::target_key(&root, &name));
            return (Read::Found(kept), stamp);
        }
        let read = match external_workspace::read_target(&cache, (&root, &name), None) {
            Read::Found(record) => Read::Found(Arc::new(record)),
            Read::Building => Read::Building,
            Read::Failed(failed) => Read::Failed(failed),
            Read::Broken(why) => Read::Broken(why),
            Read::Missing => Read::Missing,
        };
        (read, stamp.unwrap_or(Stamp::NONE))
    })
    .await;
    let Ok((read, stamp)) = read else {
        return Err(Box::new(shutting_down()));
    };
    let root = &workspace.root;
    match read {
        Read::Found(record) => {
            recorded
                .targets_read
                .lock()
                .insert(target.as_str().into(), (stamp, Arc::clone(&record)));
            let before = record.planned_from != recorded.at;
            Ok((record, before))
        },
        Read::Building => Err(Box::new(recording(root, Some(target)))),
        Read::Failed(_) | Read::Missing => Err(Box::new(no_record(root, Some(target)))),
        Read::Broken(why) => Err(Box::new(broken(
            &format!("the workspace at {} for {target}", root.display()),
            &why,
            &call_again(root, Some(target)),
        ))),
    }
}

/// The feature sets of the build of `workspace`, outside the default project, for `target`, as
/// `build_external_docs` recorded what `cargo tree` told; `None` when it has not recorded them,
/// or they cannot be read.
pub(super) async fn feature_sets(
    state: &ToolState,
    workspace: &Workspace,
    target: &TargetTriple,
) -> Featured {
    let root = &workspace.root;
    let not = |why: String| Err(Arc::new(FeaturesError::NotRecorded(why)));
    let Ok(recorded) = read(state, workspace).await else {
        return not(format!(
            "build_external_docs has not recorded the workspace: call it with {}",
            again(root, None)
        ));
    };
    let Ok((record, _)) = for_target(state, workspace, &recorded, target).await else {
        return not(format!(
            "build_external_docs has not recorded the workspace for {target}, or is recording it: \
             call it with {}",
            again(root, Some(target))
        ));
    };
    match crate::task::spawn_blocking(move || record.trees.parse()).await {
        Ok(Ok(sets)) => Ok(Arc::new(sets)),
        Ok(Err(error)) => not(format!(
            "what build_external_docs recorded of them cannot be read: {error}"
        )),
        Err(error) => not(error.to_string()),
    }
}

/// What the crate `name`, as `asked`, is in `workspace`, outside the default project, for
/// `target`: a member's crate that `build_external_docs` recorded, as [`Member::Recorded`].
pub(super) async fn member(
    state: &ToolState,
    workspace: &Workspace,
    (name, asked): (&CrateName, Asked<'_>),
    target: &TargetTriple,
) -> Result<Member, Box<CallToolResult>> {
    let recorded = read(state, workspace).await?;
    let Some(metadata) = recorded.metadata() else {
        return Ok(Member::Not);
    };
    let crate_name = name.as_str().replace('-', "_");
    let found = match asked {
        Asked::Package { name, version } => build::member_package(metadata, name, version)
            .and_then(|package| Some((package, build::crate_of(package, &crate_name)?))),
        Asked::Explicit { version } => match build::member_crate(metadata, &crate_name) {
            Some((package, _)) if package.version != *version => {
                return Ok(Member::OtherVersion(package.version.clone()));
            },
            found => found,
        },
    };
    let Some((package, documented)) = found else {
        return Ok(Member::Not);
    };
    let documented_name = build::documented_name(package, &documented);
    let (record, before) = for_target(state, workspace, &recorded, target).await?;
    let Some(row) = record
        .packages
        .iter()
        .find(|row| *row.package == *package.id.0 && *row.crate_name == *documented_name)
    else {
        // Its other crate, as a bin of a member that has a lib, which this tool did not document.
        return Ok(Member::Not);
    };
    let Outcome::Built { target: built_for } = &row.outcome else {
        return Ok(Member::Not);
    };
    let since = recorded
        .since(&workspace.root)
        .await
        .unwrap_or(Since::Same)
        .or_changed(before);
    let root = &workspace.root;
    Ok(Member::Recorded(Box::new(RecordedCrate {
        key: key(root, &package.id.0, &row.crate_name, built_for),
        package: package.id.clone(),
        target: built_for.parse().unwrap_or_else(|_| target.clone()),
        again: again(root, Some(target)).into(),
        recorded_at: recorded.at.clone(),
        since,
        error: Box::new(no_record(root, Some(target))),
    })))
}

/// What the project's package `package`, not a member, is in `workspace`, outside the default
/// project, for `target`: what `build_external_docs` recorded of it, as [`Member::Recorded`]; or
/// the response that tells why it is not documented.
pub(super) async fn dependency(
    state: &ToolState,
    workspace: &Workspace,
    package: &ProjectPackage,
    target: &TargetTriple,
) -> Result<Member, Box<CallToolResult>> {
    let recorded = read(state, workspace).await?;
    let Some(metadata) = recorded.metadata() else {
        return Ok(Member::Not);
    };
    let Some(found) = metadata.packages.iter().find(|found| {
        found.name == package.name
            && found.version == package.version
            && Source::parse(found.source.as_deref()) == package.source
            && !metadata.workspace_members.contains(&found.id)
    }) else {
        return Ok(Member::Not);
    };
    let what = format!("{} {}", package.name, package.version);
    let (record, before) = for_target(state, workspace, &recorded, target).await?;
    let Some(row) = record
        .packages
        .iter()
        .find(|row| *row.package == *found.id.0)
    else {
        return Err(Box::new(not_built_for(&what, target)));
    };
    let root = &workspace.root;
    let (built_for, again, error) = match &row.outcome {
        Outcome::Built { target: built_for } => (
            built_for,
            again(root, Some(target)),
            no_record(root, Some(target)),
        ),
        // What this tool builds in the workspace when asked for it (decided in M2e's E4b, 4).
        Outcome::DocsRs {
            target: Some(built_for),
        } => {
            let mut arguments = Map::new();
            arguments.insert(
                "crate".to_owned(),
                json!(format!("{}@{}", package.name, package.version)),
            );
            arguments.insert("project".to_owned(), json!(root.display().to_string()));
            arguments.insert("target".to_owned(), json!(target.as_str()));
            let arguments = Value::Object(arguments).to_string();
            let error = not_built_here(&what, &arguments);
            (built_for, arguments, error)
        },
        Outcome::DocsRs { target: None } | Outcome::Unnameable => {
            return Err(Box::new(later(&what)));
        },
    };
    let since = recorded
        .since(root)
        .await
        .unwrap_or(Since::Same)
        .or_changed(before);
    Ok(Member::Recorded(Box::new(RecordedCrate {
        key: key(root, &found.id.0, &row.crate_name, built_for),
        package: found.id.clone(),
        target: built_for.parse().unwrap_or_else(|_| target.clone()),
        again: again.into(),
        recorded_at: recorded.at.clone(),
        since,
        error: Box::new(error),
    })))
}

/// The key of the record of `build_external_docs` of the crate `crate_name` of `package` of the
/// workspace at `root`, for `target`.
fn key(root: &Path, package: &str, crate_name: &str, target: &str) -> Key {
    Key::Target {
        workspace: root.display().to_string().into(),
        package: package.into(),
        name: crate_name.into(),
        target: target.into(),
        hidden: false,
    }
}

/// `docs_unavailable` with `no_record` for `what`, a dependency of crates.io of a workspace outside
/// the default project, which docs.rs has no rustdoc JSON of: `build_external_docs` builds it in
/// the workspace, called with `arguments` (decided in M2e's E4b, 4).
fn not_built_here(what: &str, arguments: &str) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!(
            "docs.rs has no rustdoc JSON of {what}, and build_external_docs has not built it in \
             the workspace"
        ),
        "retryable": false,
        "suggestions": [format!(
            "call build_external_docs with {arguments}, which builds it in the workspace, and then \
             look it up again"
        )],
        "reason": "no_record",
    }))
}

/// `docs_unavailable` with `build_unsupported` for `what`, which `cargo rustdoc -p` cannot name in
/// a workspace outside the default project (in a later version, decided in M2e's E4b, 2).
fn later(what: &str) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!(
            "{what} cannot be named by cargo rustdoc -p in the workspace, and is not documented \
             here (in a later version)"
        ),
        "retryable": false,
        "suggestions": [],
        "reason": "build_unsupported",
    }))
}
