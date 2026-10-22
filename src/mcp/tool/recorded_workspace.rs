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
        external::{Key, Read},
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
}

impl Recorded {
    /// The packages of the workspace, as `cargo metadata` resolved them.
    fn metadata(&self) -> Option<&Metadata> {
        match &*self.resolution {
            Resolution::Metadata { metadata, .. } => Some(metadata),
            Resolution::CargoLockRaw { .. } => None,
        }
    }

    /// Whether what the workspace at `root` is resolved from changed since it was recorded: its
    /// manifests, Cargo.lock and cargo's settings, or the directories that its `members` patterns
    /// match. `None` when the task did not end.
    pub(super) async fn changed(self: &Arc<Self>, root: &Path) -> Option<bool> {
        let (recorded, root) = (Arc::clone(self), root.to_path_buf());
        crate::task::spawn_blocking(move || {
            !freshness::unchanged_as_recorded(&recorded.inputs)
                || external_workspace::members_of(&root) != recorded.members
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
pub(super) fn is_outside(state: &ToolState, workspace: &Workspace) -> bool {
    !state
        .project
        .as_ref()
        .is_some_and(|default| default.contains(workspace))
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
                    stamp: stamp.unwrap_or(Stamp {
                        len: 0,
                        modified: None,
                        inode: 0,
                    }),
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
) -> Option<(Box<str>, String)> {
    if !is_outside(state, workspace) {
        return None;
    }
    let recorded = read(state, workspace).await.ok()?;
    recorded
        .changed(&workspace.root)
        .await?
        .then(|| (recorded.at.clone(), again(&workspace.root, target)))
}

/// Marks `response`, read from the record of a workspace that `build_external_docs` made at
/// `recorded_at`, as stale, as what the workspace is resolved from changed since: `stale` and
/// `stale_reason` in its provenance, a warning, and a hint to record it again, passing `again`
/// (decided in M2e's E4b-3, 1).
pub(super) fn mark_changed(response: &mut Value, recorded_at: &str, again: &str) {
    let Some(fields) = response.as_object_mut() else {
        return;
    };
    if let Some(Value::Object(provenance)) = fields.get_mut("provenance") {
        provenance.insert("stale".to_owned(), json!(true));
        provenance.insert("stale_reason".to_owned(), json!("inputs_changed"));
    }
    let warning = format!(
        "the workspace changed since build_external_docs recorded it at {recorded_at}: its \
         versions, features and crates are those of then"
    );
    if let Value::Array(warnings) = fields
        .entry("warnings")
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        warnings.push(Value::from(warning));
    }
    let hint = format!("to read it as it is now, call build_external_docs with {again}");
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

/// What `build_external_docs` recorded of `workspace`, which `recorded` tells, for `target`, with
/// whether it was planned from another record of the workspace than `recorded`, as when the
/// workspace was recorded again and the target is not yet; or the response while it is recorded,
/// when it is broken, or `no_record`.
async fn for_target(
    state: &ToolState,
    workspace: &Workspace,
    recorded: &Recorded,
    target: &TargetTriple,
) -> Result<(TargetRecord, bool), Box<CallToolResult>> {
    let (cache, root, name) = (
        state.cache.clone(),
        workspace.root.clone(),
        target.as_str().to_owned(),
    );
    let read = crate::task::spawn_blocking(move || {
        external_workspace::read_target(&cache, (&root, &name), None)
    })
    .await;
    let Ok(read) = read else {
        return Err(Box::new(shutting_down()));
    };
    let root = &workspace.root;
    match read {
        Read::Found(record) => {
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
    let changed = before || recorded.changed(&workspace.root).await.unwrap_or(false);
    let root = &workspace.root;
    Ok(Member::Recorded(Box::new(RecordedCrate {
        key: key(root, &package.id.0, &row.crate_name, built_for),
        package: package.id.clone(),
        target: built_for.parse().unwrap_or_else(|_| target.clone()),
        again: again(root, Some(target)).into(),
        changed,
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
    let changed = before || recorded.changed(root).await.unwrap_or(false);
    Ok(Member::Recorded(Box::new(RecordedCrate {
        key: key(root, &found.id.0, &row.crate_name, built_for),
        package: found.id.clone(),
        target: built_for.parse().unwrap_or_else(|_| target.clone()),
        again: again.into(),
        changed,
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
