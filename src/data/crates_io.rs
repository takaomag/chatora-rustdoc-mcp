//! The crates.io API: a crate's downloads, versions and dependents, and the search for crates
//! (requirements R-NW-1, R-SR-6; decided in M3f, decisions 92 to 103).
//!
//! crates.io asks crawlers for one request a second and a `User-Agent` that says how to reach them,
//! so the requests of the process take turns a second apart, and a request whose turn would come
//! after the time given is not sent ([`ApiError::NoTurn`]). Redirects are not followed, so that
//! none is sent out of turn.
//!
//! A crate is read with `?include=downloads` (decided after measuring, in F1a's review): it has the
//! downloads of the last 90 days, but its newest versions read `0.0.0`, so the versions are taken
//! from the sparse index instead.
//!
//! Only the fields that the tools use are read: who published a version (`published_by`), its
//! audit trail and the owners are not, so that they are kept nowhere (decision 103).

use std::{
    io,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::{
    blocking,
    cache::{self, Cache},
    crate_name::CrateName,
    http::{self, BodyError, Network, NoTurn, Pacer},
    layout,
    system_time,
    unix_seconds,
};

/// Where the crates.io API is. Kept in one place so that the tests can replace it.
pub const DEFAULT_BASE_URL: &str = "https://crates.io";

/// The least time between two requests to the API (crates.io's crawler policy).
pub const INTERVAL: Duration = Duration::from_secs(1);

/// The most items that the API gives in one page.
pub const MAX_PER_PAGE: u32 = 100;

/// How long what the API says of a crate is used without asking again (decision 94).
///
/// It is its downloads, its versions and its dependents. The versions themselves come from the
/// sparse index, which is read more often.
pub const CRATE_TTL: Duration = Duration::from_hours(24);

/// How long a search is used without asking again (decision 94).
pub const SEARCH_TTL: Duration = Duration::from_hours(1);

/// The directory under the format's in the cache with a directory for each crate, keyed as the
/// cache keys crates. The searches are beside it, a file each.
pub const CRATES: &str = "crates";

/// The directory under the format's in the cache with the searches, by the SHA-256 of what was
/// asked; the query itself is not written.
const SEARCHES: &str = layout::API_SEARCHES;

/// Largest response held in memory. A page of 100 versions of serde is about 170 KB.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// A client for the crates.io API.
#[derive(Debug, Clone)]
pub struct CratesIo {
    client: reqwest::Client,
    cache: Cache,
    // `Arc` so that cloning the client does not copy the URL.
    base_url: Arc<str>,
    pacer: Pacer,
    /// Whether no request may be sent (`network.offline`, requirement R-CA-7).
    offline: bool,
}

/// The turns of the requests to crates.io of this process.
static TURNS: OnceLock<Pacer> = OnceLock::new();

/// What a crate is, from `/api/v1/crates/{name}?include=downloads`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateInfo {
    /// The name as published, which may differ from the one asked for in case and in `-` and `_`.
    pub name: Box<str>,
    /// The description of the newest version.
    #[serde(default)]
    pub description: Option<Box<str>>,
    /// All downloads of all versions.
    pub downloads: u64,
    /// The downloads of the last 90 days.
    #[serde(default)]
    pub recent_downloads: Option<u64>,
    /// The version that crates.io shows first: the newest stable one that is not yanked, if any.
    /// (`max_version` and the like read `0.0.0` in this answer, and are not read.)
    #[serde(default)]
    pub default_version: Option<Box<str>>,
    /// How many versions there are, yanked included.
    #[serde(default)]
    pub num_versions: Option<u64>,
    /// The repository of the newest version.
    #[serde(default)]
    pub repository: Option<Box<str>>,
    /// The homepage of the newest version.
    #[serde(default)]
    pub homepage: Option<Box<str>>,
    /// The documentation of the newest version.
    #[serde(default)]
    pub documentation: Option<Box<str>>,
    /// When the crate was first published (RFC 3339).
    pub created_at: Box<str>,
    /// When the crate last changed (RFC 3339).
    pub updated_at: Box<str>,
}

/// A version of a crate, from `/api/v1/crates/{name}/versions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// The version.
    pub num: Box<str>,
    /// Its license expression, as declared: SPDX, or the old `MIT/Apache-2.0`.
    #[serde(default)]
    pub license: Option<Box<str>>,
    /// When it was published (RFC 3339).
    pub created_at: Box<str>,
    /// Whether it is yanked.
    pub yanked: bool,
    /// Its `rust-version`.
    #[serde(default)]
    pub rust_version: Option<Box<str>>,
    /// Its edition.
    #[serde(default)]
    pub edition: Option<Box<str>>,
}

/// A page of the versions of a crate, newest first by semver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionsPage {
    /// The versions of this page.
    pub versions: Vec<VersionInfo>,
    /// How many versions there are in all.
    pub total: u64,
    /// Where the next page starts, if there is one.
    pub next: Option<Next>,
}

/// A crate that a search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    /// The name as published.
    pub name: Box<str>,
    /// The description of the newest version.
    #[serde(default)]
    pub description: Option<Box<str>>,
    /// The newest version that is not a pre-release and not yanked, if any.
    #[serde(default)]
    pub max_stable_version: Option<Box<str>>,
    /// The newest version by semver, pre-releases included.
    pub max_version: Box<str>,
    /// All downloads of all versions.
    pub downloads: u64,
    /// The downloads of the last 90 days.
    #[serde(default)]
    pub recent_downloads: Option<u64>,
    /// The repository of the newest version.
    #[serde(default)]
    pub repository: Option<Box<str>>,
    /// When the crate last changed (RFC 3339).
    pub updated_at: Box<str>,
    /// Whether the name is the query.
    #[serde(default)]
    pub exact_match: bool,
}

/// A page of a search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    /// The crates of this page.
    pub crates: Vec<Found>,
    /// How many crates the search found in all.
    pub total: u64,
    /// Where the next page starts, if there is one.
    pub next: Option<Next>,
}

/// Where the next page of a list starts, as the API says in `meta.next_page`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Next {
    /// After the key `seek` (the API's own cursor).
    Seek(Box<str>),
    /// At the page number.
    Page(u32),
}

