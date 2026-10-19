//! The disk cache of rustdoc JSON (requirements R-CA-1 and R-CA-4).
//!
//! M0a keeps only the JSON fetched from docs.rs, at a provisional path. The final keys (R-CA-2),
//! content digests and the cleanup of leftover temporary files (R-CA-4), locks across processes
//! (M2), and the GC (R-CA-5) come later.

use std::{
    ffi::OsString,
    fs::{self, File, Metadata},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use etcetera::BaseStrategy as _;
use semver::Version;

use super::{crate_name::CrateName, docs_rs::MAX_BYTES};

/// Environment variable that sets where the cache is (setting `cache.dir`, requirement R-CA-1).
pub const DIR_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_CACHE__DIR";

/// Directory of this server under the OS cache directory.
const APP_DIR: &str = "chatora-rustdoc-mcp";

/// Counts the writes of this process, so that no two temporary files share a name (requirement
/// R-CA-4).
static WRITE_COUNT: AtomicU64 = AtomicU64::new(0);

/// The disk cache. Cloning it is cheap.
#[derive(Debug, Clone)]
pub struct Cache {
    root: Arc<Path>,
}

/// Why the cache directory could not be decided.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The environment variable holds a relative path. The server's working directory depends on
    /// how it was started, so a relative path would point somewhere unpredictable.
    #[error("{DIR_ENV_VAR} must be an absolute path, but it is {0:?}")]
    RelativeDir(Box<Path>),
    /// The home directory, under which the default cache directory is, could not be found.
    #[error("the home directory could not be found to place the cache")]
    NoHome(#[source] etcetera::HomeDirError),
}

impl Cache {
    /// The cache at the directory that [`DIR_ENV_VAR`] sets, or at the OS default:
    /// `$XDG_CACHE_HOME/chatora-rustdoc-mcp` (`~/.cache/…` when unset) on Linux and macOS, and
    /// `%LOCALAPPDATA%\chatora-rustdoc-mcp\cache` on Windows (requirement R-CA-1).
    ///
    /// # Errors
    ///
    /// Returns an error when the environment variable holds a relative path, or when the home
    /// directory cannot be found.
    pub fn open_default() -> Result<Self, CacheError> {
        let root = root_dir(std::env::var_os(DIR_ENV_VAR), || {
            etcetera::choose_base_strategy().map(|strategy| strategy.cache_dir())
        })?;
        Ok(Self::at(&root))
    }

    /// The cache at `root`.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self { root: root.into() }
    }

    /// Where the rustdoc JSON fetched from docs.rs for `name` at `version` is kept.
    #[must_use]
    pub fn docs_rs_json_path(&self, name: &CrateName, version: &Version) -> PathBuf {
        self.root
            .join("docs.rs")
            .join(name.as_str())
            .join(version.to_string())
            .join("rustdoc.json")
    }

    /// Where the crates.io sparse index file at `index_path` (`se/rd/serde`) is kept. The record
    /// of how it was fetched is next to it, with `.meta.json` added (no crate name has a `.`).
    #[must_use]
    pub fn sparse_index_path(&self, index_path: &str) -> PathBuf {
        self.root.join("index.crates.io").join(index_path)
    }

    /// Reads the rustdoc JSON fetched from docs.rs for `name` at `version`, or returns `None` if
    /// it is not cached. A symlink in or on the way to its place, and anything but a regular file,
    /// count as not cached (requirement R-CA-4).
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read, or is larger than the JSON that
    /// is held in memory.
    pub fn read_docs_rs_json(
        &self,
        name: &CrateName,
        version: &Version,
    ) -> io::Result<Option<Box<[u8]>>> {
        self.read_file(&self.docs_rs_json_path(name, version))
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

    /// Writes the rustdoc JSON fetched from docs.rs for `name` at `version`, replacing what was
    /// there. A reader sees either the old file or the whole new one, never a part.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or the file cannot be written.
    pub fn write_docs_rs_json(
        &self,
        name: &CrateName,
        version: &Version,
        json: &[u8],
    ) -> io::Result<()> {
        write_atomically(&self.docs_rs_json_path(name, version), json)
    }
}

/// [`Cache::read_file`] with the cache directory `root` and a size limit of `max_len` bytes.
fn read_regular_file(root: &Path, path: &Path, max_len: u64) -> io::Result<Option<Box<[u8]>>> {
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
    let mut contents = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    file.take(max_len.saturating_add(1))
        .read_to_end(&mut contents)?;
    if u64::try_from(contents.len()).map_or(true, |len| len > max_len) {
        return Err(too_large(path, max_len));
    }
    Ok(Some(contents.into_boxed_slice()))
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
        return if dir.is_absolute() {
            Ok(dir)
        } else {
            Err(CacheError::RelativeDir(dir.into_boxed_path()))
        };
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
    result
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

    #[test]
    fn written_json_is_read_back() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let (name, version) = serde();
        assert_eq!(
            cache.read_docs_rs_json(&name, &version).expect("reads"),
            None
        );
        cache
            .write_docs_rs_json(&name, &version, b"first")
            .expect("writes");
        cache
            .write_docs_rs_json(&name, &version, b"second")
            .expect("overwrites");
        assert_eq!(
            cache
                .read_docs_rs_json(&name, &version)
                .expect("reads")
                .as_deref(),
            Some(&b"second"[..])
        );
        let path = cache.docs_rs_json_path(&name, &version);
        assert!(path.ends_with("docs.rs/serde/1.0.228/rustdoc.json"));
        // No temporary file is left behind.
        let parent = path.parent().expect("the file has a directory");
        assert_eq!(file_names(parent), ["rustdoc.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_in_the_cache_is_not_followed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let (name, version) = serde();
        let path = cache.docs_rs_json_path(&name, &version);
        let parent = path.parent().expect("the file has a directory");
        fs::create_dir_all(parent).expect("the directory is made");
        let secret = dir.path().join("secret");
        fs::write(&secret, b"secret").expect("the file is written");
        std::os::unix::fs::symlink(&secret, &path).expect("the symlink is made");
        assert_eq!(
            cache.read_docs_rs_json(&name, &version).expect("reads"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_in_the_cache_is_not_followed() {
        // `docs.rs/serde` points to a directory outside the cache that has the file.
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("cache");
        let cache = Cache::at(&root);
        let (name, version) = serde();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(elsewhere.join("1.0.228")).expect("the directory is made");
        fs::write(elsewhere.join("1.0.228/rustdoc.json"), b"secret").expect("the file is written");
        fs::create_dir_all(root.join("docs.rs")).expect("the directory is made");
        std::os::unix::fs::symlink(&elsewhere, root.join("docs.rs/serde"))
            .expect("the symlink is made");
        assert_eq!(
            cache.read_docs_rs_json(&name, &version).expect("reads"),
            None
        );
    }

    #[test]
    fn what_is_not_a_regular_file_is_not_read() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let (name, version) = serde();
        let path = cache.docs_rs_json_path(&name, &version);
        fs::create_dir_all(&path).expect("a directory is made in place of the file");
        assert_eq!(cache.read_file(&path).expect("reads"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_in_the_cache_is_not_read_and_does_not_block() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let (name, version) = serde();
        let path = cache.docs_rs_json_path(&name, &version);
        fs::create_dir_all(path.parent().expect("the file has a directory"))
            .expect("the directory is made");
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
        assert!(matches!(
            root_dir(Some("relative/cache".into()), no_home),
            Err(CacheError::RelativeDir(_))
        ));
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
