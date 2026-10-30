//! What the cache directory has, place by place, for `cache ls` and the garbage collection.
//!
//! Each place tells what it is, which limit of the garbage collection it counts toward, how much it
//! takes and when it was last used (requirement R-CA-5, decided in M2f's F1a).
//!
//! A place is what is removed at once: the rustdoc JSON of a release for a target, a target
//! directory, the documentation of a local build, a record of `build_external_docs`. Symbolic
//! links are not followed, and a file with several hard links is counted once.

use std::{
    collections::HashSet,
    fs,
    io,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use super::{
    formats,
    layout::{self, Class},
    used,
};

/// What is added to the name of a file of the sparse index to name its record.
const INDEX_META: &str = ".meta.json";

/// What a place of the cache is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The rustdoc JSON of docs.rs of a release for a target, with its archive.
    DocsRs,
    /// The JSON of std of a nightly for a target, or a file about the nightlies.
    Std,
    /// The sparse index of crates.io.
    Index,
    /// The `.crate` files of a release.
    Crate,
    /// What the toolchains were found to do, which is small and not removed by its age.
    Toolchains,
    /// A target directory of the local builds.
    TargetDir,
    /// The documentation of a local build.
    Local,
    /// A record of `build_external_docs`.
    External,
    /// What an earlier version put where this one does not, which it knows.
    OldLayout,
    /// What another version of the format of the cache put, which this version does not read.
    OtherFormat,
    /// The locks, the records of the servers' children and their working directories, the marker
    /// and the tag, which the garbage collection does not remove.
    Kept,
    /// What this server does not put in its cache, which it does not touch.
    Unknown,
}

impl Kind {
    /// What it is called in `cache ls`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DocsRs => "docs.rs JSON",
            Self::Std => "std",
            Self::Index => "index",
            Self::Crate => ".crate",
            Self::Toolchains => "toolchains",
            Self::TargetDir => "target dir",
            Self::Local => "local build",
            Self::External => "external record",
            Self::OldLayout => "old layout",
            Self::OtherFormat => "other format",
            Self::Kept => "kept",
            Self::Unknown => "unknown",
        }
    }
}

/// A place of the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What it is.
    pub kind: Kind,
    /// The limit that it counts toward; `None` for what this server does not know.
    pub class: Option<Class>,
    /// Its path, relative to the cache directory.
    pub path: PathBuf,
    /// How much it takes, in bytes: not counted for what this server does not know.
    pub bytes: u64,
    /// When it was last used, when that can be told.
    pub last_used: Option<SystemTime>,
    /// What it is of, when it tells: the workspace of a target directory, or the crate of a local
    /// build.
    pub about: Option<String>,
}

/// What the cache directory has.
#[derive(Debug, Default)]
pub struct Inventory {
    /// Its places.
    pub entries: Vec<Entry>,
    /// Whether every place was looked at: not when the time ran out.
    pub complete: bool,
}

/// What the cache directory at `root` has, looked at until `until`, if given: an inventory that is
/// not `complete` past it. A directory that is not there has nothing.
///
/// It reads the file system, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when the directory cannot be read.
pub fn take(root: &Path, until: Option<Instant>) -> io::Result<Inventory> {
    let mut taking = Taking {
        root,
        until,
        seen: HashSet::new(),
        inventory: Inventory {
            entries: Vec::new(),
            complete: true,
        },
    };
    let names = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(taking.inventory),
        Err(error) => return Err(error),
    };
    let mut names: Vec<String> = names
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        if taking.out_of_time() {
            break;
        }
        taking.top(&name);
    }
    taking.inventory.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(taking.inventory)
}

/// An inventory being taken.
struct Taking<'a> {
    /// The cache directory.
    root: &'a Path,
    /// Until when it is taken.
    until: Option<Instant>,
    /// The files with several hard links counted, by device and inode.
    seen: HashSet<(u64, u64)>,
    /// What it has found.
    inventory: Inventory,
}

