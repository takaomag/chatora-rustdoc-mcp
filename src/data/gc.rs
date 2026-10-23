//! The garbage collection of the cache (requirement R-CA-5, decided in M2f's F1 and F1b).
//!
//! It plans what to remove from an inventory of the cache, and then removes each place under the
//! locks that its writers take, which it tries without waiting: what is in use is skipped. A place
//! is moved out of its way first, to `work/gc`, and deleted there: what reads it finds it whole or
//! not at all. What was used in the last [`GRACE`] is kept. Places of what builds make are removed once they are
//! unused for `cache.target_max_age_days`, the old layouts and the other formats too, and then the
//! least recently used of each class while it takes more than its limit. The std of the nightlies
//! goes by its own rule, a week unused, which the reads of std apply too.

use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::BuildHasher,
    io,
    path::{Component, Path, PathBuf},
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;

use super::{
    cache::{Cache, PathGuard},
    inventory::{self, Entry, Inventory, Kind},
    layout::{self, Class, Limits, Ownership},
    process_lock::{self, LockMode, ProcessLock},
    used,
};

/// How long after its last use a place is kept, whatever the limits: it may be used again at once.
pub const GRACE: Duration = Duration::from_hours(1);

/// How long after the last note of its use a place is kept: the use of a place is noted once in
/// [`used::NOTE_EVERY`], so that it may have been used since, up to then.
const KEPT_FOR: Duration = GRACE.saturating_add(used::NOTE_EVERY);

/// Where the places removed are moved, to be deleted there, in the directory `work` of the cache:
/// what could not be deleted, as a file that a process maps on Windows, is deleted by the next
/// collection.
const TRASH: &str = "gc";

/// The classes of the cache that a collection removes from: not what builds make, in a server
/// that does not build, as a server that builds may share the cache (decided in M2f's F1b).
#[cfg(feature = "local")]
const COLLECTED: &[Class] = &[Class::Docs, Class::Targets];

/// The classes of the cache that a collection removes from: not what builds make, in a server
/// that does not build, as a server that builds may share the cache (decided in M2f's F1b).
#[cfg(not(feature = "local"))]
const COLLECTED: &[Class] = &[Class::Docs];

/// How long the server's collection looks at the cache: what it did not see is collected the next
/// time, by its age only (decided in M2f's F1b, 5).
pub const IN_BACKGROUND_FOR: Duration = Duration::from_secs(30);

/// How often a server collects after it built, at most (decided in M2f's F1b, 6).
pub const AFTER_BUILDS_EVERY: Duration = Duration::from_hours(1);

/// The key of the lock that one collection at a time holds, across processes.
const GC_KEY: &str = "gc";

/// The places that the records of `build_external_docs` are found by, which are small and kept.
const NOT_COLLECTED: [&str; 2] = ["ext/v1/alias", "ext/v1/git_commits"];

/// Why a place is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// It is of a layout of an earlier version.
    OldLayout,
    /// It is of another format of the cache, unused for `cache.target_max_age_days`.
    OtherFormat,
    /// The std of a nightly, unused for a week.
    UnusedStd,
    /// What builds made, unused for `cache.target_max_age_days`.
    Unused,
    /// The least recently used of its class, which took more than its limit.
    OverLimit,
}

impl Reason {
    /// What `cache gc` calls it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OldLayout => "old layout",
            Self::OtherFormat => "other format, unused",
            Self::Unused => "unused",
            Self::UnusedStd => "nightly unused for a week",
            Self::OverLimit => "over the limit",
        }
    }
}

/// A place to remove, with the bodies of the records of `build_external_docs` that go with it,
/// removed after it, and how much they take together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// The place.
    pub entry: Entry,
    /// Why it is removed.
    pub reason: Reason,
    /// The builds that its record is of, relative to the cache.
    pub bodies: Vec<PathBuf>,
    /// How much it takes with them.
    pub bytes: u64,
}

/// What a collection did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// What it removed, or would remove in a dry run.
    pub removed: Vec<Removal>,
    /// What it skipped, as it was in use.
    pub in_use: Vec<PathBuf>,
    /// What it could not remove, with why: left for the next collection, as a file that another
    /// process maps on Windows (decided in M2f's F1, 7).
    pub left: Vec<(PathBuf, String)>,
    /// Whether it saw the whole cache; when not, it removed only by age (decided in M2f's F1b, 5).
    pub complete: bool,
}

