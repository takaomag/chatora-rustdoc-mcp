//! The disk cache of rustdoc JSON (requirements R-CA-1 and R-CA-4).
//!
//! M0a keeps only the JSON fetched from docs.rs, at a provisional path. The final keys (R-CA-2),
//! content digests and the cleanup of leftover temporary files (R-CA-4), locks across processes
//! (M2), and the GC (R-CA-5) come later.

use std::{
    ffi::OsString,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use etcetera::BaseStrategy as _;
use semver::Version;

use super::crate_name::CrateName;

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

    /// Reads the rustdoc JSON fetched from docs.rs for `name` at `version`, or returns `None` if
    /// it is not cached.
    ///
    /// A symlink in its place is not followed and counts as not cached (requirement R-CA-4).
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read.
    pub fn read_docs_rs_json(
        &self,
        name: &CrateName,
        version: &Version,
    ) -> io::Result<Option<Box<[u8]>>> {
        let path = self.docs_rs_json_path(name, version);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                tracing::warn!(path = %path.display(), "ignoring a symlink in the cache");
                Ok(None)
            },
            Ok(_) => fs::read(&path).map(|json| Some(json.into_boxed_slice())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
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
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
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
    let mut file = fs::File::create_new(temp)?;
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