/// How a search is sorted (decision 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sort {
    /// As crates.io ranks the crates for the query, with the crate of the very name first.
    Relevance,
    /// By all downloads.
    Downloads,
    /// By the downloads of the last 90 days.
    RecentDownloads,
    /// By when the crate last changed.
    RecentUpdates,
    /// By when the crate was first published.
    New,
}

impl Sort {
    /// The value of `sort` that the API takes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Downloads => "downloads",
            Self::RecentDownloads => "recent-downloads",
            Self::RecentUpdates => "recent-updates",
            Self::New => "new",
        }
    }
}

/// Why the API did not answer.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// crates.io has no crate of this name.
    #[error("crates.io has no crate of this name")]
    NotFound,
    /// The turn to send the request would not come in time (one request a second).
    #[error(
        "the crates.io API is asked at most once a second, and the turn of this request would not come in time"
    )]
    NoTurn,
    /// The API is overloaded or failing; trying again later may work.
    #[error("the crates.io API answered {status}; try again later")]
    Transient {
        /// The HTTP status.
        status: StatusCode,
    },
    /// The API answered with a status this client does not expect.
    #[error("the crates.io API answered with an unexpected status {status}")]
    UnexpectedStatus {
        /// The HTTP status.
        status: StatusCode,
    },
    /// The request or the transfer of the body failed.
    #[error("the request to the crates.io API failed")]
    Network(#[source] reqwest::Error),
    /// The response is larger than the limit.
    #[error("the response of the crates.io API is larger than {limit} bytes")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// The response is not what the API is known to answer.
    #[error("the response of the crates.io API could not be read")]
    Malformed(#[source] serde_json::Error),
    /// No request may be sent (`network.offline`, requirement R-CA-7).
    #[error("the server is offline (network.offline)")]
    Offline,
    /// The URL of the API is not valid, as a test server's may not be.
    #[error("the URL of the crates.io API is not valid: {0}")]
    BadUrl(Box<str>),
    /// Asking the same a moment ago failed transiently (requirement R-ERR-3).
    #[error("asking the crates.io API failed a moment ago; it is asked again in {} seconds", retry_after.as_secs())]
    RecentFailure {
        /// How long until it is asked again.
        retry_after: Duration,
    },
    /// The cache could not be read.
    #[error("the cache of the crates.io API could not be used")]
    Cache(#[source] io::Error),
}

impl ApiError {
    /// Whether an answer in the cache, however old, stands in for the one that did not come.
    const fn has_stand_in(&self) -> bool {
        match self {
            Self::NoTurn
            | Self::Transient { .. }
            | Self::UnexpectedStatus { .. }
            | Self::Network(_)
            | Self::TooLarge { .. }
            | Self::Malformed(_)
            | Self::Offline
            | Self::RecentFailure { .. } => true,
            Self::NotFound | Self::BadUrl(_) | Self::Cache(_) => false,
        }
    }

    /// Whether the failure is remembered for a while, so that it is not asked again meanwhile.
    const fn is_remembered(&self) -> bool {
        matches!(self, Self::Transient { .. } | Self::Network(_))
    }
}

/// An answer of the API, from the cache or just fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached<T> {
    /// The answer.
    pub value: T,
    /// When it was fetched.
    pub fetched_at: SystemTime,
    /// Why an answer older than its TTL is given, when it is: the API did not answer.
    pub stale: Option<Box<str>>,
}

/// An answer as the cache keeps it.
#[derive(Serialize, Deserialize)]
struct Stored<T> {
    /// When it was fetched, in seconds since the Unix epoch.
    fetched_at: u64,
    value: T,
}

impl From<NoTurn> for ApiError {
    fn from(NoTurn: NoTurn) -> Self {
        Self::NoTurn
    }
}

/// The body of `/api/v1/crates/{name}`.
#[derive(Deserialize)]
struct CrateBody {
    #[serde(rename = "crate")]
    krate: CrateInfo,
}

/// The body of `/api/v1/crates/{name}/versions`.
#[derive(Deserialize)]
struct VersionsBody {
    versions: Vec<VersionInfo>,
    meta: Meta,
}

/// The body of `/api/v1/crates/{name}/reverse_dependencies`.
#[derive(Deserialize)]
struct DependentsBody {
    meta: Meta,
}

/// The body of `/api/v1/crates?q=`.
#[derive(Deserialize)]
struct SearchBody {
    crates: Vec<Found>,
    meta: Meta,
}

/// The `meta` of a list.
#[derive(Deserialize)]
struct Meta {
    total: u64,
    #[serde(default)]
    next_page: Option<String>,
}

impl CratesIo {
    /// A client for crates.io itself, caching in `cache`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn new(cache: Cache, network: &Network) -> Result<Self, reqwest::Error> {
        Self::with_base_url(cache, DEFAULT_BASE_URL, network)
    }

    /// A client for a server at `base_url` that serves the crates.io API. `base_url` has no
    /// trailing `/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn with_base_url(
        cache: Cache,
        base_url: &str,
        network: &Network,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: client(network)?,
            cache,
            base_url: base_url.into(),
            pacer: TURNS.get_or_init(|| Pacer::new(INTERVAL)).clone(),
            offline: network.offline,
        })
    }

    /// A client for a test server on this machine, which bypasses proxies, with turns `interval`
    /// apart.
    #[cfg(test)]
    fn for_local_test(cache: Cache, base_url: &str, interval: Duration) -> Self {
        Self {
            client: client(&Network::default()).expect("the client builds"),
            cache,
            base_url: base_url.into(),
            pacer: Pacer::new(interval),
            offline: false,
        }
    }

    /// Whether no request may be sent (`network.offline`, requirement R-CA-7).
    #[must_use]
    pub const fn is_offline(&self) -> bool {
        self.offline
    }

    /// What the crate `name` is (as `fetch_crate_info` asks), from the cache while it is younger
    /// than [`CRATE_TTL`] unless `fresh` asks for it again.
    ///
    /// # Errors
    ///
    /// Returns an error when the API does not answer and the cache has no answer to stand in, and
    /// [`ApiError::NotFound`] when crates.io has no such crate.
    pub async fn crate_info(
        &self,
        name: &CrateName,
        fresh: bool,
        until: tokio::time::Instant,
    ) -> Result<Cached<CrateInfo>, ApiError> {
        let dir = self.crate_dir(name);
        self.cached(
            (dir.join("crate.json"), &dir),
            (CRATE_TTL, fresh, until),
            self.fetch_crate_info(name, until),
        )
        .await
    }

    /// A page of the versions of the crate `name` (as `fetch_versions` asks), from the cache as
    /// [`Self::crate_info`] is.
    ///
    /// # Errors
    ///
    /// Returns an error when the API does not answer and the cache has no answer to stand in, and
    /// [`ApiError::NotFound`] when crates.io has no such crate.
    pub async fn versions(
        &self,
        name: &CrateName,
        next: Option<&Next>,
        fresh: bool,
        until: tokio::time::Instant,
    ) -> Result<Cached<VersionsPage>, ApiError> {
        let file = next.map_or_else(
            || "versions.json".to_owned(),
            |next| format!("versions-{}.json", key_of(&next)),
        );
        let dir = self.crate_dir(name);
        self.cached(
            (dir.join(file), &dir),
            (CRATE_TTL, fresh, until),
            self.fetch_versions(name, next, until),
        )
        .await
    }

    /// How many crates depend on the crate `name` (as `fetch_dependents` asks), from the cache as
    /// [`Self::crate_info`] is.
    ///
    /// # Errors
    ///
    /// Returns an error when the API does not answer and the cache has no answer to stand in, and
    /// [`ApiError::NotFound`] when crates.io has no such crate.
    pub async fn dependents(
        &self,
        name: &CrateName,
        fresh: bool,
        until: tokio::time::Instant,
    ) -> Result<Cached<u64>, ApiError> {
        let dir = self.crate_dir(name);
        self.cached(
            (dir.join("dependents.json"), &dir),
            (CRATE_TTL, fresh, until),
            self.fetch_dependents(name, until),
        )
        .await
    }

    /// A page of a search (as `fetch_search` asks), from the cache while it is younger than
    /// [`SEARCH_TTL`]. The file in the cache is named by the SHA-256 of what was asked, so that
    /// the query is not written.
    ///
    /// # Errors
    ///
    /// Returns an error when the API does not answer and the cache has no answer to stand in, and
    /// [`ApiError::NotFound`] when crates.io has no such crate.
    pub async fn search(
        &self,
        query: &str,
        sort: Sort,
        per_page: u32,
        next: Option<&Next>,
        until: tokio::time::Instant,
    ) -> Result<Cached<SearchPage>, ApiError> {
        let per_page = per_page.clamp(1, MAX_PER_PAGE);
        let asked = (query, sort, per_page, next);
        let path = self
            .api_dir()
            .join(SEARCHES)
            .join(format!("{}.json", key_of(&asked)));
        self.cached(
            (path.clone(), &path),
            (SEARCH_TTL, false, until),
            self.fetch_search(query, sort, per_page, next, until),
        )
        .await
    }

    /// Forgets that asking about the crate `name` failed a moment ago, as the argument `refresh`
    /// asks (§6.4).
    pub async fn forget(&self, name: &CrateName) {
        super::forget_failures(self.crate_dir(name), false).await;
    }

    /// The directory of the answers of this format in the cache.
    fn api_dir(&self) -> PathBuf {
        self.cache.crates_io_dir().join(super::formats::api_dir())
    }

    /// The directory of the answers about the crate `name` in the cache.
    fn crate_dir(&self, name: &CrateName) -> PathBuf {
        self.api_dir().join(CRATES).join(name.cache_key())
    }

    /// The answer kept at `path` while it is younger than `ttl` and not asked `fresh`, and else
    /// one that `fetch` gets, which is kept. When the API does not answer, the one kept is given
    /// however old, with why ([`Cached::stale`]). A transient failure is remembered for a while,
    /// for every process that shares the cache (requirement R-ERR-3).
    ///
    /// It holds the lock of `place`, the place of the inventory that `path` is in, as the garbage
    /// collection does, at most until `until`.
    ///
    /// # Errors
    ///
    /// Returns `fetch`'s error when the cache has no answer to stand in, [`ApiError::NoTurn`] when
    /// the lock is not had in time and the cache has none, and [`ApiError::NotFound`] whatever the
    /// cache has.
    async fn cached<T>(
        &self,
        (path, place): (PathBuf, &std::path::Path),
        (ttl, fresh, until): (Duration, bool, tokio::time::Instant),
        fetch: impl Future<Output = Result<T, ApiError>>,
    ) -> Result<Cached<T>, ApiError>
    where
        T: Serialize + serde::de::DeserializeOwned + Send + 'static,
    {
        // Another call that asks the same goes first, and this one then finds it in the cache; but
        // not past `until` (decision 93). What is kept is written whole, so it is read without the
        // lock then.
        let Ok(_held) = tokio::time::timeout_at(until, self.cache.lock(place)).await else {
            return match read_stored::<T>(&self.cache, path).await? {
                Some(stored) => Ok(Cached {
                    fetched_at: system_time(stored.fetched_at),
                    value: stored.value,
                    stale: Some(
                        "another request to the crates.io API for the same did not end in time, \
                         so the answer kept from before is given"
                            .into(),
                    ),
                }),
                None => Err(ApiError::NoTurn),
            };
        };
        let mut stored = read_stored::<T>(&self.cache, path.clone()).await?;
        let now = SystemTime::now();
        let young = |stored: &mut Stored<T>| {
            now.duration_since(system_time(stored.fetched_at))
                .is_ok_and(|age| age < ttl)
        };
        if !fresh && let Some(stored) = stored.take_if(young) {
            return Ok(Cached {
                value: stored.value,
                fetched_at: system_time(stored.fetched_at),
                stale: None,
            });
        }
        let fetched = if self.offline {
            Err(ApiError::Offline)
        } else if let Some(retry_after) = super::recent_failure(&self.cache, &path).await {
            Err(ApiError::RecentFailure { retry_after })
        } else {
            let fetched = fetch.await;
            if let Err(error) = &fetched
                && error.is_remembered()
            {
                super::remember_failure(&path).await;
            }
            fetched
        };
        match (fetched, stored) {
            (Ok(value), _) => {
                let stored = Stored {
                    fetched_at: unix_seconds(now),
                    value,
                };
                let value = write_stored(path, stored).await;
                Ok(Cached {
                    value,
                    fetched_at: now,
                    stale: None,
                })
            },
            (Err(error), Some(stored)) if error.has_stand_in() => Ok(Cached {
                stale: Some(if matches!(error, ApiError::Offline) {
                    "the server is offline (network.offline), so the answer kept from before is \
                     given"
                        .into()
                } else {
                    format!("the crates.io API did not answer ({error}), so the answer kept from before is given").into()
                }),
                fetched_at: system_time(stored.fetched_at),
                value: stored.value,
            }),
            (Err(error), _) => Err(error),
        }
    }

    /// What the crate `name` is, with the downloads of the last 90 days and without its versions
    /// (decision 95), unless the turn to ask would come after `until`.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::NotFound`] when crates.io has no such crate, and another error when
    /// the API does not answer.
    async fn fetch_crate_info(
        &self,
        name: &CrateName,
        until: tokio::time::Instant,
    ) -> Result<CrateInfo, ApiError> {
        let url = self.url(&format!("/api/v1/crates/{name}"), &[(
            "include",
            "downloads",
        )])?;
        let body: CrateBody = self.json(url, until).await?;
        Ok(body.krate)
    }

    /// A page of the versions of the crate `name`, newest first: the first one, or the one that
    /// `next` starts.
    ///
    /// # Errors
    ///
    /// As [`Self::crate_info`].
    async fn fetch_versions(
        &self,
        name: &CrateName,
        next: Option<&Next>,
        until: tokio::time::Instant,
    ) -> Result<VersionsPage, ApiError> {
        let per_page = MAX_PER_PAGE.to_string();
        let mut url = self.url(&format!("/api/v1/crates/{name}/versions"), &[(
            "per_page", &per_page,
        )])?;
        with_next(&mut url, next);
        let body: VersionsBody = self.json(url, until).await?;
        Ok(VersionsPage {
            versions: body.versions,
            total: body.meta.total,
            next: next_from(&body.meta),
        })
    }

    /// How many crates depend on the crate `name` (the API counts each crate once, by its newest
    /// version).
    ///
    /// # Errors
    ///
    /// As [`Self::crate_info`].
    async fn fetch_dependents(
        &self,
        name: &CrateName,
        until: tokio::time::Instant,
    ) -> Result<u64, ApiError> {
        let url = self.url(&format!("/api/v1/crates/{name}/reverse_dependencies"), &[(
            "per_page", "1",
        )])?;
        let body: DependentsBody = self.json(url, until).await?;
        Ok(body.meta.total)
    }

    /// A page of `per_page` crates (at most [`MAX_PER_PAGE`]) that `query` finds, sorted by
    /// `sort`: the first one, or the one that `next` starts.
    ///
    /// # Errors
    ///
    /// Returns an error when the API does not answer.
    async fn fetch_search(
        &self,
        query: &str,
        sort: Sort,
        per_page: u32,
        next: Option<&Next>,
        until: tokio::time::Instant,
    ) -> Result<SearchPage, ApiError> {
        let per_page = per_page.clamp(1, MAX_PER_PAGE).to_string();
        let mut url = self.url("/api/v1/crates", &[
            ("q", query),
            ("sort", sort.as_str()),
            ("per_page", &per_page),
        ])?;
        with_next(&mut url, next);
        let body: SearchBody = self.json(url, until).await?;
        Ok(SearchPage {
            crates: body.crates,
            total: body.meta.total,
            next: next_from(&body.meta),
        })
    }

    /// The URL of `path` on the API with the parameters `query`.
    fn url(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Url, ApiError> {
        reqwest::Url::parse_with_params(&format!("{}{path}", self.base_url), query)
            .map_err(|error| ApiError::BadUrl(error.to_string().into()))
    }

    /// Asks for `url` in its turn, and reads the body as `T`.
    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        url: reqwest::Url,
        until: tokio::time::Instant,
    ) -> Result<T, ApiError> {
        if self.offline {
            return Err(ApiError::Offline);
        }
        let request = self.client.get(url);
        let mut response = http::send_in_turn(request, &(&self.pacer, until))
            .await?
            // The URL of the error has the query, which may be a search the user typed.
            .map_err(|error| ApiError::Network(error.without_url()))?;
        match response.status() {
            StatusCode::OK => {},
            StatusCode::NOT_FOUND => return Err(ApiError::NotFound),
            status if http::is_transient(status) => return Err(ApiError::Transient { status }),
            status => return Err(ApiError::UnexpectedStatus { status }),
        }
        let body =
            http::read_body(&mut response, MAX_BYTES)
                .await
                .map_err(|error| match error {
                    BodyError::Network(error) => ApiError::Network(error.without_url()),
                    BodyError::TooLarge { limit } => ApiError::TooLarge { limit },
                })?;
        serde_json::from_slice(&body).map_err(ApiError::Malformed)
    }
}

