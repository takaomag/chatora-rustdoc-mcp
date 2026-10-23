//! Data layer: fetches rustdoc JSON, normalizes its format version, and caches it.

pub mod archive;
#[cfg(feature = "local")]
pub mod build;
#[cfg(feature = "local")]
pub mod build_scope;
pub mod cache;
#[cfg(feature = "local")]
pub mod cargo;
#[cfg(feature = "local")]
pub mod cargo_config;
#[cfg(feature = "local")]
pub mod cargo_lock;
#[cfg(feature = "local")]
mod cargo_tree;
#[cfg(feature = "local")]
pub mod child;
pub mod crate_file;
pub mod crate_name;
pub mod dist;
pub mod docs_rs;
#[cfg(feature = "local")]
pub mod external;
#[cfg(feature = "local")]
pub mod external_workspace;
#[cfg(feature = "local")]
pub mod features;
pub mod fetches;
pub mod flight;
pub mod format_version;
#[cfg(feature = "local")]
pub mod freshness;
pub mod gc;
pub mod git_source;
mod http;
pub mod inventory;
pub mod layout;
#[cfg(feature = "local")]
pub mod local_std;
#[cfg(feature = "local")]
pub mod metadata;
#[cfg(feature = "local")]
pub mod owner;
#[cfg(feature = "local")]
pub mod packages;
#[cfg(feature = "local")]
pub mod process_id;
pub mod process_lock;
#[cfg(feature = "local")]
pub mod project;
pub mod rustdoc_json;
pub mod sparse_index;
pub mod std_json;
pub mod target;
#[cfg(feature = "local")]
pub mod temp_project;
#[cfg(test)]
pub(crate) mod test_http;
#[cfg(feature = "local")]
mod tool;
#[cfg(feature = "local")]
pub mod toolchain;
pub mod used;
pub mod version;
pub mod waits;

use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use semver::Version;
use serde::{Deserialize, Serialize};

pub use self::http::Network;
use self::{
    cache::{Cache, CachedJson, with_suffix},
    crate_name::CrateName,
    docs_rs::{DocsRs, FetchError},
    format_version::Header,
    target::TargetTriple,
};

/// How long it is remembered that docs.rs has no JSON for a release (requirement R-ERR-3, about
/// an hour). docs.rs may build it later (requirement R-DS-3). A JSON that cannot be parsed and
/// one with no format this server reads are remembered as long (decided in M1): docs.rs changes
/// them only when it builds the release again.
const MISSING_TTL: Duration = Duration::from_hours(1);

/// How long a transient failure to fetch a JSON is remembered (requirement R-ERR-3, about
/// 30 seconds), so that every lookup does not wait for the same failure again. A JSON that the
/// cache keeps damaging is remembered as long (decided in M1).
const TRANSIENT_TTL: Duration = Duration::from_secs(30);

/// The name of the file that remembers a failure, in the directory of the JSON.
const MISS_FILE: &str = "miss.json";

/// A rustdoc JSON, and the warnings about how it was loaded.
#[derive(Debug)]
pub struct Loaded {
    /// The JSON.
    pub json: Box<[u8]>,
    /// What the response warns about, such as a cached JSON that was damaged and fetched again
    /// (requirement R-CA-4).
    pub warnings: Vec<Box<str>>,
}

/// Why rustdoc JSON could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// It was not cached, and fetching it from docs.rs failed.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// docs.rs returned the JSON of another version than the one asked for, which happens with
    /// build metadata: `2.1.0` may return `2.1.2+zstd.1.5.7`. It is neither used nor cached
    /// (requirement R-VR-4).
    #[error("docs.rs returned the rustdoc JSON of version {found}")]
    WrongVersion {
        /// The `crate_version` of the JSON.
        found: Box<str>,
    },
    /// docs.rs has rustdoc JSON of the release, but not for this target: since 2026-05-01 it
    /// builds most new releases for one target only. Documentation for another target is not
    /// shown in its place (requirement R-TG-1).
    #[error("docs.rs has no rustdoc JSON of this release for {target}")]
    NoTargetJson {
        /// The target asked for.
        target: TargetTriple,
    },
    /// Fetching the same JSON failed transiently a moment ago (requirement R-ERR-3).
    #[error("fetching this rustdoc JSON failed a moment ago; try again in {} seconds", retry_after.as_secs())]
    RecentFailure {
        /// How long until it is tried again.
        retry_after: Duration,
    },
    /// The JSON could not be parsed, even after it was fetched again, a while ago (requirement
    /// R-CA-4).
    #[error("this rustdoc JSON could not be parsed even when fetched again; it is tried again in {} seconds", retry_after.as_secs())]
    Unparsable {
        /// How long until it is fetched again.
        retry_after: Duration,
    },
    /// The cached JSON did not match its digest when it was read back after being written again:
    /// the disk may be failing (`corrupt`, requirement R-CA-4).
    #[error(
        "the cached rustdoc JSON was damaged again after it was fetched again; check the disk of the cache"
    )]
    Corrupt {
        /// How long until it is fetched again.
        retry_after: Duration,
    },
    /// The cached file exists but could not be read, or the blocking task that uses the cache was
    /// cancelled (when the runtime shuts down).
    #[error("the cache of rustdoc JSON could not be used")]
    Cache(#[source] io::Error),
}

