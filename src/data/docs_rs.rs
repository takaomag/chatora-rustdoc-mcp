//! Fetching rustdoc JSON from docs.rs (requirements R-DS-2, R-DS-4, R-DS-6, R-NW-1 to R-NW-3).
//!
//! M0a fetches only the docs.rs default target of an exact version. Retries (R-NW-2), the target
//! choice (R-TG-1), and the format-numbered URLs (R-DS-2) come in M1.

use std::{io::Read as _, sync::Arc, time::Duration};

use reqwest::StatusCode;
use semver::Version;

use super::crate_name::CrateName;

/// Where docs.rs is. Kept in one place so that a mirror can replace it later (requirement R-CA-8).
pub const DEFAULT_BASE_URL: &str = "https://docs.rs";

/// Identifies this server and how to reach its maintainers (requirement R-NW-1).
const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// Longest wait for a connection (requirement R-NW-2, initial value).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest silence while the body is received (requirement R-NW-2, initial value). It is not a
/// limit on the whole transfer, so that a large body over a slow line is not cut off.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// Largest body, before and after each zstd layer is decompressed, held in memory (requirement
/// R-NW-3, initial value).
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// The magic number at the start of a zstd frame.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// Most zstd layers to decompress. Some early builds were compressed twice (requirement R-DS-4).
const MAX_ZSTD_LAYERS: usize = 3;

/// A client for the rustdoc JSON on docs.rs.
#[derive(Debug, Clone)]
pub struct DocsRs {
    client: reqwest::Client,
    // `Arc` so that cloning the client does not copy the URL (`reqwest::Client` is an `Arc` too).
    base_url: Arc<str>,
}

