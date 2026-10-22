//! The records of `build_external_docs` (requirement R-WS-10, decided in M2e).
//!
//! A record is an index in `ext/v<format>`, by what the query tools look up, of a build published
//! in `local/v1` as the builds of the default project are.
//!
//! The query tools read the records, and start nothing for them (requirement R-MCP-3). While the
//! external tool makes the record of a key, it holds the lock of the key, which the query tools
//! try in shared mode to tell that it is being made (Q-29, decided in M2e, 13).

use std::{
    fs,
    io,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{
    build::{self, Built, Failed},
    cache::{self, Cache},
    process_lock::{self, LockMode, ProcessLock},
};

/// The directory of the records in the cache.
pub const EXT_DIR: &str = "ext";

/// The format of the records, which their path has: a server of another format finds none.
const INDEX_FORMAT: u32 = 1;

/// The index of a record in its directory.
const INDEX: &str = "record.json";

/// The file whose time tells when a record was last read, for the GC (requirement R-CA-5).
const USED: &str = "used";

/// How often a read of a record is noted, at most, for each record in a process.
const NOTE_EVERY: Duration = Duration::from_hours(1);

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
}

impl Key {
    /// The kind, which names the directory of its records.
    const fn kind(&self) -> &'static str {
        match self {
            Self::Release { .. } => "release",
        }
    }

    /// The first 32 hex digits of the SHA-256 of its JSON.
    fn digest(&self) -> String {
        let text = serde_json::to_string(self).unwrap_or_default();
        let digest = cache::sha256_hex(text.as_bytes());
        digest.get(..32).unwrap_or(&digest).to_owned()
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

/// The records noted as read, and when, in this process.
static NOTED: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

/// Notes that the record in `dir` is read now, at most once in [`NOTE_EVERY`] in this process
/// (requirement R-CA-5). A note that cannot be written is logged.
fn note_use(dir: &Path) {
    {
        let mut noted = NOTED.lock();
        if noted
            .iter()
            .any(|(noted, at)| noted == dir && at.elapsed() < NOTE_EVERY)
        {
            return;
        }
        noted.retain(|(noted, _)| noted != dir);
        noted.push((dir.to_path_buf(), Instant::now()));
    }
    if let Err(error) = fs::write(dir.join(USED), b"") {
        tracing::warn!(path = %dir.display(), %error, "the use of an external record could not be noted");
    }
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
        assert!(key.dir(dir.path()).join(USED).is_file());
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
