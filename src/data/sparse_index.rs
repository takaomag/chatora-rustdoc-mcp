//! The crates.io sparse index: the published versions of a crate (requirements R-VR-6, R-CA-3,
//! R-PR-1, R-NW-1).
//!
//! An index file lists every version of one crate, one JSON object per line. It is cached for
//! [`TTL`]; after that, it is checked again with a conditional request, so that a crate that has
//! not changed costs no body (`304`).
//!
//! Retries and merging the same requests (R-NW-2), and caching that a crate does not exist
//! (R-ERR-3), come later in M1.

use std::{
    ffi::OsString,
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{StatusCode, header};
use semver::Version;
use serde::{Deserialize, Serialize};

use super::{
    blocking,
    cache::{self, Cache},
    crate_name::CrateName,
    http::{self, BodyError},
};

/// Where the sparse index is. Kept in one place so that a mirror can replace it later
/// (requirement R-CA-8).
pub const DEFAULT_BASE_URL: &str = "https://index.crates.io";

/// How long an index file is used without checking it again (requirement R-CA-3). It matches the
/// `cache-control: max-age=600` of the index.
pub const TTL: Duration = Duration::from_secs(600);

/// Largest index file held in memory. serde's, with a few hundred versions, is about 165 KB.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// A client for the crates.io sparse index, with its cache.
#[derive(Debug, Clone)]
pub struct SparseIndex {
    client: reqwest::Client,
    // `Arc` so that cloning the client does not copy the URL.
    base_url: Arc<str>,
    cache: Cache,
    ttl: Duration,
}

/// The index entry of a crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The name as published (`Inflector`, `tokio-util`), which may differ from the one asked
    /// for in case and in `-` and `_` (requirement R-PR-1).
    pub name: Box<str>,
    /// The versions, in the order of the index (the order of publication).
    pub versions: Box<[IndexVersion]>,
    /// When the index file was fetched or last confirmed unchanged.
    pub fetched_at: SystemTime,
}

/// One published version of a crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexVersion {
    /// The version, with its build metadata (`2.1.0+zstd.1.5.7`).
    pub version: Version,
    /// The sha256 of the `.crate` file, in hex.
    pub checksum: Box<str>,
    /// Whether the version was yanked.
    pub yanked: bool,
    /// The MSRV it declares, if any.
    pub rust_version: Option<Box<str>>,
}

/// Why the index entry of a crate could not be read.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// The index has no crate of this name, in any spelling of `-` and `_`.
    #[error("crates.io has no crate of this name")]
    NotFound,
    /// The index is overloaded or failing; trying again later may work.
    #[error("the crates.io index answered {status}; try again later")]
    Transient {
        /// The HTTP status.
        status: StatusCode,
    },
    /// The index answered with a status this client does not expect.
    #[error("the crates.io index answered with an unexpected status {status}")]
    UnexpectedStatus {
        /// The HTTP status.
        status: StatusCode,
    },
    /// The request or the transfer of the body failed.
    #[error("the request to the crates.io index failed")]
    Network(#[source] reqwest::Error),
    /// The index file is larger than the limit.
    #[error("the index file is larger than {limit} bytes")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// The index file is not what the index format says.
    #[error("the index file could not be parsed")]
    Malformed(#[source] MalformedLine),
    /// The cached index file could not be read or written.
    #[error("the cache of the crates.io index could not be used")]
    Cache(#[source] io::Error),
}

/// A line of an index file that could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum MalformedLine {
    /// The line is not the JSON of a version.
    #[error("line {line} is not the JSON of a version")]
    Json {
        /// The line number, from 1.
        line: usize,
        /// What serde reported.
        #[source]
        source: serde_json::Error,
    },
    /// The version of the line is not a semantic version.
    #[error("line {line} has an invalid version")]
    Version {
        /// The line number, from 1.
        line: usize,
        /// What semver reported.
        #[source]
        source: semver::Error,
    },
    /// The file lists no version.
    #[error("the index file lists no version")]
    Empty,
}

/// The fields of an index line that are used. The others are ignored.
#[derive(Debug, Deserialize)]
struct Line {
    name: String,
    vers: String,
    cksum: String,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    rust_version: Option<String>,
}

/// How a cached index file was fetched, kept next to it.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    /// The `etag` of the response, for `If-None-Match`.
    etag: Option<String>,
    /// The `last-modified` of the response, for `If-Modified-Since` when there is no `etag`.
    last_modified: Option<String>,
    /// When it was fetched or last confirmed unchanged, in seconds since the Unix epoch.
    fetched_at: u64,
}

