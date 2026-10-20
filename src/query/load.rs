//! Loading a crate for queries: the archive of its JSON in the cache when there is one, or else
//! the JSON parsed, indexed and archived (requirements R-PF-1, R-PF-2 and R-CA-4).

use std::{
    io,
    path::{Path, PathBuf},
};

use super::across::{ARCHIVE_SCHEMA, Indexed};
use crate::data::{
    archive::{self, Header, Mapped},
    cache::{self, Cache},
    rustdoc_json::{self, ParseError},
};

/// Why a crate could not be loaded from its JSON.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The JSON could not be parsed.
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// The crate could not be archived, not even in memory.
    #[error("the crate could not be archived")]
    Archive(#[source] io::Error),
    /// The JSON could not be read.
    #[error("the rustdoc JSON could not be read")]
    Read(#[source] io::Error),
}

/// Where the archive of the JSON in `format` with `sha256` is kept, in its directory `dir`. The
/// name has the SHA-256 of the JSON, so that a new JSON gets a new archive and an archive that a
/// process has mapped is never replaced (which Windows does not allow, requirement R-CA-4).
fn archive_path(dir: &Path, format: u32, sha256: &str) -> PathBuf {
    let short = sha256.get(..16).unwrap_or(sha256);
    dir.join(format!("{format}.{short}.v{ARCHIVE_SCHEMA}.rkyv"))
}

/// The header of the archive of the JSON in `format` with `sha256`.
const fn header(format: u32, sha256: Box<str>) -> Header {
    Header {
        schema: ARCHIVE_SCHEMA,
        format_version: format,
        source_sha256: sha256,
    }
}

/// The archive of the rustdoc JSON in the cache directory `dir`, if it is there and checks out.
/// The JSON itself is not read: its format and SHA-256 are in its file names.
///
/// # Errors
///
/// Returns an error when the cache cannot be read.
pub fn open_archived(cache: &Cache, dir: &Path) -> io::Result<Option<Mapped<Indexed>>> {
    let Some((format, sha256)) = cache.docs_rs_source(dir)? else {
        return Ok(None);
    };
    let path = archive_path(dir, format, &sha256);
    Mapped::open(cache, &path, &header(format, sha256))
}

/// Parses `json`, indexes it, and archives it in the cache directory `dir`, then opens the
/// archive. When the archive cannot be written or read back, it is kept in memory for this time,
/// with a log line.
///
/// # Errors
///
/// Returns an error when the JSON cannot be parsed, or the crate cannot be archived at all.
pub fn archive_json(cache: &Cache, dir: &Path, json: &[u8]) -> Result<Mapped<Indexed>, LoadError> {
    let parsed = rustdoc_json::parse(json)?;
    let format = parsed.krate.format_version;
    let sha256: Box<str> = cache::sha256_hex(json).into();
    let indexed = Indexed::new(parsed).with_source(&sha256);
    let path = archive_path(dir, format, &sha256);
    let header = header(format, sha256);
    match archive::write(cache, &path, &header, &indexed)
        .and_then(|()| Mapped::open(cache, &path, &header))
    {
        Ok(Some(mapped)) => return Ok(mapped),
        Ok(None) => {
            tracing::warn!(path = %path.display(), "an archive just written does not check out");
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "the archive could not be written");
        },
    }
    Mapped::in_memory(&indexed).map_err(LoadError::Archive)
}

/// The archive of the JSON of the crate `stem` in the cache directory `dir`, made the first time.
///
/// It is keyed by `source`: the SHA-256 of the JSON, or another id of 64 hex digits that changes
/// with it. Without it, the JSON that `json` reads is parsed, indexed and archived there (the std
/// of a nightly, requirement R-STD-1).
///
/// # Errors
///
/// Returns an error when the JSON cannot be read or parsed, or the crate cannot be archived.
pub fn archive_of(
    cache: &Cache,
    dir: &Path,
    stem: &str,
    source: &str,
    json: impl FnOnce() -> io::Result<Box<[u8]>>,
) -> Result<Mapped<Indexed>, LoadError> {
    let short = source.get(..16).unwrap_or(source);
    let path = dir.join(format!("{stem}.{short}.v{ARCHIVE_SCHEMA}.rkyv"));
    // The format is not compared when an archive is opened.
    let opened = Mapped::open(cache, &path, &header(0, source.into()));
    match opened {
        Ok(Some(mapped)) => return Ok(mapped),
        Ok(None) => {},
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "an archive could not be read");
        },
    }
    let json = json().map_err(LoadError::Read)?;
    let parsed = rustdoc_json::parse(&json)?;
    drop(json);
    let header = header(parsed.krate.format_version, source.into());
    let indexed = Indexed::new(parsed).with_source(source);
    match archive::write(cache, &path, &header, &indexed)
        .and_then(|()| Mapped::open(cache, &path, &header))
    {
        Ok(Some(mapped)) => return Ok(mapped),
        Ok(None) => {
            tracing::warn!(path = %path.display(), "an archive just written does not check out");
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "the archive could not be written");
        },
    }
    Mapped::in_memory(&indexed).map_err(LoadError::Archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::view::CrateView;

    const JSON: &[u8] = br#"{"root":0,"crate_version":"1.0.0","includes_private":false,"index":{"0":{"id":0,"crate_id":0,"name":"c","span":null,"visibility":"public","docs":null,"links":{},"attrs":[],"deprecation":null,"inner":{"module":{"is_crate":true,"items":[],"is_stripped":false}}}},"paths":{},"external_crates":{},"target":{"triple":"x86_64-unknown-linux-gnu","target_features":[]},"format_version":61}"#;

    #[test]
    fn a_crate_is_archived_once_and_then_read_without_its_json() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = dir.path().join("docs.rs/c/1.0.0/_default");
        assert!(open_archived(&cache, &place).expect("no error").is_none());
        cache
            .write_docs_rs_json(&place, 61, JSON)
            .expect("the JSON is cached");
        assert!(open_archived(&cache, &place).expect("no error").is_none());
        let made = archive_json(&cache, &place, JSON).expect("archived");
        assert_eq!(CrateView::new(made.get()).crate_version(), Some("1.0.0"));
        let opened = open_archived(&cache, &place)
            .expect("no error")
            .expect("the archive is there");
        let view = CrateView::new(opened.get());
        assert_eq!(
            view.item(view.root()).and_then(|root| root.name.as_deref()),
            Some("c")
        );
        assert_eq!(
            view.index()
                .canonical_path(view.root())
                .map(|path| path.joined())
                .as_deref(),
            Some("c")
        );
    }

    #[test]
    fn an_archive_keyed_by_its_source_is_made_once() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let source = "cd".repeat(32);
        let mut reads = 0;
        for _ in 0..2 {
            let mapped = archive_of(&cache, dir.path(), "std", &source, || {
                reads += 1;
                Ok(Box::from(JSON))
            })
            .expect("archived");
            assert_eq!(CrateView::new(mapped.get()).crate_version(), Some("1.0.0"));
        }
        assert_eq!(
            reads, 1,
            "the second time, the archive is opened without the JSON"
        );
    }

    #[test]
    fn json_that_does_not_parse_is_an_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        assert!(matches!(
            archive_json(&cache, dir.path(), br#"{"format_version":61}"#),
            Err(LoadError::Parse(_))
        ));
    }
}
