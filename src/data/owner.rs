//! The server as the owner of what it leaves in the cache while it runs: its work directories, and
//! the records of its children (requirements R-WS-6 and R-CA-4; `doc/m2-plan.md` §6.4, the
//! decisions of C4).
//!
//! Each server has an id of its own, and holds the lock `owner:<id>` across processes from start
//! to end. What it owns is named by the id, so another process tells whether the owner is alive by
//! trying its lock, without reading what it owns or knowing the version that wrote it. The OS lets
//! a lock go when its process ends, so a PID that is used again does not matter, and the children
//! do not hold it (lock files are not inherited).

use std::{
    fs,
    hash::{BuildHasher as _, Hasher as _, RandomState},
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    cache::Cache,
    process_lock::{self, LockMode, ProcessLock},
};

/// The directory of the work directories in the cache, one directory for each server.
pub const WORK_DIR: &str = "work";

/// The directory of the records of the servers' children in the cache. The records of each
/// format are in a directory `v<format>` of their own (decided in C1).
pub const RECORDS_DIR: &str = "procs";

/// The format of the records that this version writes.
pub const RECORDS_FORMAT: u32 = 1;

/// How long the records of other versions are kept after they were last written, once their
/// owner has stopped: their children have ended long before (decided in C4).
const OTHER_RECORDS_KEPT: Duration = Duration::from_hours(7 * 24);

/// This server, as the owner of what it leaves in the cache.
#[derive(Debug)]
pub struct Owner {
    /// The cache.
    root: PathBuf,
    /// 16 hex digits, random.
    id: Box<str>,
    /// The lock that tells that this server is alive, if the OS could lock it. Without it, other
    /// servers would take this one for stopped, so it makes no work directory.
    alive: parking_lot::Mutex<Option<ProcessLock>>,
}

/// This process's server, once it has started.
static THIS: std::sync::OnceLock<Owner> = std::sync::OnceLock::new();

/// The key of the lock that the server `id` holds while it runs. Servers of every version find
/// each other's by it, so it may not change.
fn owner_key(id: &str) -> String {
    format!("owner:{id}")
}

/// A new id, unlike that of any other server.
fn new_id() -> Box<str> {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    format!("{:016x}", hasher.finish()).into()
}

/// Whether `name` is an id that [`new_id`] makes.
fn is_id(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Owner {
    /// This server, with a new id, holding its lock in the cache.
    #[must_use]
    pub fn start(cache: &Cache) -> Self {
        let root = cache.dir().to_owned();
        let id = new_id();
        let alive = match process_lock::try_lock(&root, &owner_key(&id), LockMode::Exclusive) {
            Ok(Some(held)) => Some(held),
            Ok(None) => {
                tracing::warn!(%id, "the lock of a new server id is held by another process");
                None
            },
            Err(error) => {
                tracing::warn!(%error, "the cache cannot be locked, so no work directory is made and none of other servers is removed");
                None
            },
        };
        let owner = Self {
            root,
            id,
            alive: parking_lot::Mutex::new(alive),
        };
        // Made at once, so that a sweep finds this server, and its lock file, however it ends.
        if owner.locked()
            && let Err(error) = owner.work_dirs()
        {
            tracing::warn!(%error, "the work directories could not be made");
        }
        owner
    }

    /// Starts this process's server with the cache `cache` (once: a later call gives the first).
    pub fn start_this(cache: &Cache) -> &'static Self {
        THIS.get_or_init(|| Self::start(cache))
    }

    /// This process's server, if it has started.
    #[must_use]
    pub fn this() -> Option<&'static Self> {
        THIS.get()
    }

    /// Whether the OS holds this server's lock, so that other servers can tell that it is alive.
    fn locked(&self) -> bool {
        self.alive.lock().is_some()
    }

    /// The id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The directory of this server's work directories, made if needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the OS could not lock this server's lock, so that other servers
    /// could not tell that it is alive, or the directory cannot be made.
    pub fn work_dirs(&self) -> io::Result<PathBuf> {
        if !self.locked() {
            return Err(io::Error::other(
                "the cache cannot be locked, so other servers could not tell that this one is \
                 alive",
            ));
        }
        let dir = self.root.join(WORK_DIR).join(&*self.id);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Removes this server's work directories and lock file, as it stops.
    pub fn leave(&self) {
        let dir = self.root.join(WORK_DIR).join(&*self.id);
        if let Err(error) = fs::remove_dir_all(&dir)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(dir = %dir.display(), %error, "the work directories could not be removed");
        }
        let held = self.alive.lock().take();
        if let Some(held) = held {
            remove_lock_file(&self.root, &self.id, held);
        }
    }
}

