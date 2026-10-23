//! The records of `build_external_docs` (requirement R-WS-10, decided in M2e).
//!
//! A record is an index in `ext/v<format>`, by what the query tools look up, of a build published
//! in `local/v1` as the builds of the default project are.
//!
//! The query tools read the records, and start nothing for them (requirement R-MCP-3). While the
//! external tool makes the record of a key, it holds the lock of the key, which the query tools
//! try in shared mode to tell that it is being made (Q-29, decided in M2e, 13).
//!
//! A crate of a git repository has two: what the ref asked for resolves to, by the ref
//! ([`Key::GitRef`]), and the build of the commit ([`Key::Git`]), which every ref that resolves to
//! it shares. The commits of a crate are listed too, for the query tools to find one by a short
//! hash (decided in M2e, 8).

use std::{
    fs,
    io,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

use super::{
    build::{self, Built, Failed},
    cache::{self, Cache},
    git_source,
    process_lock::{self, LockMode, ProcessLock},
};

/// The directory of the records in the cache.
pub const EXT_DIR: &str = super::layout::EXTERNAL;

/// The format of the records, which their path has: a server of another format finds none.
const INDEX_FORMAT: u32 = 1;

/// The index of a record in its directory.
const INDEX: &str = "record.json";

/// What the external tool builds, as the query tools look it up (requirement R-WS-10).
///
/// The text of its JSON, whose fields are in this order, names its directory and its lock, so
/// that servers of different versions find each other's: neither may change (a test pins them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Key {
    /// A release of a crate from crates.io that the project does not use: not a dependency, or a
    /// version that its Cargo.lock does not have.
    Release {
        /// The crate, by its cache key (`-` as `_`, in lower case).
        #[serde(rename = "crate")]
        name: Box<str>,
        /// The exact version.
        version: Box<str>,
        /// The target it is documented for.
        target: Box<str>,
        /// Whether it documents the hidden items (requirement R-AD-3), which no build does
        /// before M4.
        hidden: bool,
    },
    /// A crate of a git repository at a commit (decided in M2e's E3).
    Git {
        /// The repository's URL, as its key ([`super::git_source::GitUrl::key`]).
        url: Box<str>,
        /// The full hash of the commit.
        commit: Box<str>,
        /// The crate, by its cache key.
        #[serde(rename = "crate")]
        name: Box<str>,
        /// The target it is documented for.
        target: Box<str>,
        /// Whether it documents the hidden items.
        hidden: bool,
    },
    /// A package of the default project, a member or a dependency, documented for another target
    /// than the project builds for (decided in M2e's E4a).
    Target {
        /// The root of the workspace, as the project's workspace is found.
        workspace: Box<str>,
        /// The package, by its id in `cargo metadata`, which tells its source and version.
        package: Box<str>,
        /// The crate of the package documented, its lib or a bin, as code writes it.
        #[serde(rename = "crate")]
        name: Box<str>,
        /// The target it is documented for.
        target: Box<str>,
        /// Whether it documents the hidden items.
        hidden: bool,
    },
    /// A workspace outside the default project, as `cargo metadata` resolved it, which the query
    /// tools read instead of running cargo there (decided in M2e's E4b).
    Workspace {
        /// The root of the workspace.
        workspace: Box<str>,
    },
    /// What a workspace outside the default project builds for a target, and what was built of it
    /// (decided in M2e's E4b).
    WorkspaceTarget {
        /// The root of the workspace.
        workspace: Box<str>,
        /// The target.
        target: Box<str>,
        /// Whether it documents the hidden items.
        hidden: bool,
    },
    /// What a ref of a git repository resolves to, for a crate in it (decided in M2e's E3).
    GitRef {
        /// The repository's URL, as its key.
        url: Box<str>,
        /// The ref, named in full ([`super::git_source::GitRef::full_name`]).
        #[serde(rename = "ref")]
        reference: Box<str>,
        /// The crate, by its cache key.
        #[serde(rename = "crate")]
        name: Box<str>,
        /// The target it is documented for.
        target: Box<str>,
        /// Whether it documents the hidden items.
        hidden: bool,
    },
}

impl Key {
    /// The kind, which names the directory of its records.
    const fn kind(&self) -> &'static str {
        match self {
            Self::Release { .. } => "release",
            Self::Git { .. } => "git",
            Self::GitRef { .. } => "git_ref",
            Self::Target { .. } => "target",
            Self::Workspace { .. } => "workspace",
            Self::WorkspaceTarget { .. } => "workspace_target",
        }
    }

    /// The first 32 hex digits of the SHA-256 of its JSON.
    fn digest(&self) -> String {
        digest_of(self)
    }

    /// The directory of its record in the cache at `root`:
    /// `ext/v<format>/<kind>/<digest>`.
    #[must_use]
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(EXT_DIR)
            .join(format!("v{INDEX_FORMAT}"))
            .join(self.kind())
            .join(self.digest())
    }

    /// The key of its lock (`process_lock`): `ext:` and its digest.
    #[must_use]
    pub fn lock_key(&self) -> String {
        format!("ext:{}", self.digest())
    }
}

