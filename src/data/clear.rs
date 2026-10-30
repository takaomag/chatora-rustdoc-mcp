//! Clearing the cache: all that it keeps, or what it keeps of one crate (requirement R-CA-5,
//! decided in M2f's F1, 9, and F1c).
//!
//! What is cleared goes as the garbage collection removes it ([`gc::run`]): under the locks that
//! its writers take, which are tried without waiting, so that what is in use is skipped and told;
//! but even when it was just used, as it was asked for. The locks, the records of the processes,
//! the work directories, the marker and what this server does not know stay.

use std::{
    collections::HashSet,
    fmt,
    path::{Component, Path, PathBuf},
};

use semver::Version;

use super::{
    cache::Cache,
    crate_name::CrateName,
    gc::{self, NotRun, Outcome, Reason, Removal},
    inventory::{self, Entry, Kind},
    layout,
};

/// What `cache clear` is asked to clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// All that the cache keeps (decided in M2f's F1c, 1).
    All,
    /// What it keeps of a crate, of one version or of all.
    Crate {
        /// The crate.
        name: CrateName,
        /// The version, in full; `None` for all of them.
        version: Option<Version>,
    },
}

impl fmt::Display for Asked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("the cache"),
            Self::Crate {
                name,
                version: None,
            } => write!(f, "{}", name.as_str()),
            Self::Crate {
                name,
                version: Some(version),
            } => write!(f, "{}@{version}", name.as_str()),
        }
    }
}

/// Clears from `cache` what `asked` names; in a dry run, only tells what it would remove.
///
/// It reads and removes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns why it did not run: the cache is not this server's, or it cannot be read.
pub fn clear(cache: &Cache, asked: &Asked, dry_run: bool) -> Result<Outcome, NotRun> {
    let root = cache.dir();
    gc::owned(root, dry_run)?;
    if !dry_run {
        gc::empty_trash_unless_collecting(root);
    }
    let inventory = inventory::take(root, None)?;
    let pairs = gc::pairs(root, &inventory);
    let bodies = gc::own_bodies(&pairs, &inventory);
    let in_use = gc::in_use(root);
    // A build goes with its records, after them, which would else find it broken: never alone.
    let of_records: HashSet<&PathBuf> = pairs.values().collect();
    let (busy, chosen): (Vec<&Entry>, Vec<&Entry>) = inventory
        .entries
        .iter()
        .filter(|entry| {
            entry
                .class
                .is_some_and(|class| gc::COLLECTED.contains(&class))
                && !matches!(entry.kind, Kind::Kept | Kind::Unknown)
                && !of_records.contains(&entry.path)
                && covers(root, asked, entry)
        })
        .partition(|entry| in_use.contains(&entry.path));
    let sizes = |path: &PathBuf| {
        inventory
            .entries
            .iter()
            .find(|entry| entry.path == *path)
            .map_or(0, |entry| entry.bytes)
    };
    let removals = chosen
        .into_iter()
        .map(|entry| {
            let body = bodies.get(&entry.path);
            Removal {
                entry: entry.clone(),
                reason: Reason::Asked,
                bodies: body.cloned().into_iter().collect(),
                bytes: entry.bytes.saturating_add(body.map_or(0, sizes)),
            }
        })
        .collect();
    let mut outcome = gc::run(cache, removals, dry_run);
    // A build of more records than one goes once they all went.
    let gone: HashSet<PathBuf> = outcome
        .removed
        .iter()
        .map(|removal| removal.entry.path.clone())
        .collect();
    let shared: Vec<Removal> = inventory
        .entries
        .iter()
        .filter(|entry| {
            entry.kind == Kind::Local
                && of_records.contains(&entry.path)
                && !bodies.values().any(|body| *body == entry.path)
                && pairs
                    .iter()
                    .filter(|(_, body)| **body == entry.path)
                    .all(|(record, _)| gone.contains(record))
        })
        .map(|entry| Removal {
            entry: entry.clone(),
            reason: Reason::Asked,
            bodies: Vec::new(),
            bytes: entry.bytes,
        })
        .collect();
    let then = gc::run(cache, shared, dry_run);
    outcome.removed.extend(then.removed);
    outcome.in_use.extend(then.in_use);
    outcome.left.extend(then.left);
    outcome
        .in_use
        .extend(busy.into_iter().map(|entry| entry.path.clone()));
    outcome.complete = inventory.complete;
    Ok(outcome)
}