/// Why rustdoc JSON could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// docs.rs has no rustdoc JSON for this version: the crate or the version does not exist, it
    /// was built before docs.rs made JSON, or its build failed (requirements R-DS-3 and R-DS-6).
    #[error("docs.rs has no rustdoc JSON for this crate version")]
    NotFound,
    /// docs.rs rejected the request as malformed.
    #[error("docs.rs rejected the request as malformed")]
    BadRequest,
    /// docs.rs is overloaded or failing; trying again later may work.
    #[error("docs.rs answered {status}; try again later")]
    Transient {
        /// The HTTP status.
        status: StatusCode,
    },
    /// docs.rs answered with a status this client does not expect.
    #[error("docs.rs answered with an unexpected status {status}")]
    UnexpectedStatus {
        /// The HTTP status.
        status: StatusCode,
    },
    /// The request or the transfer of the body failed, including timeouts and a body that ended
    /// early.
    #[error("the request to docs.rs failed")]
    Network(#[source] reqwest::Error),
    /// The body, or one of its decompressed layers, is larger than the limit.
    #[error("the rustdoc JSON is larger than {limit} bytes")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// The body is still zstd after the most layers this client decompresses.
    #[error("the rustdoc JSON is compressed more than {MAX_ZSTD_LAYERS} times")]
    TooManyLayers,
    /// A zstd layer could not be decompressed.
    #[error("the rustdoc JSON could not be decompressed")]
    Corrupt(#[source] std::io::Error),
}

impl DocsRs {
    /// A client for docs.rs itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    /// A client for a server at `base_url` that serves the docs.rs URLs, such as a mirror or a
    /// test server. `base_url` has no trailing `/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn with_base_url(base_url: &str) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    /// Fetches the rustdoc JSON of `name` at exactly `version`, built for the docs.rs default
    /// target, and returns it decompressed. It holds no spare capacity.
    ///
    /// # Errors
    ///
    /// Returns a [`FetchError`] that tells a missing JSON, a transient failure and a broken body
    /// apart.
    pub async fn fetch_rustdoc_json(
        &self,
        name: &CrateName,
        version: &Version,
    ) -> Result<Box<[u8]>, FetchError> {
        // Both parts are validated, so they cannot change the shape of the URL.
        let url = format!("{}/crate/{name}/{version}/json", self.base_url);
        let mut response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(FetchError::Network)?;
        match response.status() {
            StatusCode::OK => {},
            StatusCode::NOT_FOUND => return Err(FetchError::NotFound),
            StatusCode::BAD_REQUEST => return Err(FetchError::BadRequest),
            status if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() => {
                return Err(FetchError::Transient { status });
            },
            status => return Err(FetchError::UnexpectedStatus { status }),
        }
        // The body is read in chunks so that it never grows past the limit (requirement R-NW-3).
        // docs.rs sends `content-length`, so the buffer is sized once instead of growing; the
        // limit caps the size taken from the header.
        let mut body = Vec::with_capacity(capacity_for(response.content_length(), MAX_BYTES));
        while let Some(chunk) = response.chunk().await.map_err(FetchError::Network)? {
            if exceeds(body.len().saturating_add(chunk.len()), MAX_BYTES) {
                return Err(FetchError::TooLarge { limit: MAX_BYTES });
            }
            body.extend_from_slice(&chunk);
        }
        decompress(body, MAX_BYTES).map(Vec::into_boxed_slice)
    }
}

/// A buffer size for a body of `expected` bytes, at most `limit` bytes (plus one, to tell "at the
/// limit" from "over it"). An unknown size gives no up-front capacity.
fn capacity_for(expected: Option<u64>, limit: u64) -> usize {
    expected
        .map(|expected| expected.min(limit.saturating_add(1)))
        .and_then(|capacity| usize::try_from(capacity).ok())
        .unwrap_or(0)
}

/// Whether `len` bytes are more than `limit`.
fn exceeds(len: usize, limit: u64) -> bool {
    u64::try_from(len).map_or(true, |len| len > limit)
}

/// Decompresses zstd layers for as long as the data starts with the zstd magic number, without
/// trusting `content-type`, up to [`MAX_ZSTD_LAYERS`] layers of at most `limit` bytes each
/// (requirement R-DS-4).
fn decompress(mut data: Vec<u8>, limit: u64) -> Result<Vec<u8>, FetchError> {
    for layers in 0..MAX_ZSTD_LAYERS {
        if !data.starts_with(&ZSTD_MAGIC) {
            tracing::debug!(layers, "decompressed the rustdoc JSON");
            return Ok(data);
        }
        // docs.rs does not declare the decompressed size in the frame header (checked on
        // 2026-10-19), so the buffer grows as it is read.
        let mut decompressed = Vec::new();
        zstd::stream::read::Decoder::with_buffer(data.as_slice())
            .map_err(FetchError::Corrupt)?
            // One byte past the limit tells "exactly at the limit" from "over it".
            .take(limit.saturating_add(1))
            .read_to_end(&mut decompressed)
            .map_err(FetchError::Corrupt)?;
        if exceeds(decompressed.len(), limit) {
            return Err(FetchError::TooLarge { limit });
        }
        data = decompressed;
    }
    if data.starts_with(&ZSTD_MAGIC) {
        Err(FetchError::TooManyLayers)
    } else {
        tracing::debug!(layers = MAX_ZSTD_LAYERS, "decompressed the rustdoc JSON");
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead as _, BufReader, Write as _},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use super::*;

    const JSON: &[u8] = br#"{"format_version":61}"#;

    /// `data` compressed `layers` times.
    fn compressed(data: &[u8], layers: usize) -> Vec<u8> {
        (0..layers).fold(data.to_vec(), |data, _| {
            zstd::stream::encode_all(data.as_slice(), 0).expect("zstd compresses")
        })
    }

    #[test]
    fn the_body_capacity_trusts_content_length_only_up_to_the_limit() {
        assert_eq!(capacity_for(Some(100), 1000), 100);
        assert_eq!(capacity_for(Some(u64::MAX), 1000), 1001);
        assert_eq!(capacity_for(None, 1000), 0);
    }

    #[test]
    fn decompress_unwraps_up_to_three_layers() {
        for layers in 0..=3 {
            let data = decompress(compressed(JSON, layers), MAX_BYTES).expect("decompresses");
            assert_eq!(data, JSON, "{layers} layers");
        }
    }

    #[test]
    fn decompress_rejects_a_fourth_layer() {
        assert!(matches!(
            decompress(compressed(JSON, 4), MAX_BYTES),
            Err(FetchError::TooManyLayers)
        ));
    }

    #[test]
    fn decompress_stops_at_the_limit() {
        let data = vec![b' '; 1000];
        assert_eq!(
            decompress(compressed(&data, 1), 1000).expect("fits").len(),
            1000
        );
        assert!(matches!(
            decompress(compressed(&data, 1), 999),
            Err(FetchError::TooLarge { limit: 999 })
        ));
    }

    #[test]
    fn decompress_rejects_broken_zstd() {
        let mut garbage = ZSTD_MAGIC.to_vec();
        garbage.extend_from_slice(b"not zstd");
        assert!(matches!(
            decompress(garbage, MAX_BYTES),
            Err(FetchError::Corrupt(_))
        ));
        let whole = compressed(&[b'x'; 10_000], 1);
        // The magic number and one more byte: a frame that ends early.
        let truncated = whole[..5].to_vec();
        assert!(matches!(
            decompress(truncated, MAX_BYTES),
            Err(FetchError::Corrupt(_))
        ));
    }

    /// Serves one HTTP response on a local port, and sends back the request head it received.
    fn serve_once(head: &str, body: &[u8]) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
        let base_url = format!("http://{}", listener.local_addr().expect("bound"));
        let (sender, request) = mpsc::channel();
        let (head, body) = (head.to_owned(), body.to_vec());
        thread::spawn(move || {
            let (stream, _) = listener.accept().expect("the client connects");
            let mut reader = BufReader::new(stream);
            let mut request_head = String::new();
            loop {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .expect("the request is readable");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                request_head.push_str(&line);
            }
            let mut stream = reader.into_inner();
            stream
                .write_all(head.as_bytes())
                .expect("the response is writable");
            stream.write_all(&body).expect("the response is writable");
            sender.send(request_head).expect("the test waits");
        });
        (base_url, request)
    }

    fn serde() -> (CrateName, Version) {
        (
            "serde".parse().expect("valid"),
            Version::parse("1.0.228").expect("valid"),
        )
    }

    #[tokio::test]
    async fn fetch_sends_the_user_agent_and_decompresses_the_body() {
        let body = compressed(JSON, 2);
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        let (base_url, request) = serve_once(&head, &body);
        let (name, version) = serde();
        let docs_rs = DocsRs::with_base_url(&base_url).expect("the client builds");
        let json = docs_rs
            .fetch_rustdoc_json(&name, &version)
            .await
            .expect("fetches");
        assert_eq!(&*json, JSON);
        let request = request.recv().expect("the server saw the request");
        assert!(
            request.starts_with("GET /crate/serde/1.0.228/json HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.to_ascii_lowercase().contains(&format!(
                "user-agent: {}\r\n",
                USER_AGENT.to_ascii_lowercase()
            )),
            "{request}"
        );
    }

    #[tokio::test]
    async fn fetch_classifies_error_statuses() {
        type Check = fn(&FetchError) -> bool;
        let cases: [(&str, Check); 4] = [
            ("404 Not Found", |error| {
                matches!(error, FetchError::NotFound)
            }),
            ("400 Bad Request", |error| {
                matches!(error, FetchError::BadRequest)
            }),
            ("503 Service Unavailable", |error| {
                matches!(error, FetchError::Transient { .. })
            }),
            ("418 I'm a teapot", |error| {
                matches!(error, FetchError::UnexpectedStatus { .. })
            }),
        ];
        let (name, version) = serde();
        for (status, expected) in cases {
            let head = format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\n\r\n");
            let (base_url, _request) = serve_once(&head, &[]);
            let docs_rs = DocsRs::with_base_url(&base_url).expect("the client builds");
            let error = docs_rs
                .fetch_rustdoc_json(&name, &version)
                .await
                .expect_err("fails");
            assert!(expected(&error), "{status}: {error:?}");
        }
    }

    /// A body that ends before its `content-length` is a network error, not a broken zstd body
    /// (the question in Q-30).
    #[tokio::test]
    async fn a_body_cut_short_is_a_network_error() {
        let body = compressed(&[b'x'; 10_000], 1);
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/zstd\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        let short = &body[..body.len().saturating_sub(1)];
        let (base_url, _request) = serve_once(&head, short);
        let (name, version) = serde();
        let docs_rs = DocsRs::with_base_url(&base_url).expect("the client builds");
        let error = docs_rs
            .fetch_rustdoc_json(&name, &version)
            .await
            .expect_err("fails");
        assert!(matches!(error, FetchError::Network(_)), "{error:?}");
    }
}
