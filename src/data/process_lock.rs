//! Locks across processes (requirement R-CA-4, Q-29; `doc/m2-plan.md`, C1).
//!
//! They are OS locks, as `File::lock` takes them (`flock` on Linux and macOS, `LockFileEx` on
//! Windows), on empty files in `locks/` in the cache directory. The locks are where what they guard is, so that the processes that share a cache exclude each
//! other even when their state directories differ (decided by the user, M2's C1). A lock file is
//! not removed, by GC or by `cache clear`: a process may have opened it to wait, and another
//! would then lock a new file of the same name. The exception is the lock `owner:<id>` of a server
//! (`super::owner`): nobody waits for it, and once its server has stopped, only a sweep that holds
//! it removes it (decided in M2's C4). The name has no format version, so that servers of
//! different versions exclude each other, too. The OS lets a lock go when its process ends,
//! however it ends; the files are opened close-on-exec, so cargo and what it starts do not hold
//! them. A process being started holds a copy of every open file until it runs its program, so a
//! lock let go just then is let go a moment later: a waiter tries again.

use std::{
    fs::{self, File, TryLockError},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

/// The directory of the lock files in the cache directory, which GC and `cache clear` leave.
pub const LOCKS_DIR: &str = super::layout::LOCKS;

/// The first pause between tries to take a lock that another holds.
const MIN_PAUSE: Duration = Duration::from_millis(5);

/// The longest pause between tries.
const MAX_PAUSE: Duration = Duration::from_millis(250);

/// How a lock is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// By one process alone: to fetch or build something, or to remove it.
    Exclusive,
    /// By any number of processes at once, and by none exclusively: to use a target dir that GC
    /// must not remove meanwhile.
    Shared,
}

/// A lock held across processes, let go when it is dropped.
#[derive(Debug)]
pub struct ProcessLock {
    /// The open lock file, whose lock closing it lets go.
    #[cfg_attr(
        not(unix),
        expect(dead_code, reason = "only Unix hands its descriptor to a child")
    )]
    file: File,
}

/// The lock file of `key` in the cache at `root`: the first 32 hex digits of the key's SHA-256,
/// so that names are short and safe on every OS.
///
/// Servers of different versions find each other's locks by these names, so neither the way the
/// name is made nor the keys may change (a test pins them). A key starts with the kind of what it
/// locks and `:` (`cache:` for a path in the cache), so that the locks of different kinds, which
/// may be shared or exclusive, never meet on one file.
#[must_use]
pub fn lock_path(root: &Path, key: &str) -> PathBuf {
    let digest = super::cache::sha256_hex(key.as_bytes());
    let name = digest.get(..32).unwrap_or(&digest);
    root.join(LOCKS_DIR).join(format!("{name}.lock"))
}

/// Takes the lock of `key` in the cache at `root` in `mode` if no other holder stands in the way,
/// or returns `None`.
///
/// # Errors
///
/// Returns an error when the lock file cannot be made or opened, or the OS fails to lock it.
pub fn try_lock(root: &Path, key: &str, mode: LockMode) -> io::Result<Option<ProcessLock>> {
    let file = open(&lock_path(root, key))?;
    try_lock_file(file, mode)
}

/// Takes the lock of the file at `path` in `mode` if no other holder stands in the way, or returns
/// `None`.
///
/// It is for a lock that goes with what it guards rather than in `locks/`, as that which the
/// descendants of a child inherit (`super::owner`).
///
/// # Errors
///
/// Returns an error when the lock file cannot be made or opened, or the OS fails to lock it.
pub fn try_lock_at(path: &Path, mode: LockMode) -> io::Result<Option<ProcessLock>> {
    try_lock_file(open(path)?, mode)
}

