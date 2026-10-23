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
pub const WORK_DIR: &str = super::layout::WORK;

/// The directory of the records of the servers' children in the cache. The records of each
/// format are in a directory `v<format>` of their own (decided in C1).
pub const RECORDS_DIR: &str = super::layout::RECORDS;

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

    /// Lets this server's lock go without removing anything, as when the server is killed: for the
    /// tests of what a later server does. It returns once the lock is free: a child that another
    /// test starts meanwhile holds a copy of it until it runs its program.
    #[cfg(test)]
    pub(super) fn killed_for_test(&self) {
        drop(self.alive.lock().take());
        let free = process_lock::lock_blocking(
            &self.root,
            &owner_key(&self.id),
            LockMode::Exclusive,
            Duration::from_secs(10),
        );
        assert!(
            matches!(free, Ok(Some(_))),
            "the lock of a killed server is let go"
        );
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

/// What a server keeps of a child while the child's group runs, on Unix (decided in C4).
///
/// That is a record of its session and when it started, and a lock that its descendants inherit,
/// so that the lock is held while any of them runs, even past the server.
///
/// When the group is stopped, the lock is tried again: if it is free, nothing of the group runs,
/// and both go; else the record stays, for the sweep of a later server (C4c). Windows has none:
/// the Job Object stops the group when the server ends, and an inheritable handle would go to
/// every child started meanwhile.
#[cfg(unix)]
#[derive(Debug)]
pub struct ChildRecord {
    /// The record, `procs/v<format>/<id>.<number>.json`.
    record: PathBuf,
    /// The lock file next to it, `<id>.<number>.lock`.
    lock_path: PathBuf,
    /// The lock, held by this server, until the group is stopped.
    lock: Option<ProcessLock>,
    /// The target directory that the child builds in, if it builds, as [`Record::target_dir`].
    target_dir: Option<String>,
}

/// What a record says of a child (decided in C4).
#[cfg(unix)]
#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Record {
    /// The machine and the boot that the child ran in, if they could be read.
    pub machine: Option<String>,
    /// The boot.
    pub boot: Option<String>,
    /// The session of the child, which is its process ID: it leads a session of its own.
    pub session: u32,
    /// When the child started, as [`super::process_id::started`] tells.
    pub started: Option<u64>,
    /// The target directory that the child builds in, if it builds: `t/<name>` in the cache,
    /// which is marked as not to be used when what runs cannot be told for sure to be the
    /// child's (decided in M2's D2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_dir: Option<String>,
}

#[cfg(unix)]
impl Owner {
    /// The directory of this version's records.
    fn records(&self) -> PathBuf {
        self.root
            .join(RECORDS_DIR)
            .join(format!("v{RECORDS_FORMAT}"))
    }

    /// The record of the child `number` that is about to start, with its lock, which this server
    /// holds: the child is to inherit it ([`ChildRecord::inherited`]). `target_dir` is the target
    /// directory in the cache that the child builds in, if it builds.
    ///
    /// # Errors
    ///
    /// Returns an error when the OS could not lock this server's lock, or the lock of the child
    /// cannot be made.
    pub fn child_record(&self, number: u64, target_dir: Option<&Path>) -> io::Result<ChildRecord> {
        if !self.locked() {
            return Err(io::Error::other(
                "the cache cannot be locked, so the children are not recorded",
            ));
        }
        let records = self.records();
        fs::create_dir_all(&records)?;
        let stem = format!("{}.{number}", self.id);
        let lock_path = records.join(format!("{stem}.lock"));
        let lock = process_lock::try_lock_at(&lock_path, LockMode::Exclusive)?
            .ok_or_else(|| io::Error::other("the lock of a new child is held"))?;
        let target_dir = target_dir.and_then(|dir| {
            let name = dir
                .strip_prefix(self.root.join(super::build::TARGET_DIRS))
                .ok()
                .and_then(|name| name.to_str())
                .filter(|name| is_target_dir_name(name));
            if name.is_none() {
                tracing::warn!(dir = %dir.display(), "a build's target directory is not one of the cache's, so it is not recorded");
            }
            name.map(|name| format!("{}/{name}", super::build::TARGET_DIRS))
        });
        Ok(ChildRecord {
            record: records.join(format!("{stem}.json")),
            lock_path,
            lock: Some(lock),
            target_dir,
        })
    }
}