impl Taking<'_> {
    /// Whether the time ran out, which makes the inventory not complete.
    fn out_of_time(&mut self) -> bool {
        let out = self.until.is_some_and(|until| Instant::now() >= until);
        if out {
            self.inventory.complete = false;
        }
        out
    }

    /// Takes in what is under `name` at the top of the cache.
    fn top(&mut self, name: &str) {
        let class = layout::class_of(name);
        // Not where a link goes, which is not this server's to remove.
        if fs::symlink_metadata(self.root.join(name)).is_ok_and(|metadata| metadata.is_symlink()) {
            self.place(Path::new(name), Kind::Unknown, None);
            return;
        }
        match name {
            layout::DOCS_RS => self.levels(name, (Kind::DocsRs, 3), Kind::OldLayout),
            layout::STD => self.std(),
            layout::STATIC_CRATES => self.levels(name, (Kind::Crate, 2), Kind::Crate),
            layout::INDEX => self.index(),
            layout::TOOLCHAINS => self.place(Path::new(name), Kind::Toolchains, class),
            layout::TARGETS => self.levels(name, (Kind::TargetDir, 1), Kind::TargetDir),
            layout::LOCAL => self.versioned((name, &formats::local_dir()), Kind::Local, 4),
            layout::EXTERNAL => {
                self.versioned((name, &formats::external_dir()), Kind::External, 2);
            },
            _ if class == Some(Class::Kept) => self.place(Path::new(name), Kind::Kept, class),
            _ => self.place(Path::new(name), Kind::Unknown, None),
        }
    }

    /// Takes in the places `depth` levels under `name`, of the kind `kind`; a file above them is a
    /// place of the kind `stray`.
    fn levels(&mut self, name: &str, (kind, depth): (Kind, usize), stray: Kind) {
        let class = layout::class_of(name);
        self.under(
            &PathBuf::from(name),
            depth,
            &mut |taking, path, at_depth| {
                let kind = if at_depth { kind } else { stray };
                taking.place(path, kind, class);
            },
        );
    }

    /// Takes in the places of `name`, whose directory `current` (`v<n>`) is of this version's
    /// format, `depth` levels under it; the other formats are places of their own.
    fn versioned(&mut self, (name, current): (&str, &str), kind: Kind, depth: usize) {
        let class = layout::class_of(name);
        for child in self.children(Path::new(name)) {
            let path = Path::new(name).join(&child);
            // A link, which may lead out of the cache, is not followed.
            let is_dir =
                fs::symlink_metadata(self.root.join(&path)).is_ok_and(|metadata| metadata.is_dir());
            if child == current && !is_dir {
                self.place(&path, Kind::Unknown, None);
            } else if child == current {
                for grandchild in self.children(&path) {
                    let path = path.join(&grandchild);
                    // The aliases of the workspaces and the commits of the repositories, which the
                    // records are found by.
                    if name == layout::EXTERNAL
                        && layout::EXTERNAL_INDEXES.contains(&grandchild.as_str())
                    {
                        self.place(&path, kind, class);
                        continue;
                    }
                    self.under(
                        &path,
                        depth.saturating_sub(1),
                        &mut |taking, path, at_depth| {
                            // Not of a layout that this version knows: not touched.
                            let (kind, class) = if at_depth {
                                (kind, class)
                            } else {
                                (Kind::Unknown, None)
                            };
                            taking.place(path, kind, class);
                        },
                    );
                }
            } else {
                self.place(&path, Kind::OtherFormat, class);
            }
        }
    }

    /// Takes in each file of the sparse index with its record of how it was fetched
    /// (`<file>.meta.json`), which tells when it was last used, as it is written again when the
    /// file is checked again (decided in M2f's F1b).
    fn index(&mut self) {
        let class = layout::class_of(layout::INDEX);
        let mut pending = vec![PathBuf::from(layout::INDEX)];
        while let Some(relative) = pending.pop() {
            if self.out_of_time() {
                return;
            }
            for child in self.children(&relative) {
                let path = relative.join(&child);
                let absolute = self.root.join(&path);
                let Ok(metadata) = fs::symlink_metadata(&absolute) else {
                    continue;
                };
                if metadata.is_dir() {
                    pending.push(path);
                } else if metadata.is_file() && !child.ends_with(INDEX_META) {
                    let meta = absolute.with_file_name(format!("{child}{INDEX_META}"));
                    let meta_metadata = fs::symlink_metadata(&meta).ok();
                    let bytes = metadata
                        .len()
                        .saturating_add(meta_metadata.as_ref().map_or(0, fs::Metadata::len));
                    let last_used = meta_metadata
                        .and_then(|meta| meta.modified().ok())
                        .or_else(|| metadata.modified().ok());
                    self.inventory.entries.push(Entry {
                        kind: Kind::Index,
                        class,
                        path,
                        bytes,
                        last_used,
                        about: None,
                    });
                }
            }
        }
    }

    /// Takes in the std of each nightly for each target, the nightlies of the local toolchains
    /// too, and each file about the nightlies.
    fn std(&mut self) {
        let class = layout::class_of(layout::STD);
        let std = Path::new(layout::STD);
        for child in self.children(std) {
            let path = std.join(&child);
            let depth = if child == "local" { 2 } else { 1 };
            self.under(&path, depth, &mut |taking, path, _| {
                taking.place(path, Kind::Std, class);
            });
        }
    }

    /// Calls `found` with each place `depth` levels under `relative`, and with each file above
    /// them, telling whether it is at that depth.
    fn under(
        &mut self,
        relative: &Path,
        depth: usize,
        found: &mut dyn FnMut(&mut Self, &Path, bool),
    ) {
        if self.out_of_time() {
            return;
        }
        let absolute = self.root.join(relative);
        let is_dir = fs::symlink_metadata(&absolute).is_ok_and(|metadata| metadata.is_dir());
        if depth == 0 || !is_dir {
            found(self, relative, depth == 0 && is_dir);
            return;
        }
        for child in self.children(relative) {
            self.under(&relative.join(child), depth.saturating_sub(1), found);
        }
    }

    /// The names in the directory at `relative`, sorted; none when it cannot be read.
    fn children(&self, relative: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.root.join(relative))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Takes in the place at `relative`, of `kind`, which counts toward `class`.
    fn place(&mut self, relative: &Path, kind: Kind, class: Option<Class>) {
        let absolute = self.root.join(relative);
        // Not what this server does not know, which may be large, as a home directory that the
        // cache was set to by mistake.
        let (bytes, newest) = if kind == Kind::Unknown {
            (0, None)
        } else {
            size(&absolute, &mut self.seen)
        };
        // What was written anywhere in it is used too, as the builds of a newer version of this
        // server in a place of another format.
        let last_used = used::last_used(&absolute)
            .or_else(|| {
                fs::symlink_metadata(&absolute)
                    .and_then(|metadata| metadata.modified())
                    .ok()
            })
            .max(newest);
        let about = about(&absolute, kind);
        self.inventory.entries.push(Entry {
            kind,
            class,
            path: relative.to_path_buf(),
            bytes,
            last_used,
            about,
        });
    }
}