/// A remembered failure, in [`MISS_FILE`] next to where the JSON would be.
#[derive(Debug, Serialize, Deserialize)]
struct Miss {
    /// What failed.
    kind: MissKind,
    /// When it failed, in seconds since the Unix epoch.
    at: u64,
    /// The version of the server that remembered it.
    server: Box<str>,
}

/// What failed to be fetched or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MissKind {
    /// docs.rs has no JSON for the release.
    Missing,
    /// docs.rs has JSON for the release, but not for the target asked for.
    NoTarget,
    /// Fetching failed transiently.
    Transient,
    /// The JSON could not be parsed, even after it was fetched again.
    Unparsable,
    /// docs.rs has the release in no format that this server reads.
    NoSupportedFormat,
    /// The JSON was damaged on the disk again after it was fetched again.
    Corrupt,
}

impl MissKind {
    /// How long it is remembered.
    const fn ttl(self) -> Duration {
        match self {
            Self::Missing | Self::NoTarget | Self::Unparsable | Self::NoSupportedFormat => {
                MISSING_TTL
            },
            Self::Transient | Self::Corrupt => TRANSIENT_TTL,
        }
    }

    /// Whether only the server that remembered it trusts it: another version may parse the JSON
    /// or read its format.
    const fn is_about_this_server(self) -> bool {
        matches!(self, Self::Unparsable | Self::NoSupportedFormat)
    }
}

impl Miss {
    /// How long it is still remembered, or `None` if it no longer is.
    fn remaining(&self) -> Option<Duration> {
        if self.kind.is_about_this_server() && *self.server != *env!("CARGO_PKG_VERSION") {
            return None;
        }
        let ttl = self.kind.ttl();
        let age = SystemTime::now()
            .duration_since(system_time(self.at))
            .unwrap_or(ttl);
        Some(ttl.saturating_sub(age)).filter(|remaining| !remaining.is_zero())
    }
}

/// Returns the rustdoc JSON of `name` at exactly `version`, from the cache or else from docs.rs.
///
/// It is for `target`, or else for the crate's default target on docs.rs. If docs.rs has the
/// release but not for `target`, that is told apart (requirement R-TG-1).
///
/// Before it is cached, its `format_version` must be readable and its `crate_version` must be
/// `version` (requirements R-CA-4 and R-VR-4). A body whose `format_version` cannot be read (such
/// as the error page of a proxy) is returned but not cached, for the caller to report as
/// unparsable. A missing JSON is remembered for an hour and a transient failure for 30 seconds
/// (requirement R-ERR-3). A failure to write the cache is logged and does not fail the load.
///
/// A cached JSON that does not match its digest is removed and fetched again, once, with a
/// warning; if the new one does not match when read back, the cache is reported corrupt
/// (requirement R-CA-4).
///
/// # Errors
///
/// Returns an error when the cached file cannot be read, the fetch fails or failed a moment ago,
/// docs.rs returns another version, or the cache damages the JSON again.
pub async fn docs_rs_json(
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
) -> Result<Loaded, LoadError> {
    // The cache is read and written on a blocking thread, so that tens of megabytes of disk I/O do
    // not hold up the async workers.
    let dir = cache.docs_rs_dir(name, version, target);
    let (cached, miss) = {
        let (cache, dir) = (cache.clone(), dir.clone());
        blocking(move || {
            let cached = cache.read_docs_rs_json(&dir)?;
            if let CachedJson::Corrupt { format } = cached {
                tracing::warn!(dir = %dir.display(), format, "a cached rustdoc JSON does not match its digest; fetching it again");
                cache.remove_docs_rs_json(&dir, format)?;
            }
            let miss = match cached {
                CachedJson::None => read_miss(&cache, &dir)?,
                CachedJson::Found(_) | CachedJson::Corrupt { .. } => None,
            };
            Ok((cached, miss))
        })
        .await
        .map_err(LoadError::Cache)?
    };
    let recovering = match cached {
        CachedJson::Found(json) => {
            return Ok(Loaded {
                json,
                warnings: Vec::new(),
            });
        },
        CachedJson::Corrupt { .. } => true,
        CachedJson::None => false,
    };
    if let Some((kind, retry_after)) = miss {
        return Err(match kind {
            MissKind::NoTarget => target.map_or(LoadError::Fetch(FetchError::NotFound), |target| {
                LoadError::NoTargetJson {
                    target: target.clone(),
                }
            }),
            MissKind::Missing | MissKind::NoSupportedFormat => {
                LoadError::Fetch(FetchError::NotFound)
            },
            MissKind::Transient => LoadError::RecentFailure { retry_after },
            MissKind::Unparsable => LoadError::Unparsable { retry_after },
            MissKind::Corrupt => LoadError::Corrupt { retry_after },
        });
    }
    match docs_rs
        .fetch_rustdoc_json(name, version, target, None)
        .await
    {
        Ok(json) => {
            let json = store(cache, &dir, version, json, recovering).await?;
            let warnings = if recovering {
                vec![Box::from(
                    "the cached rustdoc JSON was damaged on the disk, so it was fetched again",
                )]
            } else {
                Vec::new()
            };
            Ok(Loaded { json, warnings })
        },
        Err(FetchError::NotFound) if target.is_some() => {
            let error = match docs_rs.has_rustdoc_json(name, version).await {
                Ok(true) => {
                    remember_miss(&dir, MissKind::NoTarget).await;
                    target.map_or(LoadError::Fetch(FetchError::NotFound), |target| {
                        LoadError::NoTargetJson {
                            target: target.clone(),
                        }
                    })
                },
                Ok(false) => {
                    remember_miss(&dir, MissKind::Missing).await;
                    LoadError::Fetch(FetchError::NotFound)
                },
                // Whether the release has JSON at all is not known: nothing is remembered.
                Err(_) => LoadError::Fetch(FetchError::NotFound),
            };
            Err(error)
        },
        Err(error) => {
            let kind = match &error {
                FetchError::NotFound | FetchError::BadRequest => Some(MissKind::Missing),
                FetchError::Transient { .. }
                | FetchError::UnexpectedStatus { .. }
                | FetchError::Network(_)
                | FetchError::Certificate(_) => Some(MissKind::Transient),
                // A broken body may be fetched whole next time, and offline nothing was asked.
                FetchError::TooLarge { .. }
                | FetchError::TooManyLayers
                | FetchError::Corrupt(_)
                | FetchError::Offline => None,
            };
            if let Some(kind) = kind {
                remember_miss(&dir, kind).await;
            }
            Err(LoadError::Fetch(error))
        },
    }
}