/// Why a collection did not run.
#[derive(Debug, thiserror::Error)]
pub enum NotRun {
    /// The cache directory is not this server's.
    #[error("the cache directory is not this server's: it has no marker, and has {}", .0.join(", "))]
    NotOwned(Vec<String>),
    /// Another process collects it now.
    #[error("another process collects the garbage of the cache now")]
    Busy,
    /// It could not be read.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Collects the garbage of `cache` with `limits`, looking at it until `until`, if given; in a dry
/// run, it only tells what it would remove, and writes nothing.
///
/// It reads and removes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns why it did not run: the cache is not this server's, another process collects it, or it
/// cannot be read.
pub fn collect(
    cache: &Cache,
    limits: &Limits,
    until: Option<Instant>,
    dry_run: bool,
) -> Result<Outcome, NotRun> {
    let root = cache.dir();
    // A dry run writes nothing, but tells what a cache that would be adopted has.
    let ownership = if dry_run {
        layout::ownership(root)?
    } else {
        layout::claim(root)?
    };
    match ownership {
        Ownership::Foreign(names) => return Err(NotRun::NotOwned(names)),
        Ownership::Created | Ownership::Owned | Ownership::Adopted => {},
    }
    let _collecting = if dry_run {
        None
    } else {
        Some(process_lock::try_lock(root, GC_KEY, LockMode::Exclusive)?.ok_or(NotRun::Busy)?)
    };
    if !dry_run {
        empty_trash(root);
    }
    let std = unused_std(cache, dry_run);
    let inventory = inventory::take(root, until)?;
    // After the inventory, so that what was used while it was taken counts as just used.
    let now = SystemTime::now();
    let pairs = pairs(root, &inventory);
    let mut removals = plan(&inventory, limits, now, (&pairs, &in_use(root)), COLLECTED);
    // What a dry run of the rule of std tells is in the inventory still.
    removals.retain(|removal| {
        std.removed
            .iter()
            .all(|std| std.entry.path != removal.entry.path)
    });
    let mut outcome = run(cache, removals, dry_run);
    outcome.removed.splice(0..0, std.removed);
    outcome.left.extend(std.left);
    #[cfg(feature = "local")]
    remove_orphan_aliases(cache, now, dry_run, &mut outcome);
    outcome.complete = inventory.complete;
    Ok(outcome)
}

/// Removes the std of the nightlies unused for a week, as the reads of std do: the same rule, in
/// one place (decided in M2f's F1b, 4); in a dry run, only tells them.
fn unused_std(cache: &Cache, dry_run: bool) -> Outcome {
    let root = cache.dir();
    let std = root.join(layout::STD);
    let unused = super::std_json::remove_unused(cache, (&std, &std), SystemTime::now(), dry_run);
    let relative = |path: &Path| path.strip_prefix(root).unwrap_or(path).to_path_buf();
    Outcome {
        removed: unused
            .removed
            .into_iter()
            .map(|(path, bytes)| Removal {
                entry: Entry {
                    kind: Kind::Std,
                    class: layout::class_of(layout::STD),
                    path: relative(&path),
                    bytes,
                    last_used: None,
                    about: None,
                },
                reason: Reason::UnusedStd,
                bodies: Vec::new(),
                bytes,
            })
            .collect(),
        left: unused
            .left
            .into_iter()
            .map(|(path, error)| (relative(&path), error.to_string()))
            .collect(),
        ..Outcome::default()
    }
}

/// Removes the aliases of the workspaces outside the projects whose records are gone, which find
/// nothing any more (decided in M2f's F1b), each under the lock of its workspace's record; in a
/// dry run, only tells them.
#[cfg(feature = "local")]
fn remove_orphan_aliases(cache: &Cache, now: SystemTime, dry_run: bool, outcome: &mut Outcome) {
    let root = cache.dir();
    let before = now.checked_sub(KEPT_FOR).unwrap_or(SystemTime::UNIX_EPOCH);
    for (path, key) in super::external_workspace::orphan_aliases(root, before) {
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let bytes = fs::symlink_metadata(&path).map_or(0, |metadata| metadata.len());
        let removal = Removal {
            entry: Entry {
                kind: Kind::External,
                class: layout::class_of(layout::EXTERNAL),
                path: relative.clone(),
                bytes,
                last_used: None,
                about: None,
            },
            reason: Reason::Unused,
            bodies: Vec::new(),
            bytes,
        };
        if dry_run {
            outcome.removed.push(removal);
            continue;
        }
        let Ok(Some(_held)) = process_lock::try_lock(root, &key.lock_key(), LockMode::Exclusive)
        else {
            outcome.in_use.push(relative);
            continue;
        };
        // Recorded again meanwhile.
        if key.dir(root).exists() {
            continue;
        }
        match remove_file(&path) {
            Ok(()) => outcome.removed.push(removal),
            Err(error) => outcome.left.push((relative, error.to_string())),
        }
    }
}

/// The target directories that what the servers that have stopped left running may still use,
/// relative to the cache: not removed.
fn in_use(root: &Path) -> HashSet<PathBuf> {
    #[cfg(all(unix, feature = "local"))]
    {
        super::owner::recorded_target_dirs(root)
            .into_iter()
            .map(|name| Path::new(layout::TARGETS).join(name))
            .collect()
    }
    #[cfg(not(all(unix, feature = "local")))]
    {
        let _ = root;
        HashSet::new()
    }
}

/// The builds that the records of `build_external_docs` in `inventory` are of, by record, relative
/// to the cache at `root`: removed with their records, after them, which would else be broken.
///
/// It reads files, so it is for a blocking thread.
#[must_use]
pub fn pairs(root: &Path, inventory: &Inventory) -> HashMap<PathBuf, PathBuf> {
    inventory
        .entries
        .iter()
        .filter(|entry| entry.kind == Kind::External)
        .filter_map(|entry| {
            let text = fs::read(root.join(&entry.path).join(layout::LOCAL_RECORD)).ok()?;
            let record: serde_json::Value = serde_json::from_slice(&text).ok()?;
            let published = record.get("published")?.as_str()?;
            let body: PathBuf = published.split('/').collect();
            // Only a place of the local builds, as it records, and nothing that leads out of it.
            let normal = body
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
            (normal && body.starts_with(layout::LOCAL)).then(|| (entry.path.clone(), body))
        })
        .collect()
}

/// What to remove of `inventory`, with `limits`, at `now`.
///
/// Only of `classes`; none of what `in_use` names, nor what was used in the last [`GRACE`], a
/// time to come counting as now; the records of `build_external_docs` with the builds that `pairs`
/// tells they are of, counted together, when a build is a place of the local builds that one
/// record alone is of. By its age, or its layout, when the inventory is not complete; by the
/// limits too, when it is.
#[must_use]
pub fn plan<S: BuildHasher, T: BuildHasher>(
    inventory: &Inventory,
    limits: &Limits,
    now: SystemTime,
    (pairs, in_use): (&HashMap<PathBuf, PathBuf, S>, &HashSet<PathBuf, T>),
    classes: &[Class],
) -> Vec<Removal> {
    let bodies: HashSet<&PathBuf> = pairs.values().collect();
    let by_path: HashMap<&PathBuf, &Entry> = inventory
        .entries
        .iter()
        .map(|entry| (&entry.path, entry))
        .collect();
    let records_of = pairs
        .values()
        .fold(HashMap::<&PathBuf, usize>::new(), |mut counts, body| {
            let count = counts.entry(body).or_default();
            *count = count.saturating_add(1);
            counts
        });
    let body_of = |record: &PathBuf| {
        pairs.get(record).filter(|body| {
            records_of.get(body) == Some(&1)
                && by_path
                    .get(body)
                    .is_some_and(|body| body.kind == Kind::Local)
        })
    };
    let age_of = |entry: &Entry| {
        entry
            .last_used
            .map(|used| now.duration_since(used).unwrap_or(Duration::ZERO))
    };
    let mut candidates: Vec<Removal> = inventory
        .entries
        .iter()
        .filter(|entry| {
            entry.class.is_some_and(|class| classes.contains(&class))
                && entry.kind != Kind::Toolchains
                && !bodies.contains(&entry.path)
                && !in_use.contains(&entry.path)
                && !NOT_COLLECTED
                    .iter()
                    .any(|kept| entry.path == Path::new(kept))
                // A place whose use is not told is old.
                && age_of(entry).is_none_or(|age| age >= KEPT_FOR)
        })
        .map(|entry| {
            let body = body_of(&entry.path);
            let bytes = entry.bytes.saturating_add(
                body.and_then(|body| by_path.get(body))
                    .map_or(0, |body| body.bytes),
            );
            Removal {
                entry: entry.clone(),
                reason: Reason::OverLimit,
                bodies: body.cloned().into_iter().collect(),
                bytes,
            }
        })
        .collect();
    let unused_for = |entry: &Entry, limit: Duration| age_of(entry).is_none_or(|age| age > limit);
    let mut removals = Vec::new();
    candidates.retain(|candidate| {
        let entry = &candidate.entry;
        let old = unused_for(entry, limits.target_max_age);
        let reason = match entry.kind {
            Kind::OldLayout => Some(Reason::OldLayout),
            Kind::OtherFormat => old.then_some(Reason::OtherFormat),
            Kind::TargetDir | Kind::Local | Kind::External => old.then_some(Reason::Unused),
            Kind::DocsRs
            | Kind::Std
            | Kind::Index
            | Kind::Crate
            | Kind::Toolchains
            | Kind::Kept
            | Kind::Unknown => None,
        };
        let Some(reason) = reason else {
            return true;
        };
        removals.push(Removal {
            reason,
            ..candidate.clone()
        });
        false
    });
    if inventory.complete {
        over_limits(inventory, (limits, classes), &candidates, &mut removals);
    }
    removals
}

/// Adds to `removals` the least recently used of `candidates` of each of `classes` while what the
/// class of `inventory` takes, but for `removals`, is over its limit in `limits`.
fn over_limits(
    inventory: &Inventory,
    (limits, classes): (&Limits, &[Class]),
    candidates: &[Removal],
    removals: &mut Vec<Removal>,
) {
    for (class, limit) in [
        (Class::Docs, limits.docs_max_size),
        (Class::Targets, limits.target_max_size),
    ] {
        if !classes.contains(&class) {
            continue;
        }
        let removed: u64 = removals
            .iter()
            .filter(|removal| removal.entry.class == Some(class))
            .fold(0, |total, removal| total.saturating_add(removal.bytes));
        let mut total = inventory
            .entries
            .iter()
            .filter(|entry| entry.class == Some(class))
            .fold(0_u64, |total, entry| total.saturating_add(entry.bytes))
            .saturating_sub(removed);
        let mut oldest: Vec<&Removal> = candidates
            .iter()
            .filter(|candidate| candidate.entry.class == Some(class))
            .collect();
        // The least recently used first; one whose use is not told before all.
        oldest.sort_by_key(|candidate| candidate.entry.last_used);
        for candidate in oldest {
            if total <= limit {
                break;
            }
            total = total.saturating_sub(candidate.bytes);
            removals.push(candidate.clone());
        }
    }
}

/// Removes `removals` from `cache`, each under the locks that its writers take, tried without
/// waiting; in a dry run, removes nothing and tells them all.
///
/// It removes files, so it is for a blocking thread.
#[must_use]
pub fn run(cache: &Cache, removals: Vec<Removal>, dry_run: bool) -> Outcome {
    let mut outcome = Outcome::default();
    if dry_run {
        outcome.removed = removals;
        return outcome;
    }
    let root = cache.dir();
    for removal in removals {
        let path = removal.entry.path.clone();
        let Some(held) = held(cache, &removal) else {
            outcome.in_use.push(path);
            continue;
        };
        // Used since the inventory was taken: kept.
        let absolute = root.join(&path);
        let now_used = fs::symlink_metadata(&absolute)
            .is_ok_and(|metadata| metadata.is_dir())
            .then(|| used::last_used(&absolute))
            .flatten();
        if let (Some(now_used), Some(then)) = (now_used, removal.entry.last_used)
            && now_used > then
        {
            outcome.in_use.push(path);
            continue;
        }
        // The record first, and its builds only when it is gone, which it would else find
        // broken.
        let mut failed = None;
        for place in std::iter::once(&path).chain(&removal.bodies) {
            if let Err(error) = remove_place(root, place, removal.entry.kind) {
                failed = Some((place.clone(), error.to_string()));
                break;
            }
            prune(root, place);
        }
        drop(held);
        match failed {
            None => outcome.removed.push(removal),
            Some(failed) => outcome.left.push(failed),
        }
    }
    outcome
}

/// A lock that a collection holds while it removes a place, let go when it is dropped.
#[derive(Debug)]
enum Held {
    /// Of a place in the cache, as its writers take it.
    Path {
        /// The lock.
        _guard: PathGuard,
    },
    /// Across processes, by its key.
    Process {
        /// The lock.
        _lock: ProcessLock,
    },
}

/// The locks of `removal` that its writers and users take, if no one holds any: the place and the
/// archives and `.crate` files directly in it, which are locked by their own paths; a target directory's builds
/// and their documentation; a record of `build_external_docs`, and its builds. `None` when one is
/// held, or the OS cannot lock it.
fn held(cache: &Cache, removal: &Removal) -> Option<Vec<Held>> {
    let root = cache.dir();
    let name = removal
        .entry
        .path
        .file_name()?
        .to_string_lossy()
        .into_owned();
    let process = |key: String| {
        process_lock::try_lock(root, &key, LockMode::Exclusive)
            .ok()
            .flatten()
            .map(|lock| Held::Process { _lock: lock })
    };
    let mut held = Vec::new();
    match removal.entry.kind {
        Kind::TargetDir => {
            held.push(process(format!("target:{name}"))?);
            held.push(process(format!("doc:{name}"))?);
        },
        Kind::External => held.push(process(format!("ext:{name}"))?),
        Kind::DocsRs
        | Kind::Std
        | Kind::Index
        | Kind::Crate
        | Kind::Toolchains
        | Kind::Local
        | Kind::OldLayout
        | Kind::OtherFormat
        | Kind::Kept
        | Kind::Unknown => {},
    }
    let places = std::iter::once(&removal.entry.path).chain(&removal.bodies);
    for place in places {
        let absolute = root.join(place);
        held.push(Held::Path {
            _guard: cache.try_lock_held(&absolute)?,
        });
        if fs::symlink_metadata(&absolute).is_ok_and(|metadata| metadata.is_dir()) {
            for entry in fs::read_dir(&absolute).into_iter().flatten().flatten() {
                let archive = entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rkyv" || extension == "crate");
                if archive && entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
                    held.push(Held::Path {
                        _guard: cache.try_lock_held(&entry.path())?,
                    });
                }
            }
        }
    }
    Some(held)
}

