//! Where the rustdoc JSON of std comes from: the nightly toolchain installed here, or the
//! distribution server (requirements R-STD-1 and R-STD-6).

use std::{borrow::Cow, io, path::Path};

use semver::Version;

#[cfg(feature = "local")]
use super::local_std::{self, LocalStd};
use super::{
    cache::Cache,
    dist::{self, Dist, DistError, StdCrate, StdDocs},
    target::TargetTriple,
};

/// The JSON of std that was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdJson {
    /// From the nightly toolchain installed here.
    #[cfg(feature = "local")]
    Local(LocalStd),
    /// From the distribution server, unpacked in the cache.
    Dist(StdDocs),
}

/// The JSON of std that was chosen, and why, for the provenance (requirement R-STD-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    /// The JSON.
    pub json: StdJson,
    /// Why it was chosen over the other.
    pub why: Cow<'static, str>,
    /// What the response warns about, such as the target of the JSON differing from the one
    /// asked for.
    pub warnings: Vec<Box<str>>,
}

impl StdJson {
    /// Reads the JSON of `krate`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, or is damaged in the cache.
    pub fn read(&self, cache: &Cache, krate: StdCrate) -> io::Result<Box<[u8]>> {
        match self {
            #[cfg(feature = "local")]
            Self::Local(local) => local_std::read_local_std_json(local, krate),
            Self::Dist(docs) => dist::read_std_json(cache, docs, krate),
        }
    }
}

/// Chooses the JSON of std for `target` (requirement R-STD-1).
///
/// That of the nightly toolchain installed here is taken if it has `rust-docs-json` for `target`
/// and is not older than `at_least`, the release of the project's toolchain; or else that of the
/// latest nightly from the distribution server. If the server has none for `target`, that for
/// `host` stands in, with a warning. rustup runs in `neutral_dir`, where no project settings apply.
///
/// # Errors
///
/// Returns an error when the JSON must come from the distribution server and cannot.
pub async fn choose(
    cache: &Cache,
    dist: &Dist,
    neutral_dir: &Path,
    target: &TargetTriple,
    host: &TargetTriple,
    at_least: Option<&Version>,
) -> Result<Chosen, DistError> {
    let why = match local(neutral_dir, target, at_least).await {
        Ok(chosen) => return Ok(chosen),
        Err(why) => why,
    };
    let (docs, mut warnings) = match dist::std_docs(cache, dist, target, None).await {
        Err(DistError::NotAvailable { .. }) if target != host => {
            let docs = dist::std_docs(cache, dist, host, None).await?;
            let warning = format!(
                "the nightly channel publishes no rust-docs-json for {target}, so that for {host} \
                 is used: std differs between targets (std::os and more), and {target} may have no \
                 std at all"
            );
            (docs, vec![warning.into()])
        },
        Err(error) => return Err(error),
        Ok(docs) => (docs, Vec::new()),
    };
    if docs.nightly.triple != *target && warnings.is_empty() {
        warnings.push(
            format!(
                "the rust-docs-json that the nightly channel gives for {target} is built for {}",
                docs.nightly.triple
            )
            .into(),
        );
    }
    warnings.extend(docs.warnings.iter().cloned());
    Ok(Chosen {
        json: StdJson::Dist(docs),
        why,
        warnings,
    })
}

/// The JSON of the nightly toolchain installed here, or why it is not used.
#[cfg(feature = "local")]
async fn local(
    neutral_dir: &Path,
    target: &TargetTriple,
    at_least: Option<&Version>,
) -> Result<Chosen, Cow<'static, str>> {
    let local = local_std::local_std(neutral_dir, at_least)
        .await
        .map_err(|why| Cow::Owned(why.to_string()))?;
    if local.rustc.host != *target {
        return Err(Cow::Owned(format!(
            "the rust-docs-json of the nightly toolchain here is for {}, not {target}",
            local.rustc.host
        )));
    }
    Ok(Chosen {
        json: StdJson::Local(local),
        why: Cow::Borrowed(
            "the nightly toolchain here has rust-docs-json and is not older than the project's",
        ),
        warnings: Vec::new(),
    })
}

/// Without local features, rustup is not run.
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs rustup"
)]
async fn local(
    _neutral_dir: &Path,
    _target: &TargetTriple,
    _at_least: Option<&Version>,
) -> Result<Chosen, Cow<'static, str>> {
    Err(Cow::Borrowed(
        "this server is built without local features, so it does not look for a nightly toolchain",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::dist::tests::{archive, json_files, linux, setup};

    #[tokio::test]
    async fn the_host_stands_in_for_a_target_without_rust_docs_json() {
        let archive = archive(&json_files());
        let (cache, dist, _requests, dir) = setup(&archive, None);
        let wasm: TargetTriple = "wasm32-unknown-unknown".parse().expect("a triple");
        let chosen = choose(&cache, &dist, dir.path(), &wasm, &linux(), None)
            .await
            .expect("the host's JSON");
        #[cfg_attr(
            not(feature = "local"),
            expect(
                clippy::infallible_destructuring_match,
                reason = "without local features, the JSON comes only from the distribution server"
            )
        )]
        let docs = match &chosen.json {
            StdJson::Dist(docs) => docs,
            #[cfg(feature = "local")]
            StdJson::Local(_) => panic!("from the distribution server: {chosen:?}"),
        };
        assert_eq!(docs.nightly.triple, linux());
        assert_eq!(chosen.warnings.len(), 1, "{:?}", chosen.warnings);
        assert!(chosen.warnings[0].contains("wasm32-unknown-unknown"));
        assert_eq!(
            &*chosen.json.read(&cache, StdCrate::Std).expect("std"),
            br#"{"format_version":61}"#
        );
    }
}
