//! The records of `build_external_docs` of a workspace outside the default project (requirement
//! R-WS-10, decided in M2e's E4b).
//!
//! The query tools run no cargo there (requirement R-MCP-3): they read what the external tool
//! recorded of it. That is what `cargo metadata` resolved, with its Cargo.lock and the files that
//! it is made from, and the targets that it builds for ([`WorkspaceRecord`]); for each target, what
//! `cargo tree` tells of its features and what became of each package ([`TargetRecord`]); and the
//! builds of its packages, recorded as [`Key::Target`]. A path that names the workspace (as given,
//! its root, or a member's directory) is an alias of it, which finds its record when the
//! directory is gone (decided in M2e, 5).

use std::{
    fs,
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    cache::{self, Cache},
    cargo_lock::CargoLock,
    external::{self, Key, Making, Read},
    features::Trees,
    freshness::Inputs,
    metadata::{Metadata, Resolution},
    project,
};

/// The file of a [`WorkspaceRecord`] in the directory of its key.
const WORKSPACE: &str = "workspace.json";

/// The file of a [`TargetRecord`] in the directory of its key.
const TARGET: &str = "target.json";

/// The directory of the aliases in the records' directory.
const ALIASES: &str = "alias";

/// What `build_external_docs` recorded of a workspace outside the default project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// What `cargo metadata --format-version 1` printed.
    pub metadata: Box<str>,
    /// Its Cargo.lock, which has the checksums that `cargo metadata` does not tell.
    pub lock: Box<str>,
    /// The files that `cargo metadata` resolved it from, as they were (requirement R-WS-4).
    pub inputs: Inputs,
    /// The directories that its `members` patterns matched, as the key of the default project's
    /// `cargo metadata` has them (requirement R-VR-2): a new one changes what cargo resolves.
    /// `None` when they could not be expanded.
    pub members: Option<Vec<PathBuf>>,
    /// The targets that it builds for, the first for a lookup that gives none (requirement
    /// R-TG-1).
    pub targets: Vec<Box<str>>,
    /// When it was recorded, in RFC 3339.
    pub recorded_at: Box<str>,
}

impl WorkspaceRecord {
    /// The resolution that it records, as `cargo metadata` gave it.
    ///
    /// It parses what may be megabytes, so it is for a blocking thread.
    ///
    /// # Errors
    ///
    /// Returns an error when what it keeps cannot be read.
    pub fn resolution(&self) -> io::Result<Resolution> {
        let metadata: Metadata = serde_json::from_str(&self.metadata)?;
        let lock = CargoLock::parse(&self.lock).map_err(io::Error::other)?;
        Ok(Resolution::Metadata { metadata, lock })
    }

    /// Whether it records the same as `other`, but for when: what `cargo metadata` resolved, its
    /// Cargo.lock, the contents of the files that it is made from, the directories that its
    /// `members` patterns matched and the targets that it builds for. The records for its targets
    /// are then still of it (decided in M2e's E4b-3).
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        let contents = |inputs: &Inputs| -> Vec<(PathBuf, Option<Box<str>>)> {
            inputs
                .files
                .iter()
                .map(|file| (file.path.clone(), file.sha256.clone()))
                .collect()
        };
        self.metadata == other.metadata
            && self.lock == other.lock
            && self.members == other.members
            && self.targets == other.targets
            && contents(&self.inputs) == contents(&other.inputs)
    }
}

/// The directories under the workspace root `root` that its `members` patterns match, sorted;
/// `None` when they cannot be expanded.
///
/// It reads directories, so it is for a blocking thread.
#[must_use]
pub fn members_of(root: &Path) -> Option<Vec<PathBuf>> {
    project::expand_members(root)
}

/// What `build_external_docs` recorded of a workspace outside the default project for a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRecord {
    /// The `recorded_at` of the record of the workspace that it was planned from: when that is
    /// recorded again, this is to be planned again.
    pub planned_from: Box<str>,
    /// What `cargo tree` printed of its feature sets for the target, for `enabled_in_project`
    /// (requirement R-FT-2).
    pub trees: Trees,
    /// What became of its packages that the build for the target builds: those that it does not
    /// are not among them.
    pub packages: Vec<RecordedPackage>,
}

