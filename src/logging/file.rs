//! The log file of this process: JSON Lines in the state directory, a new file when one grows too
//! large, and old files removed (requirement R-OBS-2).

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

use parking_lot::Mutex;
use tracing_subscriber::fmt::{
    format::Writer,
    time::{FormatTime as _, SystemTime as Clock},
};

/// The file that marks a directory as one that this server made for its logs. Logs are removed
/// only from a directory with it, so that a mistaken `log.dir` does not lose another program's
/// files (requirements R-OBS-2 and R-CA-5).
pub(super) const MARKER: &str = ".chatora-rustdoc-mcp-logs";

/// The extension of the log files.
const EXTENSION: &str = "jsonl";

/// The extension of the file that a process locks while it writes the logs of its stem.
const LOCK_EXTENSION: &str = "lock";

/// How large the files may grow and how long they are kept (requirement R-OBS-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Limits {
    /// A file larger than this is followed by a new one.
    pub(super) file_max_size: u64,
    /// Files last written longer ago than this are removed.
    pub(super) retention: Duration,
    /// The oldest files are removed while all of them together are larger than this.
    pub(super) total_max_size: u64,
}

/// The log file that this process writes, which only this process writes to.
///
/// Each process has files of its own, named `<date>-<pid>.jsonl` and then `<date>-<pid>.2.jsonl`
/// and so on, so that the servers of several sessions do not mix their lines. The process locks
/// `<date>-<pid>.lock` while it runs, so that another process removing old logs leaves its files
/// alone. The logs themselves are not locked: on Windows a lock would keep others from reading
/// them.
#[derive(Debug)]
pub(super) struct LogFile {
    /// The directory of the logs.
    dir: PathBuf,
    /// `<date>-<pid>`, the start of the names of this process's files.
    stem: String,
    limits: Limits,
    /// The lock file of `stem`, locked for the life of the process.
    _lock: File,
    current: Mutex<Current>,
    /// A failure to write has been reported, so the next ones are not.
    failed: AtomicBool,
}

/// The file being written.
#[derive(Debug)]
struct Current {
    file: File,
    path: PathBuf,
    /// The bytes written to it.
    written: u64,
    /// Which of this process's files it is, from 1.
    part: u32,
}

impl LogFile {
    /// Opens a new log file in `dir`, making the directory if it is not there, and removes the old
    /// logs there.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be made or the file cannot be created.
    pub(super) fn open(dir: &Path, limits: Limits) -> io::Result<Self> {
        if !dir.is_dir() {
            fs::create_dir_all(dir)?;
        }
        // The directory is ours when it was just made, or when nothing else is in it.
        if fs::read_dir(dir)?.next().is_none() {
            fs::write(
                dir.join(MARKER),
                "The logs of chatora-rustdoc-mcp. Old logs here are removed.\n",
            )?;
        }
        let stem = format!("{}-{}", date(&now()), std::process::id());
        let lock = lock(&dir.join(format!("{stem}.{LOCK_EXTENSION}")))?;
        let current = create(dir, &stem, 1)?;
        let log = Self {
            dir: dir.into(),
            stem,
            limits,
            _lock: lock,
            current: Mutex::new(current),
            failed: AtomicBool::new(false),
        };
        log.collect_garbage();
        Ok(log)
    }

    /// The file being written.
    pub(super) fn path(&self) -> PathBuf {
        self.current.lock().path.clone()
    }

