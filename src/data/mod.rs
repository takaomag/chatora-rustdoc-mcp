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

use semver::Version;

use self::{
    cache::Cache,
    crate_name::CrateName,
    docs_rs::{DocsRs, FetchError},
};

/// Why rustdoc JSON could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// It was not cached, and fetching it from docs.rs failed.
    #[error(transparent)]
    Fetch(#[from] FetchError),
    /// The cached file exists but could not be read, or the blocking task that uses the cache was
    /// cancelled (when the runtime shuts down).
    #[error("the cache of rustdoc JSON could not be used")]
    Cache(#[source] std::io::Error),
}

/// Returns the rustdoc JSON of `name` at exactly `version` for the docs.rs default target: from
/// the cache if it is there, or else fetched from docs.rs and then cached.
///
/// Only a body whose `format_version` can be read is cached, so that a body that is not rustdoc
/// JSON (such as the error page of a proxy) is fetched again next time rather than kept (requirement
/// R-CA-4). It is still returned, for the caller to report as unparsable. A failure to write the
/// cache is logged and does not fail the load.
///
/// # Errors
///
/// Returns an error when the cached file cannot be read or the fetch fails.
pub async fn docs_rs_json(
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
) -> Result<Box<[u8]>, LoadError> {
    // The cache is read and written on a blocking thread, so that tens of megabytes of disk I/O do
    // not hold up the async workers. The path moves to the thread and back, rather than copies of
    // the cache, the name and the version.
    let path = cache.docs_rs_json_path(name, version);
    let reader = cache.clone();
    let (path, cached) = blocking(move || reader.read_file(&path).map(|cached| (path, cached)))
        .await
        .map_err(LoadError::Cache)?;
    if let Some(json) = cached {
        return Ok(json);
    }
    let json = docs_rs.fetch_rustdoc_json(name, version).await?;
    // The JSON moves to the blocking thread and comes back, so that tens of megabytes are not
    // copied.
    let (json, written) = blocking(move || {
        let written = match format_version::read(&json) {
            Ok(_) => Some(cache::write_atomically(&path, &json)),
            Err(_) => None,
        };
        Ok((json, written))
    })
    .await
    .map_err(LoadError::Cache)?;
    match written {
        Some(Ok(())) => {},
        Some(Err(error)) => {
            tracing::warn!(%name, %version, %error, "failed to write the rustdoc JSON to the cache");
        },
        None => {
            tracing::warn!(%name, %version, "not caching a body from docs.rs that is not rustdoc JSON");
        },
    }
    Ok(json)
}

/// Runs `work` on a blocking thread. A panic in it is resumed here.
async fn blocking<T, F>(work: F) -> std::io::Result<T>
where
    F: FnOnce() -> std::io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| std::io::Error::other(crate::task::resume_if_panic(error)))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serde() -> (CrateName, Version) {
        (
            "serde".parse().expect("valid"),
            Version::parse("1.0.228").expect("valid"),
        )
    }

    /// Loads the JSON of serde from a server that answers `body`, with an empty cache.
    async fn load(body: &[u8]) -> (Box<[u8]>, Option<Box<[u8]>>) {
        let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len());
        let (base_url, _request) = test_http::serve_once(&head, body);
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let docs_rs = DocsRs::for_local_test(&base_url);
        let (name, version) = serde();
        let json = docs_rs_json(&cache, &docs_rs, &name, &version)
            .await
            .expect("loads");
        let cached = cache.read_docs_rs_json(&name, &version).expect("reads");
        (json, cached)
    }

    #[tokio::test]
    async fn rustdoc_json_is_cached() {
        let json = br#"{"root":0,"format_version":61}"#;
        let (loaded, cached) = load(json).await;
        assert_eq!(&*loaded, json);
        assert_eq!(cached.as_deref(), Some(&json[..]));
    }

    #[tokio::test]
    async fn a_body_that_is_not_rustdoc_json_is_returned_but_not_cached() {
        let page = b"<html>blocked by the proxy</html>";
        let (loaded, cached) = load(page).await;
        assert_eq!(&*loaded, page);
        assert_eq!(cached, None);
    }
}