/// What the place at `path`, of `kind`, is of, when it tells: the workspace of a target directory,
/// as its builds write it, or the crate, version and target of a local build, as its record has
/// them.
fn about(path: &Path, kind: Kind) -> Option<String> {
    let (file, fields): (&str, &[&str]) = match kind {
        Kind::TargetDir => (layout::TARGET_ABOUT, &["workspace", "target"]),
        Kind::Local => (layout::LOCAL_RECORD, &[
            "crate_name",
            "crate_version",
            "target",
        ]),
        Kind::DocsRs
        | Kind::Std
        | Kind::Index
        | Kind::Crate
        | Kind::Toolchains
        | Kind::External
        | Kind::OldLayout
        | Kind::OtherFormat
        | Kind::Kept
        | Kind::Unknown => return None,
    };
    let text = fs::read(path.join(file)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&text).ok()?;
    let parts: Vec<&str> = fields
        .iter()
        .filter_map(|field| value.get(*field)?.as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// How much what is at `path` takes, in bytes, as an inventory counts it.
///
/// It reads the file system, so it is for a blocking thread.
#[must_use]
pub fn size_of(path: &Path) -> u64 {
    size(path, &mut HashSet::new()).0
}

/// How much what is at `path` takes, in bytes, without following symbolic links, counting a file
/// whose hard links `seen` has once, and the latest time that it was written, not of what tells
/// a time to come, as a file unpacked from an archive. What cannot be read counts for nothing.
///
/// It reads the file system, so it is for a blocking thread.
fn size(path: &Path, seen: &mut HashSet<(u64, u64)>) -> (u64, Option<SystemTime>) {
    let now = SystemTime::now();
    let mut total = 0_u64;
    let mut newest = None;
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if let Ok(modified) = metadata.modified()
            && modified <= now
        {
            newest = newest.max(Some(modified));
        }
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path()),
            );
        } else if metadata.is_file() && first_link(&metadata, seen) {
            total = total.saturating_add(metadata.len());
        }
    }
    (total, newest)
}