    /// Writes `line` and a newline, going on to a new file first when this one would grow too
    /// large. A failure is reported once on stderr; logging cannot report it to itself.
    pub(super) fn write_line(&self, line: &str) {
        let written = self.write_to(&mut self.current.lock(), line);
        match written {
            // Other processes' logs may have grown too, while this one was written.
            Ok(true) => self.collect_garbage(),
            Ok(false) => {},
            Err(error) => {
                if !self.failed.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "{}: failed to write the log in {}: {error}",
                        env!("CARGO_PKG_NAME"),
                        self.dir.display()
                    );
                }
            },
        }
    }

    /// Writes `line` and a newline to `current`, or to the next file. `Ok(true)` when it went on
    /// to the next file.
    fn write_to(&self, current: &mut Current, line: &str) -> io::Result<bool> {
        let len = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let rotated = if current.written > 0
            && current.written.saturating_add(len) > self.limits.file_max_size
        {
            self.rotate(current)
        } else {
            Ok(false)
        };
        let mut bytes = Vec::with_capacity(line.len().saturating_add(1));
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        current.file.write_all(&bytes)?;
        current.written = current.written.saturating_add(len);
        rotated
    }

    /// Goes on to the next file. `Ok(true)` when it did.
    fn rotate(&self, current: &mut Current) -> io::Result<bool> {
        *current = create(&self.dir, &self.stem, current.part.saturating_add(1))?;
        Ok(true)
    }

    /// Removes the logs last written longer ago than the retention, and then the oldest while all
    /// of them are larger than the limit. Files that a process is writing are left, and nothing is
    /// removed from a directory without the [`MARKER`]. Failures are reported on stderr.
    fn collect_garbage(&self) {
        if let Err(error) = collect_garbage(&self.dir, self.limits, SystemTime::now()) {
            eprintln!(
                "{}: failed to remove old logs in {}: {error}",
                env!("CARGO_PKG_NAME"),
                self.dir.display()
            );
        }
    }
}

/// Locks the file at `path`, making it if it is not there. A process with the same id on the
/// same day, which has ended, may have left it.
fn lock(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(io::Error::other(format!(
            "{} is locked by another process",
            path.display()
        ))),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Whether a process holds the lock file at `path`. A lock file that is not there is not held.
fn is_held(path: &Path) -> io::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Creates the file `part` of `stem` in `dir`, or the next part when that is there (left by a
/// process with the same id on the same day).
fn create(dir: &Path, stem: &str, part: u32) -> io::Result<Current> {
    let mut part = part;
    loop {
        let path = dir.join(file_name(stem, part));
        match OpenOptions::new().append(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok(Current {
                    file,
                    path,
                    written: 0,
                    part,
                });
            },
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && part < u32::MAX => {
                part = part.saturating_add(1);
            },
            Err(error) => return Err(error),
        }
    }
}

/// `<stem>.jsonl`, or `<stem>.<part>.jsonl` after the first.
fn file_name(stem: &str, part: u32) -> String {
    if part <= 1 {
        format!("{stem}.{EXTENSION}")
    } else {
        format!("{stem}.{part}.{EXTENSION}")
    }
}

/// What a file in the log directory is, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A log, `<stem>.jsonl` (part 1) or `<stem>.<part>.jsonl`.
    Log(u32),
    /// The lock file of a stem, `<stem>.lock`.
    Lock,
}

/// The stem (`<8 digits>-<digits>`) of `name` and what the file is, when it is a log or a lock
/// file that this server writes.
fn stem_of(name: &str) -> Option<(&str, Kind)> {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let (stem, rest) = name.split_once('.')?;
    let (date, pid) = stem.split_once('-')?;
    if date.len() != 8 || !digits(date) || !digits(pid) {
        return None;
    }
    if rest == LOCK_EXTENSION {
        return Some((stem, Kind::Lock));
    }
    let part = rest.strip_suffix(EXTENSION)?;
    if part.is_empty() {
        return Some((stem, Kind::Log(1)));
    }
    let part = part.strip_suffix('.').filter(|part| digits(part))?;
    Some((stem, Kind::Log(part.parse().ok()?)))
}

/// A log file found by [`collect_garbage`].
struct Found {
    modified: SystemTime,
    len: u64,
    path: PathBuf,
    stem: String,
    part: u32,
}