/// The error of a load of the rustdoc JSON of `name` at `version` for `target`, while docs.rs is
/// remembered to have none of it, or none for `target` (requirements R-DS-3 and R-TG-1).
///
/// For a lookup that builds the release here instead, before its `.crate` is fetched (decided in
/// M2's D7). A failure of another kind is not told.
pub async fn remembered_no_json(
    cache: &Cache,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
) -> Option<LoadError> {
    let dir = cache.docs_rs_dir(name, version, target);
    let cache = cache.clone();
    let (kind, _) = blocking(move || read_miss(&cache, &dir)).await.ok()??;
    match kind {
        MissKind::NoTarget => Some(target.map_or(
            LoadError::Fetch(FetchError::NotFound),
            |target| LoadError::NoTargetJson {
                target: target.clone(),
            },
        )),
        MissKind::Missing | MissKind::NoSupportedFormat => {
            Some(LoadError::Fetch(FetchError::NotFound))
        },
        MissKind::Transient | MissKind::Unparsable | MissKind::Corrupt => None,
    }
}

/// Looks for the rustdoc JSON of `name` at `version` in another format that this server reads,
/// when the one docs.rs returned by default, in format `found`, could not be read.
///
/// The formats are tried newest first (requirement R-DS-2, step 3). docs.rs has a format only if
/// it built the release in it, so a 404 is expected.
///
/// Every format is tried, whatever fails on the way (decided in M1): a JSON that is found is
/// cached next to the default one, under its own format, and returned. If none is found, `None`
/// is returned, unless a transient failure may have hidden one, which is then returned as the
/// error. That none is found is remembered for an hour by this version of the server (R-FV-6,
/// decided in M1).
///
/// # Errors
///
/// Returns the last transient failure when no format was found, or an error of the cache.
pub async fn docs_rs_json_in_another_format(
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
    found: u32,
) -> Result<Option<Box<[u8]>>, LoadError> {
    let dir = cache.docs_rs_dir(name, version, target);
    let miss = {
        let (cache, dir) = (cache.clone(), dir.clone());
        blocking(move || read_miss(&cache, &dir))
            .await
            .map_err(LoadError::Cache)?
    };
    if let Some((MissKind::NoSupportedFormat, _)) = miss {
        return Ok(None);
    }
    let mut transient = None;
    for format in rustdoc_json::supported_formats()
        .rev()
        .filter(|&format| format != found)
    {
        let error = match docs_rs
            .fetch_rustdoc_json(name, version, target, Some(format))
            .await
        {
            Ok(json) => match store(cache, &dir, version, json, false).await {
                Ok(json) if format_version::peek(&json) == Some(format) => {
                    return Ok(Some(json));
                },
                Ok(_) | Err(LoadError::WrongVersion { .. }) => continue,
                Err(error) => return Err(error),
            },
            Err(error) => error,
        };
        match error {
            FetchError::NotFound
            | FetchError::BadRequest
            | FetchError::TooLarge { .. }
            | FetchError::TooManyLayers
            | FetchError::Corrupt(_) => {},
            // Offline, no format was looked for, so none is known to be missing.
            FetchError::Transient { .. }
            | FetchError::UnexpectedStatus { .. }
            | FetchError::Network(_)
            | FetchError::Certificate(_)
            | FetchError::Offline => transient = Some(error),
        }
    }
    if let Some(error) = transient {
        return Err(LoadError::Fetch(error));
    }
    remember_miss(&dir, MissKind::NoSupportedFormat).await;
    Ok(None)
}

