//! The HTTP client settings and body reading that the sources share (requirements R-NW-1 to
//! R-NW-3).

use std::{convert::Infallible, error::Error, sync::Arc, time::Duration};

use reqwest::{StatusCode, header};

use crate::config::Settings;

/// Identifies this server and how to reach its maintainers (requirement R-NW-1).
pub(super) const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// `network.connect_timeout_secs` unless set: the longest wait for a connection (requirement
/// R-NW-2, initial value).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// `network.read_timeout_secs` unless set: the longest silence while the body is received
/// (requirement R-NW-2, initial value). It is not a limit on the whole transfer, so that a large
/// body over a slow line is not cut off.
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// The settings of the requests to the sources, `network.*` (requirements R-NW-2 and R-DIST-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    /// The longest wait for a connection.
    pub connect_timeout: Duration,
    /// The longest silence while a body is received.
    pub read_timeout: Duration,
    /// No request is sent: only what the cache has is used (`network.offline`, requirement
    /// R-CA-7).
    pub offline: bool,
}

impl Default for Network {
    fn default() -> Self {
        Self {
            connect_timeout: CONNECT_TIMEOUT,
            read_timeout: READ_TIMEOUT,
            offline: false,
        }
    }
}

impl Network {
    /// The settings that `settings` give, or the defaults.
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        let seconds = |key| {
            settings.get(key, |value| {
                value
                    .whole_number()
                    .filter(|seconds| *seconds > 0)
                    .map(Duration::from_secs)
                    .ok_or("a whole number of seconds from 1")
            })
        };
        let default = Self::default();
        Self {
            connect_timeout: seconds("network.connect_timeout_secs")
                .unwrap_or(default.connect_timeout),
            read_timeout: seconds("network.read_timeout_secs").unwrap_or(default.read_timeout),
            offline: settings
                .get("network.offline", |value| {
                    value.boolean().ok_or("true or false")
                })
                .unwrap_or(default.offline),
        }
    }
}

/// Most memory reserved for a body before it arrives, whatever its `content-length` says. A
/// larger body grows the buffer as it comes, so a server cannot make the client reserve the whole
/// limit for each of many requests up front.
const MAX_INITIAL_CAPACITY: u64 = 64 * 1024 * 1024;

/// How many times a request is sent at most when it fails transiently (requirement R-NW-2).
const MAX_ATTEMPTS: u32 = 3;

/// The wait before the first retry. It doubles for each retry.
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(500);

/// The longest `Retry-After` that is waited for. A server that asks for more is not retried, so
/// that a request does not wait for most of a tool call's time (requirement R-ARCH-6).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);

/// The most redirects that one request follows (reqwest's default).
const MAX_REDIRECTS: usize = 10;

/// Why a body could not be read.
#[derive(Debug)]
pub(super) enum BodyError {
    /// The transfer failed, including a timeout and a body that ended early.
    Network(reqwest::Error),
    /// The body is larger than the limit.
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
}

/// The settings of the HTTP clients.
pub(super) fn client_builder(network: &Network) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(network.connect_timeout)
        .read_timeout(network.read_timeout)
        .redirect(reqwest::redirect::Policy::custom(redirect))
}

/// Follows a redirect only to the same host and port with the same scheme as the request, so
/// that the server talks only to the sources it names (README) and never goes from https to
/// http. Other redirects are errors.
fn redirect(attempt: reqwest::redirect::Attempt<'_>) -> reqwest::redirect::Action {
    let Some(first) = attempt.previous().first() else {
        return attempt.stop();
    };
    let next = attempt.url();
    let same_place = next.scheme() == first.scheme()
        && next.host_str() == first.host_str()
        && next.port_or_known_default() == first.port_or_known_default();
    if !same_place {
        let error = format!(
            "a redirect from {} to another host: {}",
            first.origin().ascii_serialization(),
            next.origin().ascii_serialization()
        );
        return attempt.error(error);
    }
    if attempt.previous().len() > MAX_REDIRECTS {
        return attempt.error("too many redirects");
    }
    attempt.follow()
}

/// Sends `request`, and sends it again after a transient failure: a 429 or 5xx status, a failed
/// connection, or a timeout (requirement R-NW-2). It waits as `Retry-After` says, or else with
/// exponential backoff, at most [`MAX_ATTEMPTS`] times in all. The last response or error is
/// returned, transient or not.
///
/// # Errors
///
/// Returns the error of the last attempt when no response came.
pub(super) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    let Ok(outcome) = send_in_turn(request, &Sleeping).await;
    outcome
}

/// What gives each attempt of [`send_in_turn`] its turn.
pub(super) trait Turns: Sync {
    /// Why no turn was given.
    type Error;

