//! The HTTP client settings and body reading that the sources share (requirements R-NW-1 to
//! R-NW-3).

use std::time::Duration;

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
    fn the_body_capacity_trusts_content_length_only_up_to_the_limit() {
        assert_eq!(capacity_for(Some(100), 1000), 100);
        assert_eq!(capacity_for(Some(u64::MAX), 1000), 1000);
        assert_eq!(capacity_for(None, 1000), 0);
    }
}
