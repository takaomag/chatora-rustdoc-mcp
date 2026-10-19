//! Data layer: fetches rustdoc JSON, normalizes its format version, and caches it.

pub mod cache;
pub mod crate_name;
pub mod docs_rs;
pub mod format_version;
mod http;
pub mod rustdoc_json;
pub mod sparse_index;
#[cfg(test)]
mod test_http;
pub mod version;

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use semver::Version;
use serde::{Deserialize, Serialize};

use self::{
    cache::Cache,
    crate_name::CrateName,
    docs_rs::{DocsRs, FetchError},
    format_version::Header,
    rustdoc_json::SUPPORTED,
};

/// How long it is remembered that docs.rs has no JSON for a release (requirement R-ERR-3, about
/// an hour). docs.rs may build it later (requirement R-DS-3).
const MISSING_TTL: Duration = Duration::from_hours(1);

/// How long a transient failure to fetch a JSON is remembered (requirement R-ERR-3, about
/// 30 seconds), so that every lookup does not wait for the same failure again.
const TRANSIENT_TTL: Duration = Duration::from_secs(30);

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
    /// Fetching the same JSON failed transiently a moment ago (requirement R-ERR-3).
    #[error("fetching this rustdoc JSON failed a moment ago; try again in {} seconds", retry_after.as_secs())]
    RecentFailure {
        /// How long until it is tried again.
        retry_after: Duration,
    },
    /// The cached file exists but could not be read, or the blocking task that uses the cache was
    /// cancelled (when the runtime shuts down).
    #[error("the cache of rustdoc JSON could not be used")]
    Cache(#[source] io::Error),
}

/// A remembered failure to fetch a JSON, kept next to where the JSON would be.
#[derive(Debug, Serialize, Deserialize)]
struct Miss {
    /// docs.rs has no JSON for the release, rather than a transient failure.
    missing: bool,
    /// When it failed, in seconds since the Unix epoch.
    at: u64,
}

/// Returns the rustdoc JSON of `name` at exactly `version` for the docs.rs default target: from
/// the cache if it is there, or else fetched from docs.rs and then cached.
///
/// Before it is cached, its `format_version` must be readable and its `crate_version` must be
/// `version` (requirements R-CA-4 and R-VR-4). A body whose `format_version` cannot be read (such
/// as the error page of a proxy) is returned but not cached, for the caller to report as
/// unparsable. A missing JSON is remembered for an hour and a transient failure for 30 seconds
/// (requirement R-ERR-3). A failure to write the cache is logged and does not fail the load.
///
/// # Errors
///
/// Returns an error when the cached file cannot be read, the fetch fails or failed a moment ago,
/// or docs.rs returns another version.
pub async fn docs_rs_json(
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
) -> Result<Box<[u8]>, LoadError> {
    // The cache is read and written on a blocking thread, so that tens of megabytes of disk I/O do
    // not hold up the async workers.
    let path = cache.docs_rs_json_path(name, version);
    let miss_path = with_suffix(&path, ".miss.json");
    let (cached, miss) = {
        let (cache, path, miss_path) = (cache.clone(), path.clone(), miss_path.clone());
        blocking(move || {
            let Some(json) = cache.read_file(&path)? else {
                let miss = cache
                    .read_file(&miss_path)?
                    .and_then(|miss| serde_json::from_slice::<Miss>(&miss).ok());
                return Ok((None, miss));
            };
            Ok((Some(json), None))
        })
        .await
        .map_err(LoadError::Cache)?
    };
    if let Some(json) = cached {
        return Ok(json);
    }
    if let Some(miss) = miss {
        let ttl = if miss.missing {
            MISSING_TTL
        } else {
            TRANSIENT_TTL
        };
        let age = SystemTime::now()
            .duration_since(system_time(miss.at))
            .unwrap_or(ttl);
        if age < ttl {
            return Err(if miss.missing {
                LoadError::Fetch(FetchError::NotFound)
            } else {
                LoadError::RecentFailure {
                    retry_after: ttl.saturating_sub(age),
                }
            });
        }
    }
    match docs_rs.fetch_rustdoc_json(name, version, None).await {
        Ok(json) => store(json, version, path, Some(miss_path)).await,
        Err(error) => {
            let missing = match &error {
                FetchError::NotFound | FetchError::BadRequest => Some(true),
                FetchError::Transient { .. }
                | FetchError::UnexpectedStatus { .. }
                | FetchError::Network(_)
                | FetchError::Certificate(_) => Some(false),
                // A broken body may be fetched whole next time.
                FetchError::TooLarge { .. }
                | FetchError::TooManyLayers
                | FetchError::Corrupt(_) => None,
            };
            if let Some(missing) = missing {
                remember_miss(miss_path, missing).await;
            }
            Err(LoadError::Fetch(error))
        },
    }
}