/// Whether `asked` covers the place `entry` of the cache at `root`.
fn covers(root: &Path, asked: &Asked, entry: &Entry) -> bool {
    let Asked::Crate { name, version } = asked else {
        return true;
    };
    let key = name.cache_key();
    let parts: Vec<&str> = entry
        .path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .collect();
    let is = |name: &str| keyed(name) == key;
    match (entry.kind, parts.as_slice()) {
        // Of every target; the old layouts named the crate as it was asked for.
        (Kind::DocsRs | Kind::OldLayout, [top, crate_name, release, ..])
            if *top == layout::DOCS_RS =>
        {
            is(crate_name) && of_version(version.as_ref(), release)
        },
        (Kind::Crate, [_, crate_name, release]) => {
            is(crate_name) && of_version(version.as_ref(), release)
        },
        // Of every version: only when they all go (decided in M2f's F1c, 4), with the failure to
        // fetch it that is remembered next to it.
        (Kind::Index, [.., file]) => {
            version.is_none() && is(file.strip_suffix(".miss.json").unwrap_or(file))
        },
        #[cfg(feature = "local")]
        (Kind::External, _) => external::covers(root, &entry.path, (&key, version.as_ref())),
        _ => {
            let _ = root;
            false
        },
    }
}

/// `name` as the cache keys a crate's name: lowercase, with `_` written as `-`.
fn keyed(name: &str) -> String {
    name.to_ascii_lowercase().replace('_', "-")
}

/// Whether the version `release`, as a directory or a record names it, is `version`: any, when it
/// is `None`; without its build metadata, when `version` has none (decided in M2f's F1c, 5).
fn of_version(version: Option<&Version>, release: &str) -> bool {
    let Some(version) = version else {
        return true;
    };
    Version::parse(release).is_ok_and(|release| {
        if version.build.is_empty() {
            (release.major, release.minor, release.patch, &release.pre)
                == (version.major, version.minor, version.patch, &version.pre)
        } else {
            release == *version
        }
    })
}

/// Which records of `build_external_docs` are of a crate (decided in M2f's F1c, 3).
#[cfg(feature = "local")]
mod external {
    use std::{fs, path::Path};

    use semver::Version;
    use serde_json::Value;

    use super::{keyed, of_version};
    use crate::data::{gc, layout, metadata::PackageId};

    /// The kinds of records of one crate's release: not those of a workspace, which has many.
    const OF_A_RELEASE: [&str; 4] = ["release", "git", "git_ref", "target"];