/// The directory of the records of this format in the cache at `root`: `ext/v<format>`.
#[must_use]
pub fn records_dir(root: &Path) -> PathBuf {
    root.join(EXT_DIR).join(format!("v{INDEX_FORMAT}"))
}

/// The first 32 hex digits of the SHA-256 of the JSON of `value`.
pub(super) fn digest_of(value: &impl Serialize) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    let digest = cache::sha256_hex(text.as_bytes());
    digest.get(..32).unwrap_or(&digest).to_owned()
}

/// What a record tells of its build besides it, for the responses (decided in M2e's E2b).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    /// The features that it was built with, as asked for: those of its `[package.metadata.docs.rs]`,
    /// as docs.rs builds it, or its default features when those broke the build (requirements
    /// R-WS-5 and R-FT-3). `None` in a record before M2e's E2b.
    #[serde(default)]
    pub features_requested: Option<Vec<Box<str>>>,
    /// What the responses warn about how it was built, as when those features broke it.
    #[serde(default)]
    pub warnings: Vec<Box<str>>,
    /// Whether it was built in the default project, not in a temporary project: what the build
    /// warns of its `RUSTC_BOOTSTRAP` is then the project's (decided in M2e's E4a).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_project: bool,
}

/// The index of a record: its key, and the build that it is of.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Index {
    /// The key, the same as the one it is found by.
    key: Key,
    /// The directory that the build is published in, relative to the cache, with `/`.
    published: Box<str>,
    /// The SHA-256 of the build's JSON: the build published there later is another's.
    sha256: Box<str>,
    /// When it was recorded, in RFC 3339.
    recorded_at: Box<str>,
    /// What it tells of the build.
    #[serde(default, flatten)]
    told: Told,
}

/// What the record of a key is, as the query tools find it (decided in M2e's E1b).
#[derive(Debug)]
pub enum Recorded {
    /// The build that it records, and what it tells of it.
    Built(Box<Built>, Told),
    /// It is being made: the external tool holds the lock of the key.
    Building,
    /// The external tool failed to make it, as this tells.
    Failed(Box<Failed>),
    /// It is broken, as this tells: the external tool makes it again.
    Broken(String),
    /// There is none.
    Missing,
}

/// The record of `key` in `cache`, without waiting (decided in M2e's E1b).
///
/// It is the build that it records, if it is whole; else whether it is being made, has failed or
/// is broken. It is noted as read when it is found (requirement R-CA-5).
///
/// It reads files, so it is for a blocking thread.
#[must_use]
pub fn read(cache: &Cache, key: &Key) -> Recorded {
    read_so(cache, key, true)
}

/// [`read`] for the holder of the lock of `key` (`making`), which is not told that it is being
/// made: its own lock is. The lock of another key is told as a broken record.
#[must_use]
pub fn read_making(cache: &Cache, key: &Key, making: &Making) -> Recorded {
    if let Err(error) = making.check(key) {
        return Recorded::Broken(error.to_string());
    }
    read_so(cache, key, false)
}