    /// Waits at least `wait`, and then for the turn.
    fn turn(&self, wait: Duration) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Every attempt in its turn after its wait: as [`send`] sends.
struct Sleeping;

impl Turns for Sleeping {
    type Error = Infallible;

    async fn turn(&self, wait: Duration) -> Result<(), Infallible> {
        tokio::time::sleep(wait).await;
        Ok(())
    }
}

/// [`send`] with each attempt sent only when `turn` gives it its turn, as a [`Pacer`] does.
/// `turn` is given the least wait before the attempt (none for the first, and the backoff or
/// `Retry-After` for a retry), and waits it out itself. When the first attempt gets no turn,
/// nothing is sent and `turn`'s error is returned; when a retry gets none, the last outcome is.
///
/// # Errors
///
/// Returns `turn`'s error when nothing was sent, and else as [`send`].
pub(super) async fn send_in_turn<T: Turns>(
    request: reqwest::RequestBuilder,
    turns: &T,
) -> Result<Result<reqwest::Response, reqwest::Error>, T::Error> {
    let mut current = request;
    let mut delay = FIRST_RETRY_DELAY;
    let mut attempt: u32 = 1;
    turns.turn(Duration::ZERO).await?;
    // The wait for the first turn is not part of the request's time in the log.
    let started = std::time::Instant::now();
    loop {
        // A request without a streaming body can always be copied for the next attempt.
        let next = current.try_clone();
        let outcome = current.send().await;
        let wait = match &outcome {
            Ok(response) if is_transient(response.status()) => {
                Some(retry_after(response).unwrap_or(delay))
            },
            Err(error) if is_transient_error(error) => Some(delay),
            Ok(_) | Err(_) => None,
        };
        match (wait, next) {
            (Some(wait), Some(next)) if attempt < MAX_ATTEMPTS && wait <= MAX_RETRY_AFTER => {
                tracing::debug!(attempt, ?wait, "retrying a request that failed transiently");
                if turns.turn(wait).await.is_err() {
                    tracing::debug!(attempt, "no turn for a retry in time");
                    return Ok(noted(outcome, attempt, started));
                }
                current = next;
                delay = delay.saturating_mul(2);
                attempt = attempt.saturating_add(1);
            },
            _ => return Ok(noted(outcome, attempt, started)),
        }
    }
}

/// `outcome`, noted for the log after `attempts` attempts since `started`.
fn noted(
    outcome: Result<reqwest::Response, reqwest::Error>,
    attempts: u32,
    started: std::time::Instant,
) -> Result<reqwest::Response, reqwest::Error> {
    let (url, status) = match &outcome {
        Ok(response) => (Some(response.url()), Some(response.status().as_u16())),
        Err(error) => (error.url(), error.status().map(|status| status.as_u16())),
    };
    super::fetches::note(url, status, attempts, started.elapsed());
    outcome
}

/// Gives turns at sending requests to one server, starts at least `interval` apart, in the order
/// asked (requirement R-NW-1: crates.io asks crawlers for one request a second). The interval is
/// between the starts of the requests, so that a slow response does not hold up the next. Clones
/// share the turns, so a process makes one for each server.
///
/// A turn is reserved when it is asked for and then waited for. A turn that is not taken, because
/// its caller stops waiting, is left unused rather than given to the next, so that two starts are
/// never closer than `interval`.
#[derive(Debug, Clone)]
pub(super) struct Pacer {
    /// When the next turn may start.
    next: Arc<parking_lot::Mutex<tokio::time::Instant>>,
    interval: Duration,
}

/// The turns of a [`Pacer`] until a time.
impl Turns for (&Pacer, tokio::time::Instant) {
    type Error = NoTurn;

    async fn turn(&self, wait: Duration) -> Result<(), NoTurn> {
        self.0.turn(wait, self.1).await
    }
}

/// No turn would come by the time given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NoTurn;

impl Pacer {
    /// Turns at least `interval` apart.
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            next: Arc::new(parking_lot::Mutex::new(tokio::time::Instant::now())),
            interval,
        }
    }

    /// Reserves the first turn at least `after` from now and waits for it, unless it would come
    /// after `until`.
    ///
    /// # Errors
    ///
    /// Returns [`NoTurn`] at once, with no turn reserved, when the turn would come after `until`.
    pub(super) async fn turn(
        &self,
        after: Duration,
        until: tokio::time::Instant,
    ) -> Result<(), NoTurn> {
        let now = tokio::time::Instant::now();
        let at = {
            let mut next = self.next.lock();
            let at = (*next).max(now.checked_add(after).unwrap_or(now));
            if at > until {
                tracing::debug!(wait = ?at.duration_since(now), "no turn in time");
                return Err(NoTurn);
            }
            *next = at.checked_add(self.interval).unwrap_or(at);
            at
        };
        let waited = at.duration_since(now);
        if waited > after {
            tracing::debug!(?waited, "waiting for a turn");
        }
        tokio::time::sleep_until(at).await;
        Ok(())
    }
}