#[cfg(unix)]
impl ChildRecord {
    /// The file descriptor of the lock, which the child is to inherit.
    #[must_use]
    pub fn inherited(&self) -> Option<std::os::fd::RawFd> {
        self.lock.as_ref().map(std::os::fd::AsRawFd::as_raw_fd)
    }

    /// Writes the record of the child `pid`, which has started.
    pub fn started(&self, pid: u32) {
        let (machine, boot) = super::process_id::machine_and_boot()
            .cloned()
            .map_or((None, None), |(machine, boot)| (Some(machine), Some(boot)));
        let record = Record {
            machine,
            boot,
            session: pid,
            started: super::process_id::started(pid),
            target_dir: self.target_dir.clone(),
        };
        // Written whole, but not synced: after a crash, the boot differs, and the record is not
        // used.
        let temporary = super::cache::with_suffix(&self.record, ".new");
        let written = serde_json::to_vec(&record)
            .map_err(io::Error::other)
            .and_then(|json| fs::write(&temporary, json))
            .and_then(|()| fs::rename(&temporary, &self.record));
        if let Err(error) = written {
            tracing::warn!(path = %self.record.display(), %error, "a child could not be recorded");
        }
    }
}

#[cfg(unix)]
impl Drop for ChildRecord {
    /// The group has been stopped: the record and the lock go unless a descendant still holds it.
    fn drop(&mut self) {
        drop(self.lock.take());
        let (record, lock_path) = (
            std::mem::take(&mut self.record),
            std::mem::take(&mut self.lock_path),
        );
        // At once, when nothing holds the lock any more, as when the server ends and starts no
        // more blocking work.
        if let Ok(Some(held)) = process_lock::try_lock_at(&lock_path, LockMode::Exclusive) {
            drop(fs::remove_file(&record));
            drop(fs::remove_file(&lock_path));
            drop(held);
            return;
        }
        // Else it waits a moment: on a blocking thread, not one that runs async work.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(move || remove_record(&record, &lock_path))),
            Err(_) => remove_record(&record, &lock_path),
        }
    }
}