/// [`read`], telling whether another makes it if `others`.
fn read_so(cache: &Cache, key: &Key, others: bool) -> Recorded {
    let dir = key.dir(cache.dir());
    let index = match fs::read(dir.join(INDEX)) {
        Ok(index) => Some(built(cache, key, &index)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(Err(format!("its index cannot be read: {error}"))),
    };
    if let Some(Ok((built, told))) = index {
        note_use(&dir);
        return Recorded::Built(Box::new(built), told);
    }
    if others && being_made(cache, key) {
        return Recorded::Building;
    }
    if let Some(failed) = build::failed(&dir) {
        return Recorded::Failed(Box::new(failed));
    }
    match index {
        Some(Err(why)) => Recorded::Broken(why),
        Some(Ok(_)) | None => Recorded::Missing,
    }
}

/// The build that the index `index` of `key` records, with what it tells of it, or why it is
/// broken.
fn built(cache: &Cache, key: &Key, index: &[u8]) -> Result<(Built, Told), String> {
    let index: Index = serde_json::from_slice(index)
        .map_err(|error| format!("its index is not valid: {error}"))?;
    if index.key != *key {
        return Err("its index is of another key".to_owned());
    }
    let published = published_dir(cache.dir(), &index.published)
        .ok_or_else(|| format!("its index names {}, not a build", index.published))?;
    let built = build::published(&published)
        .ok_or_else(|| format!("the build at {} is not whole", published.display()))?;
    if built.record.sha256 != index.sha256 {
        return Err(format!(
            "the build at {} is not the one recorded",
            published.display()
        ));
    }
    Ok((built, index.told))
}

/// The directory of a published build at `relative` in the cache at `root`, if it is one: under
/// `local`, without going up.
fn published_dir(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    let mut components = relative.components();
    let first = components.next()?;
    (first == Component::Normal(build::LOCAL_DIR.as_ref())
        && components.all(|component| matches!(component, Component::Normal(_))))
    .then(|| root.join(relative))
}

/// Whether there is a record of `key` in `cache`, whole or not, or a failure to make it, or it is
/// being made: not the directory alone, which the external tool may leave when it does not get the
/// lock.
///
/// It looks at a few files, as the check of a cached JSON of docs.rs does on the caller's thread.
#[must_use]
pub fn exists(cache: &Cache, key: &Key) -> bool {
    let dir = key.dir(cache.dir());
    dir.join(INDEX).is_file() || dir.join(build::FAILED).is_file() || being_made(cache, key)
}

/// Whether another holds the lock of `key` in `cache`, as the external tool does while it makes
/// the record.
///
/// A lock that cannot be tried is not. Without the directory of the record, which the external
/// tool makes before it takes the lock, none is: a lookup makes no lock file for each release it
/// looks up. The try holds the lock for a moment, in which [`try_make`] does not get it.
#[must_use]
pub fn being_made(cache: &Cache, key: &Key) -> bool {
    if !key.dir(cache.dir()).is_dir() {
        return false;
    }
    match process_lock::try_lock(cache.dir(), &key.lock_key(), LockMode::Shared) {
        Ok(held) => held.is_none(),
        Err(error) => {
            tracing::warn!(%error, "the lock of an external record could not be tried");
            false
        },
    }
}

/// The lock of a key, held while its record is made.
#[derive(Debug)]
pub struct Making {
    /// The lock.
    _lock: ProcessLock,
    /// The digest of the key.
    digest: String,
}

impl Making {
    /// Fails unless it is the lock of `key`.
    fn check(&self, key: &Key) -> io::Result<()> {
        if self.digest == key.digest() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the lock held is of another key",
            ))
        }
    }
}

/// Takes the lock of `key` in `cache` for making its record, if no other holds it, after making
/// the directory of the record, where the query tools look for it.
///
/// `None` may be a query tool that tries the lock for a moment ([`being_made`]), not only another
/// that makes the record: the caller tries again a little later before it takes it for that.
///
/// It makes a directory, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when the directory or the lock file cannot be made or opened, or the OS fails
/// to lock it.
pub fn try_make(cache: &Cache, key: &Key) -> io::Result<Option<Making>> {
    let dir = key.dir(cache.dir());
    cache.check_inside(&dir)?;
    fs::create_dir_all(&dir)?;
    let lock = process_lock::try_lock(cache.dir(), &key.lock_key(), LockMode::Exclusive)?;
    Ok(lock.map(|lock| Making {
        _lock: lock,
        digest: key.digest(),
    }))
}

/// Records `built`, published in the cache `cache`, as what `key` names, with what the record
/// tells of it, `told`, and forgets a failure of it, under the lock of the key (`making`).
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of `key`, `built` is not in the cache, or the
/// index cannot be written.
pub fn record(
    cache: &Cache,
    key: &Key,
    (built, told): (&Built, &Told),
    making: &Making,
) -> io::Result<()> {
    making.check(key)?;
    let published = built.json.parent().unwrap_or(&built.json);
    let relative = published
        .strip_prefix(cache.dir())
        .ok()
        .map(|relative| {
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|relative| published_dir(cache.dir(), relative).is_some())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a build in the cache", published.display()),
            )
        })?;
    let index = Index {
        key: key.clone(),
        published: relative.into(),
        sha256: built.record.sha256.clone(),
        recorded_at: super::rfc3339(SystemTime::now()).into(),
        told: told.clone(),
    };
    let dir = key.dir(cache.dir());
    let path = dir.join(INDEX);
    cache.check_inside(&path)?;
    cache::write_atomically(&path, &serde_json::to_vec_pretty(&index)?)?;
    match fs::remove_file(dir.join(build::FAILED)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Remembers that making the record of `key` failed, as `failed` tells, under the lock of the
/// key (`making`): the query tools tell it until the external tool makes it again (requirement
/// R-WS-10).
///
/// It writes a file, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of `key`, or it cannot be written.
pub fn remember_failure(
    cache: &Cache,
    key: &Key,
    failed: &Failed,
    making: &Making,
) -> io::Result<()> {
    making.check(key)?;
    let path = key.dir(cache.dir()).join(build::FAILED);
    cache.check_inside(&path)?;
    cache::write_atomically(&path, &serde_json::to_vec_pretty(failed)?)
}

/// The file of what a ref resolves to, in the directory of its [`Key::GitRef`].
const RESOLVED: &str = "resolved.json";

/// What a ref of a git repository resolved to, for a crate in it (decided in M2e's E3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    /// The full hash of the commit.
    pub commit: Box<str>,
    /// The name of the crate's package, as the repository spells it.
    pub package: Box<str>,
    /// Its version at the commit.
    pub version: Box<str>,
    /// When it was resolved, in RFC 3339: what a branch or a tag resolves to may move since
    /// (decided in M2e, 7).
    #[serde(rename = "resolved_at")]
    pub at: Box<str>,
    /// The commit that it resolved to before, when it moved: the query tools give that while the
    /// new one is built (decided in M2e's E3b-2, 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<str>>,
}