/// The answer kept at `path`, if there is one that this format reads.
async fn read_stored<T>(cache: &Cache, path: PathBuf) -> Result<Option<Stored<T>>, ApiError>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let cache = cache.clone();
    blocking(move || {
        let Some(bytes) = cache.read_file(&path)? else {
            return Ok(None);
        };
        let stored = serde_json::from_slice(&bytes);
        if stored.is_err() {
            tracing::warn!(path = %path.display(), "ignoring an unreadable answer of the crates.io API in the cache");
        }
        Ok(stored.ok())
    })
    .await
    .map_err(ApiError::Cache)
}

/// Keeps `stored` at `path`, and gives back its value. A failure is logged; it is asked again next
/// time.
async fn write_stored<T: Serialize>(path: PathBuf, stored: Stored<T>) -> T {
    let written = match serde_json::to_vec(&stored) {
        Ok(json) => blocking(move || cache::write_atomically(&path, &json)).await,
        Err(error) => Err(io::Error::other(error)),
    };
    if let Err(error) = written {
        tracing::warn!(%error, "failed to keep an answer of the crates.io API in the cache");
    }
    stored.value
}

/// A key of `asked` for a file name: the start of the SHA-256 of its JSON.
fn key_of(asked: &impl Serialize) -> String {
    let json = serde_json::to_vec(asked).unwrap_or_default();
    let mut key = cache::sha256_hex(&json);
    key.truncate(32);
    key
}

