//! The project that lookups are about: the default project, and the Cargo workspace that a path
//! belongs to (requirement R-MCP-3).
//!
//! Manifests are read as data, with Cargo's rules, without running cargo: the lookup tools run
//! cargo only for the default project, so whether a path is in it has to be known first.

use std::{
    ffi::OsString,
    fs,
    io,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

/// A Cargo workspace, or a package that is not in one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Workspace {
    /// The directory of the root `Cargo.toml`, absolute and with symlinks resolved.
    pub root: PathBuf,
}

/// Where the default project came from, as the provenance shows it (`project_from`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFrom {
    /// The environment variable `CLAUDE_PROJECT_DIR`.
    ClaudeProjectDir,
    /// The working directory of the server.
    Cwd,
}

/// The default project: the workspace of the directory Claude Code was opened in, or, when that
/// directory has no `Cargo.toml`, the workspaces directly inside it (requirement R-MCP-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultProject {
    /// The directory it was found from.
    pub dir: PathBuf,
    /// Where `dir` came from.
    pub from: ProjectFrom,
    /// The workspaces, by root. Empty when there is none; more than one when `dir` has none
    /// itself but has several directly inside it.
    pub workspaces: Box<[Workspace]>,
}

/// Why the workspace of a path could not be found.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    /// There is no `Cargo.toml` at the path.
    #[error("there is no Cargo.toml at {0}")]
    NoManifest(Box<Path>),
    /// A manifest could not be read.
    #[error("{} could not be read", .path.display())]
    Unreadable {
        /// The manifest.
        path: Box<Path>,
        /// What failed.
        #[source]
        source: io::Error,
    },
    /// A manifest is not valid TOML of a Cargo manifest.
    #[error("{} is not a valid Cargo.toml", .path.display())]
    Malformed {
        /// The manifest.
        path: Box<Path>,
        /// What the parser reported.
        #[source]
        source: toml::de::Error,
    },
}

/// The parts of a `Cargo.toml` that decide the workspace.
#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    workspace: Option<WorkspaceTable>,
    #[serde(default)]
    package: Option<PackageTable>,
}

#[derive(Debug, Default, Deserialize)]
struct WorkspaceTable {
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PackageTable {
    /// `package.workspace`: the path to the workspace root, for a member outside it.
    #[serde(default)]
    workspace: Option<String>,
}

impl DefaultProject {
    /// The default project, from `CLAUDE_PROJECT_DIR` if it is set and not empty, or else from the
    /// working directory `cwd`. It is decided once, at startup, so that a repository cloned
    /// inside later does not join it (requirement R-MCP-3). `None` when neither is known.
    #[must_use]
    pub fn find(claude_project_dir: Option<OsString>, cwd: Option<PathBuf>) -> Option<Self> {
        let (dir, from) = match claude_project_dir.filter(|dir| !dir.is_empty()) {
            Some(dir) => (PathBuf::from(dir), ProjectFrom::ClaudeProjectDir),
            None => (cwd?, ProjectFrom::Cwd),
        };
        let dir = dunce::canonicalize(&dir).unwrap_or(dir);
        let mut workspaces = if dir.join("Cargo.toml").is_file() {
            readable_workspace(&dir, None).into_iter().collect()
        } else {
            children_workspaces(&dir)
        };
        workspaces.sort();
        workspaces.dedup();
        Some(Self {
            dir,
            from,
            workspaces: workspaces.into(),
        })
    }

    /// Whether `workspace` is (one of) the default project's.
    #[must_use]
    pub fn contains(&self, workspace: &Workspace) -> bool {
        self.workspaces.contains(workspace)
    }
}

/// The workspaces of the directories directly inside `dir` that have a `Cargo.toml`, without
/// looking above `dir` for a workspace they belong to. Symlinked directories are not followed.
fn children_workspaces(dir: &Path) -> Vec<Workspace> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|child| child.join("Cargo.toml").is_file())
        .filter_map(|child| readable_workspace(&child, Some(dir)))
        .collect()
}