/// The file of what a ref resolved to: its key and that.
#[derive(Debug, Serialize, Deserialize)]
struct ResolvedFile {
    /// The key, the same as the one it is found by.
    key: Key,
    /// What the ref resolved to.
    #[serde(flatten)]
    resolved: Resolved,
}

/// What the record of a ref is, as [`resolution`] finds it.
#[derive(Debug)]
pub enum Resolution {
    /// What it resolved to.
    Resolved(Resolved),
    /// It is being resolved, or the commit built: the external tool holds the lock of the key.
    Building,
    /// The external tool failed to resolve it, as this tells.
    Failed(Box<Failed>),
    /// It is broken, as this tells: the external tool resolves it again.
    Broken(String),
    /// There is none.
    Missing,
}

/// What the ref of `key`, a [`Key::GitRef`], resolved to in `cache`, without waiting.
///
/// Else it is whether it is being resolved, failed or is broken (decided in M2e's E3). It is
/// noted as read when it is found (requirement R-CA-5).
///
/// It reads files, so it is for a blocking thread.
#[must_use]
pub fn resolution(cache: &Cache, key: &Key) -> Resolution {
    resolution_so(cache, key, true)
}

/// [`resolution`] for the holder of the lock of `key` (`making`), as [`read_making`].
#[must_use]
pub fn resolution_making(cache: &Cache, key: &Key, making: &Making) -> Resolution {
    if let Err(error) = making.check(key) {
        return Resolution::Broken(error.to_string());
    }
    resolution_so(cache, key, false)
}