/// Waits at most `limit` for the lock of `key` in the cache at `root` in `mode`, and takes it.
///
/// It returns `None` after `limit`: another process that holds it may have stopped (a stopped
/// session, a debugger), and work that waits for it would wait as long.
///
/// Dropping the future stops the wait: the lock is tried again and again, with longer pauses up to
/// a quarter second, instead of a blocking call that could not be called off.
///
/// # Errors
///
/// Returns an error when the lock file cannot be made or opened, or the OS fails to lock it.
pub async fn lock(
    root: &Path,
    key: &str,
    mode: LockMode,
    limit: Duration,
) -> io::Result<Option<ProcessLock>> {
    let path = lock_path(root, key);
    let mut file = super::blocking(move || open(&path)).await?;
    let start = tokio::time::Instant::now();
    let mut pause = MIN_PAUSE;
    loop {
        // The same open file each time: a lock is the open file's, and trying does not block.
        let again = file.try_clone()?;
        if let Some(lock) = try_lock_file(file, mode)? {
            return Ok(Some(lock));
        }
        if start.elapsed() >= limit {
            return Ok(None);
        }
        file = again;
        tokio::time::sleep(pause).await;
        pause = pause.saturating_mul(2).min(MAX_PAUSE);
    }
}

/// [`lock`] for a blocking thread, which waits at most `limit`, or returns `None`. It must not be
/// called from async code.
///
/// It tries again and again, as [`lock`] does, instead of waiting in the OS: a blocking thread
/// cannot be called off, and another process that holds the lock may have stopped (a stopped
/// session, a debugger), which would keep the thread, and this process when it ends, waiting.
///
/// # Errors
///
/// Returns an error when the lock file cannot be made or opened, or the OS fails to lock it.
pub fn lock_blocking(
    root: &Path,
    key: &str,
    mode: LockMode,
    limit: Duration,
) -> io::Result<Option<ProcessLock>> {
    lock_blocking_at(&lock_path(root, key), mode, limit)
}