/// A cached index file and how it was fetched.
struct Cached {
    body: Box<[u8]>,
    record: Record,
}

impl SparseIndex {
    /// A client for the crates.io index itself, caching in `cache`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn new(cache: Cache) -> Result<Self, reqwest::Error> {
        Self::with_base_url(cache, DEFAULT_BASE_URL)
    }

    /// A client for a server at `base_url` that serves the sparse index, such as a mirror.
    /// `base_url` has no trailing `/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn with_base_url(cache: Cache, base_url: &str) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: http::client_builder().build()?,
            base_url: base_url.into(),
            cache,
            ttl: TTL,
        })
    }

    /// A client for a test server on this machine, which bypasses proxies, with its own TTL.
    #[cfg(test)]
    fn for_local_test(cache: Cache, base_url: &str, ttl: Duration) -> Self {
        Self {
            client: http::client_builder()
                .no_proxy()
                .build()
                .expect("the client builds"),
            base_url: base_url.into(),
            cache,
            ttl,
        }
    }

    /// The index entry of the crate `name`, which may be spelled with `-` and `_` swapped or in
    /// another case than published (requirement R-PR-1).
    ///
    /// # Errors
    ///
    /// Returns [`IndexError::NotFound`] when no spelling is in the index, and another error when
    /// the index or the cache cannot be read.
    pub async fn entry(&self, name: &CrateName) -> Result<IndexEntry, IndexError> {
        for spelling in spellings(name) {
            match self.entry_at(&spelling).await {
                Err(IndexError::NotFound) => {},
                found => return found,
            }
        }
        Err(IndexError::NotFound)
    }

    /// The index entry at the lowercase `name`: the cached one while it is fresh, and else one
    /// fetched or confirmed with a conditional request.
    async fn entry_at(&self, name: &str) -> Result<IndexEntry, IndexError> {
        let body_path = self.cache.sparse_index_path(&index_path(name));
        let record_path = with_suffix(&body_path, ".meta.json");
        let cached = read_cached(&self.cache, body_path.clone(), record_path.clone()).await?;
        let now = SystemTime::now();
        if let Some(cached) = &cached {
            let fetched_at = system_time(cached.record.fetched_at);
            if now
                .duration_since(fetched_at)
                .is_ok_and(|age| age < self.ttl)
            {
                return parse(&cached.body, fetched_at).map_err(IndexError::Malformed);
            }
        }

        let mut request = self
            .client
            .get(format!("{}/{}", self.base_url, index_path(name)));
        if let Some(cached) = &cached {
            if let Some(etag) = &cached.record.etag {
                request = request.header(header::IF_NONE_MATCH, etag);
            } else if let Some(last_modified) = &cached.record.last_modified {
                request = request.header(header::IF_MODIFIED_SINCE, last_modified);
            }
        }
        let mut response = request.send().await.map_err(IndexError::Network)?;
        match (response.status(), cached) {
            (StatusCode::NOT_MODIFIED, Some(cached)) => {
                let entry = parse(&cached.body, now).map_err(IndexError::Malformed)?;
                let record = Record {
                    fetched_at: unix_seconds(now),
                    ..cached.record
                };
                write_record_later(record_path, record).await;
                Ok(entry)
            },
            (StatusCode::OK, _) => {
                let record = Record {
                    etag: header_value(&response, header::ETAG),
                    last_modified: header_value(&response, header::LAST_MODIFIED),
                    fetched_at: unix_seconds(now),
                };
                let body = http::read_body(&mut response, MAX_BYTES)
                    .await
                    .map_err(|error| match error {
                        BodyError::Network(error) => IndexError::Network(error),
                        BodyError::TooLarge { limit } => IndexError::TooLarge { limit },
                    })?;
                // Only a file that parses is cached, as with rustdoc JSON (requirement R-CA-4).
                let entry = parse(&body, now).map_err(IndexError::Malformed)?;
                write_cached(body_path, body, record_path, record).await;
                Ok(entry)
            },
            (StatusCode::NOT_FOUND, _) => Err(IndexError::NotFound),
            (status, _) if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() => {
                Err(IndexError::Transient { status })
            },
            (status, _) => Err(IndexError::UnexpectedStatus { status }),
        }
    }
}