/// Removes the record of a child at `record`, and its lock file at `lock_path`, unless a
/// descendant of the child still holds the lock. What was killed lets the lock go as it ends, a
/// moment later, as does a child being started meanwhile, which holds a copy until it runs its
/// program.
#[cfg(unix)]
fn remove_record(record: &Path, lock_path: &Path) {
    match process_lock::lock_blocking_at(lock_path, LockMode::Exclusive, Duration::from_millis(100))
    {
        Ok(Some(held)) => {
            drop(fs::remove_file(record));
            drop(fs::remove_file(lock_path));
            drop(held);
        },
        Ok(None) => tracing::info!(
            record = %record.display(),
            "a descendant of a child that was stopped still runs, so its record is kept"
        ),
        Err(error) => {
            tracing::debug!(%error, "whether a descendant of a child still runs cannot be told");
        },
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
    /// The records of children of servers that have stopped, removed once nothing of the child
    /// ran or what ran was stopped.
    pub child_records: usize,
    /// The processes stopped: descendants of those children that outlived their server.
    pub stopped: usize,
    /// The records kept, as what runs could not be told for sure to be the child's.
    pub unsure: usize,
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
    // Before the work directories, whose sweep removes the lock files of their servers.
    #[cfg(unix)]
    if !settle_children(me, until, &mut swept) {
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

/// Settles the records of the children of servers that have stopped, until `until` (decided in
/// C4). Returns `false` when it stopped at `until`.
#[cfg(unix)]
fn settle_children(me: &Owner, until: Instant, swept: &mut Swept) -> bool {
    use std::collections::BTreeMap;

    let mut servers: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for (id, record) in owned(&me.records()) {
        // Not those of a server that runs.
        if id != *me.id
            && record
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            servers.entry(id).or_default().push(record);
        }
    }
    for (id, records) in servers {
        if Instant::now() >= until {
            swept.unfinished = true;
            return false;
        }
        // Nor those of one that this one cannot tell of. Its lock is held over all its records:
        // a process that another thread starts meanwhile holds a copy of it until it runs its
        // program, and the server would look alive again.
        let Some(held) = stopped(&me.root, &id) else {
            continue;
        };
        let mut finished = true;
        for record in records {
            if Instant::now() >= until {
                swept.unfinished = true;
                finished = false;
                break;
            }
            match settle(&me.root, &record, until) {
                Settled::Ended => swept.child_records = swept.child_records.saturating_add(1),
                Settled::Stopped(count) => {
                    swept.child_records = swept.child_records.saturating_add(1);
                    swept.stopped = swept.stopped.saturating_add(count);
                },
                Settled::Unsure => swept.unsure = swept.unsure.saturating_add(1),
            }
        }
        // The check made its lock file again if an earlier sweep removed it with its work
        // directories; then nothing else would remove it.
        if !me.root.join(WORK_DIR).join(&id).exists() {
            remove_lock_file(&me.root, &id, held);
        }
        if !finished {
            return false;
        }
    }
    true
}

/// How the record of a child of a server that has stopped was settled.
#[cfg(unix)]
enum Settled {
    /// Nothing of the child ran: the lock that its descendants inherited was free.
    Ended,
    /// So many of its descendants were stopped.
    Stopped(usize),
    /// What holds the lock could not be told for sure to be its descendants, and the record is
    /// kept.
    Unsure,
}

/// Settles the record at `record`: removes it, with its lock file, once nothing of the child runs,
/// after stopping the descendants that are known for sure to be the child's (decided in C4).
#[cfg(unix)]
fn settle(root: &Path, record: &Path, until: Instant) -> Settled {
    let lock_path = record.with_extension("lock");
    let remove = |held: ProcessLock| {
        drop(fs::remove_file(record));
        drop(fs::remove_file(&lock_path));
        drop(held);
    };
    if let Ok(Some(held)) = process_lock::try_lock_at(&lock_path, LockMode::Exclusive) {
        remove(held);
        return Settled::Ended;
    }
    let read: Option<Record> = fs::read(record)
        .ok()
        .and_then(|json| serde_json::from_slice(&json).ok());
    let Some(descendants) = read.as_ref().and_then(|read| descendants(read, &lock_path)) else {
        tracing::info!(record = %record.display(), "what still runs of a child of a server that has stopped cannot be told for sure to be its, so it is left");
        mark_unusable(root, read.as_ref());
        return Settled::Unsure;
    };
    for pid in &descendants {
        super::process_id::kill(*pid);
    }
    // They let the lock go as they end.
    let limit = until
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(1));
    if let Ok(Some(held)) = process_lock::lock_blocking_at(&lock_path, LockMode::Exclusive, limit) {
        tracing::info!(record = %record.display(), stopped = descendants.len(), "stopped what a child of a server that has stopped left running");
        remove(held);
        Settled::Stopped(descendants.len())
    } else {
        tracing::info!(record = %record.display(), "a descendant of a child of a server that has stopped still runs outside its session, so the record is left");
        mark_unusable(root, read.as_ref());
        Settled::Unsure
    }
}

/// The target directories in `t/` of the cache at `root` that the children recorded there build in.
///
/// What they left running may still use them, which the garbage collection does not remove
/// (decided in M2f's F1b). The records of servers that run are of children that run.
///
/// It reads files, so it is for a blocking thread.
#[cfg(unix)]
#[must_use]
pub fn recorded_target_dirs(root: &Path) -> std::collections::HashSet<String> {
    let records = root.join(RECORDS_DIR).join(format!("v{RECORDS_FORMAT}"));
    owned(&records)
        .into_iter()
        .filter_map(|(_, path)| {
            let record: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
            let dir = record.get("target_dir")?.as_str()?;
            let name = dir
                .strip_prefix(super::build::TARGET_DIRS)?
                .strip_prefix('/')?;
            is_target_dir_name(name).then(|| name.to_owned())
        })
        .collect()
}

/// Marks the target directory of the cache at `root` that the child of `record` built in, if it
/// did, as not to be used: what still runs may hold cargo's lock there, and the next build uses
/// another (decided in M2's D2).
#[cfg(unix)]
fn mark_unusable(root: &Path, record: Option<&Record>) {
    let Some(name) = record
        .and_then(|record| record.target_dir.as_deref())
        .and_then(|dir| {
            dir.strip_prefix(super::build::TARGET_DIRS)?
                .strip_prefix('/')
        })
        .filter(|name| is_target_dir_name(name))
    else {
        return;
    };
    let dir = root.join(super::build::TARGET_DIRS).join(name);
    if !dir.is_dir() {
        return;
    }
    match fs::write(
        dir.join(super::build::UNUSABLE),
        "what a stopped server's build left running may still use this directory\n",
    ) {
        Ok(()) => {
            tracing::info!(dir = %dir.display(), "a target directory is not to be used, as what a stopped server's build left running may still use it");
        },
        Err(error) => {
            tracing::warn!(dir = %dir.display(), %error, "a target directory could not be marked as not to be used");
        },
    }
}

/// Whether `name` can name a target directory in the cache's `t/`: a name of one part, which
/// leads nowhere else.
#[cfg(unix)]
fn is_target_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
        && !name.starts_with('.')
}