/// [`find_workspace`], leaving out with a log line a workspace whose manifest cannot be read.
fn readable_workspace(path: &Path, boundary: Option<&Path>) -> Option<Workspace> {
    find_workspace(path, boundary)
        .inspect_err(|error| tracing::warn!(%error, "leaving out a workspace that cannot be read"))
        .ok()
}

/// The workspace that `path`, a `Cargo.toml` or a directory with one, belongs to (requirement
/// R-MCP-3). A member's workspace is its root; a package that no workspace above it lists is its
/// own.
///
/// # Errors
///
/// Returns an error when there is no `Cargo.toml` at `path`, or a manifest cannot be read.
pub fn workspace_of(path: &Path) -> Result<Workspace, ProjectError> {
    find_workspace(path, None)
}

/// [`workspace_of`], looking for a workspace above the package no higher than `boundary`.
fn find_workspace(path: &Path, boundary: Option<&Path>) -> Result<Workspace, ProjectError> {
    let manifest = if path.file_name().is_some_and(|name| name == "Cargo.toml") {
        path.to_path_buf()
    } else {
        path.join("Cargo.toml")
    };
    if !manifest.is_file() {
        return Err(ProjectError::NoManifest(manifest.into()));
    }
    let dir = manifest
        .parent()
        .map(|dir| dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()))
        .unwrap_or_default();
    let parsed = read_manifest(&manifest)?;
    if parsed.workspace.is_some() {
        return Ok(Workspace { root: dir });
    }
    if let Some(root) = parsed.package.and_then(|package| package.workspace) {
        let root = dir.join(root);
        return Ok(Workspace {
            root: dunce::canonicalize(&root).unwrap_or(root),
        });
    }
    // As Cargo does, the first workspace above that does not exclude the package decides: it is
    // the root if it lists the package. One that does not list it is not picked up, and the
    // package is taken as one of its own, which Cargo refuses to build.
    for ancestor in dir.ancestors().skip(1) {
        if boundary.is_some_and(|boundary| !ancestor.starts_with(boundary)) {
            break;
        }
        let candidate = ancestor.join("Cargo.toml");
        if !candidate.is_file() {
            continue;
        }
        let Some(table) = read_manifest(&candidate)?.workspace else {
            continue;
        };
        if excludes(&table, ancestor, &dir) {
            continue;
        }
        if lists(&table, ancestor, &dir) {
            return Ok(Workspace {
                root: ancestor.to_path_buf(),
            });
        }
        break;
    }
    Ok(Workspace { root: dir })
}