/// Looks for the rustdoc JSON of `name` at `version` in another format that this server reads,
/// when the one docs.rs returned by default, in format `found`, could not be read.
///
/// The formats are tried newest first (requirement R-DS-2, step 3). docs.rs has a format only if
/// it built the release in it, so a 404 is expected.
///
/// Every format is tried, whatever fails on the way (decided in M1): a JSON that is found is
/// cached in place of the default one and returned. If none is found, `None` is returned, unless
/// a transient failure may have hidden one, which is then returned as the error.
///
/// # Errors
///
/// Returns the last transient failure when no format was found, or an error of the cache.
pub async fn docs_rs_json_in_another_format(
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
    found: u32,
) -> Result<Option<Box<[u8]>>, LoadError> {
    let mut transient = None;
    for format in SUPPORTED.rev().filter(|&format| format != found) {
        let error = match docs_rs
            .fetch_rustdoc_json(name, version, Some(format))
            .await
        {
            Ok(json) => {
                let path = cache.docs_rs_json_path(name, version);
                match store(json, version, path, None).await {
                    Ok(json) if format_version::peek(&json) == Some(format) => {
                        return Ok(Some(json));
                    },
                    Ok(_) | Err(LoadError::WrongVersion { .. }) => continue,
                    Err(error) => return Err(error),
                }
            },
            Err(error) => error,
        };
        match error {
            FetchError::NotFound
            | FetchError::BadRequest
            | FetchError::TooLarge { .. }
            | FetchError::TooManyLayers
            | FetchError::Corrupt(_) => {},
            FetchError::Transient { .. }
            | FetchError::UnexpectedStatus { .. }
            | FetchError::Network(_)
            | FetchError::Certificate(_) => transient = Some(error),
        }
    }
    transient.map_or(Ok(None), |error| Err(LoadError::Fetch(error)))
}

/// Checks a fetched `json` and caches it at `path` when it is rustdoc JSON of `version`, and
/// forgets a remembered failure at `miss_path`. A JSON of another version is an error; one whose
/// header cannot be read is returned without being cached.
async fn store(
    json: Box<[u8]>,
    version: &Version,
    path: PathBuf,
    miss_path: Option<PathBuf>,
) -> Result<Box<[u8]>, LoadError> {
    let version = version.clone();
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
            Ok(_) => {
                if let Some(miss_path) = miss_path {
                    remove_if_there(&miss_path);
                }
                Stored::Written(cache::write_atomically(&path, &json))
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
    }
    Ok(json)
}

/// What [`store`] did with a fetched JSON.
enum Stored {
    Written(io::Result<()>),
    NotRustdocJson,
    WrongVersion(Box<str>),
}

