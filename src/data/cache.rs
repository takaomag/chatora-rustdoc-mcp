//! The disk cache (requirements R-CA-1, R-CA-2 and R-CA-4).
//!
//! The rustdoc JSON fetched from docs.rs is kept as it came, with a digest of its contents, and the
//! crates.io sparse index next to it. Locks across processes (M2) and the GC (R-CA-5) come later.

use std::{
    cmp::Reverse,
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::{self, File, Metadata},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use etcetera::BaseStrategy as _;
use semver::Version;
use sha2::{Digest as _, Sha256};
use tokio::sync::OwnedMutexGuard;

use super::{
    crate_name::CrateName,
    docs_rs::MAX_BYTES,
    rustdoc_json::SUPPORTED,
    target::TargetTriple,
};

/// Environment variable that sets where the cache is (setting `cache.dir`, requirement R-CA-1).
pub const DIR_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_CACHE__DIR";

/// Directory of this server under the OS cache directory.
const APP_DIR: &str = "chatora-rustdoc-mcp";

/// The directory of the JSON for the crate's default target on docs.rs, which no target triple can
/// be, as a triple starts with a letter or a digit.
const DEFAULT_TARGET_DIR: &str = "_default";

/// How old a temporary file must be to count as left behind by a write that did not finish. A
/// write takes seconds, so one that is still going on is never removed (decided in M1).
const STALE_TEMPORARY_AGE: Duration = Duration::from_hours(1);

/// Counts the writes of this process, so that no two temporary files share a name (requirement
/// R-CA-4).
static WRITE_COUNT: AtomicU64 = AtomicU64::new(0);

/// The disk cache. Cloning it is cheap, and the clones share their locks.
#[derive(Debug, Clone)]
pub struct Cache {
    root: Arc<Path>,
    locks: Arc<DirLocks>,
}

/// Locks on paths in the cache, within this process: tasks that fetch or make the same thing take
/// turns, so that the second finds what the first made instead of fetching and parsing it again
/// (requirements R-NW-2 and R-WS-6). Locks across processes come in M2 (R-CA-4).
#[derive(Debug, Default)]
struct DirLocks(parking_lot::Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>);

/// The lock on a path in the cache, held until it is dropped. See [`Cache::lock`].
#[derive(Debug)]
#[must_use = "the lock is released when the guard is dropped"]
pub struct PathGuard(#[expect(dead_code, reason = "held for its drop")] OwnedMutexGuard<()>);

impl DirLocks {
    /// The lock of `path`, made when no task holds or waits for it.
    fn of(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.0.lock();
        if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
            return lock;
        }
        // Forget the locks that nobody holds any more, so that the map does not grow.
        locks.retain(|_, lock| lock.strong_count() > 0);
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(path.into(), Arc::downgrade(&lock));
        lock
    }
}

/// A rustdoc JSON read from the cache.
#[derive(Debug, PartialEq, Eq)]
pub enum CachedJson {
    /// The JSON, whose contents match its digest.
    Found(Box<[u8]>),
    /// The JSON in `format` does not match its digest: it was damaged on the disk (requirement
    /// R-CA-4).
    Corrupt {
        /// Its format version, from its file name.
        format: u32,
    },
    /// No JSON is cached.
    None,
}

/// A file read from the cache with its digest.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verified {
    /// The contents, which match the digest.
    Found(Box<[u8]>),
    /// The contents do not match the digest: they were damaged on the disk.
    Mismatch,
    /// The file or its digest is not there.
    None,
}

/// Why the cache directory could not be decided.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The home directory, under which the default cache directory is, could not be found.
    #[error("the home directory could not be found to place the cache")]
    NoHome(#[source] etcetera::HomeDirError),
}

impl Cache {
    /// The cache at the directory that [`DIR_ENV_VAR`] sets, or at the OS default:
    /// `$XDG_CACHE_HOME/chatora-rustdoc-mcp` (`~/.cache/…` when unset) on Linux and macOS, and
    /// `%LOCALAPPDATA%\chatora-rustdoc-mcp\cache` on Windows (requirement R-CA-1). A relative path
    /// in the variable is logged and replaced by the default, as other settings that are not
    /// valid are: the server's working directory depends on how it was started, so it would
    /// point somewhere unpredictable.
    ///
    /// # Errors
    ///
    /// Returns an error when the home directory, under which the default is, cannot be found.
    pub fn open_default() -> Result<Self, CacheError> {
        let root = root_dir(std::env::var_os(DIR_ENV_VAR), || {
            etcetera::choose_base_strategy().map(|strategy| strategy.cache_dir())
        })?;
        Ok(Self::at(&root))
    }