/// Fetches the rustdoc JSON of `name` at `version` again, when `json` could not be parsed.
///
/// Its digest matched, so docs.rs may have served it broken (requirement R-CA-4). The cached one
/// is removed first. If docs.rs returns the same contents, that is remembered for
/// an hour by this version of the server (decided in M1); if the new one cannot be parsed
/// either, the caller calls [`remember_unparsable`].
///
/// # Errors
///
/// Returns an error when the fetch fails, docs.rs returns the same JSON or another version, or
/// the cache cannot be used.
pub async fn refetch_unparsable(
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
    json: &[u8],
) -> Result<Loaded, LoadError> {
    let dir = cache.docs_rs_dir(name, version, target);
    forget(cache, &dir, json).await?;
    let fresh = docs_rs
        .fetch_rustdoc_json(name, version, target, None)
        .await?;
    if *fresh == *json {
        remember_miss(&dir, MissKind::Unparsable).await;
        return Err(LoadError::Unparsable {
            retry_after: MISSING_TTL,
        });
    }
    let json = store(cache, &dir, version, fresh, false).await?;
    Ok(Loaded {
        json,
        warnings: vec![Box::from(
            "the cached rustdoc JSON could not be parsed, so it was fetched again",
        )],
    })
}

/// Removes the cached `json` of `name` at `version`, which could not be parsed even after it was
/// fetched again, and remembers that for an hour by this version of the server (decided in M1).
///
/// # Errors
///
/// Returns an error when the cache cannot be used.
pub async fn remember_unparsable(
    cache: &Cache,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
    json: &[u8],
) -> Result<(), LoadError> {
    let dir = cache.docs_rs_dir(name, version, target);
    forget(cache, &dir, json).await?;
    remember_miss(&dir, MissKind::Unparsable).await;
    Ok(())
}

/// Removes `json` from the cache directory `dir`, by the format it declares.
async fn forget(cache: &Cache, dir: &Path, json: &[u8]) -> Result<(), LoadError> {
    let Some(format) = format_version::peek(json) else {
        return Ok(());
    };
    let (cache, dir) = (cache.clone(), dir.to_path_buf());
    blocking(move || cache.remove_docs_rs_json(&dir, format))
        .await
        .map_err(LoadError::Cache)
}

/// Checks a fetched `json` and caches it in `dir` when it is rustdoc JSON of `version`, and
/// forgets a remembered failure. A JSON of another version is an error; one whose header cannot
/// be read is returned without being cached. With `read_back`, the cached file is read back: if
/// it does not match, the cache is damaging what it is given, which is remembered for 30 seconds
/// and returned as an error (requirement R-CA-4).
async fn store(
    cache: &Cache,
    dir: &Path,
    version: &Version,
    json: Box<[u8]>,
    read_back: bool,
) -> Result<Box<[u8]>, LoadError> {
    let (cache, dir_path, version) = (cache.clone(), dir.to_path_buf(), version.clone());
    // The JSON moves to the blocking thread and comes back, so that tens of megabytes are not
    // copied. Reading the header goes through the whole JSON.
    let (json, outcome) = blocking(move || {
        let outcome = match format_version::read_header(&json) {
            Ok(Header {
                crate_version: Some(found),
                ..
            }) if Version::parse(&found).ok().as_ref() != Some(&version) => {
                Stored::WrongVersion(found)
            },
            Ok(Header { format_version, .. }) => {
                remove_if_there(&dir_path.join(MISS_FILE));
                match cache.write_docs_rs_json(&dir_path, format_version, &json) {
                    Ok(()) if read_back => match cache.read_docs_rs_json(&dir_path)? {
                        CachedJson::Found(again) if *again == *json => Stored::Written(Ok(())),
                        CachedJson::Found(_) | CachedJson::Corrupt { .. } | CachedJson::None => {
                            Stored::Damaged
                        },
                    },
                    written => Stored::Written(written),
                }
            },
            Err(_) => Stored::NotRustdocJson,
        };
        Ok((json, outcome))
    })
    .await
    .map_err(LoadError::Cache)?;
    match outcome {
        Stored::Written(Ok(())) => {},
        Stored::Written(Err(error)) => {
            tracing::warn!(%error, "failed to write the rustdoc JSON to the cache");
        },
        Stored::NotRustdocJson => {
            tracing::warn!("not caching a body from docs.rs that is not rustdoc JSON");
        },
        Stored::WrongVersion(found) => return Err(LoadError::WrongVersion { found }),
        Stored::Damaged => {
            tracing::error!(dir = %dir.display(), "the cache damaged a rustdoc JSON again after it was fetched again");
            remember_miss(dir, MissKind::Corrupt).await;
            return Err(LoadError::Corrupt {
                retry_after: TRANSIENT_TTL,
            });
        },
    }
    Ok(json)
}

/// What [`store`] did with a fetched JSON.
enum Stored {
    Written(io::Result<()>),
    NotRustdocJson,
    WrongVersion(Box<str>),
    /// Read back, it did not match what was written.
    Damaged,
}