/// What became of a package of a workspace outside the default project, for a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedPackage {
    /// Its id in `cargo metadata`.
    pub package: Box<str>,
    /// The crate of it that is documented, as code writes it.
    #[serde(rename = "crate")]
    pub crate_name: Box<str>,
    /// What became of it.
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// What became of a package of a workspace outside the default project, for a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// `build_external_docs` built it, or failed to, as the [`Key::Target`] record for `target`
    /// tells: the target's, or the host's for what the build for the target builds for the host.
    Built {
        /// The target of its record.
        target: Box<str>,
    },
    /// A release of crates.io, which docs.rs may have: asked of docs.rs when it is looked up, and
    /// else built by `build_external_docs` when asked for it (requirement R-WS-10, decided in M2e's
    /// E4b, 4).
    DocsRs {
        /// The target of the record that that build makes: the target's, or the host's for what
        /// the build for the target builds for the host. `None` when `cargo rustdoc -p` cannot
        /// name it, which is not built here.
        #[serde(default)]
        target: Option<Box<str>>,
    },
    /// One that `cargo rustdoc -p` cannot name, which is not documented here (in a later version,
    /// decided in M2e's E4b, 2).
    Unnameable,
}

/// The key of the record of the workspace at `root`.
#[must_use]
pub fn workspace_key(root: &Path) -> Key {
    Key::Workspace {
        workspace: root.display().to_string().into(),
    }
}

/// The key of the record of the workspace at `root` for `target`.
#[must_use]
pub fn target_key(root: &Path, target: &str) -> Key {
    Key::WorkspaceTarget {
        workspace: root.display().to_string().into(),
        target: target.into(),
        hidden: false,
    }
}

/// The file of the record of the workspace at `root` in `cache`, whose length and time tell that
/// it was recorded again.
#[must_use]
pub fn workspace_file(cache: &Cache, root: &Path) -> PathBuf {
    workspace_key(root).dir(cache.dir()).join(WORKSPACE)
}

/// The record of the workspace at `root` in `cache`, without waiting; for the holder of its lock,
/// `making`, which is not told that it is being made.
///
/// It reads files, so it is for a blocking thread.
#[must_use]
pub fn read_workspace(
    cache: &Cache,
    root: &Path,
    making: Option<&Making>,
) -> Read<WorkspaceRecord> {
    external::read_file(cache, (&workspace_key(root), WORKSPACE), making)
}

/// Records `record` of the workspace at `root` in `cache`, under the lock of its key (`making`).
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of the key, or a file cannot be written.
pub fn record_workspace(
    cache: &Cache,
    root: &Path,
    record: &WorkspaceRecord,
    making: &Making,
) -> io::Result<()> {
    external::write_file(cache, (&workspace_key(root), WORKSPACE), record, making)
}

/// The record of the workspace at `root` for `target` in `cache`, without waiting; for the holder
/// of its lock, `making`, which is not told that it is being made.
///
/// It reads files, so it is for a blocking thread.
#[must_use]
pub fn read_target(
    cache: &Cache,
    (root, target): (&Path, &str),
    making: Option<&Making>,
) -> Read<TargetRecord> {
    external::read_file(cache, (&target_key(root, target), TARGET), making)
}

/// Records `record` of the workspace at `root` for `target` in `cache`, under the lock of its key
/// (`making`).
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of the key, or a file cannot be written.
pub fn record_target(
    cache: &Cache,
    (root, target): (&Path, &str),
    record: &TargetRecord,
    making: &Making,
) -> io::Result<()> {
    external::write_file(cache, (&target_key(root, target), TARGET), record, making)
}

