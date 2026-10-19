//! The HTTP client settings and body reading that the sources share (requirements R-NW-1 to
//! R-NW-3).

use std::{error::Error, time::Duration};

use reqwest::{StatusCode, header};

/// Identifies this server and how to reach its maintainers (requirement R-NW-1).
pub(super) const USER_AGENT: &str = concat!(
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
pub(super) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
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
    let mut current = request;
    let mut delay = FIRST_RETRY_DELAY;
    let mut attempt: u32 = 1;
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
                tokio::time::sleep(wait).await;
                current = next;
                delay = delay.saturating_mul(2);
                attempt = attempt.saturating_add(1);
            },
            _ => return outcome,
        }
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
    use super::*;

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