/// The failure remembered in `dir` that still holds, and how long it does.
fn read_miss(cache: &Cache, dir: &Path) -> io::Result<Option<(MissKind, Duration)>> {
    read_miss_file(cache, &dir.join(MISS_FILE))
}

/// The failure remembered in the file `path`, and how long it is still remembered.
fn read_miss_file(cache: &Cache, path: &Path) -> io::Result<Option<(MissKind, Duration)>> {
    Ok(cache
        .read_file(path)?
        .and_then(|miss| serde_json::from_slice::<Miss>(&miss).ok())
        .and_then(|miss| Some((miss.kind, miss.remaining()?))))
}

/// Remembers in `dir` that fetching or reading failed. A failure to write it is logged.
async fn remember_miss(dir: &Path, kind: MissKind) {
    remember_miss_file(&dir.join(MISS_FILE), kind).await;
}

/// Remembers in the file `path` that `kind` of failure happened now.
async fn remember_miss_file(path: &Path, kind: MissKind) {
    let miss = Miss {
        kind,
        at: unix_seconds(SystemTime::now()),
        server: env!("CARGO_PKG_VERSION").into(),
    };
    let path = path.to_owned();
    let written = blocking(move || {
        let json = serde_json::to_vec(&miss).map_err(io::Error::other)?;
        cache::write_atomically(&path, &json)
    })
    .await;
    if let Err(error) = written {
        tracing::warn!(%error, "failed to remember a failure in the cache");
    }
}

/// The file next to `path` in the cache that remembers a transient failure to fetch it.
fn failure_file(path: &Path) -> PathBuf {
    with_suffix(path, ".miss.json")
}

/// How long a transient failure to fetch what goes at `path` in the cache is still remembered,
/// if one is, so that it is not fetched again meanwhile: every call, and every process that
/// shares the cache, would wait for the same failure in turn (requirement R-ERR-3, about 30
/// seconds).
async fn recent_failure(cache: &Cache, path: &Path) -> Option<Duration> {
    let (cache, file) = (cache.clone(), failure_file(path));
    match blocking(move || read_miss_file(&cache, &file)).await {
        Ok(Some((MissKind::Transient, remaining))) => Some(remaining),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(%error, "a failure remembered in the cache could not be read");
            None
        },
    }
}

/// Remembers that fetching what goes at `path` in the cache failed transiently, for
/// [`recent_failure`].
async fn remember_failure(path: &Path) {
    remember_miss_file(&failure_file(path), MissKind::Transient).await;
}

/// Forgets the failures remembered in `dir`, and with `below`, in the directories below it too.
///
/// They are `miss.json` and the files next to what failed to be fetched. The argument `refresh`
/// asks for it (§6.4, decided in M2's D5).
pub async fn forget_failures(dir: PathBuf, below: bool) {
    let forgotten = blocking(move || {
        let mut dirs = vec![dir];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    if below {
                        dirs.push(path);
                    }
                } else if name == MISS_FILE || name.ends_with(".miss.json") {
                    tracing::info!(path = %path.display(), "a failure is forgotten, as asked");
                    remove_if_there(&path);
                }
            }
        }
        Ok(())
    })
    .await;
    if let Err(error) = forgotten {
        tracing::debug!(%error, "the failures could not be forgotten");
    }
}

/// Forgets that fetching what goes at `path` failed a moment ago, as the argument `refresh` asks
/// (§6.4, decided in M2's D5).
pub fn forget_failure_of(path: &Path) {
    let failure = failure_file(path);
    if failure.is_file() {
        tracing::info!(path = %failure.display(), "a failure is forgotten, as asked");
        remove_if_there(&failure);
    }
}

/// Removes the file at `path`, if there is one. A failure is logged.
fn remove_if_there(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {},
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => tracing::warn!(path = %path.display(), %error, "failed to remove a file"),
    }
}

/// Reads a semantic version from a string, for `#[serde(deserialize_with)]`.
#[cfg(feature = "local")]
fn deserialize_version<'de, D>(deserializer: D) -> Result<Version, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
    text.parse().map_err(serde::de::Error::custom)
}

/// `$CARGO_HOME`, or `~/.cargo` when it is unset or empty, as cargo finds it. `None` without a
/// home directory.
#[must_use]
pub fn cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| etcetera::home_dir().ok().map(|home| home.join(".cargo")))
}

/// `time` in RFC 3339 and UTC, to the second: `2026-10-20T01:02:03Z`. A time before the Unix
/// epoch is the epoch.
#[must_use]
pub fn rfc3339(time: SystemTime) -> String {
    const DAY: u64 = 24 * 60 * 60;
    let seconds = unix_seconds(time);
    let (days, of_day) = (
        seconds.checked_div(DAY).unwrap_or_default(),
        seconds.checked_rem(DAY).unwrap_or_default(),
    );
    let (year, month, day) = i64::try_from(days)
        .ok()
        .and_then(std_json::civil_from_days)
        .unwrap_or((1970, 1, 1));
    let (hour, minute, second) = (
        of_day.checked_div(3600).unwrap_or_default(),
        of_day
            .checked_rem(3600)
            .unwrap_or_default()
            .checked_div(60)
            .unwrap_or_default(),
        of_day.checked_rem(60).unwrap_or_default(),
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The time that `text` writes as [`rfc3339`] does, `2026-10-20T01:02:03Z`; `None` for another
/// form, or a time before the Unix epoch.
#[must_use]
pub fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let bytes = text.as_bytes();
    let separated = bytes.len() == 20
        && [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'Z'),
        ]
        .iter()
        .all(|&(at, separator)| bytes.get(at) == Some(&separator));
    if !separated {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> {
        let digits = text.get(from..to)?;
        digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = std_json::days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour.checked_mul(3600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?;
    UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(seconds).ok()?))
}

/// Seconds since the Unix epoch, or 0 for a time before it.
fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The time `seconds` after the Unix epoch.
fn system_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH)
}

