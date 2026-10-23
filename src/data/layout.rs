//! The layout of the cache directory: the names at its top, and the limit that each counts toward.
//!
//! Which of the two limits of the garbage collection what is under a name counts toward is
//! requirement R-CA-5 (decided in M2f's F1, 4). The directory is this server's when it has its
//! marker; one without it is adopted only when it has nothing but these names (decided in M2f's
//! F1, 5, and F1a).
//!
//! A binary without local builds knows the names of what they make, too, as the servers of both
//! share the cache.

use std::{fs, io, path::Path, time::Duration};

use super::cache;
use crate::config::Settings;

/// The marker of the cache directory, which tells that this server owns what is in it: a visible
/// file, whose presence alone tells it (decided in M2f's F1a, 1).
pub const MARKER: &str = "chatora-rustdoc-mcp.cache";

/// What [`MARKER`] says to a person who finds it.
const MARKER_TEXT: &str = "This directory is the cache of chatora-rustdoc-mcp, which removes what \
                           it no longer needs. `chatora-rustdoc-mcp cache ls` shows what is in \
                           it.\n";

/// The tag that tells backup tools that the directory is a cache (<https://bford.info/cachedir/>).
/// cargo's target directories have one too, so it does not tell that this server owns it.
pub const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

/// What [`CACHEDIR_TAG`] has: its signature, then comments.
const CACHEDIR_TAG_TEXT: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a \
                                 cache directory tag created by chatora-rustdoc-mcp.\n# For \
                                 information about cache directory tags see \
                                 https://bford.info/cachedir/\n";

/// The rustdoc JSON of docs.rs and its archives.
pub const DOCS_RS: &str = "docs.rs";
/// The files of the sparse index of crates.io.
pub const INDEX: &str = "index.crates.io";
/// The `.crate` files of static.crates.io.
pub const STATIC_CRATES: &str = "static.crates.io";
/// The JSON of std and its archives.
pub const STD: &str = "std";
/// What the toolchains were found to do.
pub const TOOLCHAINS: &str = "toolchains";
/// The locks between processes (requirement R-CA-4).
pub const LOCKS: &str = "locks";
/// The records of the children of the servers.
pub const RECORDS: &str = "procs";
/// The working directories of the servers.
pub const WORK: &str = "work";
/// The target directories of the local builds.
pub const TARGETS: &str = "t";
/// The documentation of the local builds.
pub const LOCAL: &str = "local";
/// The records of `build_external_docs`.
pub const EXTERNAL: &str = "ext";

/// The file in a target directory that tells what it is the target directory of, for `cache ls`
/// (decided in M2f's F1a).
pub const TARGET_ABOUT: &str = "about.json";

/// The file in the place of a local build that records it.
pub const LOCAL_RECORD: &str = "record.json";

/// Which limit of the garbage collection what is under a name of the cache counts toward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// What is fetched, and what is made of it, which fetching again makes again:
    /// `cache.docs_max_size`.
    Docs,
    /// What builds make, which only a build makes again: `cache.target_max_size` and
    /// `cache.target_max_age_days`, for the local builds too (decided in M2f's F1, 4).
    Targets,
    /// What is kept as long as a server needs it, which the garbage collection does not remove:
    /// the locks, and what the servers that stopped left, which they sweep themselves.
    Kept,
}

/// The class of what is under the name `name` at the top of the cache, if it is one of this
/// server's.
#[must_use]
pub fn class_of(name: &str) -> Option<Class> {
    match name {
        DOCS_RS | INDEX | STATIC_CRATES | STD | TOOLCHAINS => Some(Class::Docs),
        TARGETS | LOCAL | EXTERNAL => Some(Class::Targets),
        LOCKS | RECORDS | WORK | MARKER | CACHEDIR_TAG => Some(Class::Kept),
        _ => None,
    }
}

/// The limits of the garbage collection, from their settings or their defaults (requirement
/// R-CA-5, §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How much what is fetched may take: `cache.docs_max_size`.
    pub docs_max_size: u64,
    /// How much what builds make may take: `cache.target_max_size`.
    pub target_max_size: u64,
    /// How long what builds make is kept unused: `cache.target_max_age_days`.
    pub target_max_age: Duration,
}

/// `cache.docs_max_size` unless set: 5GB.
const DEFAULT_DOCS_MAX_SIZE: u64 = 5_000_000_000;

/// `cache.target_max_size` unless set: 20GB.
const DEFAULT_TARGET_MAX_SIZE: u64 = 20_000_000_000;

/// `cache.target_max_age_days` unless set.
const DEFAULT_TARGET_MAX_AGE_DAYS: u64 = 30;