/// Whether a response with `status` is worth trying again: 429 or a server error.
pub(super) fn is_transient(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Whether a failed request is worth trying again: the connection failed or timed out, other than
/// because the certificate was rejected.
fn is_transient_error(error: &reqwest::Error) -> bool {
    (error.is_connect() || error.is_timeout()) && !is_certificate_error(error)
}

/// Whether `error` or one of its causes is about a TLS certificate that was rejected, as when a
/// proxy intercepts TLS or a container has no CA certificates. rustls does not reach the error
/// types of reqwest, so its message is matched; this is a best effort.
pub(super) fn is_certificate_error(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate")
        {
            return true;
        }
        current = error.source();
    }
    false
}

/// The wait that the `Retry-After` header asks for, if it is a number of seconds.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

/// Reads the body of `response` in chunks, so that it never grows past `limit` bytes
/// (requirement R-NW-3).
pub(super) async fn read_body(
    response: &mut reqwest::Response,
    limit: u64,
) -> Result<Vec<u8>, BodyError> {
    // The servers send `content-length`, so the buffer is sized once instead of growing; the
    // capacity taken from the header is capped.
    let mut body = Vec::with_capacity(capacity_for(
        response.content_length(),
        limit.min(MAX_INITIAL_CAPACITY),
    ));
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Network)? {
        if exceeds(body.len().saturating_add(chunk.len()), limit) {
            return Err(BodyError::TooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// A buffer size for a body of `expected` bytes, at most `limit` bytes. An unknown size gives no
/// up-front capacity.
fn capacity_for(expected: Option<u64>, limit: u64) -> usize {
    expected
        .map(|expected| expected.min(limit))
        .and_then(|capacity| usize::try_from(capacity).ok())
        .unwrap_or(0)
}

/// Whether `len` bytes are more than `limit`.
pub(super) fn exceeds(len: usize, limit: u64) -> bool {
    u64::try_from(len).map_or(true, |len| len > limit)
}

#[cfg(test)]
mod tests {
    use super::{super::test_http::serve_sequence_with, *};

    #[test]
    fn the_timeouts_are_read_from_the_settings() {
        let settings = Settings::from_sources(
            None,
            [
                ("CHATORA_RUSTDOC_MCP_NETWORK__CONNECT_TIMEOUT_SECS", "3"),
                ("CHATORA_RUSTDOC_MCP_NETWORK__READ_TIMEOUT_SECS", "0"),
            ]
            .map(|(name, value)| (name.into(), value.into())),
        );
        assert_eq!(Network::from_settings(&settings), Network {
            connect_timeout: Duration::from_secs(3),
            read_timeout: READ_TIMEOUT,
            offline: false,
        });
        assert_eq!(settings.problems().len(), 1, "0 is not valid");
    }

    #[tokio::test]
    async fn redirects_are_followed_only_within_the_host() {
        let (base_url, _requests) = serve_sequence_with(|base_url| {
            vec![
                (
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {base_url}/next\r\nContent-Length: \
                         0\r\n\r\n"
                    ),
                    Vec::new(),
                ),
                (
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n".to_owned(),
                    b"ok".to_vec(),
                ),
            ]
        });
        let client = client_builder(&Network::default())
            .build()
            .expect("a client");
        let response = send(client.get(format!("{base_url}/first")))
            .await
            .expect("followed");
        assert_eq!(response.url().path(), "/next");

        // 127.0.0.1 and localhost are different hosts.
        let (base_url, _requests) = serve_sequence_with(|base_url| {
            let port = base_url.rsplit(':').next().unwrap_or_default();
            vec![(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/elsewhere\r\n\
                     Content-Length: 0\r\n\r\n"
                ),
                Vec::new(),
            )]
        });
        let error = send(client.get(format!("{base_url}/first")))
            .await
            .expect_err("not followed");
        assert!(error.is_redirect(), "{error}");
    }

    #[test]
    fn a_rejected_certificate_is_found_among_the_causes() {
        let rustls = std::io::Error::other("invalid peer certificate: UnknownIssuer");
        let connect = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, rustls);
        assert!(is_certificate_error(&connect));
        assert!(!is_certificate_error(&std::io::Error::other(
            "connection refused"
        )));
    }

    #[test]
    fn the_body_capacity_trusts_content_length_only_up_to_the_limit() {
        assert_eq!(capacity_for(Some(100), 1000), 100);
        assert_eq!(capacity_for(Some(u64::MAX), 1000), 1000);
        assert_eq!(capacity_for(None, 1000), 0);
    }
}