/// The processes that run as descendants of the child of `record`, when that can be told for
/// sure, or `None`.
///
/// They are, on the same machine since the same boot, the processes of the child's session that
/// started since the child, whose leader, if it runs, is the child itself (else the ID was used
/// again). On Linux, each must also have the lock `lock` open, which only the child's descendants
/// inherited: the leader, cargo, dies with the server there, and a session whose members all
/// ended may have the ID again. On macOS, where that cannot be read, the leader must run: cargo
/// does not die with the server there.
#[cfg(unix)]
fn descendants(record: &Record, lock: &Path) -> Option<Vec<u32>> {
    let (machine, boot) = super::process_id::machine_and_boot()?;
    if record.machine.as_ref() != Some(machine) || record.boot.as_ref() != Some(boot) {
        return None;
    }
    let started = record.started?;
    let members = super::process_id::session_members(record.session)?;
    let leader = members
        .iter()
        .find(|(pid, _)| *pid == record.session)
        .map(|(_, since)| *since);
    let used_again = leader.is_some_and(|since| since != Some(started));
    if used_again || (leader.is_none() && cfg!(target_os = "macos")) {
        return None;
    }
    let own = std::process::id();
    let found: Vec<u32> = members
        .into_iter()
        .filter(|(pid, since)| *pid != own && since.is_some_and(|since| since >= started))
        .filter(|(pid, _)| {
            cfg!(not(target_os = "linux")) || super::process_id::has_open(*pid, lock) == Some(true)
        })
        .map(|(pid, _)| pid)
        .collect();
    (!found.is_empty()).then_some(found)
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
    /// server has stopped; those of this version are settled by whether their children run
    /// (decided in C4).
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
        // That of this version is a child's, which is settled by whether it runs: nothing holds
        // its lock, so it has ended. Windows records no child, and leaves it.
        if cfg!(unix) {
            assert_eq!(swept.child_records, 1);
            assert!(!this_version.exists());
        } else {
            assert!(this_version.exists());
        }
        assert!(
            !process_lock::lock_path(dir.path(), &owner_key(&stopped)).exists(),
            "the lock file that the check made is removed"
        );
        assert!(recent.exists() && alive.exists());
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
        gone.killed_for_test();
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