/// Removes the lock file of the server `id`, which `held` holds: nobody locks it again, as no
/// other server has the id. On Windows, a file that is open cannot be removed, so it is closed
/// first; a sweep that has just opened it meanwhile removes it then.
fn remove_lock_file(root: &Path, id: &str, held: ProcessLock) {
    let path = process_lock::lock_path(root, &owner_key(id));
    #[cfg(windows)]
    drop(held);
    drop(fs::remove_file(&path));
    #[cfg(not(windows))]
    drop(held);
}

/// What a sweep removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    /// The work directories of servers that have stopped.
    pub work_dirs: usize,
    /// The records of other versions.
    pub records: usize,
    /// It stopped at its time limit, and the next server's sweep goes on.
    pub unfinished: bool,
}

/// Removes what the servers that have stopped left in the cache, until `until`.
///
/// That is their work directories, and the records of other versions that were last written more
/// than a week ago (decided in C4). The records of this version, whose children may still run, are
/// not touched.
///
/// Nothing is removed when the OS could not lock `me`'s lock: then whether the others are alive
/// cannot be told either.
#[must_use]
pub fn sweep(me: &Owner, until: Instant) -> Swept {
    let mut swept = Swept::default();
    if !me.locked() {
        return swept;
    }
    let root = &me.root;
    for (id, dir) in owned(&root.join(WORK_DIR)) {
        if Instant::now() >= until {
            swept.unfinished = true;
            return swept;
        }
        if id == *me.id {
            continue;
        }
        let Some(held) = stopped(root, &id) else {
            continue;
        };
        match fs::remove_dir_all(&dir) {
            Ok(()) => swept.work_dirs = swept.work_dirs.saturating_add(1),
            // Its server removed it as it stopped, but not its lock file, which this one held.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => {
                tracing::warn!(dir = %dir.display(), %error, "the work directories of a server that has stopped could not be removed");
                continue;
            },
        }
        remove_lock_file(root, &id, held);
    }
    let current = format!("v{RECORDS_FORMAT}");
    for version in subdirectories(&root.join(RECORDS_DIR)) {
        if version.file_name().is_some_and(|name| *name == *current) {
            continue;
        }
        for (id, record) in owned(&version) {
            if Instant::now() >= until {
                swept.unfinished = true;
                return swept;
            }
            let old = fs::metadata(&record)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age > OTHER_RECORDS_KEPT);
            if !old {
                continue;
            }
            // The lock file that this makes, if its server made none, is removed with the record.
            if let Some(held) = stopped(root, &id) {
                if fs::remove_file(&record).is_ok() {
                    swept.records = swept.records.saturating_add(1);
                }
                remove_lock_file(root, &id, held);
            }
        }
        // Gone when nothing is left in it.
        drop(fs::remove_dir(&version));
    }
    swept
}

/// The entries of `dir` named by a server's id, before the first `.`, and their paths.
fn owned(dir: &Path) -> Vec<(String, PathBuf)> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let id = name.split('.').next()?.to_owned();
            is_id(&id).then(|| (id, entry.path()))
        })
        .collect()
}