/// The spellings of `name` to look up, the given one first. crates.io treats `-` and `_` as the
/// same and ignores case, and the index file is at the published spelling in lowercase. A name
/// that mixes `-` and `_` in another way than published is not found.
fn spellings(name: &CrateName) -> Vec<String> {
    let given = name.as_str().to_ascii_lowercase();
    let mut spellings = Vec::with_capacity(3);
    for spelling in [given.replace('_', "-"), given.replace('-', "_")] {
        if spelling != given && !spellings.contains(&spelling) {
            spellings.push(spelling);
        }
    }
    spellings.insert(0, given);
    spellings
}

/// The path of the index file of the lowercase `name`: `1/a`, `2/ab`, `3/a/abc`, `ab/cd/abcd…`.
fn index_path(name: &str) -> String {
    let mut chars = name.chars();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", chars.next().unwrap_or_default()),
        _ => {
            let first: String = chars.by_ref().take(2).collect();
            let second: String = chars.take(2).collect();
            format!("{first}/{second}/{name}")
        },
    }
}

/// Parses an index file, with `fetched_at` as when it was fetched.
fn parse(body: &[u8], fetched_at: SystemTime) -> Result<IndexEntry, MalformedLine> {
    let mut name = None;
    let mut versions = Vec::new();
    let lines = body
        .split(|&byte| byte == b'\n')
        .enumerate()
        .filter(|(_, line)| !line.iter().all(u8::is_ascii_whitespace));
    for (index, line) in lines {
        let number = index.saturating_add(1);
        let line: Line = serde_json::from_slice(line).map_err(|source| MalformedLine::Json {
            line: number,
            source,
        })?;
        let version = Version::parse(&line.vers).map_err(|source| MalformedLine::Version {
            line: number,
            source,
        })?;
        name.get_or_insert(line.name);
        versions.push(IndexVersion {
            version,
            checksum: line.cksum.into(),
            yanked: line.yanked,
            rust_version: line.rust_version.map(String::into_boxed_str),
        });
    }
    Ok(IndexEntry {
        name: name.ok_or(MalformedLine::Empty)?.into(),
        versions: versions.into(),
        fetched_at,
    })
}

/// Reads the cached index file and its record, or `None` if either is missing or the record
/// cannot be parsed.
async fn read_cached(
    cache: &Cache,
    body_path: PathBuf,
    record_path: PathBuf,
) -> Result<Option<Cached>, IndexError> {
    let cache = cache.clone();
    blocking(move || {
        let Some(record) = cache.read_file(&record_path)? else {
            return Ok(None);
        };
        let Ok(record) = serde_json::from_slice::<Record>(&record) else {
            tracing::warn!(path = %record_path.display(), "ignoring an unreadable index record");
            return Ok(None);
        };
        Ok(cache
            .read_file(&body_path)?
            .map(|body| Cached { body, record }))
    })
    .await
    .map_err(IndexError::Cache)
}

/// Writes the index file, then its record. A failure is logged; the file is fetched again next
/// time.
async fn write_cached(body_path: PathBuf, body: Vec<u8>, record_path: PathBuf, record: Record) {
    let written = blocking(move || {
        cache::write_atomically(&body_path, &body)?;
        write_record(&record_path, &record)
    })
    .await;
    if let Err(error) = written {
        tracing::warn!(%error, "failed to write an index file to the cache");
    }
}

/// Writes the record of a cached index file. A failure is logged.
async fn write_record_later(record_path: PathBuf, record: Record) {
    if let Err(error) = blocking(move || write_record(&record_path, &record)).await {
        tracing::warn!(%error, "failed to write an index record to the cache");
    }
}

fn write_record(path: &std::path::Path, record: &Record) -> io::Result<()> {
    let json = serde_json::to_vec(record).map_err(io::Error::other)?;
    cache::write_atomically(path, &json)
}