impl Limits {
    /// The limits that `settings` set, or their defaults. A value that is not one is reported, and
    /// the default used: 0 days too, as for the logs (decided in M2f's F1a, 2).
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        let size = |value: crate::config::Value<'_>| {
            value.size().ok_or("a size with its unit, such as \"5GB\"")
        };
        let days = settings
            .get("cache.target_max_age_days", |value| {
                value
                    .whole_number()
                    .filter(|days| *days > 0)
                    .ok_or("a whole number from 1")
            })
            .unwrap_or(DEFAULT_TARGET_MAX_AGE_DAYS);
        Self {
            docs_max_size: settings
                .get("cache.docs_max_size", size)
                .unwrap_or(DEFAULT_DOCS_MAX_SIZE),
            target_max_size: settings
                .get("cache.target_max_size", size)
                .unwrap_or(DEFAULT_TARGET_MAX_SIZE),
            target_max_age: Duration::from_secs(days.saturating_mul(24 * 60 * 60)),
        }
    }
}

/// Whether the cache directory is this server's.
#[derive(Debug, PartialEq, Eq)]
pub enum Ownership {
    /// It was not there, or empty: it is made, with the marker.
    Created,
    /// It has the marker.
    Owned,
    /// It had no marker, but nothing other than this server's names: the marker is written.
    Adopted,
    /// It has no marker, and these names that are not this server's: nothing in it is removed.
    Foreign(Vec<String>),
}

impl Ownership {
    /// Whether what is in the directory may be removed.
    #[must_use]
    pub const fn owned(&self) -> bool {
        !matches!(self, Self::Foreign(_))
    }
}

/// Whether the cache directory at `root` has the marker, as a regular file, not a link.
///
/// It reads the file system, so it is for a blocking thread.
#[must_use]
pub fn is_owned(root: &Path) -> bool {
    fs::symlink_metadata(root.join(MARKER)).is_ok_and(|metadata| metadata.is_file())
}

/// Tells whether the cache directory at `root` is this server's, and makes it so when it may.
///
/// It is made with the marker when it is not there or empty, and adopted when it has nothing but
/// this server's names, as an earlier version made it (decided in M2f's F1, 5). The tag of a cache
/// directory is written too, unless it is foreign.
///
/// It reads and writes the file system, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when the directory cannot be read, or the marker cannot be written.
pub fn claim(root: &Path) -> io::Result<Ownership> {
    let ownership = ownership(root)?;
    match ownership {
        Ownership::Foreign(_) => return Ok(ownership),
        Ownership::Created | Ownership::Adopted => {
            cache::write_atomically(&root.join(MARKER), MARKER_TEXT.as_bytes())?;
        },
        Ownership::Owned => {},
    }
    if fs::symlink_metadata(root.join(CACHEDIR_TAG)).is_err() {
        cache::write_atomically(&root.join(CACHEDIR_TAG), CACHEDIR_TAG_TEXT.as_bytes())?;
    }
    Ok(ownership)
}

/// What [`claim`] finds the cache directory at `root` to be, writing nothing, as a dry run of the
/// garbage collection tells it.
///
/// It reads the file system, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when the directory cannot be read.
pub fn ownership(root: &Path) -> io::Result<Ownership> {
    if is_owned(root) {
        return Ok(Ownership::Owned);
    }
    let (names, unknown) = names_in(root)?;
    if !unknown.is_empty() {
        return Ok(Ownership::Foreign(unknown));
    }
    // Not names that other tools use too alone, as `t` or `work`.
    if !names.is_empty()
        && !names
            .iter()
            .any(|name| DISTINCTIVE.contains(&name.as_str()))
    {
        return Ok(Ownership::Foreign(names));
    }
    Ok(if names.is_empty() {
        Ownership::Created
    } else {
        Ownership::Adopted
    })
}

/// The names at the top of the cache directory at `root` that this server does not put there,
/// sorted, which keep it from adopting a directory without its marker.
///
/// It reads the file system, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when the directory cannot be read.
pub fn unknown_names(root: &Path) -> io::Result<Vec<String>> {
    names_in(root).map(|(_, unknown)| unknown)
}

/// The names that only this server puts at the top of its cache, one of which a directory without
/// the marker has to have to be adopted.
const DISTINCTIVE: [&str; 5] = [DOCS_RS, INDEX, STATIC_CRATES, TOOLCHAINS, RECORDS];

/// The names at the top of the cache directory at `root`, none when it is not there, and those of
/// them that this server does not put there, both sorted.
fn names_in(root: &Path) -> io::Result<(Vec<String>, Vec<String>)> {
    let mut names = match fs::read_dir(root) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<io::Result<Vec<String>>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    names.sort();
    let unknown: Vec<String> = names
        .iter()
        .filter(|name| !is_known(name))
        .cloned()
        .collect();
    Ok((names, unknown))
}