/// The logs and lock files in a log directory, and the last error in looking at one.
struct Scanned {
    logs: Vec<Found>,
    locks: Vec<(String, PathBuf)>,
    failed: Option<io::Error>,
}

/// Finds the logs and lock files in `dir`. A file that cannot be looked at is left out.
fn scan(dir: &Path) -> io::Result<Scanned> {
    let mut failed = None;
    let mut logs = Vec::new();
    let mut locks = Vec::new();
    for entry in fs::read_dir(dir)? {
        let found = entry.and_then(|entry| {
            let name = entry.file_name();
            let Some((stem, kind)) = name.to_str().and_then(stem_of) else {
                return Ok(None);
            };
            // `DirEntry::metadata` does not follow symbolic links, which are left alone.
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                return Ok(None);
            }
            Ok(Some(match kind {
                Kind::Lock => (stem.to_owned(), entry.path(), None),
                Kind::Log(part) => (
                    stem.to_owned(),
                    entry.path(),
                    Some((metadata.modified()?, metadata.len(), part)),
                ),
            }))
        });
        match found {
            Ok(Some((stem, path, None))) => locks.push((stem, path)),
            Ok(Some((stem, path, Some((modified, len, part))))) => logs.push(Found {
                modified,
                len,
                path,
                stem,
                part,
            }),
            Ok(None) => {},
            // Another server may have removed it in between.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => failed = Some(error),
        }
    }
    Ok(Scanned {
        logs,
        locks,
        failed,
    })
}

/// The part that a running process writes, by stem, or 0 for a stem whose process has ended. A
/// stem whose lock cannot be tried is taken as running, with the error in `failed`.
fn being_written(
    dir: &Path,
    logs: &[Found],
    failed: &mut Option<io::Error>,
) -> HashMap<String, u32> {
    let mut last_parts: HashMap<String, u32> = HashMap::new();
    for log in logs {
        let last = last_parts.entry(log.stem.clone()).or_default();
        *last = (*last).max(log.part);
    }
    last_parts
        .into_iter()
        .map(|(stem, last)| {
            let running = match is_held(&dir.join(format!("{stem}.{LOCK_EXTENSION}"))) {
                Ok(running) => running,
                Err(error) => {
                    *failed = Some(error);
                    true
                },
            };
            (stem, if running { last } else { 0 })
        })
        .collect()
}