/// `path` with `suffix` added to its file name.
fn with_suffix(path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// The value of the header `name`, if it is text.
fn header_value(response: &reqwest::Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn system_time(seconds: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::{super::test_http::serve_sequence, *};

    const SERDE_INDEX: &[u8] = b"{\"name\":\"serde\",\"vers\":\"1.0.0\",\"deps\":[],\"cksum\":\"aa\",\"features\":{},\"yanked\":true}\n\
        {\"name\":\"serde\",\"vers\":\"1.0.228\",\"deps\":[],\"cksum\":\"bb\",\"features\":{},\"yanked\":false,\"rust_version\":\"1.56\",\"v\":2}\n";

    fn ok(body: &[u8], etag: &str) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\netag: {etag}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body.to_vec(),
        )
    }

    fn status(line: &str) -> (String, Vec<u8>) {
        (
            format!("HTTP/1.1 {line}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
            Vec::new(),
        )
    }

    fn name(name: &str) -> CrateName {
        name.parse().expect("a crate name")
    }

    /// An index with a fresh cache that serves `responses` in turn, and the request heads.
    fn index(
        responses: Vec<(String, Vec<u8>)>,
        ttl: Duration,
    ) -> (SparseIndex, mpsc::Receiver<String>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (base_url, requests) = serve_sequence(responses);
        let index = SparseIndex::for_local_test(Cache::at(dir.path()), &base_url, ttl);
        (index, requests, dir)
    }

    #[test]
    fn index_paths_follow_the_length_of_the_name() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("serde"), "se/rd/serde");
        assert_eq!(index_path("tokio-util"), "to/ki/tokio-util");
    }

    #[test]
    fn spellings_swap_dashes_and_underscores() {
        assert_eq!(spellings(&name("serde")), ["serde"]);
        assert_eq!(spellings(&name("tokio_util")), ["tokio_util", "tokio-util"]);
        assert_eq!(spellings(&name("Inflector")), ["inflector"]);
        assert_eq!(spellings(&name("a-b_c")), ["a-b_c", "a-b-c", "a_b_c"]);
    }

    #[tokio::test]
    async fn an_entry_is_fetched_parsed_and_cached() {
        let (index, requests, _dir) = index(vec![ok(SERDE_INDEX, "\"v1\"")], TTL);
        let entry = index.entry(&name("serde")).await.expect("found");
        assert_eq!(&*entry.name, "serde");
        assert_eq!(entry.versions.len(), 2);
        let latest = &entry.versions[1];
        assert_eq!(latest.version, Version::new(1, 0, 228));
        assert_eq!(&*latest.checksum, "bb");
        assert!(!latest.yanked);
        assert_eq!(latest.rust_version.as_deref(), Some("1.56"));
        assert!(entry.versions[0].yanked);
        let request = requests.recv().expect("the server saw the request");
        assert!(
            request.starts_with("GET /se/rd/serde HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.to_ascii_lowercase().contains(&format!(
                "user-agent: {}\r\n",
                http::USER_AGENT.to_ascii_lowercase()
            )),
            "{request}"
        );
        // While fresh, the cache answers: the server serves no second response.
        let again = index.entry(&name("serde")).await.expect("cached");
        assert_eq!(again.versions, entry.versions);
    }

    #[tokio::test]
    async fn an_unchanged_file_is_confirmed_without_its_body() {
        let (index, requests, _dir) = index(
            vec![ok(SERDE_INDEX, "\"v1\""), status("304 Not Modified")],
            Duration::ZERO,
        );
        let first = index.entry(&name("serde")).await.expect("found");
        let second = index.entry(&name("serde")).await.expect("confirmed");
        assert_eq!(second.versions, first.versions);
        let _first_request = requests.recv().expect("the first request");
        let second_request = requests.recv().expect("the second request");
        assert!(
            second_request
                .to_ascii_lowercase()
                .contains("if-none-match: \"v1\"\r\n"),
            "{second_request}"
        );
    }

    #[tokio::test]
    async fn another_spelling_is_tried_when_the_given_one_is_not_found() {
        let body = b"{\"name\":\"tokio-util\",\"vers\":\"0.7.0\",\"cksum\":\"cc\"}\n";
        let (index, requests, _dir) = index(vec![status("404 Not Found"), ok(body, "\"t\"")], TTL);
        let entry = index.entry(&name("tokio_util")).await.expect("found");
        assert_eq!(&*entry.name, "tokio-util");
        let first = requests.recv().expect("the first request");
        let second = requests.recv().expect("the second request");
        assert!(first.starts_with("GET /to/ki/tokio_util "), "{first}");
        assert!(second.starts_with("GET /to/ki/tokio-util "), "{second}");
    }

    #[tokio::test]
    async fn a_missing_crate_is_not_found() {
        let (index, _requests, _dir) = index(vec![status("404 Not Found")], TTL);
        assert!(matches!(
            index.entry(&name("nope")).await,
            Err(IndexError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_body_that_is_not_an_index_file_is_an_error_and_not_cached() {
        let page = b"<html>blocked by the proxy</html>";
        let (index, _requests, dir) = index(vec![ok(page, "\"x\"")], TTL);
        assert!(matches!(
            index.entry(&name("serde")).await,
            Err(IndexError::Malformed(MalformedLine::Json { line: 1, .. }))
        ));
        let cached = Cache::at(dir.path())
            .read_file(&index.cache.sparse_index_path("se/rd/serde"))
            .expect("reads");
        assert_eq!(cached, None);
    }
}