/// Whether `name`, at the top of a cache directory without the marker, is one that this server
/// makes there: one of its names, a temporary file of a write of one, or a file that an OS makes
/// in every directory.
fn is_known(name: &str) -> bool {
    class_of(name).is_some()
        || matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini")
        || name
            .strip_prefix('.')
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .is_some_and(|rest| rest.starts_with(MARKER) || rest.starts_with(CACHEDIR_TAG))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_in(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .expect("readable")
            .map(|entry| {
                entry
                    .expect("readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn settings(variables: &[(&str, &str)]) -> Settings {
        Settings::from_sources(
            None,
            variables
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into())),
        )
    }

    #[test]
    fn the_limits_are_set_or_their_defaults() {
        let set = settings(&[
            ("CHATORA_RUSTDOC_MCP_CACHE__DOCS_MAX_SIZE", "1GB"),
            ("CHATORA_RUSTDOC_MCP_CACHE__TARGET_MAX_SIZE", "2GiB"),
            ("CHATORA_RUSTDOC_MCP_CACHE__TARGET_MAX_AGE_DAYS", "7"),
        ]);
        assert_eq!(Limits::from_settings(&set), Limits {
            docs_max_size: 1_000_000_000,
            target_max_size: 2 * 1024 * 1024 * 1024,
            target_max_age: Duration::from_hours(7 * 24),
        });
        let wrong = settings(&[
            ("CHATORA_RUSTDOC_MCP_CACHE__DOCS_MAX_SIZE", "a lot"),
            ("CHATORA_RUSTDOC_MCP_CACHE__TARGET_MAX_AGE_DAYS", "0"),
        ]);
        assert_eq!(Limits::from_settings(&wrong), Limits {
            docs_max_size: DEFAULT_DOCS_MAX_SIZE,
            target_max_size: DEFAULT_TARGET_MAX_SIZE,
            target_max_age: Duration::from_hours(DEFAULT_TARGET_MAX_AGE_DAYS * 24),
        });
        assert_eq!(wrong.problems().len(), 2, "{:?}", wrong.problems());
    }

    #[test]
    fn a_cache_that_is_not_there_is_made_with_the_marker_and_the_tag() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("cache");
        assert_eq!(claim(&root).expect("claimed"), Ownership::Created);
        assert_eq!(names_in(&root), [CACHEDIR_TAG, MARKER]);
        let tag = fs::read_to_string(root.join(CACHEDIR_TAG)).expect("readable");
        assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55"));
        assert!(is_owned(&root));
        assert_eq!(claim(&root).expect("claimed"), Ownership::Owned);
    }

    #[test]
    fn a_cache_of_an_earlier_version_is_adopted() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        for name in [DOCS_RS, STD, LOCKS, TARGETS, ".DS_Store"] {
            fs::create_dir_all(root.join(name)).expect("made");
        }
        assert_eq!(claim(root).expect("claimed"), Ownership::Adopted);
        assert!(is_owned(root));
    }

    #[test]
    fn a_directory_with_other_things_is_not_adopted() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        // As cargo's target directory, which has a tag of its own.
        fs::create_dir_all(root.join("debug")).expect("made");
        fs::write(
            root.join(CACHEDIR_TAG),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .expect("written");
        fs::create_dir_all(root.join(DOCS_RS)).expect("made");
        let ownership = claim(root).expect("claimed");
        assert_eq!(ownership, Ownership::Foreign(vec!["debug".to_owned()]));
        assert!(!ownership.owned());
        assert!(!is_owned(root));
    }

    #[test]
    fn names_that_other_tools_use_too_are_not_adopted_alone() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for name in [TARGETS, WORK] {
            fs::create_dir_all(dir.path().join(name)).expect("made");
        }
        assert_eq!(
            claim(dir.path()).expect("claimed"),
            Ownership::Foreign(vec![TARGETS.to_owned(), WORK.to_owned()])
        );
    }

    #[test]
    fn a_marker_that_is_a_directory_does_not_tell_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        fs::create_dir_all(dir.path().join(MARKER)).expect("made");
        assert!(!is_owned(dir.path()));
    }

    #[test]
    fn the_temporary_files_of_the_marker_are_known() {
        assert!(is_known(&format!(".{MARKER}.12.0.tmp")));
        assert!(is_known(&format!(".{CACHEDIR_TAG}.12.0.tmp")));
        assert!(!is_known(".other.12.0.tmp"));
        assert!(!is_known("config.toml"));
    }
}