/// Reads the parts of the manifest at `path` that decide the workspace.
fn read_manifest(path: &Path) -> Result<Manifest, ProjectError> {
    let text = fs::read_to_string(path).map_err(|source| ProjectError::Unreadable {
        path: path.into(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ProjectError::Malformed {
        path: path.into(),
        source,
    })
}

/// Whether the workspace `table` at `root` excludes the package at `dir`, as Cargo decides it: an
/// `exclude` path contains it, and no `members` entry taken as a plain path does.
fn excludes(table: &WorkspaceTable, root: &Path, dir: &Path) -> bool {
    let contains = |entry: &String| dir.starts_with(normalize(&root.join(entry)));
    table.exclude.iter().any(contains) && !table.members.iter().any(contains)
}

/// Whether the workspace `table` at `root` lists the package at `dir` among its `members`. Path
/// dependencies that Cargo makes members without being listed are not seen.
fn lists(table: &WorkspaceTable, root: &Path, dir: &Path) -> bool {
    table.members.iter().any(|member| {
        let member = normalize(&root.join(member));
        // Only the part under the root is a pattern, so that a root whose path has `*` in it
        // still matches.
        match (member.strip_prefix(root), dir.strip_prefix(root)) {
            (Ok(pattern), Ok(relative)) => matches_pattern(&parts(pattern), &parts(relative)),
            (Ok(_) | Err(_), Ok(_) | Err(_)) => member == dir,
        }
    })
}

/// Most directory entries looked at to expand the `members` patterns of one workspace.
const MAX_EXPANSION_ENTRIES: usize = 10_000;

/// Deepest directory that a `**` in a `members` pattern is followed into.
const MAX_EXPANSION_DEPTH: usize = 32;

/// The directories under the workspace root `root` that its `members` patterns match, sorted, as
/// the glob that Cargo uses would expand them. A new directory that a pattern matches changes the
/// result, so it goes into the key of the `cargo metadata` cache (requirement R-VR-2). `None`
/// when the root manifest cannot be read, or the patterns reach more than
/// [`MAX_EXPANSION_ENTRIES`] entries or deeper than [`MAX_EXPANSION_DEPTH`].
pub(super) fn expand_members(root: &Path) -> Option<Vec<PathBuf>> {
    let Some(table) = read_manifest(&root.join("Cargo.toml")).ok()?.workspace else {
        return Some(Vec::new());
    };
    let mut budget = MAX_EXPANSION_ENTRIES;
    let mut found = Vec::new();
    for member in &table.members {
        // A member outside the root is a plain path: its manifest is in the key already.
        if let Ok(pattern) = normalize(&root.join(member)).strip_prefix(root) {
            expand(root, &parts(pattern), 0, &mut budget, &mut found)?;
        }
    }
    found.sort();
    found.dedup();
    Some(found)
}

/// Adds the directories under `dir` that `pattern` matches to `found`. `None` when the budget or
/// the depth runs out.
fn expand(
    dir: &Path,
    pattern: &[Vec<char>],
    depth: usize,
    budget: &mut usize,
    found: &mut Vec<PathBuf>,
) -> Option<()> {
    let Some((first, rest)) = pattern.split_first() else {
        found.push(dir.to_path_buf());
        return Some(());
    };
    if depth >= MAX_EXPANSION_DEPTH {
        return None;
    }
    let deeper = depth.saturating_add(1);
    let any_depth = *first == ['*', '*'];
    if !any_depth && !first.iter().any(|&c| c == '*' || c == '?') {
        let next = dir.join(first.iter().collect::<String>());
        return if next.is_dir() {
            expand(&next, rest, deeper, budget, found)
        } else {
            Some(())
        };
    }
    if any_depth {
        expand(dir, rest, depth, budget, found)?;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Some(());
    };
    for entry in entries {
        *budget = budget.checked_sub(1)?;
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if any_depth {
            expand(&entry.path(), pattern, deeper, budget, found)?;
        } else {
            let name: Vec<char> = entry.file_name().to_string_lossy().chars().collect();
            if matches_glob(first, &name) {
                expand(&entry.path(), rest, deeper, budget, found)?;
            }
        }
    }
    Some(())
}

/// `path` with `.` dropped and `..` taken back, without looking at the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                normal.pop();
            },
            Component::Normal(_) | Component::RootDir | Component::Prefix(_) => {
                normal.push(component);
            },
        }
    }
    normal
}

/// The components of a relative path, as text to match against.
fn parts(path: &Path) -> Vec<Vec<char>> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().chars().collect())
        .collect()
}

/// Whether the path `parts` matches a `members` pattern, as the glob crate that Cargo uses does:
/// a `**` component matches any number of directories, and in the others `*` matches any
/// characters and `?` one. Character classes (`[...]`) are taken literally.
fn matches_pattern(pattern: &[Vec<char>], parts: &[Vec<char>]) -> bool {
    match pattern.split_first() {
        None => parts.is_empty(),
        Some((first, rest)) if *first == ['*', '*'] => {
            matches_pattern(rest, parts)
                || parts
                    .split_first()
                    .is_some_and(|(_, parts_rest)| matches_pattern(pattern, parts_rest))
        },
        Some((first, rest)) => parts.split_first().is_some_and(|(part, parts_rest)| {
            matches_glob(first, part) && matches_pattern(rest, parts_rest)
        }),
    }
}

