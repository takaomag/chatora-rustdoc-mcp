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
    sync::{Arc, OnceLock},
    time::Duration,
};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::{
    crate_name::CrateName,
    http::{self, BodyError, Network, NoTurn, Pacer},
};

/// Where the crates.io API is. Kept in one place so that the tests can replace it.
pub const DEFAULT_BASE_URL: &str = "https://crates.io";

/// The least time between two requests to the API (crates.io's crawler policy).
pub const INTERVAL: Duration = Duration::from_secs(1);

/// The most items that the API gives in one page.
pub const MAX_PER_PAGE: u32 = 100;

/// Largest response held in memory. A page of 100 versions of serde is about 170 KB.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// A client for the crates.io API.
#[derive(Debug, Clone)]
pub struct CratesIo {
    client: reqwest::Client,
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
    /// A client for crates.io itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn new(network: &Network) -> Result<Self, reqwest::Error> {
        Self::with_base_url(DEFAULT_BASE_URL, network)
    }

    /// A client for a server at `base_url` that serves the crates.io API. `base_url` has no
    /// trailing `/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn with_base_url(base_url: &str, network: &Network) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: client(network)?,
            base_url: base_url.into(),
            pacer: TURNS.get_or_init(|| Pacer::new(INTERVAL)).clone(),
            offline: network.offline,
        })
    }

    /// A client for a test server on this machine, which bypasses proxies, with turns `interval`
    /// apart.
    #[cfg(test)]
    fn for_local_test(base_url: &str, interval: Duration) -> Self {
        Self {
            client: client(&Network::default()).expect("the client builds"),
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

    /// What the crate `name` is, with the downloads of the last 90 days and without its versions
    /// (decision 95), unless the turn to ask would come after `until`.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::NotFound`] when crates.io has no such crate, and another error when
    /// the API does not answer.
    pub async fn crate_info(
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
    pub async fn versions(
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
    pub async fn dependents(
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
    pub async fn search(
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
        let mut response =
            http::send_in_turn(request, async |wait| self.pacer.turn(wait, until).await)
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
    use super::{super::test_http::serve_sequence, *};

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
        let client = CratesIo::for_local_test(&base_url, Duration::ZERO);
        let info = client
            .crate_info(&name("Serde"), later(10))
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
        let client = CratesIo::for_local_test(&base_url, Duration::ZERO);
        let first = client
            .versions(&name("serde"), None, later(10))
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
            .versions(&name("serde"), first.next.as_ref(), later(10))
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
        let client = CratesIo::for_local_test(&base_url, Duration::ZERO);
        let count = client
            .dependents(&name("serde"), later(10))
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
        let client = CratesIo::for_local_test(&base_url, Duration::ZERO);
        let page = client
            .search("json parser&x", Sort::Downloads, 0, None, later(10))
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
        let missing = r#"{"errors":[{"detail":"crate `nope` does not exist"}]}"#;
        let (base_url, _requests) = serve_sequence(vec![
            (
                format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n",
                    missing.len()
                ),
                missing.as_bytes().to_vec(),
            ),
            status("503 Service Unavailable"),
            status("503 Service Unavailable"),
            status("503 Service Unavailable"),
            ok("{\"crate\":{}}"),
        ]);
        let client = CratesIo::for_local_test(&base_url, Duration::ZERO);
        assert!(matches!(
            client.crate_info(&name("nope"), later(10)).await,
            Err(ApiError::NotFound)
        ));
        assert!(matches!(
            client.crate_info(&name("serde"), later(10)).await,
            Err(ApiError::Transient { status }) if status == StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(matches!(
            client.crate_info(&name("serde"), later(10)).await,
            Err(ApiError::Malformed(_))
        ));
    }

    #[tokio::test]
    async fn offline_nothing_is_asked() {
        let network = Network {
            offline: true,
            ..Network::default()
        };
        let client = CratesIo::with_base_url("http://127.0.0.1:9", &network).expect("a client");
        assert!(client.is_offline());
        assert!(matches!(
            client.crate_info(&name("serde"), later(10)).await,
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
        let client = CratesIo::for_local_test(&base_url, Duration::ZERO);
        let error = client
            .search("secret words", Sort::Relevance, 500, None, later(10))
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
        let client = CratesIo::for_local_test(&base_url, interval);
        let start = tokio::time::Instant::now();
        let info = client
            .crate_info(&name("serde"), later(10))
            .await
            .expect("an answer on the retry");
        assert_eq!(&*info.name, "serde");
        // The retry waited 500 ms, and then for its turn.
        assert!(start.elapsed() >= interval, "{:?}", start.elapsed());
        assert_eq!(requests.iter().take(2).count(), 2);

        // The retry's turn would come after the time given: the failure is returned.
        let (base_url, requests) = serve_sequence(vec![status("503 Service Unavailable")]);
        let client = CratesIo::for_local_test(&base_url, interval);
        let error = client
            .crate_info(
                &name("serde"),
                tokio::time::Instant::now() + Duration::from_millis(600),
            )
            .await
            .expect_err("no retry");
        assert!(matches!(error, ApiError::Transient { .. }), "{error}");
        assert_eq!(requests.iter().count(), 1);

        // No turn for the first attempt: nothing is sent.
        let (base_url, requests) = serve_sequence(vec![ok(SERDE)]);
        let client = CratesIo::for_local_test(&base_url, interval);
        client
            .crate_info(&name("serde"), later(10))
            .await
            .expect("an answer");
        let error = client
            .crate_info(
                &name("serde"),
                tokio::time::Instant::now() + Duration::from_millis(100),
            )
            .await
            .expect_err("no turn");
        assert!(matches!(error, ApiError::NoTurn), "{error}");
        assert_eq!(requests.iter().count(), 1);
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