/// The HTTP client for the API: redirects are not followed, as they would be sent out of turn.
fn client(network: &Network) -> Result<reqwest::Client, reqwest::Error> {
    let builder = http::client_builder(network).redirect(reqwest::redirect::Policy::none());
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder.build()
}

/// Where the next page starts, as `meta` says.
fn next_from(meta: &Meta) -> Option<Next> {
    let next_page = meta.next_page.as_deref()?;
    let next = next_of(next_page);
    if next.is_none() {
        tracing::warn!(
            "the crates.io API told of a next page in a form this client does not know; the list \
             ends here"
        );
    }
    next
}

/// Makes `url` ask for the page that `next` starts.
fn with_next(url: &mut reqwest::Url, next: Option<&Next>) {
    match next {
        None => {},
        Some(Next::Seek(seek)) => {
            url.query_pairs_mut().append_pair("seek", seek);
        },
        Some(Next::Page(page)) => {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        },
    }
}

/// Where the next page starts, from the query of the API's `next_page` (`?per_page=3&seek=…` or
/// `?q=…&page=3`). The query is taken apart rather than sent back as it is, so that only the
/// parameters this client sets are sent.
fn next_of(next_page: &str) -> Option<Next> {
    // A relative reference of only a query, resolved against any URL, decodes it.
    let url = reqwest::Url::parse(DEFAULT_BASE_URL)
        .and_then(|base| base.join(next_page))
        .ok()?;
    let mut page = None;
    for (key, value) in url.query_pairs() {
        match &*key {
            "seek" if !value.is_empty() => return Some(Next::Seek(value.into())),
            "page" => page = value.parse().ok().map(Next::Page),
            _ => {},
        }
    }
    page
}

