//! When what is in the cache was last used, for its garbage collection.
//!
//! The collection removes the least recently used (requirement R-CA-5, decided in M2f's F1, 3): a
//! file `used` in its place tells it, written when it is read, at most once in [`NOTE_EVERY`] for
//! each place in a process, so that reads stay cheap.

use std::{
    fs,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;

/// The file whose time tells when its place was last used.
pub const USED: &str = "used";

/// How often the use of a place is noted in a process.
pub const NOTE_EVERY: Duration = Duration::from_hours(1);

/// The places noted as used, and when, in this process.
static NOTED: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

/// Whether the use of `dir` is to be noted now: not when it was in the last [`NOTE_EVERY`] in this
/// process. It counts as noted from now.
pub fn due(dir: &Path) -> bool {
    let mut noted = NOTED.lock();
    if noted
        .iter()
        .any(|(noted, at)| noted == dir && at.elapsed() < NOTE_EVERY)
    {
        return false;
    }
    noted.retain(|(noted, at)| noted != dir && at.elapsed() < NOTE_EVERY);
    noted.push((dir.to_path_buf(), Instant::now()));
    true
}

/// Notes that `dir` is used now, if it is due ([`due`]). A place that is not there, as when it was
/// removed meanwhile, is not made again. A note that cannot be written is logged.
///
/// It may write a file, so it is for a blocking thread.
pub fn note(dir: &Path) {
    if due(dir) {
        write(dir);
    }
}

/// [`note`] from async code: written on a blocking thread, which the caller does not wait for.
pub fn note_later(dir: &Path) {
    if due(dir) {
        let dir = dir.to_path_buf();
        drop(crate::task::spawn_blocking(move || write(&dir)));
    }
}

/// Writes the file [`USED`] in `dir`, if it is there.
///
/// It writes a file, so it is for a blocking thread.
pub fn write(dir: &Path) {
    match fs::write(dir.join(USED), b"") {
        Ok(()) => {},
        // Removed meanwhile, which is not made again.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => {
            tracing::warn!(path = %dir.display(), %error, "the use of a place of the cache could not be noted");
        },
    }
}

/// When `dir` was last used: the latest time of itself and of what is directly in it, its file
/// [`USED`] among them. `None` when it cannot be read.
///
/// What was written again in it was used too, as a record made again over an old [`USED`].
///
/// It reads the file system, so it is for a blocking thread.
#[must_use]
pub fn last_used(dir: &Path) -> Option<SystemTime> {
    let own = fs::symlink_metadata(dir)
        .and_then(|metadata| metadata.modified())
        .ok()?;
    let latest = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.metadata().ok()?.modified().ok())
        .max();
    Some(latest.map_or(own, |latest| latest.max(own)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_use_is_noted_once_in_a_while() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let place = dir.path().join("place");
        fs::create_dir_all(&place).expect("made");
        note(&place);
        assert!(place.join(USED).is_file());
        fs::remove_file(place.join(USED)).expect("removed");
        // Not again so soon.
        note(&place);
        assert!(!place.join(USED).exists());
    }

    #[test]
    fn a_place_that_is_gone_is_not_made_again() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let gone = dir.path().join("gone");
        note(&gone);
        assert!(!gone.exists());
    }

    #[test]
    fn without_the_file_the_latest_time_in_the_place_tells_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let place = dir.path().join("place");
        fs::create_dir_all(&place).expect("made");
        fs::write(place.join("a.json"), b"{}").expect("written");
        let found = last_used(&place).expect("a time");
        assert!(found <= SystemTime::now());
        write(&place);
        let noted = last_used(&place).expect("a time");
        assert!(noted >= found);
        assert!(last_used(&dir.path().join("nothing")).is_none());
    }

    /// A record written again in a place whose use was noted long ago tells that it was used then
    /// (M2f's F1b).
    #[test]
    fn what_was_written_after_the_note_tells_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let place = dir.path().join("place");
        fs::create_dir_all(&place).expect("made");
        write(&place);
        let long_ago = SystemTime::now()
            .checked_sub(Duration::from_hours(24 * 40))
            .expect("a time");
        fs::File::options()
            .write(true)
            .open(place.join(USED))
            .and_then(|file| file.set_modified(long_ago))
            .expect("set");
        fs::write(place.join("record.json"), b"{}").expect("written");
        let found = last_used(&place).expect("a time");
        assert!(found > long_ago, "{found:?}");
    }
}