/// Whether the file of `metadata` is counted for the first time: not when another of its hard
/// links was.
#[cfg(unix)]
fn first_link(metadata: &fs::Metadata, seen: &mut HashSet<(u64, u64)>) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    metadata.nlink() <= 1 || seen.insert((metadata.dev(), metadata.ino()))
}

/// Whether the file of `metadata` is counted for the first time: always, where its hard links are
/// not told.
#[cfg(not(unix))]
const fn first_link(_metadata: &fs::Metadata, _seen: &mut HashSet<(u64, u64)>) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, bytes: usize) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("made");
        fs::write(path, vec![b'x'; bytes]).expect("written");
    }

    fn entry<'a>(inventory: &'a Inventory, path: &str) -> &'a Entry {
        inventory
            .entries
            .iter()
            .find(|entry| entry.path == Path::new(path))
            .unwrap_or_else(|| panic!("no {path}: {inventory:?}"))
    }

    #[test]
    fn each_place_is_found_with_its_kind_class_and_size() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "docs.rs/serde/1.0.0/_default/61.json", 10);
        write(root, "docs.rs/serde/1.0.0/_default/used", 0);
        write(root, "docs.rs/old/0.1.0/rustdoc.json", 5);
        write(root, "std/abc123/x86_64-unknown-linux-gnu/std.json", 20);
        write(root, "std/local/def/x86_64-unknown-linux-gnu/std.json", 7);
        write(root, "std/nightly.json", 1);
        write(root, "static.crates.io/serde/1.0.0/serde-1.0.0.crate", 30);
        write(root, "static.crates.io/serde/1.0.0/src.v1/src/lib.rs", 8);
        write(root, "static.crates.io/serde/1.0.0/src.v1.json", 2);
        write(root, "index.crates.io/se/rd/serde", 2);
        write(root, "index.crates.io/se/rd/serde.meta.json", 3);
        write(root, "t/0123456789ab/target/debug/a", 100);
        write(root, "local/v1/ws/pkg/x86_64/lib/61.json", 40);
        write(root, "local/v2/whatever", 3);
        write(root, "ext/v1/release/digest/record.json", 4);
        write(root, "ext/v1/alias/0123", 1);
        write(root, "ext/v1/git_commits/4567", 1);
        write(root, "local/v1/stray.json", 1);
        write(root, "locks/a.lock", 0);
        write(root, "config.toml", 6);
        write(root, layout::MARKER, 1);
        let inventory = take(root, None).expect("taken");
        assert!(inventory.complete);
        let docs = entry(&inventory, "docs.rs/serde/1.0.0/_default");
        assert_eq!(
            (docs.kind, docs.class, docs.bytes),
            (Kind::DocsRs, Some(Class::Docs), 10)
        );
        assert_eq!(
            entry(&inventory, "docs.rs/old/0.1.0/rustdoc.json").kind,
            Kind::OldLayout
        );
        assert_eq!(
            entry(&inventory, "std/local/def/x86_64-unknown-linux-gnu").bytes,
            7
        );
        assert_eq!(entry(&inventory, "std/nightly.json").kind, Kind::Std);
        let index = entry(&inventory, "index.crates.io/se/rd/serde");
        assert_eq!((index.kind, index.bytes), (Kind::Index, 5));
        assert!(
            inventory
                .entries
                .iter()
                .all(|entry| !entry.path.ends_with("serde.meta.json")),
            "{inventory:?}"
        );
        // With the sources unpacked from it.
        let crate_file = entry(&inventory, "static.crates.io/serde/1.0.0");
        assert_eq!((crate_file.kind, crate_file.bytes), (Kind::Crate, 40));
        let target = entry(&inventory, "t/0123456789ab");
        assert_eq!(
            (target.kind, target.class, target.bytes),
            (Kind::TargetDir, Some(Class::Targets), 100)
        );
        assert_eq!(
            entry(&inventory, "local/v1/ws/pkg/x86_64/lib").kind,
            Kind::Local
        );
        assert_eq!(entry(&inventory, "local/v2").kind, Kind::OtherFormat);
        assert_eq!(
            entry(&inventory, "ext/v1/release/digest").kind,
            Kind::External
        );
        // This version's, which the records are found by.
        assert_eq!(entry(&inventory, "ext/v1/alias").kind, Kind::External);
        assert_eq!(entry(&inventory, "ext/v1/git_commits").kind, Kind::External);
        // Not of a layout that it knows: not touched.
        let stray = entry(&inventory, "local/v1/stray.json");
        assert_eq!((stray.kind, stray.class), (Kind::Unknown, None));
        assert_eq!(entry(&inventory, "locks").class, Some(Class::Kept));
        let unknown = entry(&inventory, "config.toml");
        assert_eq!(
            (unknown.kind, unknown.class, unknown.bytes),
            (Kind::Unknown, None, 0)
        );
        assert!(docs.last_used.is_some());
    }

    #[test]
    fn a_cache_that_is_not_there_has_nothing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let inventory = take(&dir.path().join("none"), None).expect("taken");
        assert_eq!(inventory.entries, []);
        assert!(inventory.complete);
    }

    #[test]
    fn out_of_time_it_is_not_complete() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        write(dir.path(), "t/a/x", 1);
        let inventory = take(dir.path(), Some(Instant::now())).expect("taken");
        assert!(!inventory.complete);
    }

    /// A place of another format, where a newer version of this server builds deep inside, is used
    /// when it last wrote there; a time to come does not count (M2f's F1b).
    #[cfg(unix)]
    #[test]
    fn a_place_is_used_when_anything_in_it_was_written() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "local/v2/ws/pkg/lib/61.json", 1);
        write(root, "local/v2/ws/pkg/lib/later.json", 1);
        let set = |path: &str, at: SystemTime| {
            fs::File::open(root.join(path))
                .and_then(|file| file.set_modified(at))
                .expect("set");
        };
        let now = SystemTime::now();
        let ago = |days: u64| {
            now.checked_sub(std::time::Duration::from_hours(days.saturating_mul(24)))
                .expect("a time")
        };
        set("local/v2/ws/pkg/lib/61.json", ago(2));
        set(
            "local/v2/ws/pkg/lib/later.json",
            now.checked_add(std::time::Duration::from_hours(24 * 400))
                .expect("a time"),
        );
        for path in [
            "local/v2/ws/pkg/lib",
            "local/v2/ws/pkg",
            "local/v2/ws",
            "local/v2",
        ] {
            set(path, ago(40));
        }
        let inventory = take(root, None).expect("taken");
        assert_eq!(entry(&inventory, "local/v2").last_used, Some(ago(2)));
    }

    /// A format of the records or the local builds that is a link is not followed: what it leads
    /// to is not this cache's.
    #[cfg(unix)]
    #[test]
    fn a_format_that_is_a_link_is_not_followed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("cache");
        let elsewhere = dir.path().join("elsewhere");
        write(&elsewhere, "w/p/t/lib/61.json", 1);
        fs::create_dir_all(root.join("local")).expect("made");
        std::os::unix::fs::symlink(&elsewhere, root.join("local/v1")).expect("linked");
        let inventory = take(&root, None).expect("taken");
        assert_eq!(entry(&inventory, "local/v1").kind, Kind::Unknown);
        assert!(
            inventory
                .entries
                .iter()
                .all(|entry| !entry.path.starts_with("local/v1/w")),
            "{inventory:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_and_hard_links_count_once() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "t/a/target/one", 50);
        fs::hard_link(root.join("t/a/target/one"), root.join("t/a/target/two")).expect("linked");
        write(root, "outside/big", 1000);
        std::os::unix::fs::symlink(root.join("outside"), root.join("t/a/target/link"))
            .expect("linked");
        // Nor a link at the top, which another directory may be.
        std::os::unix::fs::symlink(root.join("outside"), root.join(layout::LOCAL)).expect("linked");
        let inventory = take(root, None).expect("taken");
        assert_eq!(entry(&inventory, "t/a").bytes, 50);
        let link = entry(&inventory, layout::LOCAL);
        assert_eq!(
            (link.kind, link.class, link.bytes),
            (Kind::Unknown, None, 0)
        );
        assert!(
            inventory
                .entries
                .iter()
                .all(|entry| entry.path == Path::new(layout::LOCAL)
                    || !entry.path.starts_with(layout::LOCAL)),
            "{inventory:?}"
        );
    }
}