    /// The cache at `root`.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.into(),
            locks: Arc::default(),
        }
    }

    /// Waits until no other task of this process holds `path`, and holds it: the directory of a
    /// crate's JSON while it is fetched and archived, for example. The path is only a name, which
    /// need not exist. A task must not take a path it holds already.
    pub async fn lock(&self, path: &Path) -> PathGuard {
        PathGuard(self.locks.of(path).lock_owned().await)
    }

    /// [`Self::lock`] for a blocking thread. It must not be called from async code.
    pub fn lock_blocking(&self, path: &Path) -> PathGuard {
        PathGuard(self.locks.of(path).blocking_lock_owned())
    }

    /// The directory of the rustdoc JSON fetched from docs.rs for `name` at `version`, built for
    /// `target`, or for the crate's default target on docs.rs: `docs.rs/<name>/<version>/<target>`
    /// (requirement R-CA-2). The name is the cache key of [`CrateName`], and the default target is
    /// `_default`, which no target triple can be. Each JSON there is `<format_version>.json`,
    /// with its digest in `<format_version>.json.sha256`.
    #[must_use]
    pub fn docs_rs_dir(
        &self,
        name: &CrateName,
        version: &Version,
        target: Option<&TargetTriple>,
    ) -> PathBuf {
        self.root
            .join("docs.rs")
            .join(name.cache_key())
            .join(version.to_string())
            .join(target.map_or(DEFAULT_TARGET_DIR, TargetTriple::as_str))
    }

    /// The cache directory, where nothing configures cargo or rustup: rustc runs there to tell the
    /// host target (requirement R-TG-1).
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.root
    }

    /// Where the `.crate` of `name` at `version` from static.crates.io is kept, with its digest
    /// next to it (requirement R-SC-2).
    #[must_use]
    pub fn crate_file_path(&self, name: &CrateName, version: &Version) -> PathBuf {
        self.root
            .join("static.crates.io")
            .join(name.cache_key())
            .join(version.to_string())
            .join(format!("{name}-{version}.crate"))
    }

    /// Where the crates.io sparse index file at `index_path` (`se/rd/serde`) is kept. The record
    /// of how it was fetched is next to it, with `.meta.json` added (no crate name has a `.`).
    #[must_use]
    pub fn sparse_index_path(&self, index_path: &str) -> PathBuf {
        self.root.join("index.crates.io").join(index_path)
    }

    /// Reads the rustdoc JSON fetched from docs.rs in `dir` ([`Self::docs_rs_dir`]) whose
    /// contents match its digest. Of several formats, the newest that this server reads is taken,
    /// or else the newest (requirement R-FV-6). A JSON without a digest, as one written by an
    /// older server or cut off before its digest, counts as not cached. A symlink and anything
    /// but a regular file count as not cached, too (requirement R-CA-4).
    ///
    /// # Errors
    ///
    /// Returns an error when a file exists but cannot be read, or is larger than the JSON that is
    /// held in memory.
    pub fn read_docs_rs_json(&self, dir: &Path) -> io::Result<CachedJson> {
        let mut formats = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|entry| json_format(&entry.file_name()))
                .collect::<Vec<u32>>(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(CachedJson::None),
            Err(error) => return Err(error),
        };
        // Supported formats first, newest first in each group.
        formats.sort_by_key(|&format| (Reverse(SUPPORTED.contains(&format)), Reverse(format)));
        for format in formats {
            match self.read_verified(&dir.join(format!("{format}.json")))? {
                Verified::Found(json) => return Ok(CachedJson::Found(json)),
                Verified::Mismatch => return Ok(CachedJson::Corrupt { format }),
                Verified::None => {},
            }
        }
        Ok(CachedJson::None)
    }

    /// The format and SHA-256 of the rustdoc JSON in `dir` that [`Self::read_docs_rs_json`] would
    /// read, from the file names and the digest next to the JSON, without reading the JSON: what
    /// an archive made from it is keyed by.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or a digest exists but cannot be read.
    pub fn docs_rs_source(&self, dir: &Path) -> io::Result<Option<(u32, Box<str>)>> {
        let mut formats = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|entry| json_format(&entry.file_name()))
                .collect::<Vec<u32>>(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        formats.sort_by_key(|&format| (Reverse(SUPPORTED.contains(&format)), Reverse(format)));
        for format in formats {
            let path = dir.join(format!("{format}.json"));
            if let Some(digest) = self.read_file(&with_suffix(&path, ".sha256"))? {
                let digest = String::from_utf8_lossy(digest.trim_ascii());
                if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Ok(Some((format, digest.into())));
                }
            }
        }
        Ok(None)
    }

    /// Reads the file at `path` and checks it against the digest next to it, `<path>.sha256`
    /// (requirement R-CA-4). A file without a digest counts as not cached.
    ///
    /// # Errors
    ///
    /// Returns an error when a file exists but cannot be read, or is too large.
    pub(super) fn read_verified(&self, path: &Path) -> io::Result<Verified> {
        let (Some(contents), Some(digest)) = (
            self.read_file(path)?,
            self.read_file(&with_suffix(path, ".sha256"))?,
        ) else {
            return Ok(Verified::None);
        };
        Ok(if digest.trim_ascii() == sha256_hex(&contents).as_bytes() {
            Verified::Found(contents)
        } else {
            Verified::Mismatch
        })
    }

    /// Writes `contents` to `path`, under the cache directory, and then its digest to
    /// `<path>.sha256` (requirement R-CA-4). A reader sees either the old file or the whole new
    /// one; between the two writes, the new file does not match the old digest.
    ///
    /// # Errors
    ///
    /// Returns an error when `path` is outside the cache, or a file cannot be written.
    pub(super) fn write_verified(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.check_inside(path)?;
        write_atomically(path, contents)?;
        write_atomically(
            &with_suffix(path, ".sha256"),
            sha256_hex(contents).as_bytes(),
        )
    }

    /// Fails unless `path` is under the cache directory, so that nothing outside is written or
    /// removed.
    pub(super) fn check_inside(&self, path: &Path) -> io::Result<()> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not in the cache", path.display()),
            ))
        }
    }

    /// Reads the cached file at `path`, which is under the cache directory, or returns `None` if
    /// there is none (requirement R-CA-4).
    ///
    /// A symlink at `path` or between the cache directory and it is not followed, and anything but
    /// a regular file is not read; both count as not cached. The cache directory itself may be a
    /// symlink. On Unix, the file is opened without following a symlink and without waiting on a
    /// FIFO, and must be the file that was checked, so that it cannot be swapped in between.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read, or is larger than the JSON that
    /// is held in memory ([`MAX_BYTES`]).
    pub(super) fn read_file(&self, path: &Path) -> io::Result<Option<Box<[u8]>>> {
        read_regular_file(&self.root, path, MAX_BYTES)
    }

    /// Opens the cached file at `path`, at most `max_len` bytes, with the checks of
    /// [`Self::read_file`], to map it into memory. `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be opened, or is too large.
    pub(super) fn open_file(&self, path: &Path, max_len: u64) -> io::Result<Option<File>> {
        open_regular_file(&self.root, path, max_len)
    }

    /// Writes a rustdoc JSON fetched from docs.rs in `format` to `dir`, replacing the one in the
    /// same format, and then its digest (requirement R-CA-4). A reader sees either the old file or
    /// the whole new one; between the two writes, the new JSON does not match the old digest and
    /// is fetched again.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or a file cannot be written.
    pub fn write_docs_rs_json(&self, dir: &Path, format: u32, json: &[u8]) -> io::Result<()> {
        self.write_verified(&dir.join(format!("{format}.json")), json)
    }

    /// Removes the rustdoc JSON in `format` from `dir`, with its digest.
    ///
    /// # Errors
    ///
    /// Returns an error when a file is there but cannot be removed.
    pub fn remove_docs_rs_json(&self, dir: &Path, format: u32) -> io::Result<()> {
        self.check_inside(dir)?;
        let path = dir.join(format!("{format}.json"));
        for path in [with_suffix(&path, ".sha256"), path] {
            match fs::remove_file(&path) {
                Ok(()) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// [`Cache::read_file`] with the cache directory `root` and a size limit of `max_len` bytes.
fn read_regular_file(root: &Path, path: &Path, max_len: u64) -> io::Result<Option<Box<[u8]>>> {
    let Some(file) = open_regular_file(root, path, max_len)? else {
        return Ok(None);
    };
    let mut contents = Vec::with_capacity(usize::try_from(file.metadata()?.len()).unwrap_or(0));
    file.take(max_len.saturating_add(1))
        .read_to_end(&mut contents)?;
    if u64::try_from(contents.len()).map_or(true, |len| len > max_len) {
        return Err(too_large(path, max_len));
    }
    Ok(Some(contents.into_boxed_slice()))
}

/// Opens the regular file at `path` under the cache directory `root`, at most `max_len` bytes,
/// with the checks of [`Cache::read_file`]. `None` when there is none.
fn open_regular_file(root: &Path, path: &Path, max_len: u64) -> io::Result<Option<File>> {
    if let Some(dirs) = path.strip_prefix(root).ok().and_then(Path::parent) {
        let mut dir = root.to_path_buf();
        for component in dirs.components() {
            dir.push(component);
            match fs::symlink_metadata(&dir) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    tracing::warn!(path = %dir.display(), "ignoring a symlink in the cache");
                    return Ok(None);
                },
                Ok(_) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }
    let checked = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !checked.is_file() {
        tracing::warn!(path = %path.display(), "ignoring what is not a regular file in the cache");
        return Ok(None);
    }
    if checked.len() > max_len {
        return Err(too_large(path, max_len));
    }
    let file = match open_without_following(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound || is_symlink_loop(&error) => {
            tracing::warn!(path = %path.display(), "the cached file changed while it was opened");
            return Ok(None);
        },
        Err(error) => return Err(error),
    };
    let opened = file.metadata()?;
    if !opened.is_file() || !is_same_file(&checked, &opened) {
        tracing::warn!(path = %path.display(), "the cached file changed while it was opened");
        return Ok(None);
    }
    Ok(Some(file))
}

fn too_large(path: &Path, max_len: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "the cached file {} is larger than {max_len} bytes",
            path.display()
        ),
    )
}

/// Opens `path` for reading, failing if it is a symlink, and without waiting for a writer if it
/// is a FIFO.
#[cfg(unix)]
fn open_without_following(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

/// Opens `path` for reading. Windows has no flag that refuses a symlink, so a symlink that
/// replaces the file after it was checked is followed.
#[cfg(not(unix))]
fn open_without_following(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Whether opening failed because the file is a symlink (`O_NOFOLLOW` gives `ELOOP`).
#[cfg(unix)]
fn is_symlink_loop(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
const fn is_symlink_loop(_: &io::Error) -> bool {
    false
}

/// Whether `checked` and `opened` are the same file. On Windows, telling files apart needs an API
/// that is not stable yet, so it is not checked there.
#[cfg(unix)]
fn is_same_file(checked: &Metadata, opened: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    checked.dev() == opened.dev() && checked.ino() == opened.ino()
}

#[cfg(not(unix))]
const fn is_same_file(_: &Metadata, _: &Metadata) -> bool {
    true
}

/// Decides the cache directory from the value of [`DIR_ENV_VAR`] and, when it is unset or empty,
/// from the OS cache directory.
fn root_dir(
    env_value: Option<OsString>,
    os_cache_dir: impl FnOnce() -> Result<PathBuf, etcetera::HomeDirError>,
) -> Result<PathBuf, CacheError> {
    if let Some(value) = env_value.filter(|value| !value.is_empty()) {
        let dir = PathBuf::from(value);
        if dir.is_absolute() {
            return Ok(dir);
        }
        tracing::warn!(
            "{DIR_ENV_VAR} must be an absolute path, but it is {}, so the default is used",
            dir.display()
        );
    }
    let dir = os_cache_dir().map_err(CacheError::NoHome)?.join(APP_DIR);
    // On Windows, the state directory is under %LOCALAPPDATA%\chatora-rustdoc-mcp too
    // (requirement R-DIST-3), so the cache gets a directory of its own.
    Ok(if cfg!(windows) {
        dir.join("cache")
    } else {
        dir
    })
}

/// Writes `contents` to a temporary file next to `path`, flushes it to the disk, and renames it
/// to `path` (requirement R-CA-4).
pub(super) fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let (Some(dir), Some(file_name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::other(format!(
            "not a file path: {}",
            path.display()
        )));
    };
    // The directories are made with the umask, like cargo's (requirement R-CA-1).
    fs::create_dir_all(dir)?;
    // Unique per process and per write, so that writers never share a temporary file.
    let temp = dir.join(format!(
        ".{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        WRITE_COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = write_and_rename(&temp, path, contents);
    if result.is_err()
        && let Err(error) = fs::remove_file(&temp)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %temp.display(), %error, "failed to remove a temporary file");
    }
    if result.is_ok() {
        remove_stale_temporaries(dir);
    }
    result
}

/// Removes the temporary files in `dir` that writes which did not finish left behind: those older
/// than [`STALE_TEMPORARY_AGE`] (requirement R-CA-4, decided in M1). The places that are never
/// written again are left to the GC (requirement R-CA-5).
fn remove_stale_temporaries(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with('.') && name.ends_with(".tmp")) {
            continue;
        }
        // Not followed if it is a symlink.
        let stale = entry.metadata().is_ok_and(|metadata| {
            metadata.is_file()
                && metadata
                    .modified()
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age > STALE_TEMPORARY_AGE)
        });
        if stale && let Err(error) = fs::remove_file(entry.path()) {
            tracing::warn!(path = %entry.path().display(), %error, "failed to remove a stale temporary file");
        }
    }
}