/// Removes the place `place` of the cache at `root`, of `kind`, not following links. A directory is
/// moved to [`TRASH`] first, at once, and then deleted there; Windows does not move one with a
/// file in it that is open or mapped, which is then left whole, for the next collection (decided
/// in M2f's F1, 7). A file of the sparse index goes with its record. One that is already gone is
/// removed.
fn remove_place(root: &Path, place: &Path, kind: Kind) -> io::Result<()> {
    let path = root.join(place);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() {
        remove_file(&path)?;
        if kind == Kind::Index {
            let mut meta = path.as_os_str().to_owned();
            meta.push(".meta.json");
            remove_file(Path::new(&meta))?;
        }
        return Ok(());
    }
    let trash = trash(root)?;
    fs::rename(&path, &trash)?;
    if let Err(error) = fs::remove_dir_all(&trash) {
        tracing::warn!(path = %trash.display(), %error, "what was removed from the cache could not all be deleted: the next collection deletes it");
    }
    Ok(())
}

/// A new name in [`TRASH`] in the cache at `root`, which is made: not where the directory `work`
/// is a link, which may lead out of the cache.
fn trash(root: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let work = root.join(layout::WORK);
    if fs::symlink_metadata(&work).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err(io::Error::other(format!(
            "{} is not a directory",
            work.display()
        )));
    }
    let dir = work.join(TRASH);
    fs::create_dir_all(&dir)?;
    let name = format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    Ok(dir.join(name))
}