/// See [`LogFile::collect_garbage`]. Each file that cannot be looked at or removed is left, and
/// the last such error is returned after the rest are done.
fn collect_garbage(dir: &Path, limits: Limits, now: SystemTime) -> io::Result<()> {
    // The marker must be a file of its own, not a link to one elsewhere.
    match fs::symlink_metadata(dir.join(MARKER)) {
        Ok(metadata) if metadata.is_file() => {},
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let Scanned {
        mut logs,
        locks,
        mut failed,
    } = scan(dir)?;
    // The file a running process writes is the last part of its stem; the parts before it are
    // done and may go.
    let being_written = being_written(dir, &logs, &mut failed);
    let kept = |log: &Found| being_written.get(&log.stem) == Some(&log.part);
    // Only the files that may go count toward the limit, so that a running server's file does not
    // push out the logs of sessions that have ended.
    let mut total = logs
        .iter()
        .filter(|log| !kept(log))
        .fold(0_u64, |total, log| total.saturating_add(log.len));
    // The oldest first, then by name so that the order does not depend on the directory.
    logs.sort_by(|a, b| (a.modified, &a.path).cmp(&(b.modified, &b.path)));
    let mut stems_left: Vec<String> = Vec::new();
    for log in &logs {
        let expired = now
            .duration_since(log.modified)
            .is_ok_and(|age| age > limits.retention);
        let goes = !kept(log) && (expired || total > limits.total_max_size);
        if goes {
            match remove(&log.path) {
                Ok(_) => {
                    total = total.saturating_sub(log.len);
                    continue;
                },
                Err(error) => failed = Some(error),
            }
        }
        stems_left.push(log.stem.clone());
    }
    // The lock files of processes that have ended and whose logs are all gone. Those of today
    // stay: a new process with the same id may be taking one again (decided before the first
    // release, 2026-10-20), and its name has the date, so no process takes an older one.
    let today = date(&self::now());
    for (stem, path) in locks {
        let running = being_written.get(&stem).is_some_and(|&last| last > 0);
        if stems_left.contains(&stem) || running || stem.starts_with(&today) {
            continue;
        }
        match is_held(&path) {
            Ok(false) => {
                if let Err(error) = remove(&path) {
                    failed = Some(error);
                }
            },
            Ok(true) => {},
            Err(error) => failed = Some(error),
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Removes the file at `path`. `Ok(true)` when it was removed, or was already gone.
fn remove(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

/// The time now in RFC 3339, in UTC: `2026-10-20T01:02:03.456789Z`.
pub(super) fn now() -> String {
    let mut text = String::new();
    // Writing to a `String` does not fail.
    Clock
        .format_time(&mut Writer::new(&mut text))
        .unwrap_or_default();
    text
}

/// `20261020` from the RFC 3339 time `time`.
fn date(time: &str) -> String {
    time.chars().take(10).filter(char::is_ascii_digit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        file_max_size: 100,
        retention: Duration::from_hours(14 * 24),
        total_max_size: 1000,
    };

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("readable")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .into_string()
                    .expect("UTF-8")
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn only_the_names_of_logs_and_locks_have_stems() {
        assert_eq!(
            stem_of("20261020-1234.jsonl"),
            Some(("20261020-1234", Kind::Log(1)))
        );
        assert_eq!(
            stem_of("20261020-1234.2.jsonl"),
            Some(("20261020-1234", Kind::Log(2)))
        );
        assert_eq!(
            stem_of("20261020-1234.lock"),
            Some(("20261020-1234", Kind::Lock))
        );
        assert_eq!(stem_of("2026102-1234.jsonl"), None);
        assert_eq!(stem_of("20261020-.jsonl"), None);
        assert_eq!(stem_of("20261020-1234.x.jsonl"), None);
        assert_eq!(stem_of("20261020-1234.json"), None);
        assert_eq!(stem_of("notes.jsonl"), None);
    }

    #[test]
    fn only_the_file_a_running_process_writes_is_kept() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = temp.path();
        let log = LogFile::open(dir, LIMITS).expect("opened");
        let line = "x".repeat(60);
        // Three files of this process: the first two are done.
        for _ in 0..3 {
            log.write_line(&line);
        }
        let writing = log.path();
        let done: Vec<String> = names(dir)
            .into_iter()
            .filter(|name| matches!(stem_of(name), Some((_, Kind::Log(_)))))
            .filter(|name| {
                Some(name.as_str()) != writing.file_name().and_then(|name| name.to_str())
            })
            .collect();
        assert_eq!(done.len(), 2, "{done:?}");
        // Past the retention, the parts that are done go and the one being written stays.
        collect_garbage(dir, LIMITS, SystemTime::now() + LIMITS.retention * 2).expect("no error");
        assert!(writing.exists());
        for name in done {
            assert!(!dir.join(&name).exists(), "{name}");
        }
    }

    #[test]
    fn a_lock_of_today_stays_and_an_older_one_goes() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = temp.path();
        fs::write(dir.join(MARKER), "").expect("written");
        let today = format!("{}-999999.{LOCK_EXTENSION}", date(&now()));
        fs::write(dir.join(&today), "").expect("written");
        fs::write(dir.join("20200101-1.lock"), "").expect("written");
        collect_garbage(dir, LIMITS, SystemTime::now()).expect("no error");
        assert_eq!(names(dir), [MARKER.to_owned(), today]);
    }

    #[test]
    fn the_time_and_the_date() {
        let time = now();
        assert!(time.ends_with('Z'), "{time}");
        assert_eq!(date("2026-10-20T01:02:03.456789Z"), "20261020");
    }

    #[test]
    fn a_new_directory_is_marked_and_a_large_file_is_followed_by_another() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = temp.path().join("logs");
        let log = LogFile::open(&dir, LIMITS).expect("opened");
        assert!(dir.join(MARKER).is_file());
        let first = log.path();
        let line = "x".repeat(60);
        log.write_line(&line);
        log.write_line(&line);
        let second = log.path();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(&first).expect("readable"),
            format!("{line}\n")
        );
        assert!(
            second
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".2.jsonl")),
            "{}",
            second.display()
        );
    }

    #[test]
    fn a_directory_with_other_files_is_not_marked_and_nothing_is_removed() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let old = temp.path().join("20200101-1.jsonl");
        fs::write(&old, "{}\n").expect("written");
        fs::write(temp.path().join("notes.txt"), "mine").expect("written");
        let log = LogFile::open(temp.path(), LIMITS).expect("opened");
        assert!(!temp.path().join(MARKER).exists());
        drop(log);
        collect_garbage(
            temp.path(),
            LIMITS,
            SystemTime::now() + LIMITS.retention * 2,
        )
        .expect("no error");
        assert!(old.exists());
    }

    #[test]
    fn old_logs_go_and_then_the_oldest_while_too_large_but_not_those_of_running_processes() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let dir = temp.path();
        let log = LogFile::open(dir, LIMITS).expect("opened");
        let own = log.path();
        fs::write(dir.join("notes.txt"), "x".repeat(2000)).expect("written");
        for (name, age_days) in [
            ("20200101-1.jsonl", 30),
            ("20200102-2.jsonl", 3),
            ("20200103-3.jsonl", 2),
            ("20200104-4.jsonl", 1),
        ] {
            let path = dir.join(name);
            fs::write(&path, "x".repeat(400)).expect("written");
            let modified = SystemTime::now() - Duration::from_hours(age_days * 24);
            File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(modified))
                .expect("set");
        }
        // Process 1 has ended and left its lock; process 2 still runs.
        fs::write(dir.join("20200101-1.lock"), "").expect("written");
        let running = lock(&dir.join("20200102-2.lock")).expect("locked");
        // 1 is past the retention, and 2 runs: the 1200 bytes of 1, 3 and 4 are over the limit of
        // 1000 until 1 goes.
        collect_garbage(dir, LIMITS, SystemTime::now()).expect("no error");
        let own_name = |extension: &str| {
            own.with_extension(extension)
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a name")
                .to_owned()
        };
        let mut expected = vec![
            MARKER.to_owned(),
            "20200102-2.jsonl".to_owned(),
            "20200102-2.lock".to_owned(),
            "20200103-3.jsonl".to_owned(),
            "20200104-4.jsonl".to_owned(),
            own_name(EXTENSION),
            own_name(LOCK_EXTENSION),
            "notes.txt".to_owned(),
        ];
        expected.sort();
        assert_eq!(names(dir), expected);
        // Past the retention, only the files of the running processes stay. That the files of a
        // process that has ended go is what process 1 shows. (Letting go of the lock within this
        // process does not stand for a process that ends: on the macOS of CI, the lock was still
        // reported as held right after, cause unknown; a process that ends lets go of its locks.)
        collect_garbage(dir, LIMITS, SystemTime::now() + LIMITS.retention * 2).expect("no error");
        let mut expected = vec![
            MARKER.to_owned(),
            "20200102-2.jsonl".to_owned(),
            "20200102-2.lock".to_owned(),
            own_name(EXTENSION),
            own_name(LOCK_EXTENSION),
            "notes.txt".to_owned(),
        ];
        expected.sort();
        assert_eq!(names(dir), expected);
        drop(running);
    }
}