/// [`resolution`], telling whether another resolves it if `others`.
fn resolution_so(cache: &Cache, key: &Key, others: bool) -> Resolution {
    let dir = key.dir(cache.dir());
    let read = match fs::read(dir.join(RESOLVED)) {
        Ok(text) => Some(
            serde_json::from_slice::<ResolvedFile>(&text)
                .map_err(|error| format!("what it resolved to is not valid: {error}"))
                .and_then(|file| {
                    if file.key != *key {
                        Err("what it resolved to is of another key".to_owned())
                    } else if !git_source::is_commit(&file.resolved.commit) {
                        Err("what it resolved to is not a commit".to_owned())
                    } else {
                        Ok(file.resolved)
                    }
                }),
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(Err(format!("what it resolved to cannot be read: {error}"))),
    };
    if let Some(Ok(resolved)) = read {
        note_use(&dir);
        return Resolution::Resolved(resolved);
    }
    if others && being_made(cache, key) {
        return Resolution::Building;
    }
    if let Some(failed) = build::failed(&dir) {
        return Resolution::Failed(Box::new(failed));
    }
    match read {
        Some(Err(why)) => Resolution::Broken(why),
        Some(Ok(_)) | None => Resolution::Missing,
    }
}

/// Records what the ref of `key`, a [`Key::GitRef`], resolved to, `resolved`, and forgets a
/// failure to resolve it, under the lock of the key (`making`); and lists the commit among those
/// of the crate ([`commits`]).
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of `key`, `key` is not of a ref, the commit is
/// not a full hash, or a file cannot be written.
pub fn record_resolution(
    cache: &Cache,
    key: &Key,
    resolved: &Resolved,
    making: &Making,
) -> io::Result<()> {
    making.check(key)?;
    let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidInput, why.to_owned());
    let Some(commits) = commits_dir(cache.dir(), key) else {
        return Err(invalid("the key is not of a ref"));
    };
    // The commit names a file.
    if !git_source::is_commit(&resolved.commit) {
        return Err(invalid("what the ref resolved to is not a commit"));
    }
    let file = ResolvedFile {
        key: key.clone(),
        resolved: resolved.clone(),
    };
    let dir = key.dir(cache.dir());
    let path = dir.join(RESOLVED);
    cache.check_inside(&path)?;
    cache::write_atomically(&path, &serde_json::to_vec_pretty(&file)?)?;
    let listed = commits.join(&*resolved.commit);
    cache.check_inside(&listed)?;
    fs::create_dir_all(&commits)?;
    fs::write(listed, b"")?;
    match fs::remove_file(dir.join(build::FAILED)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Remembers that resolving the ref of `key`, a [`Key::GitRef`], failed, as `failed` tells, under
/// the lock of the key (`making`).
///
/// What it resolved to before is forgotten, unless the failure may pass ([`Failed::transient`]):
/// the query tools then tell the failure of a branch deleted since, not its old commit.
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of `key`, or a file cannot be removed or
/// written.
pub fn remember_resolution_failure(
    cache: &Cache,
    key: &Key,
    failed: &Failed,
    making: &Making,
) -> io::Result<()> {
    making.check(key)?;
    if !failed.transient {
        match fs::remove_file(key.dir(cache.dir()).join(RESOLVED)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            Ok(()) | Err(_) => {},
        }
    }
    remember_failure(cache, key, failed, making)
}

/// The repository and the crate of a [`Key::Git`] or a [`Key::GitRef`], which key its commits.
#[derive(Serialize)]
struct GitCrate<'a> {
    /// The repository's URL, as its key.
    url: &'a str,
    /// The crate, by its cache key.
    #[serde(rename = "crate")]
    name: &'a str,
    /// The target.
    target: &'a str,
    /// Whether the hidden items are documented.
    hidden: bool,
}

/// The directory that lists the commits resolved for the crate of `key`, a [`Key::Git`] or a
/// [`Key::GitRef`], in the cache at `root`: `ext/v<format>/git_commits/<digest>`, with a file
/// for each.
fn commits_dir(root: &Path, key: &Key) -> Option<PathBuf> {
    let of = match key {
        Key::Release { .. }
        | Key::Target { .. }
        | Key::Workspace { .. }
        | Key::WorkspaceTarget { .. } => return None,
        Key::Git {
            url,
            name,
            target,
            hidden,
            ..
        }
        | Key::GitRef {
            url,
            name,
            target,
            hidden,
            ..
        } => GitCrate {
            url,
            name,
            target,
            hidden: *hidden,
        },
    };
    Some(
        root.join(EXT_DIR)
            .join(format!("v{INDEX_FORMAT}"))
            .join("git_commits")
            .join(digest_of(&of)),
    )
}

/// The commits that refs resolved to for the crate of `key`, a [`Key::Git`] or a
/// [`Key::GitRef`], in `cache`, sorted: those whose records the query tools find by a short hash
/// (decided in M2e, 8).
///
/// It reads a directory, so it is for a blocking thread.
#[must_use]
pub fn commits(cache: &Cache, key: &Key) -> Vec<Box<str>> {
    let Some(dir) = commits_dir(cache.dir(), key) else {
        return Vec::new();
    };
    let mut commits: Vec<Box<str>> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| git_source::is_commit(name))
        .map(String::into_boxed_str)
        .collect();
    commits.sort_unstable();
    commits
}

/// A record in a file of the directory of its key, as the query tools find it (decided in M2e's
/// E4b).
#[derive(Debug)]
pub enum Read<T> {
    /// What it records.
    Found(T),
    /// It is being made: the external tool holds the lock of the key.
    Building,
    /// The external tool failed to make it, as this tells.
    Failed(Box<Failed>),
    /// It is broken, as this tells: the external tool makes it again.
    Broken(String),
    /// There is none.
    Missing,
}

/// A record with the key that it is found by.
#[derive(Debug, Serialize, Deserialize)]
struct Filed<T> {
    /// The key.
    key: Key,
    /// What it records: not flattened, which `serde` does for no `u128`, as the time of inputs.
    record: T,
}