/// An alias: a path that names a workspace, and the root of the workspace.
#[derive(Debug, Serialize, Deserialize)]
struct Alias {
    /// The path.
    path: Box<str>,
    /// The root of the workspace.
    root: Box<str>,
}

/// The file of the alias `path` in the cache at `root`: `ext/v<format>/alias/<digest>`.
fn alias_file(root: &Path, path: &Path) -> PathBuf {
    let digest = external::digest_of(&project::normalize(path).display().to_string());
    external::records_dir(root).join(ALIASES).join(digest)
}

/// Records each of `paths` as an alias of the workspace at `root` in `cache`, as `root` itself is
/// one: as they read without `.` and `..`.
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when a file cannot be written.
pub fn record_aliases(cache: &Cache, paths: &[&Path], root: &Path) -> io::Result<()> {
    for path in paths.iter().copied().chain([root]) {
        let file = alias_file(cache.dir(), path);
        cache.check_inside(&file)?;
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let alias = Alias {
            path: project::normalize(path).display().to_string().into(),
            root: root.display().to_string().into(),
        };
        cache::write_atomically(&file, &serde_json::to_vec(&alias)?)?;
    }
    Ok(())
}

/// The root of the workspace that `path` is an alias of in `cache`, if it is one: as it reads
/// without `.` and `..`.
///
/// It reads a file, so it is for a blocking thread.
#[must_use]
pub fn aliased_root(cache: &Cache, path: &Path) -> Option<PathBuf> {
    let text = fs::read(alias_file(cache.dir(), path)).ok()?;
    let alias: Alias = serde_json::from_slice(&text).ok()?;
    (*alias.path == *project::normalize(path).display().to_string())
        .then(|| PathBuf::from(&*alias.root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::freshness;

    const METADATA: &str = r#"{
        "workspace_root": "/w",
        "workspace_members": ["path+file:///w#app@0.1.0"],
        "packages": [{
            "id": "path+file:///w#app@0.1.0",
            "name": "app",
            "version": "0.1.0",
            "source": null,
            "manifest_path": "/w/Cargo.toml",
            "targets": [{ "name": "app", "kind": ["lib"] }]
        }],
        "resolve": null,
        "version": 1
    }"#;

    fn workspace_record() -> WorkspaceRecord {
        WorkspaceRecord {
            metadata: METADATA.into(),
            lock: "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n".into(),
            inputs: freshness::collect(&[], &[], None).expect("collected"),
            members: Some(vec![PathBuf::from("/w/crates/a")]),
            targets: vec!["x86_64-unknown-linux-gnu".into()],
            recorded_at: "2026-10-22T00:00:00Z".into(),
        }
    }

    #[test]
    fn the_names_of_the_keys_of_a_workspace_do_not_change() {
        let root = Path::new("/w");
        assert_eq!(
            serde_json::to_string(&workspace_key(root)).expect("JSON"),
            r#"{"kind":"workspace","workspace":"/w"}"#
        );
        assert_eq!(
            serde_json::to_string(&target_key(root, "wasm32-wasip1")).expect("JSON"),
            r#"{"kind":"workspace_target","workspace":"/w","target":"wasm32-wasip1","hidden":false}"#
        );
        assert!(
            workspace_key(root)
                .dir(Path::new("/c"))
                .starts_with("/c/ext/v1/workspace")
        );
    }

    #[test]
    fn a_workspace_is_recorded_and_read_with_its_resolution() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let root = Path::new("/w");
        assert!(matches!(read_workspace(&cache, root, None), Read::Missing));
        let making = external::try_make(&cache, &workspace_key(root))
            .expect("locked")
            .expect("not held");
        assert!(matches!(read_workspace(&cache, root, None), Read::Building));
        let record = workspace_record();
        record_workspace(&cache, root, &record, &making).expect("recorded");
        assert!(matches!(
            read_workspace(&cache, root, Some(&making)),
            Read::Found(_)
        ));
        drop(making);
        let Read::Found(found) = read_workspace(&cache, root, None) else {
            panic!("not found");
        };
        assert_eq!(found, record);
        let Resolution::Metadata { metadata, .. } = found.resolution().expect("resolved") else {
            panic!("not of cargo metadata");
        };
        assert_eq!(metadata.workspace_root, Path::new("/w"));
        // Another workspace's record is not it.
        let other = Path::new("/v");
        fs::create_dir_all(workspace_key(other).dir(dir.path())).expect("made");
        fs::copy(
            workspace_key(root).dir(dir.path()).join(WORKSPACE),
            workspace_key(other).dir(dir.path()).join(WORKSPACE),
        )
        .expect("copied");
        assert!(matches!(
            read_workspace(&cache, other, None),
            Read::Broken(_)
        ));
    }

    #[test]
    fn what_was_built_for_a_target_is_recorded() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let root = Path::new("/w");
        let target = "wasm32-wasip1";
        let making = external::try_make(&cache, &target_key(root, target))
            .expect("locked")
            .expect("not held");
        let record = TargetRecord {
            planned_from: "2026-10-22T00:00:00Z".into(),
            trees: Trees {
                normal: "0app v0.1.0 (/w)\t\n".into(),
                with_dev: "0app v0.1.0 (/w)\t\n".into(),
            },
            packages: vec![
                RecordedPackage {
                    package: "path+file:///w#app@0.1.0".into(),
                    crate_name: "app".into(),
                    outcome: Outcome::Built {
                        target: target.into(),
                    },
                },
                RecordedPackage {
                    package: "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0"
                        .into(),
                    crate_name: "serde".into(),
                    outcome: Outcome::DocsRs {
                        target: Some(target.into()),
                    },
                },
            ],
        };
        record_target(&cache, (root, target), &record, &making).expect("recorded");
        drop(making);
        let Read::Found(found) = read_target(&cache, (root, target), None) else {
            panic!("not found");
        };
        assert_eq!(found, record);
        let text = fs::read_to_string(target_key(root, target).dir(dir.path()).join(TARGET))
            .expect("read");
        assert!(text.contains(r#""outcome": "docs_rs""#), "{text}");
        found.trees.parse().expect("feature sets");
    }

    #[test]
    fn a_new_directory_that_the_members_patterns_match_changes_the_members() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .expect("written");
        fs::create_dir_all(root.join("crates/a")).expect("made");
        let record = WorkspaceRecord {
            members: members_of(root),
            ..workspace_record()
        };
        assert_eq!(record.members, Some(vec![root.join("crates/a")]));
        assert_eq!(members_of(root), record.members);
        fs::create_dir_all(root.join("crates/b")).expect("made");
        assert_ne!(members_of(root), record.members);
    }

    #[test]
    fn a_record_of_the_same_is_the_same_but_for_when() {
        let record = workspace_record();
        let later = WorkspaceRecord {
            recorded_at: "2026-10-23T00:00:00Z".into(),
            ..workspace_record()
        };
        assert!(record.same_as(&later));
        let other = WorkspaceRecord {
            lock: "version = 4\n".into(),
            ..workspace_record()
        };
        assert!(!record.same_as(&other));
        let other = WorkspaceRecord {
            targets: vec!["wasm32-wasip1".into()],
            ..workspace_record()
        };
        assert!(!record.same_as(&other));
    }

    #[test]
    fn an_alias_finds_the_root_of_its_workspace() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let root = Path::new("/w");
        let given = Path::new("/w/app/../app");
        assert_eq!(aliased_root(&cache, given), None);
        record_aliases(&cache, &[given, Path::new("/w/app")], root).expect("recorded");
        assert_eq!(aliased_root(&cache, given), Some(PathBuf::from("/w")));
        assert_eq!(aliased_root(&cache, root), Some(PathBuf::from("/w")));
        assert_eq!(aliased_root(&cache, Path::new("/w/other")), None);
        // As they read without `.` and `..`.
        assert_eq!(
            aliased_root(&cache, Path::new("/w/./app/../")),
            Some(PathBuf::from("/w"))
        );
    }
}