#[cfg(unix)]
impl std::os::fd::AsRawFd for ProcessLock {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

/// [`try_lock_at`], trying again for at most `limit`, as [`lock_blocking`] does. It must not be
/// called from async code, but with a short `limit`.
///
/// # Errors
///
/// Returns an error when the lock file cannot be made or opened, or the OS fails to lock it.
pub fn lock_blocking_at(
    path: &Path,
    mode: LockMode,
    limit: Duration,
) -> io::Result<Option<ProcessLock>> {
    let start = std::time::Instant::now();
    let mut file = open(path)?;
    let mut pause = MIN_PAUSE;
    loop {
        let again = file.try_clone()?;
        if let Some(lock) = try_lock_file(file, mode)? {
            return Ok(Some(lock));
        }
        if start.elapsed() >= limit {
            return Ok(None);
        }
        file = again;
        std::thread::sleep(pause);
        pause = pause.saturating_mul(2).min(MAX_PAUSE);
    }
}

/// Locks `file` in `mode` if no other holder stands in the way.
fn try_lock_file(file: File, mode: LockMode) -> io::Result<Option<ProcessLock>> {
    let locked = match mode {
        LockMode::Exclusive => file.try_lock(),
        LockMode::Shared => file.try_lock_shared(),
    };
    match locked {
        Ok(()) => Ok(Some(ProcessLock { file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Opens the lock file at `path`, made if missing, without following a symlink (requirement
/// R-CA-4; Windows has no flag for it).
fn open(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut options = fs::OpenOptions::new();
    // `LockFileEx` needs a handle that may read or write.
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead as _, BufReader},
        time::Instant,
    };

    use super::*;

    #[test]
    fn an_exclusive_lock_stands_in_the_way_of_any_other() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        let held = try_lock(root, "key", LockMode::Exclusive)
            .expect("locked")
            .expect("free");
        // Another open file of the same name, as another process would have.
        for mode in [LockMode::Exclusive, LockMode::Shared] {
            assert!(
                try_lock(root, "key", mode).expect("tried").is_none(),
                "{mode:?}"
            );
        }
        assert!(
            try_lock(root, "other", LockMode::Exclusive)
                .expect("tried")
                .is_some(),
            "another key"
        );
        drop(held);
        assert!(taken_within(root, Duration::from_secs(5)), "let go");
    }

    /// Whether the exclusive lock of `key` in `root` is taken within `limit`: a process that
    /// another test starts holds a copy of the lock file a moment (see the module's comment).
    fn taken_within(root: &Path, limit: Duration) -> bool {
        let start = Instant::now();
        loop {
            if try_lock(root, "key", LockMode::Exclusive)
                .expect("tried")
                .is_some()
            {
                return true;
            }
            if start.elapsed() > limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shared_locks_stand_in_the_way_of_an_exclusive_one_only() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        let one = try_lock(root, "key", LockMode::Shared).expect("locked");
        let two = try_lock(root, "key", LockMode::Shared).expect("locked");
        assert!(one.is_some() && two.is_some());
        assert!(
            try_lock(root, "key", LockMode::Exclusive)
                .expect("tried")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_waiter_takes_the_lock_once_it_is_let_go() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().to_path_buf();
        let held = try_lock(&root, "key", LockMode::Exclusive)
            .expect("locked")
            .expect("free");
        let waiter = tokio::spawn({
            let root = root.clone();
            async move { lock(&root, "key", LockMode::Exclusive, Duration::from_secs(60)).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished(), "it waits");
        drop(held);
        let taken = tokio::time::timeout(Duration::from_secs(5), waiter).await;
        assert!(matches!(taken, Ok(Ok(Ok(Some(_))))), "{taken:?}");
        // Held by another, which `taken` holds, for longer than the wait: given up.
        let start = Instant::now();
        let given_up = lock(
            &root,
            "key",
            LockMode::Exclusive,
            Duration::from_millis(100),
        )
        .await;
        assert!(matches!(given_up, Ok(None)), "{given_up:?}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_blocking_wait_gives_up_at_its_limit() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        let _held = try_lock(root, "key", LockMode::Exclusive)
            .expect("locked")
            .expect("free");
        let start = Instant::now();
        let waited = lock_blocking(root, "key", LockMode::Exclusive, Duration::from_millis(100))
            .expect("tried");
        assert!(waited.is_none(), "held by another");
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(
            lock_blocking(root, "other", LockMode::Exclusive, Duration::ZERO)
                .expect("tried")
                .is_some()
        );
    }

    /// The name is shared with servers of other versions, so it must not change.
    #[test]
    fn the_name_of_a_lock_does_not_change() {
        let path = lock_path(Path::new("cache"), "cache:docs.rs/serde/1.0.228/_default");
        assert_eq!(
            path,
            Path::new("cache")
                .join(LOCKS_DIR)
                .join("6d886635d2339ea8faa503738ed94f39.lock")
        );
    }

    #[test]
    fn the_lock_file_is_in_the_locks_directory_and_kept() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = lock_path(dir.path(), "docs.rs/serde/1.0.0");
        assert_eq!(path.parent(), Some(dir.path().join(LOCKS_DIR).as_path()));
        drop(try_lock(
            dir.path(),
            "docs.rs/serde/1.0.0",
            LockMode::Exclusive,
        ));
        assert!(path.is_file(), "an empty file, which stays");
    }

    /// The environment variable that tells [`hold_a_lock_until_killed`] where the cache is.
    const HOLDER_ROOT: &str = "CHATORA_RUSTDOC_TEST_LOCK_ROOT";

    /// Not a test by itself: the process that [`a_lock_goes_with_its_process`] starts, which takes
    /// the lock, says so, and waits to be killed.
    #[test]
    #[ignore = "started by a_lock_goes_with_its_process"]
    fn hold_a_lock_until_killed() {
        let Some(root) = std::env::var_os(HOLDER_ROOT) else {
            return;
        };
        let _held = try_lock(Path::new(&root), "key", LockMode::Exclusive)
            .expect("locked")
            .expect("free");
        println!("locked");
        // Killed long before; gone even if the test that started it is not.
        std::thread::sleep(Duration::from_secs(600));
    }

    /// Another process stands in the way while it holds the lock, and the OS lets the lock go when
    /// the process is killed, on every OS (Windows lets it go once it has the resources to).
    #[test]
    fn a_lock_goes_with_its_process() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        let mut holder = crate::test_process::start(
            "data::process_lock::tests::hold_a_lock_until_killed",
            &[(HOLDER_ROOT, root.as_os_str())],
        );
        let stdout = holder.stdout.take().expect("piped");
        let locked = BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            // With one test thread (`RUST_TEST_THREADS=1`, or one CPU), libtest writes the line
            // that names the test before what the test prints, on the same line.
            .any(|line| line.ends_with("locked"));
        assert!(locked, "the other process took the lock");
        assert!(
            try_lock(root, "key", LockMode::Exclusive)
                .expect("tried")
                .is_none(),
            "the other process holds it"
        );
        holder.kill().expect("killed");
        holder.wait().expect("waited");
        assert!(
            taken_within(root, Duration::from_secs(10)),
            "the lock went with the process"
        );
    }
}