/// Deletes what earlier collections moved to [`TRASH`] in the cache at `root` and could not
/// delete.
fn empty_trash(root: &Path) {
    let dir = root.join(layout::WORK).join(TRASH);
    if !fs::symlink_metadata(&dir).is_ok_and(|metadata| metadata.is_dir()) {
        return;
    }
    if let Err(error) = fs::remove_dir_all(&dir) {
        tracing::warn!(path = %dir.display(), %error, "what was removed from the cache could not all be deleted: the next collection deletes it");
    }
}

/// Removes the file at `path`, a link itself and not where it leads, if it is there.
fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Removes the directories above the place `path` of the cache at `root` that its removal left
/// empty, up to the name at the top, which stays.
fn prune(root: &Path, path: &Path) {
    let mut parent = path.parent();
    while let Some(dir) = parent {
        if dir.components().count() <= 1 {
            return;
        }
        // An error means it is not empty, or another made something in it meanwhile.
        if fs::remove_dir(root.join(dir)).is_err() {
            return;
        }
        parent = dir.parent();
    }
}

/// The limits that the server collects with, from its settings at startup.
static LIMITS: Mutex<Option<Limits>> = Mutex::new(None);

/// When this process last collected after a build.
static LAST_AFTER_BUILD: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(Mutex::default);

