//! Data layer: fetches rustdoc JSON, normalizes its format version, and caches it.

pub mod cache;
pub mod crate_name;
pub mod docs_rs;
pub mod format_version;
pub mod rustdoc_json;

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
/// A failure to write the cache is logged and does not fail the load.
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
    // not hold up the async workers.
    let cached = {
        let (cache, name, version) = (cache.clone(), name.clone(), version.clone());
        blocking(move || cache.read_docs_rs_json(&name, &version)).await
    }
    .map_err(LoadError::Cache)?;
    if let Some(json) = cached {
        return Ok(json);
    }
    let json = docs_rs.fetch_rustdoc_json(name, version).await?;
    // The JSON moves to the blocking thread and comes back, so that tens of megabytes are not
    // copied.
    let (json, written) = {
        let (cache, name, version) = (cache.clone(), name.clone(), version.clone());
        blocking(move || {
            let written = cache.write_docs_rs_json(&name, &version, &json);
            Ok((json, written))
        })
        .await
    }
    .map_err(LoadError::Cache)?;
    if let Err(error) = written {
        tracing::warn!(%name, %version, %error, "failed to write the rustdoc JSON to the cache");
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