#[cfg(test)]
mod tests {
    use super::{
        super::test_http::{serve_sequence, serve_sequence_after},
        *,
    };

    fn ok(body: &str) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            ),
            body.as_bytes().to_vec(),
        )
    }

    fn status(line: &str) -> (String, Vec<u8>) {
        (
            format!("HTTP/1.1 {line}\r\nContent-Length: 0\r\n\r\n"),
            Vec::new(),
        )
    }

    /// A client of the server at `base_url` with turns `interval` apart, caching in a temporary
    /// directory.
    fn api(base_url: &str, interval: Duration) -> (CratesIo, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let client = CratesIo::for_local_test(Cache::at(dir.path()), base_url, interval);
        (client, dir)
    }

    fn name(name: &str) -> CrateName {
        name.parse().expect("a crate name")
    }

    fn later(seconds: u64) -> tokio::time::Instant {
        let now = tokio::time::Instant::now();
        now.checked_add(Duration::from_secs(seconds)).unwrap_or(now)
    }

    /// A real answer for serde to `?include=downloads` (2026-11-04), less its `links`.
    const SERDE: &str = r#"{"crate":{"id":"serde","name":"serde","updated_at":"2026-07-18T23:05:13.266456Z","versions":null,"keywords":null,"categories":null,"badges":[],"created_at":"2014-12-05T20:20:39.487502Z","downloads":1642090576,"recent_downloads":401554042,"default_version":"1.0.229","num_versions":316,"yanked":false,"max_version":"0.0.0","newest_version":"0.0.0","max_stable_version":null,"description":"A generic serialization/deserialization framework","homepage":"https://serde.rs","documentation":"https://docs.rs/serde","repository":"https://github.com/serde-rs/serde","exact_match":false,"trustpub_only":false},"versions":null,"keywords":null,"categories":null}"#;

    #[tokio::test]
    async fn a_crate_is_read_with_its_recent_downloads() {
        let (base_url, requests) = serve_sequence(vec![ok(SERDE)]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let info = client
            .fetch_crate_info(&name("Serde"), later(10))
            .await
            .expect("an answer");
        assert_eq!(&*info.name, "serde");
        assert_eq!(info.downloads, 1_642_090_576);
        assert_eq!(info.recent_downloads, Some(401_554_042));
        assert_eq!(info.default_version.as_deref(), Some("1.0.229"));
        let head = requests.recv().expect("a request");
        assert!(
            head.starts_with("GET /api/v1/crates/Serde?include=downloads "),
            "{head}"
        );
        assert!(head.contains("chatora-rustdoc-mcp/"), "{head}");
    }

    #[tokio::test]
    async fn versions_are_read_a_page_at_a_time() {
        let page = r#"{"versions":[{"num":"1.0.229","license":"MIT OR Apache-2.0","created_at":"2026-07-18T23:05:13Z","yanked":false,"rust_version":"1.56","edition":"2021","published_by":{"login":"someone","name":"Someone"}},{"num":"1.0.0","license":"MIT/Apache-2.0","created_at":"2017-04-20T00:00:00Z","yanked":true,"rust_version":null}],"meta":{"total":316,"next_page":"?per_page=100&seek=WyIxLjAuMTMxIl0"}}"#;
        let last = r#"{"versions":[],"meta":{"total":316,"next_page":null}}"#;
        let (base_url, requests) = serve_sequence(vec![ok(page), ok(last)]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let first = client
            .fetch_versions(&name("serde"), None, later(10))
            .await
            .expect("a page");
        assert_eq!(first.total, 316);
        assert_eq!(first.versions.len(), 2);
        assert_eq!(first.versions[1].license.as_deref(), Some("MIT/Apache-2.0"));
        assert!(first.versions[1].yanked);
        assert_eq!(first.next, Some(Next::Seek("WyIxLjAuMTMxIl0".into())));
        // Nothing about who published is kept.
        let kept = serde_json::to_string(&first).expect("serializable");
        assert!(!kept.contains("someone"), "{kept}");
        let second = client
            .fetch_versions(&name("serde"), first.next.as_ref(), later(10))
            .await
            .expect("a page");
        assert_eq!(second.next, None);
        let heads: Vec<_> = requests.iter().take(2).collect();
        assert!(
            heads[0].starts_with("GET /api/v1/crates/serde/versions?per_page=100 "),
            "{}",
            heads[0]
        );
        assert!(
            heads[1].starts_with(
                "GET /api/v1/crates/serde/versions?per_page=100&seek=WyIxLjAuMTMxIl0 "
            ),
            "{}",
            heads[1]
        );
    }

    #[tokio::test]
    async fn dependents_are_counted() {
        let body =
            r#"{"dependencies":[{"id":1}],"versions":[{"num":"1.0.0"}],"meta":{"total":137336}}"#;
        let (base_url, requests) = serve_sequence(vec![ok(body)]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let count = client
            .fetch_dependents(&name("serde"), later(10))
            .await
            .expect("a count");
        assert_eq!(count, 137_336);
        let head = requests.recv().expect("a request");
        assert!(
            head.starts_with("GET /api/v1/crates/serde/reverse_dependencies?per_page=1 "),
            "{head}"
        );
    }

    #[tokio::test]
    async fn a_search_sends_the_query_encoded_and_reads_the_next_page() {
        let body = r#"{"crates":[{"name":"serde_json","description":"A JSON serialization file format","max_stable_version":"1.0.145","max_version":"1.0.145","downloads":900,"recent_downloads":100,"repository":null,"updated_at":"2026-01-01T00:00:00Z","exact_match":false}],"meta":{"total":7743,"next_page":"?q=json+parser&per_page=1&page=2","prev_page":null}}"#;
        let (base_url, requests) = serve_sequence(vec![ok(body)]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let page = client
            .fetch_search("json parser&x", Sort::Downloads, 0, None, later(10))
            .await
            .expect("a page");
        assert_eq!(page.total, 7743);
        assert_eq!(&*page.crates[0].name, "serde_json");
        assert_eq!(page.next, Some(Next::Page(2)));
        let head = requests.recv().expect("a request");
        assert!(
            head.starts_with("GET /api/v1/crates?q=json+parser%26x&sort=downloads&per_page=1 "),
            "{head}"
        );
    }

    #[tokio::test]
    async fn a_missing_crate_and_a_failing_api_are_told_apart() {
        // A server for each, so that a retry of one cannot take another's answer.
        let missing = r#"{"errors":[{"detail":"crate `nope` does not exist"}]}"#;
        let (base_url, _requests) = serve_sequence(vec![(
            format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                missing.len()
            ),
            missing.as_bytes().to_vec(),
        )]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let answer = client.fetch_crate_info(&name("nope"), later(10)).await;
        assert!(matches!(answer, Err(ApiError::NotFound)), "{answer:?}");
        let (base_url, _requests) = serve_sequence(vec![
            status("503 Service Unavailable"),
            status("503 Service Unavailable"),
            status("503 Service Unavailable"),
        ]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let answer = client.fetch_crate_info(&name("serde"), later(10)).await;
        assert!(
            matches!(&answer, Err(ApiError::Transient { status }) if *status == StatusCode::SERVICE_UNAVAILABLE),
            "{answer:?}"
        );
        let (base_url, _requests) = serve_sequence(vec![ok("{\"crate\":{}}")]);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let answer = client.fetch_crate_info(&name("serde"), later(10)).await;
        assert!(matches!(answer, Err(ApiError::Malformed(_))), "{answer:?}");
    }

    #[tokio::test]
    async fn offline_nothing_is_asked() {
        let network = Network {
            offline: true,
            ..Network::default()
        };
        let dir = tempfile::tempdir().expect("a temporary directory");
        let client = CratesIo::with_base_url(Cache::at(dir.path()), "http://127.0.0.1:9", &network)
            .expect("a client");
        assert!(client.is_offline());
        assert!(matches!(
            client.fetch_crate_info(&name("serde"), later(10)).await,
            Err(ApiError::Offline)
        ));
    }

    /// The interval of the turns in the tests of the pacer, short so that they run fast.
    const SHORT: Duration = Duration::from_millis(300);

    #[tokio::test]
    async fn turns_are_an_interval_apart_and_none_is_reserved_past_the_time_given() {
        let pacer = Pacer::new(SHORT);
        let start = tokio::time::Instant::now();
        pacer
            .turn(Duration::ZERO, later(10))
            .await
            .expect("the first turn at once");
        pacer
            .turn(Duration::ZERO, later(10))
            .await
            .expect("the second turn");
        assert!(start.elapsed() >= SHORT);
        // The next turn would come after the time given: none, at once and not reserved.
        let asked = tokio::time::Instant::now();
        assert_eq!(pacer.turn(Duration::ZERO, asked).await, Err(NoTurn));
        assert_eq!(
            pacer.turn(Duration::from_secs(60), later(10)).await,
            Err(NoTurn),
            "a wait asked for that ends past the time given"
        );
        assert!(asked.elapsed() < SHORT, "{:?}", asked.elapsed());
        pacer
            .turn(Duration::ZERO, later(10))
            .await
            .expect("the third turn");
        assert!(start.elapsed() >= SHORT * 2);
    }

    #[tokio::test]
    async fn a_turn_given_up_is_left_unused() {
        let pacer = Pacer::new(SHORT);
        let start = tokio::time::Instant::now();
        pacer
            .turn(Duration::ZERO, later(10))
            .await
            .expect("the first turn");
        // The second turn is reserved and given up while it is waited for.
        let given_up = tokio::time::timeout(SHORT / 3, pacer.turn(Duration::ZERO, later(10))).await;
        assert!(given_up.is_err(), "still waiting");
        // The third comes after it, not in its place.
        pacer
            .turn(Duration::ZERO, later(10))
            .await
            .expect("the third turn");
        assert!(start.elapsed() >= SHORT * 2, "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn waiting_turns_go_in_the_order_asked() {
        let pacer = Pacer::new(SHORT);
        let start = tokio::time::Instant::now();
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for index in 0..3 {
            let (pacer, order) = (pacer.clone(), Arc::clone(&order));
            tasks.push(tokio::spawn(async move {
                pacer.turn(Duration::ZERO, later(10)).await.expect("a turn");
                order.lock().push((index, tokio::time::Instant::now()));
            }));
            // The next task asks after this one.
            tokio::time::sleep(SHORT / 10).await;
        }
        for task in tasks {
            task.await.expect("the task ends");
        }
        let order = order.lock();
        let indexes: Vec<_> = order.iter().map(|(index, _)| *index).collect();
        assert_eq!(indexes, [0, 1, 2]);
        // Each turn is taken no sooner than its place in the order allows.
        for (place, (_, at)) in (0_u32..).zip(order.iter()) {
            assert!(at.duration_since(start) >= SHORT * place, "{place}");
        }
    }

    #[tokio::test]
    async fn a_failed_search_does_not_tell_its_query() {
        // Nothing listens on the port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port is free");
        let base_url = format!("http://{}", listener.local_addr().expect("bound"));
        drop(listener);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let error = client
            .fetch_search("secret words", Sort::Relevance, 500, None, later(10))
            .await
            .expect_err("no server");
        let mut told = format!("{error:?}");
        let mut cause: Option<&dyn std::error::Error> = Some(&error);
        while let Some(error) = cause {
            told.push(' ');
            told.push_str(&error.to_string());
            cause = error.source();
        }
        assert!(!told.contains("secret"), "{told}");
    }

    #[tokio::test]
    async fn a_retry_waits_for_its_turn_and_is_not_sent_past_the_time_given() {
        let interval = Duration::from_millis(800);
        let (base_url, requests) =
            serve_sequence(vec![status("503 Service Unavailable"), ok(SERDE)]);
        let (client, _dir) = api(&base_url, interval);
        let start = tokio::time::Instant::now();
        let info = client
            .fetch_crate_info(&name("serde"), later(10))
            .await
            .expect("an answer on the retry");
        assert_eq!(&*info.name, "serde");
        // The retry waited 500 ms, and then for its turn.
        assert!(start.elapsed() >= interval, "{:?}", start.elapsed());
        assert_eq!(requests.iter().take(2).count(), 2);

        // The retry's turn would come after the time given: the failure is returned.
        let (base_url, requests) = serve_sequence(vec![status("503 Service Unavailable")]);
        let (client, _dir) = api(&base_url, interval);
        let error = client
            .fetch_crate_info(
                &name("serde"),
                tokio::time::Instant::now() + Duration::from_millis(600),
            )
            .await
            .expect_err("no retry");
        assert!(matches!(error, ApiError::Transient { .. }), "{error}");
        assert_eq!(requests.iter().count(), 1);

        // No turn for the first attempt: nothing is sent.
        let (base_url, requests) = serve_sequence(vec![ok(SERDE)]);
        let (client, _dir) = api(&base_url, interval);
        client
            .fetch_crate_info(&name("serde"), later(10))
            .await
            .expect("an answer");
        let error = client
            .fetch_crate_info(
                &name("serde"),
                tokio::time::Instant::now() + Duration::from_millis(100),
            )
            .await
            .expect_err("no turn");
        assert!(matches!(error, ApiError::NoTurn), "{error}");
        assert_eq!(requests.iter().count(), 1);
    }

    /// Writes `value` at `path` in the cache as fetched `age` ago.
    fn keep(path: &std::path::Path, value: &str, age: Duration) {
        let fetched_at = unix_seconds(SystemTime::now()).saturating_sub(age.as_secs());
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("made");
        std::fs::write(
            path,
            format!(r#"{{"fetched_at":{fetched_at},"value":{value}}}"#),
        )
        .expect("written");
    }

    /// The `crate` of [`SERDE`], as the cache keeps it.
    fn serde_info() -> String {
        let body: serde_json::Value = serde_json::from_str(SERDE).expect("JSON");
        body["crate"].to_string()
    }

    #[tokio::test]
    async fn an_answer_is_kept_and_used_while_young_unless_asked_fresh() {
        let (base_url, requests) = serve_sequence(vec![ok(SERDE), ok(SERDE)]);
        let (client, dir) = api(&base_url, Duration::ZERO);
        let first = client
            .crate_info(&name("serde"), false, later(10))
            .await
            .expect("fetched");
        assert_eq!(first.stale, None);
        let again = client
            .crate_info(&name("Serde"), false, later(10))
            .await
            .expect("kept");
        assert_eq!(again.value, first.value);
        assert_eq!(
            requests.try_iter().count(),
            1,
            "the second is from the cache"
        );
        assert!(
            dir.path()
                .join("crates.io/v1/crates/serde/crate.json")
                .is_file()
        );
        client
            .crate_info(&name("serde"), true, later(10))
            .await
            .expect("fetched again");
        assert_eq!(requests.iter().take(1).count(), 1, "asked fresh");
    }

    #[tokio::test]
    async fn an_old_answer_stands_in_when_the_api_fails_and_the_failure_is_remembered() {
        let (base_url, requests) = serve_sequence(vec![
            status("503 Service Unavailable"),
            status("503 Service Unavailable"),
            status("503 Service Unavailable"),
        ]);
        let (client, dir) = api(&base_url, Duration::ZERO);
        let path = dir.path().join("crates.io/v1/crates/serde/crate.json");
        keep(&path, &serde_info(), CRATE_TTL * 2);
        let answer = client
            .crate_info(&name("serde"), false, later(10))
            .await
            .expect("the old answer");
        assert_eq!(answer.value.downloads, 1_642_090_576);
        let stale = answer.stale.expect("told stale");
        assert!(stale.contains("503"), "{stale}");
        assert_eq!(requests.iter().take(3).count(), 3);
        // Asked again at once: the failure is remembered, and nothing is sent.
        let answer = client
            .crate_info(&name("serde"), false, later(10))
            .await
            .expect("the old answer");
        assert!(
            answer
                .stale
                .is_some_and(|stale| stale.contains("a moment ago")),
            "remembered"
        );
        // Forgotten as `refresh` asks.
        assert!(
            dir.path()
                .join("crates.io/v1/crates/serde/crate.json.miss.json")
                .is_file()
        );
        client.forget(&name("serde")).await;
        assert!(
            !dir.path()
                .join("crates.io/v1/crates/serde/crate.json.miss.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn a_crate_that_is_not_there_is_not_answered_from_the_cache() {
        let missing = r#"{"errors":[{"detail":"crate `serde` does not exist"}]}"#;
        let (base_url, _requests) = serve_sequence(vec![(
            format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                missing.len()
            ),
            missing.as_bytes().to_vec(),
        )]);
        let (client, dir) = api(&base_url, Duration::ZERO);
        keep(
            &dir.path().join("crates.io/v1/crates/serde/crate.json"),
            &serde_info(),
            CRATE_TTL * 2,
        );
        assert!(matches!(
            client.crate_info(&name("serde"), false, later(10)).await,
            Err(ApiError::NotFound)
        ));
    }

    #[tokio::test]
    async fn offline_the_kept_answer_is_used_however_old() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let network = Network {
            offline: true,
            ..Network::default()
        };
        let client = CratesIo::with_base_url(Cache::at(dir.path()), "http://127.0.0.1:9", &network)
            .expect("a client");
        assert!(matches!(
            client.dependents(&name("serde"), false, later(10)).await,
            Err(ApiError::Offline)
        ));
        keep(
            &dir.path().join("crates.io/v1/crates/serde/dependents.json"),
            "137336",
            CRATE_TTL * 30,
        );
        let answer = client
            .dependents(&name("serde"), false, later(10))
            .await
            .expect("kept");
        assert_eq!(answer.value, 137_336);
        assert!(answer.stale.is_some_and(|stale| stale.contains("offline")));
    }

    #[tokio::test]
    async fn a_search_is_kept_without_its_query() {
        let body = r#"{"crates":[],"meta":{"total":0,"next_page":null}}"#;
        let (base_url, requests) = serve_sequence(vec![ok(body), ok(body)]);
        let (client, dir) = api(&base_url, Duration::ZERO);
        for _ in 0..2 {
            client
                .search("secret words", Sort::Relevance, 10, None, later(10))
                .await
                .expect("a page");
        }
        assert_eq!(
            requests.try_iter().count(),
            1,
            "the second is from the cache"
        );
        // Another sort is another search.
        client
            .search("secret words", Sort::Downloads, 10, None, later(10))
            .await
            .expect("a page");
        let searches: Vec<_> = std::fs::read_dir(dir.path().join("crates.io/v1/search"))
            .expect("read")
            .flatten()
            .collect();
        assert_eq!(searches.len(), 2);
        for search in searches {
            let name = search.file_name().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(search.path()).expect("read");
            assert!(
                !name.contains("secret") && !text.contains("secret"),
                "{name}: {text}"
            );
        }
    }

    #[tokio::test]
    async fn an_unreadable_answer_is_asked_again_and_pages_are_kept_apart() {
        let page =
            |next: &str| format!(r#"{{"versions":[],"meta":{{"total":316,"next_page":{next}}}}}"#);
        let (base_url, requests) = serve_sequence(vec![
            ok(SERDE),
            ok(&page(r#""?per_page=100&seek=abc""#)),
            ok(&page("null")),
        ]);
        let (client, dir) = api(&base_url, Duration::ZERO);
        let crate_dir = dir.path().join("crates.io/v1/crates/serde");
        std::fs::create_dir_all(&crate_dir).expect("made");
        std::fs::write(crate_dir.join("crate.json"), "not JSON").expect("written");
        let info = client
            .crate_info(&name("serde"), false, later(10))
            .await
            .expect("asked again");
        assert_eq!(info.stale, None);
        let first = client
            .versions(&name("serde"), None, false, later(10))
            .await
            .expect("a page");
        let second = client
            .versions(&name("serde"), first.value.next.as_ref(), false, later(10))
            .await
            .expect("a page");
        assert_eq!(second.value.next, None);
        assert_eq!(requests.iter().take(3).count(), 3);
        // Both pages are kept, apart; and asked again, from the cache.
        let again = client
            .versions(&name("serde"), first.value.next.as_ref(), false, later(10))
            .await
            .expect("kept");
        assert_eq!(again.value, second.value);
        let mut files: Vec<_> = std::fs::read_dir(&crate_dir)
            .expect("read")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files.len(), 3, "{files:?}");
        assert!(
            files.iter().any(|file| file.starts_with("versions-")),
            "{files:?}"
        );
    }

    #[tokio::test]
    async fn the_same_asked_at_once_is_asked_once() {
        let (base_url, requests) = serve_sequence_after(|_| vec![ok(SERDE)], SHORT);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let serde = name("serde");
        let (first, second) = tokio::join!(
            client.crate_info(&serde, false, later(10)),
            client.crate_info(&serde, false, later(10)),
        );
        assert_eq!(
            first.expect("an answer").value,
            second.expect("an answer").value
        );
        assert_eq!(requests.iter().count(), 1);
    }

    #[tokio::test]
    async fn the_lock_is_not_waited_for_past_the_time_given() {
        let (base_url, _requests) = serve_sequence_after(|_| vec![ok(SERDE)], SHORT * 3);
        let (client, _dir) = api(&base_url, Duration::ZERO);
        let serde = name("serde");
        let short = tokio::time::Instant::now()
            .checked_add(SHORT)
            .expect("an instant");
        let (first, second) = tokio::join!(client.crate_info(&serde, false, later(10)), async {
            // The first holds the lock.
            tokio::time::sleep(SHORT / 10).await;
            client.crate_info(&serde, false, short).await
        },);
        first.expect("an answer");
        assert!(matches!(second, Err(ApiError::NoTurn)), "{second:?}");
    }

    #[test]
    fn the_next_page_is_taken_from_the_query() {
        assert_eq!(
            next_of("?per_page=3&seek=WyIxLjAuMjI3Il0"),
            Some(Next::Seek("WyIxLjAuMjI3Il0".into()))
        );
        assert_eq!(
            next_of("?q=json+parser&per_page=3&page=3"),
            Some(Next::Page(3))
        );
        assert_eq!(next_of("?q=json"), None);
        assert_eq!(next_of("?page=x"), None);
    }
}