/// The record of `key` in its file `file`, without waiting, as [`read`] reads a record of a build.
///
/// The holder of the lock of the key, `making`, is not told that it is being made. It is noted as
/// read when it is found (requirement R-CA-5).
///
/// It reads files, so it is for a blocking thread.
#[must_use]
pub fn read_file<T: serde::de::DeserializeOwned>(
    cache: &Cache,
    (key, file): (&Key, &str),
    making: Option<&Making>,
) -> Read<T> {
    if let Some(making) = making
        && let Err(error) = making.check(key)
    {
        return Read::Broken(error.to_string());
    }
    let dir = key.dir(cache.dir());
    let read = match fs::read(dir.join(file)) {
        Ok(text) => Some(
            serde_json::from_slice::<Filed<T>>(&text)
                .map_err(|error| format!("its {file} is not valid: {error}"))
                .and_then(|filed| {
                    if filed.key == *key {
                        Ok(filed.record)
                    } else {
                        Err(format!("its {file} is of another key"))
                    }
                }),
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(Err(format!("its {file} cannot be read: {error}"))),
    };
    let read = match read {
        Some(Ok(value)) => {
            note_use(&dir);
            return Read::Found(value);
        },
        read => read,
    };
    if making.is_none() && being_made(cache, key) {
        return Read::Building;
    }
    if let Some(failed) = build::failed(&dir) {
        return Read::Failed(Box::new(failed));
    }
    match read {
        Some(Err(why)) => Read::Broken(why),
        Some(Ok(_)) | None => Read::Missing,
    }
}

/// Writes `value` as the record of `key` in its file `file`, under the lock of the key
/// (`making`), and forgets a failure to make it.
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when `making` is not the lock of `key`, or a file cannot be written.
pub fn write_file<T: Serialize>(
    cache: &Cache,
    (key, file): (&Key, &str),
    value: &T,
    making: &Making,
) -> io::Result<()> {
    making.check(key)?;
    let filed = Filed {
        key: key.clone(),
        record: value,
    };
    let dir = key.dir(cache.dir());
    let path = dir.join(file);
    cache.check_inside(&path)?;
    cache::write_atomically(&path, &serde_json::to_vec_pretty(&filed)?)?;
    match fs::remove_file(dir.join(build::FAILED)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Notes that the record of `key` in `cache` is read now, as when it was read before and kept
/// (requirement R-CA-5), at most once in a while, as the reads of the files do.
///
/// It may write a file, so it is for a blocking thread.
pub fn note_read(cache: &Cache, key: &Key) {
    note_use(&key.dir(cache.dir()));
}

/// Notes that the record in `dir` is read now, at most once in a while in this process
/// (requirement R-CA-5). A note that cannot be written is logged.
fn note_use(dir: &Path) {
    super::used::note(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokio_key() -> Key {
        Key::Release {
            name: "tokio".into(),
            version: "1.47.1".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            hidden: false,
        }
    }

    /// Servers of different versions find each other's records and locks.
    #[test]
    fn the_names_of_a_key_do_not_change() {
        let key = tokio_key();
        assert_eq!(
            serde_json::to_string(&key).expect("JSON"),
            r#"{"kind":"release","crate":"tokio","version":"1.47.1","target":"x86_64-unknown-linux-gnu","hidden":false}"#
        );
        assert_eq!(
            key.dir(Path::new("/c")),
            Path::new("/c/ext/v1/release").join(key.digest())
        );
        assert_eq!(key.digest().len(), 32);
        assert_eq!(key.lock_key(), format!("ext:{}", key.digest()));
    }

    /// Publishes in the cache at `root` a build of `json`, as a build here does, and returns it.
    fn publish(root: &Path, json: &[u8]) -> Built {
        let dir = root.join("local/v1/w/p/x86_64-unknown-linux-gnu/lib");
        fs::create_dir_all(&dir).expect("made");
        let sha256 = cache::sha256_hex(json);
        fs::write(dir.join("57.json"), json).expect("written");
        fs::write(dir.join("57.json.sha256"), &sha256).expect("written");
        let record = serde_json::json!({
            "package": "registry+https://github.com/rust-lang/crates.io-index#tokio@1.47.1",
            "crate_name": "tokio",
            "crate_version": "1.47.1",
            "format_version": 57,
            "built_at": "2026-10-22T00:00:00Z",
            "bootstrap_all": false,
            "cfg_docsrs": true,
            "sha256": sha256,
            "private": false,
        });
        fs::write(dir.join("record.json"), record.to_string()).expect("written");
        build::published(&dir).expect("published")
    }

    #[test]
    fn a_record_is_found_until_its_build_is_another() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let key = tokio_key();
        assert!(matches!(read(&cache, &key), Recorded::Missing));
        let built = publish(dir.path(), b"{}");
        let making = try_make(&cache, &key).expect("locked").expect("not held");
        record(&cache, &key, (&built, &Told::default()), &making).expect("recorded");
        // Being made again.
        assert!(matches!(read(&cache, &key), Recorded::Built(..)));
        drop(making);
        let Recorded::Built(found, _) = read(&cache, &key) else {
            panic!("not found");
        };
        assert_eq!(found.record.sha256, built.record.sha256);
        assert!(key.dir(dir.path()).join(crate::data::used::USED).is_file());
        // Another build published there since.
        publish(dir.path(), b"{ }");
        assert!(matches!(read(&cache, &key), Recorded::Broken(_)));
        // Of another key.
        let other = Key::Release {
            name: "tokio".into(),
            version: "1.47.1".into(),
            target: "x86_64-pc-windows-msvc".into(),
            hidden: false,
        };
        fs::create_dir_all(other.dir(dir.path())).expect("made");
        fs::copy(
            key.dir(dir.path()).join(INDEX),
            other.dir(dir.path()).join(INDEX),
        )
        .expect("copied");
        let Recorded::Broken(why) = read(&cache, &other) else {
            panic!("not broken");
        };
        assert!(why.contains("another key"), "{why}");
    }

    #[test]
    fn a_directory_left_without_the_lock_is_no_record() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let key = tokio_key();
        let making = try_make(&cache, &key).expect("locked").expect("not held");
        assert!(exists(&cache, &key));
        // Another's lock does not record it.
        let other = Key::Release {
            name: "tokio".into(),
            version: "1.0.0".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            hidden: false,
        };
        record(
            &cache,
            &other,
            (&publish(dir.path(), b"{}"), &Told::default()),
            &making,
        )
        .expect_err("the lock of another key");
        drop(making);
        assert!(key.dir(dir.path()).is_dir());
        assert!(!exists(&cache, &key));
        assert!(matches!(read(&cache, &key), Recorded::Missing));
    }

    #[test]
    fn a_record_being_made_failed_or_broken_is_told_so() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let key = tokio_key();
        let making = try_make(&cache, &key).expect("locked").expect("not held");
        assert!(matches!(read(&cache, &key), Recorded::Building));
        let failed = Failed::now(
            None,
            None,
            false,
            serde_json::json!({ "code": "build_failed" }),
        );
        remember_failure(&cache, &key, &failed, &making).expect("remembered");
        // Its holder is told what is there, not its own lock.
        assert!(matches!(read(&cache, &key), Recorded::Building));
        assert!(matches!(
            read_making(&cache, &key, &making),
            Recorded::Failed(_)
        ));
        drop(making);
        let Recorded::Failed(found) = read(&cache, &key) else {
            panic!("not failed");
        };
        assert_eq!(found.body["code"], "build_failed");
        fs::write(key.dir(dir.path()).join(INDEX), b"not JSON").expect("written");
        // The failure is told before the broken index, which the next build writes again.
        assert!(matches!(read(&cache, &key), Recorded::Failed(_)));
        fs::remove_file(key.dir(dir.path()).join(build::FAILED)).expect("removed");
        assert!(matches!(read(&cache, &key), Recorded::Broken(_)));
        // A record forgets the failure.
        let making = try_make(&cache, &key).expect("locked").expect("not held");
        remember_failure(&cache, &key, &failed, &making).expect("remembered");
        record(
            &cache,
            &key,
            (&publish(dir.path(), b"{}"), &Told::default()),
            &making,
        )
        .expect("recorded");
        drop(making);
        assert!(!key.dir(dir.path()).join(build::FAILED).exists());
        assert!(matches!(read(&cache, &key), Recorded::Built(..)));
    }

    /// The index of a server before M2e's E2b, without what it tells, is read.
    #[test]
    fn an_index_without_what_it_tells_is_read() {
        let index: Index = serde_json::from_value(serde_json::json!({
            "key": tokio_key(),
            "published": "local/v1/w/p/t/lib",
            "sha256": "0",
            "recorded_at": "2026-10-22T00:00:00Z",
        }))
        .expect("an index");
        assert_eq!(index.told, Told::default());
        let told = Told {
            features_requested: Some(vec!["full".into()]),
            warnings: vec!["w".into()],
            in_project: true,
        };
        let text = serde_json::to_value(Index {
            told: told.clone(),
            ..index
        })
        .expect("JSON");
        assert_eq!(text["features_requested"], serde_json::json!(["full"]));
        let read: Index = serde_json::from_value(text).expect("an index");
        assert_eq!(read.told, told);
    }

    const COMMIT: &str = "9a41be9f3db7140981e276cfc1fdb993ed8ae07f";

    fn git_keys(target: &str) -> (Key, Key) {
        let commit = Key::Git {
            url: "https://example.invalid/dep".into(),
            commit: COMMIT.into(),
            name: "git_dep".into(),
            target: target.into(),
            hidden: false,
        };
        let reference = Key::GitRef {
            url: "https://example.invalid/dep".into(),
            reference: "refs/heads/main".into(),
            name: "git_dep".into(),
            target: target.into(),
            hidden: false,
        };
        (commit, reference)
    }

    #[test]
    fn the_names_of_a_key_of_git_do_not_change() {
        let (commit, reference) = git_keys("x86_64-unknown-linux-gnu");
        assert_eq!(
            serde_json::to_string(&commit).expect("JSON"),
            format!(
                r#"{{"kind":"git","url":"https://example.invalid/dep","commit":"{COMMIT}","crate":"git_dep","target":"x86_64-unknown-linux-gnu","hidden":false}}"#
            )
        );
        assert_eq!(
            serde_json::to_string(&reference).expect("JSON"),
            r#"{"kind":"git_ref","url":"https://example.invalid/dep","ref":"refs/heads/main","crate":"git_dep","target":"x86_64-unknown-linux-gnu","hidden":false}"#
        );
        assert!(commit.dir(Path::new("/c")).starts_with("/c/ext/v1/git"));
        assert!(
            reference
                .dir(Path::new("/c"))
                .starts_with("/c/ext/v1/git_ref")
        );
        // The commit and the ref list the commits of one crate.
        assert_eq!(
            commits_dir(Path::new("/c"), &commit),
            commits_dir(Path::new("/c"), &reference)
        );
        assert!(commits_dir(Path::new("/c"), &tokio_key()).is_none());
    }

    #[test]
    fn the_names_of_a_key_of_another_target_do_not_change() {
        let key = Key::Target {
            workspace: "/p".into(),
            package: "path+file:///p#app@0.1.0".into(),
            name: "app".into(),
            target: "wasm32-wasip1".into(),
            hidden: false,
        };
        assert_eq!(
            serde_json::to_string(&key).expect("JSON"),
            r#"{"kind":"target","workspace":"/p","package":"path+file:///p#app@0.1.0","crate":"app","target":"wasm32-wasip1","hidden":false}"#
        );
        assert!(key.dir(Path::new("/c")).starts_with("/c/ext/v1/target"));
        assert!(commits_dir(Path::new("/c"), &key).is_none());
    }

    #[test]
    fn what_a_ref_resolved_to_is_recorded_with_its_commit_listed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let (commit, reference) = git_keys("x86_64-unknown-linux-gnu");
        assert!(matches!(
            resolution(&cache, &reference),
            Resolution::Missing
        ));
        let making = try_make(&cache, &reference)
            .expect("locked")
            .expect("not held");
        assert!(matches!(
            resolution(&cache, &reference),
            Resolution::Building
        ));
        let failed = Failed::now(
            None,
            None,
            false,
            serde_json::json!({ "code": "crate_not_found" }),
        );
        remember_failure(&cache, &reference, &failed, &making).expect("remembered");
        assert!(matches!(
            resolution_making(&cache, &reference, &making),
            Resolution::Failed(_)
        ));
        let resolved = Resolved {
            commit: COMMIT.into(),
            package: "git-dep".into(),
            version: "0.3.0".into(),
            at: "2026-10-22T00:00:00Z".into(),
            previous: None,
        };
        // Not of the lock of another key, nor a commit that is not a full hash.
        record_resolution(&cache, &commit, &resolved, &making).expect_err("another key");
        let short = Resolved {
            commit: "9a41be9f".into(),
            ..resolved.clone()
        };
        record_resolution(&cache, &reference, &short, &making).expect_err("not a commit");
        record_resolution(&cache, &reference, &resolved, &making).expect("recorded");
        drop(making);
        let Resolution::Resolved(found) = resolution(&cache, &reference) else {
            panic!("not resolved");
        };
        assert_eq!(found, resolved);
        assert!(!reference.dir(dir.path()).join(build::FAILED).exists());
        assert_eq!(commits(&cache, &commit), vec![Box::from(COMMIT)]);
        // Of another target, none.
        let (other, _) = git_keys("x86_64-pc-windows-msvc");
        assert_eq!(commits(&cache, &other), Vec::<Box<str>>::new());
        // Of another key.
        let (_, moved) = git_keys("x86_64-pc-windows-msvc");
        fs::create_dir_all(moved.dir(dir.path())).expect("made");
        fs::copy(
            reference.dir(dir.path()).join(RESOLVED),
            moved.dir(dir.path()).join(RESOLVED),
        )
        .expect("copied");
        let Resolution::Broken(why) = resolution(&cache, &moved) else {
            panic!("not broken");
        };
        assert!(why.contains("another key"), "{why}");

        // A failure that may pass keeps what it resolved to; one that does not, as a branch
        // deleted since, forgets it.
        let making = try_make(&cache, &reference)
            .expect("locked")
            .expect("not held");
        let transient = Failed::now(None, None, true, serde_json::json!({ "code": "x" }));
        remember_resolution_failure(&cache, &reference, &transient, &making).expect("remembered");
        assert!(matches!(
            resolution_making(&cache, &reference, &making),
            Resolution::Resolved(_)
        ));
        let lasting = Failed::now(
            None,
            None,
            false,
            serde_json::json!({ "code": "fetch_failed" }),
        );
        remember_resolution_failure(&cache, &reference, &lasting, &making).expect("remembered");
        drop(making);
        assert!(matches!(
            resolution(&cache, &reference),
            Resolution::Failed(_)
        ));
        // The commit stays listed, for its record.
        assert_eq!(commits(&cache, &commit), vec![Box::from(COMMIT)]);
    }

    #[test]
    fn only_a_build_under_local_is_named() {
        let root = Path::new("/c");
        assert_eq!(
            published_dir(root, "local/v1/a/b/t/lib"),
            Some(PathBuf::from("/c/local/v1/a/b/t/lib"))
        );
        for relative in ["", "docs.rs/x", "local/../std", "/local/v1", "../local/v1"] {
            assert_eq!(published_dir(root, relative), None, "{relative}");
        }
    }
}
