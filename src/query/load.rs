//! Loading a crate for queries: the archive of its JSON in the cache when there is one, or else
//! the JSON parsed, indexed and archived (requirements R-PF-1, R-PF-2 and R-CA-4).

use std::{
    collections::{HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use parking_lot::{Condvar, Mutex};

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

/// The most archives kept open in the process. Each is a mapping of its file, which the kernel can
/// page out (requirement R-MEM-1), so the limit only bounds the mappings.
const MAX_OPEN: usize = 32;

/// Archives that this process has opened and checked, by path, the latest last (requirements
/// R-PF-1 and §9.1). Checking the bytes of a large archive takes tens of milliseconds (core), so it
/// is done once. An archive's name has the SHA-256 of its JSON, so the file at a path does not
/// change.
static OPEN: LazyLock<Mutex<Opened>> = LazyLock::new(Mutex::default);

#[derive(Default)]
struct Opened {
    by_path: HashMap<PathBuf, Arc<Mapped<Indexed>>>,
    order: VecDeque<PathBuf>,
}

/// Gives the memory that parsing a crate freed back to the system. glibc's allocator keeps most of
/// it otherwise: about 220 MB after core (requirement R-MEM-1, Q-27, measured in M1).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[expect(unsafe_code, reason = "malloc_trim is a C function")]
fn give_back_memory() {
    // SAFETY: malloc_trim only releases free memory of the allocator; it has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
const fn give_back_memory() {}

/// The most crates parsed and archived at once in the process. Parsing takes about 5 to 7 times
/// the size of the JSON in memory (core: about 500 MB), and a search across many dependencies
/// would otherwise parse them all at once (decided before the first release, 2026-10-20).
const MAX_PARSES: usize = 2;

/// The crates being parsed, at most [`MAX_PARSES`].
static PARSING: Mutex<usize> = Mutex::new(0);

/// Signalled when a crate has been parsed.
static PARSED: Condvar = Condvar::new();

/// A place among the crates being parsed, given back when it is dropped.
struct Parsing;

impl Parsing {
    /// Waits for a place. It blocks, so it is taken on a blocking thread.
    fn enter() -> Self {
        let mut parsing = PARSING.lock();
        while *parsing >= MAX_PARSES {
            PARSED.wait(&mut parsing);
        }
        *parsing = parsing.saturating_add(1);
        Self
    }
}

impl Drop for Parsing {
    fn drop(&mut self) {
        let mut parsing = PARSING.lock();
        *parsing = parsing.saturating_sub(1);
        drop(parsing);
        PARSED.notify_one();
    }
}

/// The archive at `path` that this process has opened, if it still exists.
fn opened(path: &Path) -> Option<Arc<Mapped<Indexed>>> {
    let mapped = OPEN.lock().by_path.get(path).map(Arc::clone)?;
    // A cache cleared since is not used, so that the archive is made again where it was.
    path.is_file().then_some(mapped)
}

/// Keeps `mapped`, opened at `path`, and lets go of the oldest beyond [`MAX_OPEN`].
fn keep(path: &Path, mapped: Mapped<Indexed>) -> Arc<Mapped<Indexed>> {
    let mapped = Arc::new(mapped);
    let mut open = OPEN.lock();
    if open
        .by_path
        .insert(path.to_path_buf(), Arc::clone(&mapped))
        .is_none()
    {
        open.order.push_back(path.to_path_buf());
    }
    while open.order.len() > MAX_OPEN {
        if let Some(oldest) = open.order.pop_front() {
            open.by_path.remove(&oldest);
        }
    }
    drop(open);
    mapped
}

/// Where the archive of the JSON in `format` with `sha256` is kept, in its directory `dir`. The
/// name has the SHA-256 of the JSON, so that a new JSON gets a new archive and an archive that a
/// process has mapped is never replaced (which Windows does not allow, requirement R-CA-4).
fn archive_path(dir: &Path, format: u32, sha256: &str) -> PathBuf {
    let short = sha256.get(..16).unwrap_or(sha256);
    dir.join(format!("{format}.{short}.{}.rkyv", archive_version()))
}

/// The header of the archive of the JSON in `format` with `sha256`.
const fn header(format: u32, sha256: Box<str>) -> Header {
    Header {
        schema: ARCHIVE_SCHEMA,
        format_version: format,
        types_version: rustdoc_types::FORMAT_VERSION,
        source_sha256: sha256,
    }
}

/// The part of an archive's name that changes with what it holds: the schema of [`Indexed`] and
/// the version of rustdoc-types, as in `v5t61`. An archive made by another version of the server
/// is left alone (the GC removes it), not replaced in place, which Windows does not allow while
/// another process maps it.
fn archive_version() -> String {
    format!("v{ARCHIVE_SCHEMA}t{}", rustdoc_types::FORMAT_VERSION)
}

/// The archive of the rustdoc JSON in the cache directory `dir`, if it is there and checks out.
/// The JSON itself is not read: its format and SHA-256 are in its file names.
///
/// # Errors
///
/// Returns an error when the cache cannot be read.
pub fn open_archived(cache: &Cache, dir: &Path) -> io::Result<Option<Arc<Mapped<Indexed>>>> {
    let Some((format, sha256)) = cache.docs_rs_source(dir)? else {
        return Ok(None);
    };
    let path = archive_path(dir, format, &sha256);
    if let Some(mapped) = opened(&path) {
        return Ok(Some(mapped));
    }
    Ok(Mapped::open(cache, &path, &header(format, sha256))?.map(|mapped| keep(&path, mapped)))
}

/// Parses `json`, indexes it, and archives it in the cache directory `dir`, then opens the
/// archive. When the archive cannot be written or read back, it is kept in memory for this time,
/// with a log line.
///
/// # Errors
///
/// Returns an error when the JSON cannot be parsed, or the crate cannot be archived at all.
pub fn archive_json(
    cache: &Cache,
    dir: &Path,
    json: &[u8],
) -> Result<Arc<Mapped<Indexed>>, LoadError> {
    let parsing = Parsing::enter();
    let parsed = rustdoc_json::parse(json)?;
    let format = parsed.krate.format_version;
    let sha256: Box<str> = cache::sha256_hex(json).into();
    let indexed = Indexed::new(parsed).with_source(&sha256);
    let path = archive_path(dir, format, &sha256);
    let header = header(format, sha256);
    match archive::write(cache, &path, &header, &indexed)
        .and_then(|()| Mapped::open(cache, &path, &header))
    {
        Ok(Some(mapped)) => {
            drop(indexed);
            give_back_memory();
            drop(parsing);
            return Ok(keep(&path, mapped));
        },
        Ok(None) => {
            tracing::warn!(path = %path.display(), "an archive just written does not check out");
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "the archive could not be written");
        },
    }
    Mapped::in_memory(&indexed)
        .map(Arc::new)
        .map_err(LoadError::Archive)
}

/// The archive of the JSON of the crate `stem` in the cache directory `dir`, made the first time.
/// It must be called on a blocking thread, not in async code.
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
) -> Result<Arc<Mapped<Indexed>>, LoadError> {
    let short = source.get(..16).unwrap_or(source);
    let path = dir.join(format!("{stem}.{short}.{}.rkyv", archive_version()));
    if let Some(mapped) = opened(&path) {
        return Ok(mapped);
    }
    // Another thread that archives the same JSON goes first, and this one then opens its archive
    // (requirement R-WS-6).
    let _held = cache.lock_blocking(&path);
    // The format is not compared when an archive is opened.
    match Mapped::open(cache, &path, &header(0, source.into())) {
        Ok(Some(mapped)) => return Ok(keep(&path, mapped)),
        Ok(None) => {},
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "an archive could not be read");
        },
    }
    let parsing = Parsing::enter();
    let json = json().map_err(LoadError::Read)?;
    let parsed = rustdoc_json::parse(&json)?;
    drop(json);
    let header = header(parsed.krate.format_version, source.into());
    let indexed = Indexed::new(parsed).with_source(source);
    match archive::write(cache, &path, &header, &indexed)
        .and_then(|()| Mapped::open(cache, &path, &header))
    {
        Ok(Some(mapped)) => {
            drop(indexed);
            give_back_memory();
            drop(parsing);
            return Ok(keep(&path, mapped));
        },
        Ok(None) => {
            tracing::warn!(path = %path.display(), "an archive just written does not check out");
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "the archive could not be written");
        },
    }
    Mapped::in_memory(&indexed)
        .map(Arc::new)
        .map_err(LoadError::Archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_two_crates_are_parsed_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let threads: Vec<_> = (0..6)
            .map(|_| {
                let (now, most) = (Arc::clone(&now), Arc::clone(&most));
                std::thread::spawn(move || {
                    let place = Parsing::enter();
                    let inside = now.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(inside, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    now.fetch_sub(1, Ordering::SeqCst);
                    drop(place);
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("no panic");
        }
        let most = most.load(Ordering::SeqCst);
        assert!((1..=MAX_PARSES).contains(&most), "{most}");
    }
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

    /// The environment variable that tells [`archive_for_another_test`] where the cache is.
    const ROOT: &str = "CHATORA_RUSTDOC_TEST_CACHE_ROOT";

    /// Not a test by itself: a process that [`two_processes_archive_a_crate_once`] starts, which
    /// archives the JSON slowly and prints `parsed` when it reads it.
    #[test]
    #[ignore = "started by two_processes_archive_a_crate_once"]
    fn archive_for_another_test() {
        let Some(root) = std::env::var_os(ROOT) else {
            return;
        };
        let root = Path::new(&root);
        let mapped = archive_of(&Cache::at(root), root, "std", &"cd".repeat(32), || {
            println!("parsed");
            // Slowly, so that the other process comes while this one archives.
            std::thread::sleep(std::time::Duration::from_millis(500));
            Ok(Box::from(JSON))
        })
        .expect("archived");
        assert_eq!(CrateView::new(mapped.get()).crate_version(), Some("1.0.0"));
    }

    /// Two processes that archive the same JSON at once take turns, and the second opens the
    /// archive of the first: the JSON is read once, and neither fails, on Windows too, where a
    /// file that another process maps cannot be replaced (requirement R-CA-4).
    #[test]
    fn two_processes_archive_a_crate_once() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let env = [(ROOT, dir.path().as_os_str())];
        let name = "query::load::tests::archive_for_another_test";
        let both = [
            crate::test_process::start(name, &env),
            crate::test_process::start(name, &env),
        ];
        let parsed: usize = both
            .into_iter()
            .map(|child| {
                crate::test_process::finish(child)
                    .iter()
                    .filter(|line| line.ends_with("parsed"))
                    .count()
            })
            .sum();
        assert_eq!(parsed, 1);
    }
}