/// The format version in the name of a JSON file of the cache, `<format_version>.json`.
fn json_format(file_name: &OsStr) -> Option<u32> {
    let digits = file_name.to_str()?.strip_suffix(".json")?;
    if digits.bytes().all(|byte| byte.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// The SHA-256 of `contents`, in lowercase hex.
#[must_use]
pub fn sha256_hex(contents: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(contents) {
        for nibble in [byte >> 4, byte & 0xf] {
            hex.extend(char::from_digit(u32::from(nibble), 16));
        }
    }
    hex
}

/// `path` with `suffix` added to its file name.
pub(super) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

fn write_and_rename(temp: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(temp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tasks_that_lock_the_same_path_take_turns() {
        let cache = Cache::at(Path::new("/nonexistent"));
        let path = Path::new("/nonexistent/docs.rs/tokio");
        let held = cache.lock(path).await;
        // Another path is not held.
        drop(cache.lock(Path::new("/nonexistent/docs.rs/serde")).await);
        let (other, path_owned) = (cache.clone(), path.to_path_buf());
        let waiting = tokio::spawn(async move { drop(other.lock(&path_owned).await) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the lock was taken twice");
        drop(held);
        tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .expect("the lock was let go")
            .expect("no panic");
        // The locks that nobody holds are forgotten when another is made.
        assert_eq!(cache.locks.0.lock().len(), 2);
        drop(cache.lock(Path::new("/nonexistent/docs.rs/rand")).await);
        assert_eq!(cache.locks.0.lock().len(), 1);
    }

    fn serde() -> (CrateName, Version) {
        (
            "serde".parse().expect("valid"),
            Version::parse("1.0.228").expect("valid"),
        )
    }

    /// The names of the files in `dir`.
    fn file_names(dir: &Path) -> Vec<OsString> {
        let mut names: Vec<OsString> = fs::read_dir(dir)
            .expect("the directory is readable")
            .map(|entry| entry.expect("the entry is readable").file_name())
            .collect();
        names.sort();
        names
    }

    /// The place of serde's JSON for the default target.
    fn serde_dir(cache: &Cache) -> PathBuf {
        let (name, version) = serde();
        cache.docs_rs_dir(&name, &version, None)
    }

    #[test]
    fn written_json_is_read_back_by_its_digest() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = serde_dir(&cache);
        assert!(place.ends_with("docs.rs/serde/1.0.228/_default"));
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::None
        );
        cache
            .write_docs_rs_json(&place, 61, b"first")
            .expect("writes");
        cache
            .write_docs_rs_json(&place, 61, b"second")
            .expect("overwrites");
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::Found(Box::from(&b"second"[..]))
        );
        // No temporary file is left behind.
        assert_eq!(file_names(&place), ["61.json", "61.json.sha256"]);
    }

    #[test]
    fn a_supported_format_is_read_before_others() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = serde_dir(&cache);
        let newest = SUPPORTED.end().saturating_add(1);
        cache
            .write_docs_rs_json(&place, newest, b"too new")
            .expect("writes");
        cache
            .write_docs_rs_json(&place, 40, b"too old")
            .expect("writes");
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::Found(Box::from(&b"too new"[..])),
            "the newest when none is supported"
        );
        cache
            .write_docs_rs_json(&place, *SUPPORTED.start(), b"read")
            .expect("writes");
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::Found(Box::from(&b"read"[..]))
        );
    }

    #[test]
    fn json_that_does_not_match_its_digest_is_corrupt() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = serde_dir(&cache);
        cache
            .write_docs_rs_json(&place, 61, b"json")
            .expect("writes");
        fs::write(place.join("61.json"), b"jsoN").expect("damaged");
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::Corrupt { format: 61 }
        );
        cache.remove_docs_rs_json(&place, 61).expect("removes");
        assert_eq!(file_names(&place), Vec::<OsString>::new());
        // Without its digest, as from an older server, it is not cached.
        fs::write(place.join("61.json"), b"json").expect("written");
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::None
        );
    }

    #[test]
    fn stale_temporary_files_are_removed_when_writing_next_to_them() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let place = dir.path().join("place");
        fs::create_dir_all(&place).expect("made");
        let stale = place.join(".61.json.1.0.tmp");
        let fresh = place.join(".61.json.2.0.tmp");
        for path in [&stale, &fresh] {
            fs::write(path, b"part").expect("written");
        }
        let two_hours_ago = SystemTime::now()
            .checked_sub(Duration::from_hours(2))
            .expect("a time");
        File::options()
            .write(true)
            .open(&stale)
            .and_then(|file| file.set_modified(two_hours_ago))
            .expect("made old");
        write_atomically(&place.join("61.json"), b"json").expect("writes");
        assert_eq!(file_names(&place), [".61.json.2.0.tmp", "61.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_the_cache_is_not_followed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = serde_dir(&cache);
        cache
            .write_docs_rs_json(&place, 61, b"secret")
            .expect("writes");
        let secret = dir.path().join("secret");
        fs::rename(place.join("61.json"), &secret).expect("moved");
        std::os::unix::fs::symlink(&secret, place.join("61.json")).expect("the symlink is made");
        assert_eq!(
            cache.read_docs_rs_json(&place).expect("reads"),
            CachedJson::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_in_the_cache_is_not_followed() {
        // `docs.rs/serde` points to a directory outside the cache that has the file.
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("cache");
        let cache = Cache::at(&root);
        let elsewhere = dir.path().join("elsewhere");
        let outside = Cache::at(&elsewhere);
        let outside_place = elsewhere.join("1.0.228/_default");
        outside
            .write_docs_rs_json(&outside_place, 61, b"secret")
            .expect("writes");
        fs::create_dir_all(root.join("docs.rs")).expect("the directory is made");
        std::os::unix::fs::symlink(&elsewhere, root.join("docs.rs/serde"))
            .expect("the symlink is made");
        assert_eq!(
            cache.read_docs_rs_json(&serde_dir(&cache)).expect("reads"),
            CachedJson::None
        );
    }

    #[test]
    fn what_is_not_a_regular_file_is_not_read() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let path = serde_dir(&cache).join("61.json");
        fs::create_dir_all(&path).expect("a directory is made in place of the file");
        assert_eq!(cache.read_file(&path).expect("reads"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_cache_is_not_read_and_does_not_block() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = serde_dir(&cache);
        let path = place.join("61.json");
        fs::create_dir_all(&place).expect("the directory is made");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo runs");
        assert!(status.success(), "mkfifo makes the FIFO");
        assert_eq!(cache.read_file(&path).expect("reads"), None);
    }

    #[test]
    fn a_file_over_the_limit_is_an_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("rustdoc.json");
        fs::write(&path, b"12345").expect("the file is written");
        assert_eq!(
            read_regular_file(dir.path(), &path, 5)
                .expect("reads")
                .as_deref(),
            Some(&b"12345"[..])
        );
        assert_eq!(
            read_regular_file(dir.path(), &path, 4)
                .expect_err("too large")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn the_environment_variable_wins_when_it_is_absolute() {
        let absolute = std::env::temp_dir().join("cache");
        let no_home = || Err(etcetera::HomeDirError);
        assert_eq!(
            root_dir(Some(absolute.clone().into()), no_home).expect("absolute"),
            absolute
        );
        // A relative path is replaced by the default.
        let os = std::env::temp_dir().join("os-cache");
        let relative = root_dir(Some("relative/cache".into()), || Ok(os.clone())).expect("default");
        assert!(relative.starts_with(&os), "{}", relative.display());
    }

    #[test]
    fn an_unset_or_empty_environment_variable_means_the_os_default() {
        let os = std::env::temp_dir().join("os-cache");
        let expected = if cfg!(windows) {
            os.join("chatora-rustdoc-mcp").join("cache")
        } else {
            os.join("chatora-rustdoc-mcp")
        };
        for env_value in [None, Some(OsString::new())] {
            assert_eq!(
                root_dir(env_value, || Ok(os.clone())).expect("the default"),
                expected
            );
        }
        assert!(matches!(
            root_dir(None, || Err(etcetera::HomeDirError)),
            Err(CacheError::NoHome(_))
        ));
    }
}