/// The directories in `dir`.
fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// The lock of the server `id`, if it has stopped: nobody holds it. `None` while it runs, or
/// when that cannot be told.
fn stopped(root: &Path, id: &str) -> Option<ProcessLock> {
    match process_lock::try_lock(root, &owner_key(id), LockMode::Exclusive) {
        Ok(held) => held,
        Err(error) => {
            tracing::debug!(%id, %error, "whether a server has stopped cannot be told");
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that has stopped: the id, and its work directories, whose lock nobody holds.
    fn stopped_server(root: &Path) -> (String, PathBuf) {
        let id = new_id().into_string();
        let server = root.join(WORK_DIR).join(&id);
        fs::create_dir_all(server.join("build")).expect("made");
        fs::write(server.join("build/out"), "left").expect("written");
        (id, server)
    }

    fn later() -> Instant {
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .expect("in range")
    }

    #[test]
    fn the_key_of_an_owner_does_not_change() {
        assert_eq!(owner_key("0123456789abcdef"), "owner:0123456789abcdef");
        let id = new_id();
        assert!(is_id(&id), "{id}");
        assert_ne!(id, new_id());
    }

    /// The work directories of a server that has stopped are removed, with its lock file; those
    /// of one that runs, and the sweeper's own, are kept (requirements R-WS-6 and R-CA-4).
    #[test]
    fn work_directories_go_with_their_server() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let me = Owner::start(&cache);
        let mine = me.work_dirs().expect("made");
        let running = Owner::start(&cache);
        let theirs = running.work_dirs().expect("made");
        let (stopped_id, stopped_dir) = stopped_server(dir.path());
        // Its lock file is left as a stopped server leaves it.
        drop(process_lock::try_lock(
            dir.path(),
            &owner_key(&stopped_id),
            LockMode::Exclusive,
        ));
        let swept = sweep(&me, later());
        assert_eq!(swept.work_dirs, 1);
        assert!(!stopped_dir.exists());
        assert!(
            !process_lock::lock_path(dir.path(), &owner_key(&stopped_id)).exists(),
            "its lock file is removed"
        );
        assert!(mine.exists() && theirs.exists());
    }

    /// The records of other versions are removed a week after they were written, once their
    /// server has stopped; those of this version are left to the check of their children (decided
    /// in C4).
    #[test]
    fn records_of_other_versions_go_a_week_after_their_server() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let me = Owner::start(&cache);
        let running = Owner::start(&cache);
        let records = dir.path().join(RECORDS_DIR);
        let record = |version: &str, id: &str, days: u64| {
            let path = records.join(version).join(format!("{id}.json"));
            fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            fs::write(&path, "{}").expect("written");
            let then = SystemTime::now()
                .checked_sub(Duration::from_secs(days * 24 * 60 * 60))
                .expect("in range");
            fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(then))
                .expect("dated");
            path
        };
        let stopped = new_id().into_string();
        let old = record("v0", &stopped, 8);
        let recent = record("v0", &new_id(), 1);
        let alive = record("v0", running.id(), 8);
        let this_version = record(&format!("v{RECORDS_FORMAT}"), &stopped, 8);
        let swept = sweep(&me, later());
        assert_eq!(swept.records, 1);
        assert!(!old.exists());
        assert!(
            !process_lock::lock_path(dir.path(), &owner_key(&stopped)).exists(),
            "the lock file that the check made is removed"
        );
        assert!(recent.exists() && alive.exists() && this_version.exists());
    }

    /// A server makes its directory at once, so that a sweep finds it, and its lock file, even
    /// when it stops before it needs a work directory.
    #[test]
    fn a_server_that_made_no_work_directory_is_found_too() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let gone = Owner::start(&cache);
        let lock = process_lock::lock_path(dir.path(), &owner_key(gone.id()));
        // Stopped without leaving, as a server that is killed.
        drop(gone.alive.lock().take());
        let me = Owner::start(&cache);
        assert_eq!(sweep(&me, later()).work_dirs, 1);
        assert!(!lock.exists());
    }

    /// A server leaves nothing behind when it stops (requirement R-CA-4).
    #[test]
    fn a_server_that_stops_removes_its_work_directories_and_lock() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let me = Owner::start(&Cache::at(dir.path()));
        let mine = me.work_dirs().expect("made");
        me.leave();
        assert!(!mine.exists());
        assert!(!process_lock::lock_path(dir.path(), &owner_key(me.id())).exists());
    }

    /// A sweep stops at its time limit, for the next server to go on (decided in C4).
    #[test]
    fn a_sweep_stops_at_its_limit() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let me = Owner::start(&Cache::at(dir.path()));
        stopped_server(dir.path());
        let swept = sweep(&me, Instant::now());
        assert_eq!(swept, Swept {
            unfinished: true,
            ..Swept::default()
        });
    }
}