/// Remembers at `miss_path` that fetching failed, missing or transiently. A failure to write it is
/// logged.
async fn remember_miss(miss_path: PathBuf, missing: bool) {
    let miss = Miss {
        missing,
        at: unix_seconds(SystemTime::now()),
    };
    let written = blocking(move || {
        let json = serde_json::to_vec(&miss).map_err(io::Error::other)?;
        cache::write_atomically(&miss_path, &json)
    })
    .await;
    if let Err(error) = written {
        tracing::warn!(%error, "failed to remember a failed fetch in the cache");
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

/// `path` with `suffix` added to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
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
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| io::Error::other(crate::task::resume_if_panic(error)))?
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

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

    /// Loads the JSON of serde from a server that answers `body`, with an empty cache, and reads
    /// back what was cached.
    async fn load(body: &[u8]) -> (Result<Box<[u8]>, LoadError>, Option<Box<[u8]>>) {
        let (cache, docs_rs, _requests, _dir) = setup(vec![ok(body)]);
        let (name, version) = serde();
        let json = docs_rs_json(&cache, &docs_rs, &name, &version).await;
        let cached = cache.read_docs_rs_json(&name, &version).expect("reads");
        (json, cached)
    }

    #[tokio::test]
    async fn rustdoc_json_of_the_version_is_cached() {
        let json = br#"{"root":0,"crate_version":"1.0.228","format_version":61}"#;
        let (loaded, cached) = load(json).await;
        assert_eq!(loaded.expect("loads").as_ref(), json);
        assert_eq!(cached.as_deref(), Some(&json[..]));
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
                docs_rs_json(&cache, &docs_rs, &name, &version).await,
                Err(LoadError::Fetch(FetchError::NotFound))
            ));
        }
        // The second load did not ask docs.rs again.
        assert_eq!(requests.iter().count(), 1);
    }

    #[tokio::test]
    async fn a_transient_failure_is_remembered_for_a_moment() {
        let (cache, docs_rs, requests, _dir) = setup(vec![status("503 Service Unavailable"); 3]);
        let (name, version) = serde();
        assert!(matches!(
            docs_rs_json(&cache, &docs_rs, &name, &version).await,
            Err(LoadError::Fetch(FetchError::Transient { .. }))
        ));
        let again = docs_rs_json(&cache, &docs_rs, &name, &version).await;
        assert!(
            matches!(&again, Err(LoadError::RecentFailure { retry_after }) if *retry_after <= TRANSIENT_TTL),
            "{again:?}"
        );
        assert_eq!(requests.iter().count(), 3);
    }

    #[tokio::test]
    async fn another_format_is_found_by_its_number_and_cached() {
        let json = br#"{"crate_version":"1.0.228","format_version":60}"#;
        let (cache, docs_rs, requests, _dir) = setup(vec![status("404 Not Found"), ok(json)]);
        let (name, version) = serde();
        let found = docs_rs_json_in_another_format(&cache, &docs_rs, &name, &version, 40)
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
        assert_eq!(
            cache
                .read_docs_rs_json(&name, &version)
                .expect("reads")
                .as_deref(),
            Some(&json[..])
        );
    }

    #[tokio::test]
    async fn a_transient_failure_on_the_way_is_reported_when_no_format_is_found() {
        // Format 61 fails transiently three times; 54 to 60 are not there.
        let mut responses = vec![status("503 Service Unavailable"); 3];
        responses.extend(vec![status("404 Not Found"); 7]);
        let (cache, docs_rs, _requests, _dir) = setup(responses);
        let (name, version) = serde();
        assert!(matches!(
            docs_rs_json_in_another_format(&cache, &docs_rs, &name, &version, 40).await,
            Err(LoadError::Fetch(FetchError::Transient { .. }))
        ));
    }

    #[tokio::test]
    async fn no_format_is_none_without_transient_failures() {
        let (cache, docs_rs, _requests, _dir) = setup(vec![status("404 Not Found"); 8]);
        let (name, version) = serde();
        assert_eq!(
            docs_rs_json_in_another_format(&cache, &docs_rs, &name, &version, 40)
                .await
                .expect("no error"),
            None
        );
    }
}