/// Whether `text` matches `pattern`, where `*` matches any run of characters and `?` one.
fn matches_glob(pattern: &[char], text: &[char]) -> bool {
    match (pattern.split_first(), text.split_first()) {
        (None, None) => true,
        (Some(('*', rest)), _) => {
            matches_glob(rest, text)
                || text
                    .split_first()
                    .is_some_and(|(_, text_rest)| matches_glob(pattern, text_rest))
        },
        (Some(('?', rest)), Some((_, text_rest))) => matches_glob(rest, text_rest),
        (Some((expected, rest)), Some((actual, text_rest))) => {
            expected == actual && matches_glob(rest, text_rest)
        },
        (Some(_), None) | (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `files` (a path and its contents each) under a new temporary directory.
    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for (path, contents) in files {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
            fs::write(path, contents).expect("the file is written");
        }
        dir
    }

    fn root(dir: &tempfile::TempDir, relative: &str) -> PathBuf {
        dunce::canonicalize(dir.path().join(relative)).expect("the directory exists")
    }

    const PACKAGE: &str = "[package]\nname = \"p\"\nversion = \"0.1.0\"\n";

    #[test]
    fn a_member_belongs_to_the_workspace_that_lists_it() {
        let dir = tree(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/old\"]\n",
            ),
            ("crates/a/Cargo.toml", PACKAGE),
            ("crates/old/Cargo.toml", PACKAGE),
            ("tools/t/Cargo.toml", PACKAGE),
        ]);
        let workspace = |path: &str| workspace_of(&dir.path().join(path)).expect("found").root;
        assert_eq!(workspace(""), root(&dir, ""));
        assert_eq!(workspace("crates/a"), root(&dir, ""));
        assert_eq!(workspace("crates/a/Cargo.toml"), root(&dir, ""));
        // Excluded, or not listed: a package of its own.
        assert_eq!(workspace("crates/old"), root(&dir, "crates/old"));
        assert_eq!(workspace("tools/t"), root(&dir, "tools/t"));
    }

    #[test]
    fn package_workspace_points_to_the_root() {
        let dir = tree(&[
            ("ws/Cargo.toml", "[workspace]\nmembers = [\"../outside\"]\n"),
            (
                "outside/Cargo.toml",
                "[package]\nname = \"o\"\nversion = \"0.1.0\"\nworkspace = \"../ws\"\n",
            ),
        ]);
        assert_eq!(
            workspace_of(&dir.path().join("outside"))
                .expect("found")
                .root,
            root(&dir, "ws")
        );
    }

    #[test]
    fn a_path_without_a_manifest_is_an_error() {
        let dir = tree(&[]);
        assert!(matches!(
            workspace_of(dir.path()),
            Err(ProjectError::NoManifest(_))
        ));
    }

    #[test]
    fn the_default_project_is_the_workspace_of_its_directory() {
        let dir = tree(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n"),
            ("a/Cargo.toml", PACKAGE),
        ]);
        let project =
            DefaultProject::find(Some(dir.path().join("a").into()), None).expect("a directory");
        assert_eq!(project.from, ProjectFrom::ClaudeProjectDir);
        assert_eq!(&*project.workspaces, [Workspace {
            root: root(&dir, "")
        }]);
    }

    #[test]
    fn without_a_manifest_the_workspaces_directly_inside_are_the_default_project() {
        let dir = tree(&[
            ("one/Cargo.toml", PACKAGE),
            ("two/Cargo.toml", "[workspace]\nmembers = [\"m\"]\n"),
            ("two/m/Cargo.toml", PACKAGE),
            ("deeper/three/Cargo.toml", PACKAGE),
            ("notes/README.md", ""),
        ]);
        let project = DefaultProject::find(Some(OsString::new()), Some(dir.path().to_path_buf()))
            .expect("a directory");
        assert_eq!(project.from, ProjectFrom::Cwd);
        assert_eq!(&*project.workspaces, [
            Workspace {
                root: root(&dir, "one")
            },
            Workspace {
                root: root(&dir, "two")
            },
        ]);
    }

    #[test]
    fn exclude_and_members_follow_cargo() {
        // Each case was checked against `cargo metadata --no-deps` (cargo 1.99.1).
        let dir = tree(&[
            // A workspace that excludes the package is passed over for one above it.
            ("b/Cargo.toml", "[workspace]\nmembers = [\"mid/x\"]\n"),
            ("b/mid/Cargo.toml", "[workspace]\nexclude = [\"x\"]\n"),
            ("b/mid/x/Cargo.toml", PACKAGE),
            // One that neither lists nor excludes it is not.
            ("e/Cargo.toml", "[workspace]\nmembers = [\"mid/y\"]\n"),
            ("e/mid/Cargo.toml", "[workspace]\nmembers = []\n"),
            ("e/mid/y/Cargo.toml", PACKAGE),
            // A member written as a path wins over `exclude`; a pattern does not.
            (
                "f/Cargo.toml",
                "[workspace]\nmembers = [\"crates/a\"]\nexclude = [\"crates\"]\n",
            ),
            ("f/crates/a/Cargo.toml", PACKAGE),
            (
                "g/Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"./crates/a\"]\n",
            ),
            ("g/crates/a/Cargo.toml", PACKAGE),
            ("g/crates/b/Cargo.toml", PACKAGE),
            ("h/Cargo.toml", "[workspace]\nmembers = [\"./x/**\"]\n"),
            ("h/x/y/z/Cargo.toml", PACKAGE),
        ]);
        let workspace = |path: &str| workspace_of(&dir.path().join(path)).expect("found").root;
        assert_eq!(workspace("b/mid/x"), root(&dir, "b"));
        assert_eq!(workspace("e/mid/y"), root(&dir, "e/mid/y"));
        assert_eq!(workspace("f/crates/a"), root(&dir, "f"));
        assert_eq!(workspace("g/crates/a"), root(&dir, "g/crates/a"));
        assert_eq!(workspace("g/crates/b"), root(&dir, "g"));
        assert_eq!(workspace("h/x/y/z"), root(&dir, "h"));
    }

    #[test]
    fn members_patterns_expand_to_directories() {
        let dir = tree(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\", \"tools/**/cli\", \"single\", \"../out\"]\n",
            ),
            ("crates/a/Cargo.toml", PACKAGE),
            ("crates/b/src/lib.rs", ""),
            ("crates/file", ""),
            ("tools/cli/Cargo.toml", PACKAGE),
            ("tools/x/y/cli/Cargo.toml", PACKAGE),
            ("single/Cargo.toml", PACKAGE),
        ]);
        let root = root(&dir, "");
        let expanded = expand_members(&root).expect("expanded");
        let relative: Vec<_> = expanded
            .iter()
            .map(|path| path.strip_prefix(&root).expect("under the root"))
            .collect();
        assert_eq!(relative, [
            Path::new("crates/a"),
            Path::new("crates/b"),
            Path::new("single"),
            Path::new("tools/cli"),
            Path::new("tools/x/y/cli"),
        ]);
    }

    #[test]
    fn members_patterns_match_path_components() {
        let matches = |pattern: &str, path: &str| {
            matches_pattern(&parts(Path::new(pattern)), &parts(Path::new(path)))
        };
        assert!(matches("crates/*", "crates/a"));
        assert!(matches("crates/*-sys", "crates/zstd-sys"));
        assert!(matches("tool?", "tools"));
        assert!(matches("ツール?", "ツール2"));
        assert!(matches("**", "a/b"));
        assert!(matches("a/**/z", "a/z"));
        assert!(matches("a/**/z", "a/b/c/z"));
        assert!(!matches("crates/*", "crates/a/b"));
        assert!(!matches("crates/*-sys", "crates/zstd"));
        assert!(!matches("a/**/z", "a/b/y"));
        assert!(!matches("a", "b"));
    }
}