/// Sets the limits that the server collects with (requirement R-CA-5).
pub fn set_limits(limits: Limits) {
    *LIMITS.lock() = Some(limits);
}

/// Collects the garbage of `cache` in the background, with the limits of the server's settings.
///
/// It looks at the cache for [`IN_BACKGROUND_FOR`] at most: when the server starts, and after a
/// build at most once in [`AFTER_BUILDS_EVERY`] (decided in M2f's F1, 2, and F1b, 6). What it
/// removes is logged.
pub fn in_background(cache: &Cache) {
    let Some(limits) = *LIMITS.lock() else {
        return;
    };
    let cache = cache.clone();
    drop(crate::task::spawn_blocking(move || {
        let until = Instant::now()
            .checked_add(IN_BACKGROUND_FOR)
            .unwrap_or_else(Instant::now);
        match collect(&cache, &limits, Some(until), false) {
            Ok(outcome) => log(&outcome),
            Err(NotRun::Busy) => {},
            Err(error) => tracing::warn!(%error, "the garbage of the cache was not collected"),
        }
    }));
}

/// [`in_background`] after a build, at most once in [`AFTER_BUILDS_EVERY`] in this process.
pub fn after_build(cache: &Cache) {
    if LIMITS.lock().is_none() {
        return;
    }
    {
        let mut last = LAST_AFTER_BUILD.lock();
        if last.is_some_and(|at| at.elapsed() < AFTER_BUILDS_EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    in_background(cache);
}

/// Logs what a collection in the background did.
fn log(outcome: &Outcome) {
    for removal in &outcome.removed {
        tracing::info!(
            path = %removal.entry.path.display(),
            bytes = removal.bytes,
            reason = removal.reason.as_str(),
            "removed from the cache"
        );
    }
    for (path, error) in &outcome.left {
        tracing::warn!(path = %path.display(), %error, "could not be removed from the cache: left for the next collection");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn days(days: u64) -> Duration {
        Duration::from_hours(days.saturating_mul(24))
    }

    fn entry(kind: Kind, path: &str, bytes: u64, used_ago: Duration, now: SystemTime) -> Entry {
        let class = match kind {
            Kind::TargetDir | Kind::Local | Kind::External => Some(Class::Targets),
            Kind::Kept => Some(Class::Kept),
            Kind::Unknown => None,
            Kind::DocsRs
            | Kind::Std
            | Kind::Index
            | Kind::Crate
            | Kind::Toolchains
            | Kind::OldLayout
            | Kind::OtherFormat => Some(Class::Docs),
        };
        Entry {
            kind,
            class,
            path: PathBuf::from(path),
            bytes,
            last_used: now.checked_sub(used_ago),
            about: None,
        }
    }

    const BOTH: &[Class] = &[Class::Docs, Class::Targets];

    fn limits() -> Limits {
        Limits {
            docs_max_size: 100,
            target_max_size: 1_000,
            target_max_age: days(30),
        }
    }

    fn paths(removals: &[Removal]) -> Vec<(&str, Reason)> {
        let mut paths: Vec<(&str, Reason)> = removals
            .iter()
            .map(|removal| {
                (
                    removal.entry.path.to_str().unwrap_or_default(),
                    removal.reason,
                )
            })
            .collect();
        paths.sort_unstable_by_key(|(path, _)| *path);
        paths
    }

    #[test]
    fn what_is_old_or_over_the_limits_goes_and_what_was_just_used_stays() {
        let now = SystemTime::now();
        let inventory = Inventory {
            entries: vec![
                entry(Kind::DocsRs, "docs.rs/a/1/_default", 60, days(3), now),
                entry(Kind::DocsRs, "docs.rs/b/1/_default", 60, days(2), now),
                entry(
                    Kind::DocsRs,
                    "docs.rs/c/1/_default",
                    60,
                    Duration::from_mins(5),
                    now,
                ),
                entry(
                    Kind::OldLayout,
                    "docs.rs/d/1/rustdoc.json",
                    5,
                    Duration::from_hours(3),
                    now,
                ),
                entry(Kind::Toolchains, "toolchains", 500, days(90), now),
                entry(Kind::TargetDir, "t/old", 10, days(40), now),
                entry(Kind::TargetDir, "t/new", 10, days(1), now),
                entry(Kind::TargetDir, "t/held", 10, days(40), now),
                entry(Kind::OtherFormat, "local/v2", 10, days(40), now),
                entry(Kind::External, "ext/v1/alias", 1, days(90), now),
                entry(Kind::Kept, "locks", 1, days(90), now),
                entry(Kind::Unknown, "config.toml", 1, days(90), now),
            ],
            complete: true,
        };
        let in_use = HashSet::from([PathBuf::from("t/held")]);
        let removals = plan(&inventory, &limits(), now, (&HashMap::new(), &in_use), BOTH);
        assert_eq!(paths(&removals), [
            // The oldest of the docs over 100 bytes, but not the one used five minutes ago.
            ("docs.rs/a/1/_default", Reason::OverLimit),
            ("docs.rs/b/1/_default", Reason::OverLimit),
            ("docs.rs/d/1/rustdoc.json", Reason::OldLayout),
            ("local/v2", Reason::OtherFormat),
            ("t/old", Reason::Unused),
        ]);
    }

    /// What was used since `now` was taken, or tells a time to come, counts as just used; so does
    /// what was noted as used less than [`KEPT_FOR`] ago, as it may have been used since without a
    /// note.
    #[test]
    fn what_was_used_lately_or_later_stays() {
        let now = SystemTime::now();
        let later = Entry {
            last_used: now.checked_add(Duration::from_mins(1)),
            ..entry(Kind::TargetDir, "t/later", 10, Duration::ZERO, now)
        };
        let inventory = Inventory {
            entries: vec![
                later,
                entry(
                    Kind::TargetDir,
                    "t/noted",
                    2_000,
                    Duration::from_mins(90),
                    now,
                ),
                entry(
                    Kind::DocsRs,
                    "docs.rs/a/1/_default",
                    600,
                    Duration::from_mins(90),
                    now,
                ),
            ],
            complete: true,
        };
        let removals = plan(
            &inventory,
            &limits(),
            now,
            (&HashMap::new(), &HashSet::new()),
            BOTH,
        );
        assert!(removals.is_empty(), "{removals:?}");
    }

    /// Only the classes asked for are collected: a server that does not build leaves what builds
    /// make.
    #[test]
    fn only_the_classes_asked_for_are_collected() {
        let now = SystemTime::now();
        let inventory = Inventory {
            entries: vec![entry(Kind::TargetDir, "t/old", 10, days(40), now)],
            complete: true,
        };
        let removals = plan(
            &inventory,
            &limits(),
            now,
            (&HashMap::new(), &HashSet::new()),
            &[Class::Docs],
        );
        assert!(removals.is_empty(), "{removals:?}");
    }

    #[test]
    fn without_the_whole_inventory_only_the_age_and_the_layout_count() {
        let now = SystemTime::now();
        let inventory = Inventory {
            entries: vec![
                entry(Kind::DocsRs, "docs.rs/a/1/_default", 600, days(3), now),
                entry(Kind::TargetDir, "t/old", 10, days(40), now),
            ],
            complete: false,
        };
        let removals = plan(
            &inventory,
            &limits(),
            now,
            (&HashMap::new(), &HashSet::new()),
            BOTH,
        );
        assert_eq!(paths(&removals), [("t/old", Reason::Unused)]);
    }

    #[test]
    fn a_record_of_the_external_tool_goes_with_its_build() {
        let now = SystemTime::now();
        let inventory = Inventory {
            entries: vec![
                entry(Kind::External, "ext/v1/release/abc", 1, days(40), now),
                entry(Kind::Local, "local/v1/w/p/t/lib.apart", 500, days(1), now),
            ],
            complete: true,
        };
        let pairs = HashMap::from([(
            PathBuf::from("ext/v1/release/abc"),
            PathBuf::from("local/v1/w/p/t/lib.apart"),
        )]);
        let removals = plan(&inventory, &limits(), now, (&pairs, &HashSet::new()), BOTH);
        assert_eq!(removals.len(), 1, "{removals:?}");
        let removal = removals.first().expect("one");
        assert_eq!(removal.bodies, [PathBuf::from("local/v1/w/p/t/lib.apart")]);
        assert_eq!(removal.bytes, 501);
    }

    /// A record goes alone when what it tells is its build is not a place of the local builds, or
    /// is the build of another record too, which stays.
    #[test]
    fn a_record_goes_alone_with_a_build_that_is_not_its_own() {
        let now = SystemTime::now();
        let inventory = Inventory {
            entries: vec![
                entry(Kind::External, "ext/v1/release/a", 1, days(40), now),
                entry(Kind::External, "ext/v1/release/b", 1, days(40), now),
                entry(Kind::External, "ext/v1/release/c", 1, days(40), now),
                entry(Kind::Local, "local/v1/w/p/t/lib", 500, days(40), now),
            ],
            complete: true,
        };
        let shared = PathBuf::from("local/v1/w/p/t/lib");
        let pairs = HashMap::from([
            (PathBuf::from("ext/v1/release/a"), shared.clone()),
            (PathBuf::from("ext/v1/release/b"), shared),
            (PathBuf::from("ext/v1/release/c"), PathBuf::from("local/v1")),
        ]);
        let removals = plan(&inventory, &limits(), now, (&pairs, &HashSet::new()), BOTH);
        assert_eq!(removals.len(), 3, "{removals:?}");
        assert!(removals.iter().all(|removal| removal.bodies.is_empty()));
    }

    /// The build that a record tells is not one that leads out of the local builds.
    #[test]
    fn a_record_does_not_lead_out_of_the_local_builds() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        for (name, published) in [
            ("a", "local/v1/w/p/t/lib"),
            ("b", "local/../../x"),
            ("c", "t/x"),
        ] {
            write(root, &format!("ext/v1/release/{name}/record.json"));
            fs::write(
                root.join(format!("ext/v1/release/{name}/record.json")),
                format!(r#"{{"published":"{published}"}}"#),
            )
            .expect("written");
        }
        let inventory = inventory::take(root, None).expect("taken");
        let pairs = pairs(root, &inventory);
        assert_eq!(
            pairs,
            HashMap::from([(
                PathBuf::from("ext/v1/release/a"),
                PathBuf::from("local/v1/w/p/t/lib")
            )])
        );
    }

    /// The place `path` of the cache at `root`, of `kind`, as an inventory finds it now.
    fn found(root: &Path, kind: Kind, path: &str) -> Entry {
        Entry {
            last_used: used::last_used(&root.join(path)),
            ..entry(kind, path, 1, Duration::ZERO, SystemTime::now())
        }
    }

    fn write(root: &Path, path: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("made");
        fs::write(path, b"x").expect("written");
    }

    /// A record goes with its build, and the directories that they leave empty with them; what
    /// was moved out of the way to be deleted is deleted.
    #[test]
    fn a_record_goes_with_its_build_and_their_parents_left_empty() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "ext/v1/release/abc/record.json");
        write(root, "local/v1/w/p/t/lib/record.json");
        write(root, "local/v1/w/p/t/lib/61.json");
        write(root, "local/v1/w/p/t/lib/deeper/x");
        write(root, "local/v1/w/other/t/lib/61.json");
        let cache = Cache::at(root);
        let removal = Removal {
            entry: found(root, Kind::External, "ext/v1/release/abc"),
            reason: Reason::Unused,
            bodies: vec![PathBuf::from("local/v1/w/p/t/lib")],
            bytes: 4,
        };
        let outcome = run(&cache, vec![removal], false);
        assert_eq!(outcome.removed.len(), 1, "{outcome:?}");
        assert!(!root.join("ext/v1").exists());
        assert!(root.join("ext").exists(), "the name at the top stays");
        assert!(!root.join("local/v1/w/p").exists());
        assert!(root.join("local/v1/w/other/t/lib/61.json").exists());
        let trash = root.join(layout::WORK).join(TRASH);
        assert_eq!(fs::read_dir(&trash).expect("made").count(), 0);
    }

    /// A file of the sparse index goes with its record of how it was fetched.
    #[test]
    fn a_file_of_the_index_goes_with_its_record() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "index.crates.io/se/rd/serde");
        write(root, "index.crates.io/se/rd/serde.meta.json");
        let cache = Cache::at(root);
        let removal = Removal {
            entry: found(root, Kind::Index, "index.crates.io/se/rd/serde"),
            reason: Reason::OverLimit,
            bodies: Vec::new(),
            bytes: 2,
        };
        let outcome = run(&cache, vec![removal], false);
        assert_eq!(outcome.removed.len(), 1, "{outcome:?}");
        assert!(!root.join("index.crates.io/se").exists());
    }

    /// A `.crate` file that is being fetched or read, under the lock of its own path, is in use,
    /// and so is its place.
    #[test]
    fn a_crate_file_being_read_is_skipped() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "static.crates.io/serde/1.0.0/serde-1.0.0.crate");
        let cache = Cache::at(root);
        let removal = Removal {
            entry: found(root, Kind::Crate, "static.crates.io/serde/1.0.0"),
            reason: Reason::OverLimit,
            bodies: Vec::new(),
            bytes: 1,
        };
        let reading = cache
            .try_lock_held(&root.join("static.crates.io/serde/1.0.0/serde-1.0.0.crate"))
            .expect("free");
        let outcome = run(&cache, vec![removal], false);
        assert_eq!(outcome.in_use, [PathBuf::from(
            "static.crates.io/serde/1.0.0"
        )]);
        drop(reading);
    }

    /// A target directory that a build uses, which holds its lock across processes, is in use.
    #[test]
    fn a_target_directory_being_built_in_is_skipped() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "t/0123456789ab/target/x");
        let cache = Cache::at(root);
        let removal = Removal {
            entry: found(root, Kind::TargetDir, "t/0123456789ab"),
            reason: Reason::Unused,
            bodies: Vec::new(),
            bytes: 1,
        };
        let building = process_lock::try_lock(root, "target:0123456789ab", LockMode::Shared)
            .expect("locked")
            .expect("free");
        let outcome = run(&cache, vec![removal.clone()], false);
        assert_eq!(outcome.in_use, [PathBuf::from("t/0123456789ab")]);
        assert!(root.join("t/0123456789ab/target/x").exists());
        drop(building);
        let outcome = run(&cache, vec![removal], false);
        assert_eq!(outcome.removed.len(), 1, "{outcome:?}");
    }

    #[test]
    fn what_is_held_is_skipped_and_a_dry_run_removes_nothing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        write(root, "docs.rs/a/1/_default/61.json");
        let cache = Cache::at(root);
        let removal = Removal {
            entry: found(root, Kind::DocsRs, "docs.rs/a/1/_default"),
            reason: Reason::OverLimit,
            bodies: Vec::new(),
            bytes: 1,
        };
        let outcome = run(&cache, vec![removal.clone()], true);
        assert_eq!(outcome.removed.len(), 1);
        assert!(root.join("docs.rs/a/1/_default/61.json").exists());
        let held = cache
            .try_lock_held(&root.join("docs.rs/a/1/_default"))
            .expect("free");
        let outcome = run(&cache, vec![removal.clone()], false);
        assert_eq!(outcome.in_use, [PathBuf::from("docs.rs/a/1/_default")]);
        drop(held);
        let outcome = run(&cache, vec![removal], false);
        assert_eq!(outcome.removed.len(), 1);
        assert!(!root.join("docs.rs/a").exists());
        assert!(root.join("docs.rs").exists(), "the name at the top stays");
    }

    /// A dry run finds the cache to be what a collection would: a directory with nothing but names
    /// that other tools use too is not adopted.
    #[test]
    fn a_dry_run_tells_what_a_collection_would_find_the_cache_to_be() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        write(dir.path(), "t/x/y");
        let cache = Cache::at(dir.path());
        assert!(matches!(
            collect(&cache, &limits(), None, true),
            Err(NotRun::NotOwned(names)) if names == ["t"]
        ));
        assert!(!dir.path().join(layout::MARKER).exists());
    }

    /// An alias of a workspace outside the projects goes once the workspace's record is gone.
    #[cfg(feature = "local")]
    #[test]
    fn an_alias_whose_record_is_gone_goes() {
        use super::super::external_workspace::{record_aliases, workspace_key};
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        let cache = Cache::at(root);
        record_aliases(&cache, &[Path::new("/w/app")], Path::new("/w")).expect("recorded");
        let aliases = || -> Vec<PathBuf> {
            fs::read_dir(super::super::external::records_dir(root).join("alias"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .collect()
        };
        let long_ago = SystemTime::now().checked_sub(days(2)).expect("a time");
        for alias in aliases() {
            fs::File::options()
                .write(true)
                .open(&alias)
                .and_then(|file| file.set_modified(long_ago))
                .expect("set");
        }
        let record = workspace_key(Path::new("/w")).dir(root);
        fs::create_dir_all(&record).expect("made");
        let mut outcome = Outcome::default();
        remove_orphan_aliases(&cache, SystemTime::now(), false, &mut outcome);
        assert!(outcome.removed.is_empty(), "{outcome:?}");
        fs::remove_dir(&record).expect("removed");
        remove_orphan_aliases(&cache, SystemTime::now(), true, &mut outcome);
        assert_eq!(outcome.removed.len(), 2, "{outcome:?}");
        assert_eq!(aliases().len(), 2);
        let mut outcome = Outcome::default();
        remove_orphan_aliases(&cache, SystemTime::now(), false, &mut outcome);
        assert_eq!(outcome.removed.len(), 2, "{outcome:?}");
        assert_eq!(aliases(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_cache_that_is_not_this_servers_is_not_collected() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        write(dir.path(), "notes.txt");
        let cache = Cache::at(dir.path());
        assert!(matches!(
            collect(&cache, &limits(), None, false),
            Err(NotRun::NotOwned(names)) if names == ["notes.txt"]
        ));
        assert!(dir.path().join("notes.txt").exists());
    }
}