    /// Whether the record `place` in the cache at `root` is of the crate whose cache key is `key`,
    /// of `version` if given: by its key, with the version that the build or the resolution of a
    /// ref tells; or, when the external tool failed to make it, by the dependency of its temporary
    /// project. One that tells neither stays.
    pub(super) fn covers(
        root: &Path,
        place: &Path,
        (key, version): (&str, Option<&Version>),
    ) -> bool {
        let dir = root.join(place);
        let of_a_release = place
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|kind| OF_A_RELEASE.iter().any(|of| kind == *of));
        if !of_a_release {
            return false;
        }
        let read = |name: &str| -> Option<Value> {
            serde_json::from_slice(&fs::read(dir.join(name)).ok()?).ok()
        };
        let index = read(layout::LOCAL_RECORD);
        let resolved = read("resolved.json");
        let Some(found) = index
            .as_ref()
            .or(resolved.as_ref())
            .and_then(|file| file.get("key"))
        else {
            return of_temporary_project(&dir, key, version);
        };
        // A package of the project, by its id, which tells its version.
        let (name, release) = if found.get("kind").and_then(Value::as_str) == Some("target") {
            let package = found.get("package").and_then(Value::as_str).unwrap_or("");
            let id = PackageId(package.into());
            let Some((name, release)) = id.name_and_version() else {
                return false;
            };
            (name.to_owned(), Some(release.to_owned()))
        } else {
            let name = found.get("crate").and_then(Value::as_str).unwrap_or("");
            let release = found
                .get("version")
                .or_else(|| resolved.as_ref().and_then(|file| file.get("version")))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| built_version(root, index.as_ref()));
            (name.to_owned(), release)
        };
        keyed(&name) == key
            && (version.is_none() || release.is_some_and(|release| of_version(version, &release)))
    }

    /// The version of the crate that the build of the record `index` in the cache at `root`
    /// documents, as the build's record tells it.
    fn built_version(root: &Path, index: Option<&Value>) -> Option<String> {
        let published = index?.get("published")?.as_str()?;
        let body = root.join(gc::published_body(published)?);
        let record: Value =
            serde_json::from_slice(&fs::read(body.join(layout::LOCAL_RECORD)).ok()?).ok()?;
        Some(record.get("crate_version")?.as_str()?.to_owned())
    }

    /// Whether the temporary project of the record in `dir`, which failed, depends on the crate
    /// whose cache key is `key`, of `version` if given: a release of crates.io names its version
    /// (`=1.0.0`), a crate of a git repository none.
    fn of_temporary_project(dir: &Path, key: &str, version: Option<&Version>) -> bool {
        ["project", "project-default"].iter().any(|project| {
            let Ok(text) = fs::read_to_string(dir.join(project).join("Cargo.toml")) else {
                return false;
            };
            let Ok(manifest) = text.parse::<toml::Table>() else {
                return false;
            };
            let Some(dependencies) = manifest.get("dependencies").and_then(toml::Value::as_table)
            else {
                return false;
            };
            dependencies.iter().any(|(name, dependency)| {
                let release = dependency
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .map(|release| release.trim_start_matches('='));
                keyed(name) == key
                    && (version.is_none()
                        || release.is_some_and(|release| of_version(version, release)))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("made");
        fs::write(path, text).expect("written");
    }

    fn serde(version: Option<&str>) -> Asked {
        Asked::Crate {
            name: "Serde_Json".parse().expect("a name"),
            version: version.map(|version| Version::parse(version).expect("a version")),
        }
    }

    fn removed(outcome: &Outcome) -> Vec<String> {
        let mut paths: Vec<String> = outcome
            .removed
            .iter()
            .map(|removal| {
                removal
                    .entry
                    .path
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        paths.sort();
        paths
    }

    /// The cache of the tests: marked as this server's, with what it keeps of two crates.
    fn cache_of_two_crates() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, layout::MARKER, "");
        write(root, "docs.rs/serde-json/1.0.0/_default/61.json", "{}");
        write(
            root,
            "docs.rs/serde-json/1.0.0+extra/_default/61.json",
            "{}",
        );
        write(
            root,
            "docs.rs/serde-json/1.0.0/x86_64-pc-windows-msvc/61.json",
            "{}",
        );
        write(root, "docs.rs/serde-json/2.0.0/_default/61.json", "{}");
        write(root, "docs.rs/serde_json/1.0.0/rustdoc.json", "{}");
        write(root, "docs.rs/serde/1.0.0/_default/61.json", "{}");
        write(
            root,
            "static.crates.io/serde-json/1.0.0/serde_json-1.0.0.crate",
            "",
        );
        write(
            root,
            "static.crates.io/serde-json/1.0.0/src.v1/src/lib.rs",
            "",
        );
        write(root, "index.crates.io/se/rd/serde_json", "");
        write(root, "index.crates.io/se/rd/serde_json.meta.json", "{}");
        write(root, "index.crates.io/se/rd/serde_json.miss.json", "{}");
        write(root, "index.crates.io/se/rd/serde", "");
        write(root, "t/0123456789ab/target/x", "");
        write(root, "locks/a", "");
        write(root, "work/0123/x", "");
        write(root, "config.toml", "");
        dir
    }

    /// A release goes of every target, and without build metadata asked for, of any; a crate
    /// goes of every version, with its file of the sparse index, whatever the spelling of its
    /// name; another crate stays (decided in M2f's F1, 9, and F1c, 4 and 5).
    #[test]
    fn what_a_crate_has_goes() {
        let dir = cache_of_two_crates();
        let cache = Cache::at(dir.path());
        let outcome = clear(&cache, &serde(Some("1.0.0")), true).expect("cleared");
        assert_eq!(removed(&outcome), [
            "docs.rs/serde-json/1.0.0+extra/_default",
            "docs.rs/serde-json/1.0.0/_default",
            "docs.rs/serde-json/1.0.0/x86_64-pc-windows-msvc",
            "docs.rs/serde_json/1.0.0/rustdoc.json",
            "static.crates.io/serde-json/1.0.0",
        ]);
        let outcome = clear(&cache, &serde(Some("1.0.0+extra")), true).expect("cleared");
        assert_eq!(removed(&outcome), [
            "docs.rs/serde-json/1.0.0+extra/_default"
        ]);
        let outcome = clear(&cache, &serde(None), false).expect("cleared");
        assert_eq!(removed(&outcome).len(), 8, "{outcome:?}");
        assert!(removed(&outcome).contains(&"index.crates.io/se/rd/serde_json".to_owned()));
        assert!(
            removed(&outcome).contains(&"index.crates.io/se/rd/serde_json.miss.json".to_owned())
        );
        let root = dir.path();
        assert!(!root.join("docs.rs/serde-json").exists());
        assert!(!root.join("static.crates.io/serde-json/1.0.0").exists());
        assert!(
            !root
                .join("index.crates.io/se/rd/serde_json.meta.json")
                .exists()
        );
        assert!(root.join("docs.rs/serde/1.0.0/_default/61.json").exists());
        assert!(root.join("index.crates.io/se/rd/serde").exists());
        assert!(root.join("t/0123456789ab/target/x").exists());
    }

    /// All that the cache keeps goes, but its locks, its work directories, its marker and what
    /// this server does not know (decided in M2f's F1c, 1).
    #[test]
    fn all_that_the_cache_keeps_goes() {
        let dir = cache_of_two_crates();
        let cache = Cache::at(dir.path());
        let outcome = clear(&cache, &Asked::All, false).expect("cleared");
        assert!(
            outcome.in_use.is_empty() && outcome.left.is_empty(),
            "{outcome:?}"
        );
        let root = dir.path();
        let mut left: Vec<String> = fs::read_dir(root)
            .expect("read")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let builds = cfg!(feature = "local");
        let mut kept = vec![
            layout::CACHEDIR_TAG,
            "config.toml",
            "docs.rs",
            "index.crates.io",
            "locks",
            layout::MARKER,
            "static.crates.io",
            "t",
            "work",
        ];
        kept.sort_unstable();
        assert_eq!(left, kept);
        assert!(!root.join("docs.rs/serde").exists());
        assert_eq!(root.join("t/0123456789ab").exists(), !builds);
        assert!(root.join("work/0123/x").exists());
        assert!(root.join("locks/a").exists());
    }

    /// What is in use is skipped and told.
    #[test]
    fn what_is_in_use_stays() {
        let dir = cache_of_two_crates();
        let root = dir.path();
        let cache = Cache::at(root);
        let held = cache
            .try_lock_held(&root.join("docs.rs/serde/1.0.0/_default"))
            .expect("free");
        let outcome = clear(
            &cache,
            &Asked::Crate {
                name: "serde".parse().expect("a name"),
                version: None,
            },
            false,
        )
        .expect("cleared");
        assert_eq!(outcome.in_use, [PathBuf::from(
            "docs.rs/serde/1.0.0/_default"
        )]);
        drop(held);
        assert!(root.join("docs.rs/serde/1.0.0/_default/61.json").exists());
    }

    /// The records of `build_external_docs` of a crate's release go with their builds: by their
    /// keys, the version of a git crate as its build tells, and that of a package of the project
    /// as its id; a record that failed by its temporary project. Those of a workspace stay
    /// (decided in M2f's F1c, 3).
    #[cfg(feature = "local")]
    #[test]
    fn the_records_of_a_crate_go_with_their_builds() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, layout::MARKER, "");
        let index = |key: &str, published: &str| {
            format!(r#"{{"key":{key},"published":"{published}","sha256":"0","recorded_at":"x"}}"#)
        };
        write(
            root,
            "ext/v1/release/a/record.json",
            &index(
                r#"{"kind":"release","crate":"serde-json","version":"1.0.0","target":"t","hidden":false}"#,
                "local/v1/w/p/t/lib.apart",
            ),
        );
        write(
            root,
            "local/v1/w/p/t/lib.apart/record.json",
            r#"{"crate_version":"1.0.0"}"#,
        );
        write(
            root,
            "ext/v1/git/b/record.json",
            &index(
                r#"{"kind":"git","url":"u","commit":"c","crate":"serde-json","target":"t","hidden":false}"#,
                "local/v1/w/q/t/lib.apart",
            ),
        );
        write(
            root,
            "local/v1/w/q/t/lib.apart/record.json",
            r#"{"crate_version":"2.0.0"}"#,
        );
        write(
            root,
            "ext/v1/target/c/record.json",
            &index(
                r#"{"kind":"target","workspace":"/w","package":"registry+https://github.com/rust-lang/crates.io-index#serde_json@1.0.0","crate":"serde_json","target":"t","hidden":false}"#,
                "local/v1/w/r/t/lib.apart",
            ),
        );
        write(root, "local/v1/w/r/t/lib.apart/record.json", "{}");
        write(root, "ext/v1/release/d/failed.json", "{}");
        write(
            root,
            "ext/v1/release/d/project/Cargo.toml",
            "[package]\nname = \"documented\"\n\n[dependencies.serde_json]\nversion = \"=1.0.0\"\n",
        );
        write(root, "ext/v1/git_ref/e/failed.json", "{}");
        write(
            root,
            "ext/v1/workspace/f/record.json",
            r#"{"key":{"kind":"workspace","workspace":"/serde-json"}}"#,
        );
        let cache = Cache::at(root);
        let outcome = clear(&cache, &serde(Some("1.0.0")), true).expect("cleared");
        assert_eq!(removed(&outcome), [
            "ext/v1/release/a",
            "ext/v1/release/d",
            "ext/v1/target/c",
        ]);
        let outcome = clear(&cache, &serde(None), false).expect("cleared");
        assert_eq!(removed(&outcome), [
            "ext/v1/git/b",
            "ext/v1/release/a",
            "ext/v1/release/d",
            "ext/v1/target/c",
        ]);
        assert!(!root.join("local/v1/w").exists());
        assert!(root.join("ext/v1/git_ref/e/failed.json").exists());
        assert!(root.join("ext/v1/workspace/f/record.json").exists());
    }

    /// A build that records of two crates are of stays while one of them does, and goes with the
    /// last; with the records of one crate that all go, after them; never on its own.
    #[cfg(feature = "local")]
    #[test]
    fn a_build_of_more_records_than_one_goes_with_the_last() {
        let records = |names: &[(&str, &str, &str)]| {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let root = dir.path();
            write(root, layout::MARKER, "");
            for (digest, name, version) in names {
                write(
                    root,
                    &format!("ext/v1/release/{digest}/record.json"),
                    &format!(
                        r#"{{"key":{{"kind":"release","crate":"{name}","version":"{version}","target":"t","hidden":false}},"published":"local/v1/w/s/t/lib.apart","sha256":"0","recorded_at":"x"}}"#
                    ),
                );
            }
            write(root, "local/v1/w/s/t/lib.apart/record.json", "{}");
            dir
        };
        let dir = records(&[("a", "serde-json", "1.0.0"), ("b", "other", "1.0.0")]);
        let root = dir.path();
        let cache = Cache::at(root);
        let outcome = clear(&cache, &serde(None), false).expect("cleared");
        assert_eq!(removed(&outcome), ["ext/v1/release/a"]);
        assert!(root.join("local/v1/w/s/t/lib.apart/record.json").exists());
        let other = Asked::Crate {
            name: "other".parse().expect("a name"),
            version: None,
        };
        let outcome = clear(&cache, &other, false).expect("cleared");
        assert_eq!(removed(&outcome), ["ext/v1/release/b"]);
        assert!(!root.join("local/v1").exists());

        let dir = records(&[("a", "serde-json", "1.0.0"), ("c", "serde-json", "2.0.0")]);
        let root = dir.path();
        let cache = Cache::at(root);
        let outcome = clear(&cache, &serde(None), false).expect("cleared");
        assert_eq!(removed(&outcome), [
            "ext/v1/release/a",
            "ext/v1/release/c",
            "local/v1/w/s/t/lib.apart"
        ]);
        assert!(!root.join("local/v1").exists());
    }
}