/// Runs `work` on a blocking thread. A panic in it is resumed here.
async fn blocking<T, F>(work: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    crate::task::spawn_blocking(work)
        .await
        .map_err(|error| io::Error::other(crate::task::resume_if_panic(error)))?
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn the_failures_in_a_directory_are_forgotten_and_nothing_else() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let below = dir.path().join("below");
        std::fs::create_dir_all(&below).expect("made");
        for path in [
            dir.path().join(MISS_FILE),
            dir.path().join("61.json.miss.json"),
            dir.path().join("61.json"),
            below.join(MISS_FILE),
        ] {
            std::fs::write(path, "{}").expect("written");
        }
        forget_failures(dir.path().to_path_buf(), false).await;
        assert!(!dir.path().join(MISS_FILE).exists());
        assert!(!dir.path().join("61.json.miss.json").exists());
        assert!(dir.path().join("61.json").exists());
        assert!(below.join(MISS_FILE).exists(), "not below");
        forget_failures(dir.path().to_path_buf(), true).await;
        assert!(!below.join(MISS_FILE).exists());
        // And one next to a file.
        let file = dir.path().join("a.crate");
        std::fs::write(failure_file(&file), "{}").expect("written");
        forget_failure_of(&file);
        assert!(!failure_file(&file).exists());
    }

    fn serde() -> (CrateName, Version) {
        (
            "serde".parse().expect("valid"),
            Version::parse("1.0.228").expect("valid"),
        )
    }

    fn ok(body: &[u8]) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body.to_vec(),
        )
    }

    fn status(line: &str) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 {line}\r\ncontent-length: 0\r\nretry-after: 0\r\nconnection: close\r\n\r\n"
            ),
            Vec::new(),
        )
    }

    /// A cache and a docs.rs that answers `responses` in turn, and the request heads.
    fn setup(
        responses: Vec<(String, Vec<u8>)>,
    ) -> (Cache, DocsRs, mpsc::Receiver<String>, tempfile::TempDir) {
        let (base_url, requests) = test_http::serve_sequence(responses);
        let dir = tempfile::tempdir().expect("a temporary directory");
        (
            Cache::at(dir.path()),
            DocsRs::for_local_test(&base_url),
            requests,
            dir,
        )
    }

    /// The JSON of serde cached for the default target, if any.
    fn cached(cache: &Cache) -> Option<Box<[u8]>> {
        let (name, version) = serde();
        match cache
            .read_docs_rs_json(&cache.docs_rs_dir(&name, &version, None))
            .expect("reads")
        {
            CachedJson::Found(json) => Some(json),
            CachedJson::Corrupt { .. } | CachedJson::None => None,
        }
    }

    /// Loads the JSON of serde from a server that answers `body`, with an empty cache, and reads
    /// back what was cached.
    async fn load(body: &[u8]) -> (Result<Box<[u8]>, LoadError>, Option<Box<[u8]>>) {
        let (cache, docs_rs, _requests, _dir) = setup(vec![ok(body)]);
        let (name, version) = serde();
        let json = docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .map(|loaded| loaded.json);
        (json, cached(&cache))
    }

    const SERDE_61: &[u8] = br#"{"root":0,"crate_version":"1.0.228","format_version":61}"#;

    #[tokio::test]
    async fn rustdoc_json_of_the_version_is_cached() {
        let (loaded, cached) = load(SERDE_61).await;
        assert_eq!(loaded.expect("loads").as_ref(), SERDE_61);
        assert_eq!(cached.as_deref(), Some(SERDE_61));
    }

    #[tokio::test]
    async fn the_json_is_kept_by_its_format_with_a_digest() {
        let (cache, docs_rs, _requests, dir) = setup(vec![ok(SERDE_61)]);
        // Names that crates.io counts as the same share the place.
        let name: CrateName = "Serde".parse().expect("valid");
        let version = Version::new(1, 0, 228);
        docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("loads");
        let place = dir.path().join("docs.rs/serde/1.0.228/_default");
        let mut names: Vec<String> = std::fs::read_dir(&place)
            .expect("the directory is there")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(names, ["61.json", "61.json.sha256"]);
        assert_eq!(
            std::fs::read_to_string(place.join("61.json.sha256")).expect("the digest"),
            cache::sha256_hex(SERDE_61)
        );
    }

    #[tokio::test]
    async fn a_damaged_cache_is_fetched_again_once() {
        let (cache, docs_rs, requests, _dir) = setup(vec![ok(SERDE_61), ok(SERDE_61)]);
        let (name, version) = serde();
        docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("loads");
        // A byte changes on the disk; the digest stays.
        let file = cache.docs_rs_dir(&name, &version, None).join("61.json");
        std::fs::write(&file, SERDE_61.to_ascii_uppercase()).expect("damaged");
        let loaded = docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("fetched again");
        assert_eq!(&*loaded.json, SERDE_61);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        let again = docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("cached");
        assert_eq!(again.warnings, Vec::<Box<str>>::new());
        assert_eq!(requests.iter().count(), 2);
    }

    #[tokio::test]
    async fn json_that_cannot_be_parsed_is_fetched_again_once_and_remembered() {
        let (cache, docs_rs, requests, _dir) = setup(vec![ok(SERDE_61), ok(SERDE_61)]);
        let (name, version) = serde();
        let loaded = docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("loads");
        // Say the parser fails on it: docs.rs returns the same.
        let refetched =
            refetch_unparsable(&cache, &docs_rs, &name, &version, None, &loaded.json).await;
        assert!(
            matches!(refetched, Err(LoadError::Unparsable { .. })),
            "{refetched:?}"
        );
        let remembered = docs_rs_json(&cache, &docs_rs, &name, &version, None).await;
        assert!(
            matches!(remembered, Err(LoadError::Unparsable { .. })),
            "{remembered:?}"
        );
        assert_eq!(requests.iter().count(), 2);
    }

    #[tokio::test]
    async fn json_that_cannot_be_parsed_is_replaced_by_a_new_build() {
        let rebuilt = br#"{"root":1,"crate_version":"1.0.228","format_version":61}"#;
        let (cache, docs_rs, _requests, _dir) = setup(vec![ok(SERDE_61), ok(rebuilt)]);
        let (name, version) = serde();
        let loaded = docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("loads");
        let refetched = refetch_unparsable(&cache, &docs_rs, &name, &version, None, &loaded.json)
            .await
            .expect("a new build");
        assert_eq!(&*refetched.json, rebuilt);
        assert_eq!(refetched.warnings.len(), 1);
        assert_eq!(cached(&cache).as_deref(), Some(&rebuilt[..]));
    }

    #[tokio::test]
    async fn what_another_server_version_remembered_about_parsing_is_not_trusted() {
        let (cache, docs_rs, _requests, _dir) = setup(vec![ok(SERDE_61)]);
        let (name, version) = serde();
        let dir = cache.docs_rs_dir(&name, &version, None);
        let miss = Miss {
            kind: MissKind::Unparsable,
            at: unix_seconds(SystemTime::now()),
            server: "0.0.0".into(),
        };
        cache::write_atomically(
            &dir.join(MISS_FILE),
            &serde_json::to_vec(&miss).expect("serializes"),
        )
        .expect("written");
        docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("fetched, as this version may parse it");
    }

    #[tokio::test]
    async fn a_body_that_is_not_rustdoc_json_is_returned_but_not_cached() {
        let page = b"<html>blocked by the proxy</html>";
        let (loaded, cached) = load(page).await;
        assert_eq!(loaded.expect("loads").as_ref(), page);
        assert_eq!(cached, None);
    }

    #[tokio::test]
    async fn json_of_another_version_is_neither_used_nor_cached() {
        // docs.rs answers `2.1.0` of zstd-sys with 2.1.2+zstd.1.5.7 (requirement R-VR-4).
        let json = br#"{"root":0,"crate_version":"1.0.229","format_version":61}"#;
        let (loaded, cached) = load(json).await;
        assert!(
            matches!(&loaded, Err(LoadError::WrongVersion { found }) if &**found == "1.0.229"),
            "{loaded:?}"
        );
        assert_eq!(cached, None);
    }

    #[tokio::test]
    async fn a_missing_json_is_remembered() {
        let (cache, docs_rs, requests, _dir) = setup(vec![status("404 Not Found")]);
        let (name, version) = serde();
        for _ in 0..2 {
            assert!(matches!(
                docs_rs_json(&cache, &docs_rs, &name, &version, None).await,
                Err(LoadError::Fetch(FetchError::NotFound))
            ));
        }
        // The second load did not ask docs.rs again.
        assert_eq!(requests.iter().count(), 1);
    }

    /// Offline, docs.rs is not asked: the cache still gives what it has, and what it lacks is not
    /// remembered as missing (requirement R-CA-7).
    #[tokio::test]
    async fn offline_only_the_cache_is_read_and_nothing_is_remembered() {
        let (cache, docs_rs, requests, _dir) = setup(vec![ok(SERDE_61)]);
        let (name, version) = serde();
        docs_rs_json(&cache, &docs_rs, &name, &version, None)
            .await
            .expect("loads");
        let offline = docs_rs.offline();
        docs_rs_json(&cache, &offline, &name, &version, None)
            .await
            .expect("from the cache");
        let other = Version::new(1, 0, 227);
        for _ in 0..2 {
            assert!(matches!(
                docs_rs_json(&cache, &offline, &name, &other, None).await,
                Err(LoadError::Fetch(FetchError::Offline))
            ));
        }
        assert_eq!(requests.iter().count(), 1);
    }

    #[test]
    fn times_are_written_in_rfc_3339() {
        let at = |seconds| rfc3339(system_time(seconds));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(at(1_709_164_799), "2024-02-28T23:59:59Z");
        assert_eq!(at(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn times_written_in_rfc_3339_are_read_back() {
        for seconds in [0, 1_700_000_000, 1_709_164_799, 1_709_164_800] {
            let time = system_time(seconds);
            assert_eq!(parse_rfc3339(&rfc3339(time)), Some(time), "{seconds}");
        }
        for not in [
            "",
            "2024-02-29",
            "2024-02-29T00:00:00",
            "2024-02-29T00:00:00+09:00",
            "2024-13-01T00:00:00Z",
            "2024-02-29T24:00:00Z",
            "+024-02-29T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "２０２４-02-29T00:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(not), None, "{not}");
        }
    }

    #[tokio::test]
    async fn a_transient_failure_is_remembered_for_a_moment() {
        let (cache, docs_rs, requests, _dir) = setup(vec![status("503 Service Unavailable"); 3]);
        let (name, version) = serde();
        assert!(matches!(
            docs_rs_json(&cache, &docs_rs, &name, &version, None).await,
            Err(LoadError::Fetch(FetchError::Transient { .. }))
        ));
        let again = docs_rs_json(&cache, &docs_rs, &name, &version, None).await;
        assert!(
            matches!(&again, Err(LoadError::RecentFailure { retry_after }) if *retry_after <= TRANSIENT_TTL),
            "{again:?}"
        );
        assert_eq!(requests.iter().count(), 3);
    }

    #[tokio::test]
    async fn a_release_without_json_for_the_target_is_told_apart_and_remembered() {
        // 404 for the target, 200 to `HEAD` for the default target.
        let (cache, docs_rs, requests, _dir) =
            setup(vec![status("404 Not Found"), status("200 OK")]);
        let (name, version) = serde();
        let target: TargetTriple = "aarch64-apple-darwin".parse().expect("a target");
        for _ in 0..2 {
            let loaded = docs_rs_json(&cache, &docs_rs, &name, &version, Some(&target)).await;
            assert!(
                matches!(&loaded, Err(LoadError::NoTargetJson { target: t }) if *t == target),
                "{loaded:?}"
            );
        }
        assert_eq!(requests.iter().count(), 2);
    }

    #[tokio::test]
    async fn a_release_without_any_json_is_not_found_for_a_target_either() {
        let (cache, docs_rs, _requests, _dir) =
            setup(vec![status("404 Not Found"), status("404 Not Found")]);
        let (name, version) = serde();
        let target: TargetTriple = "aarch64-apple-darwin".parse().expect("a target");
        assert!(matches!(
            docs_rs_json(&cache, &docs_rs, &name, &version, Some(&target)).await,
            Err(LoadError::Fetch(FetchError::NotFound))
        ));
    }

    #[tokio::test]
    async fn another_format_is_found_by_its_number_and_cached() {
        let json = br#"{"crate_version":"1.0.228","format_version":60}"#;
        let (cache, docs_rs, requests, _dir) = setup(vec![status("404 Not Found"), ok(json)]);
        let (name, version) = serde();
        let found = docs_rs_json_in_another_format(&cache, &docs_rs, &name, &version, None, 40)
            .await
            .expect("no error");
        assert_eq!(found.as_deref(), Some(&json[..]));
        let paths: Vec<String> = requests
            .iter()
            .map(|request| request.lines().next().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(paths, [
            "GET /crate/serde/1.0.228/json/61 HTTP/1.1",
            "GET /crate/serde/1.0.228/json/60 HTTP/1.1",
        ]);
        assert_eq!(cached(&cache).as_deref(), Some(&json[..]));
    }

    #[tokio::test]
    async fn a_transient_failure_on_the_way_is_reported_when_no_format_is_found() {
        // Format 61 fails transiently three times; the older ones are not there.
        let older = rustdoc_json::supported_formats().count().saturating_sub(1);
        let mut responses = vec![status("503 Service Unavailable"); 3];
        responses.extend(vec![status("404 Not Found"); older]);
        let (cache, docs_rs, _requests, _dir) = setup(responses);
        let (name, version) = serde();
        assert!(matches!(
            docs_rs_json_in_another_format(&cache, &docs_rs, &name, &version, None, 40).await,
            Err(LoadError::Fetch(FetchError::Transient { .. }))
        ));
    }

    #[tokio::test]
    async fn no_format_is_none_without_transient_failures_and_remembered() {
        let formats = rustdoc_json::supported_formats().count();
        let (cache, docs_rs, requests, _dir) = setup(vec![status("404 Not Found"); formats]);
        let (name, version) = serde();
        for _ in 0..2 {
            assert_eq!(
                docs_rs_json_in_another_format(&cache, &docs_rs, &name, &version, None, 40)
                    .await
                    .expect("no error"),
                None
            );
        }
        // Format 47 was not asked for, and the second search did not ask docs.rs again.
        assert_eq!(requests.iter().count(), formats);
    }
}
