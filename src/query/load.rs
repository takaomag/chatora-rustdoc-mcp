//! Loading a crate for queries: the archive of its JSON in the cache when there is one, or else
//! the JSON parsed, indexed and archived (requirements R-PF-1, R-PF-2 and R-CA-4).

use std::{
    collections::{HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use parking_lot::{Condvar, Mutex};
use semver::Version;
use tokio::time::Instant;

use super::across::{ARCHIVE_SCHEMA, Indexed};
use crate::data::{
    self,
    archive::{self, Header, Mapped},
    cache::{self, Cache, PathGuard},
    crate_name::CrateName,
    docs_rs::DocsRs,
    flight::{Doing, Flights, Joined, Stage},
    rustdoc_json::{self, ParseError},
    target::TargetTriple,
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

/// The archive of a release's rustdoc JSON from docs.rs, and the warnings about how the JSON was
/// loaded, such as a cached JSON that was damaged and fetched again (requirement R-CA-4).
#[derive(Debug)]
pub struct Archived {
    /// The archive.
    pub mapped: Arc<Mapped<Indexed>>,
    /// What the response warns about.
    pub warnings: Vec<Box<str>>,
}

/// Why the archive of a release's rustdoc JSON from docs.rs could not be made.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The JSON could not be loaded.
    #[error(transparent)]
    Load(#[from] data::LoadError),
    /// The archive could not be made from the JSON.
    #[error(transparent)]
    Make(#[from] LoadError),
    /// The work was stopped, as the server is shutting down.
    #[error("the work that makes the archive was stopped")]
    Stopped,
}

/// The archive of a release for the calls that wanted it at once, or why it could not be made.
pub type SharedArchive = Result<Arc<Archived>, Arc<ArchiveError>>;

/// The archive of `name` at `version` for `target`, made from its rustdoc JSON on docs.rs by work
/// that the calls for the release share (requirements R-NW-2 and R-WS-6).
///
/// A call waits for the work until `until`. The work then goes on, so that a later call finds the
/// archive, and [`Joined::Going`] says what it is doing (requirements R-ARCH-6 and R-OUT-9).
///
/// The work holds the lock of the release's directory in the cache, so that other calls and
/// processes open the archive instead of making it again (requirement R-CA-4). When every call
/// stops waiting for it before any has been told that it goes on, the work stops; the lock is
/// then let go at once, but not while the JSON is parsed and archived, which cannot be stopped.
pub async fn docs_rs_archive(
    flights: &Flights<SharedArchive>,
    cache: &Cache,
    docs_rs: &DocsRs,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
    until: Instant,
) -> Joined<SharedArchive> {
    let dir = cache.docs_rs_dir(name, version, target);
    let key = dir.to_string_lossy().into_owned();
    let release = Release {
        cache: cache.clone(),
        docs_rs: docs_rs.clone(),
        name: name.clone(),
        version: version.clone(),
        target: target.cloned(),
        dir,
    };
    let what = format!("{name} {version}");
    flights
        .join(&key, &what, until, async move |stage| {
            release
                .archive(&stage)
                .await
                .map(Arc::new)
                .map_err(Arc::new)
        })
        .await
}

/// A release on docs.rs, whose archive is made in its directory of the cache.
struct Release {
    cache: Cache,
    docs_rs: DocsRs,
    name: CrateName,
    version: Version,
    target: Option<TargetTriple>,
    dir: PathBuf,
}

impl Release {
    /// The archive, made from the JSON unless another call or process has made it, and the
    /// JSON fetched again, or in another format, when it cannot be parsed (requirements R-CA-4
    /// and R-DS-2). The work tells `stage` what it does.
    async fn archive(&self, stage: &Stage) -> Result<Archived, ArchiveError> {
        let (cache, dir) = (self.cache.clone(), self.dir.clone());
        let (opened, held) = holding(self.cache.lock(&self.dir).await, move || {
            open_archived(&cache, &dir)
        })
        .await?;
        match opened {
            Ok(Some(mapped)) => {
                return Ok(Archived {
                    mapped,
                    warnings: Vec::new(),
                });
            },
            Ok(None) => {},
            Err(error) => tracing::warn!(%error, "the archive of the crate could not be read"),
        }
        let target = self.target.as_ref();
        let loaded = data::docs_rs_json(
            &self.cache,
            &self.docs_rs,
            &self.name,
            &self.version,
            target,
        )
        .await?;
        let mut warnings = loaded.warnings;
        stage.set(Doing::Indexing);
        let (made, json, held) = self.make(loaded.json, held).await?;
        let made = match &made {
            // A format this server does not read: docs.rs may have the release in another one
            // (requirement R-DS-2).
            Err(LoadError::Parse(ParseError::UnsupportedFormat { found, .. })) => {
                stage.set(Doing::Fetching);
                match data::docs_rs_json_in_another_format(
                    &self.cache,
                    &self.docs_rs,
                    &self.name,
                    &self.version,
                    target,
                    *found,
                )
                .await?
                {
                    Some(json) => {
                        stage.set(Doing::Indexing);
                        self.make(json, held).await?.0
                    },
                    None => made,
                }
            },
            // Its header was read when it was fetched, yet it does not parse: docs.rs may have
            // served it broken, so it is fetched again, once (requirement R-CA-4).
            Err(LoadError::Parse(
                ParseError::Json { .. } | ParseError::FormatMismatch { .. } | ParseError::TooDeep,
            )) => {
                stage.set(Doing::Fetching);
                let again = data::refetch_unparsable(
                    &self.cache,
                    &self.docs_rs,
                    &self.name,
                    &self.version,
                    target,
                    &json,
                )
                .await?;
                warnings.extend(again.warnings);
                stage.set(Doing::Indexing);
                let (made, json, _held) = self.make(again.json, held).await?;
                if let Err(LoadError::Parse(_)) = &made
                    && let Err(error) = data::remember_unparsable(
                        &self.cache,
                        &self.name,
                        &self.version,
                        target,
                        &json,
                    )
                    .await
                {
                    tracing::warn!(%error, "failed to remember a JSON that cannot be parsed");
                }
                made
            },
            Ok(_) | Err(_) => {
                drop(held);
                made
            },
        };
        Ok(Archived {
            mapped: made?,
            warnings,
        })
    }

    /// Parses `json`, indexes it and archives it, holding the lock `held` until it is done. The
    /// JSON and the lock come back with the result.
    async fn make(
        &self,
        json: Box<[u8]>,
        held: PathGuard,
    ) -> Result<
        (
            Result<Arc<Mapped<Indexed>>, LoadError>,
            Box<[u8]>,
            PathGuard,
        ),
        ArchiveError,
    > {
        let (cache, dir) = (self.cache.clone(), self.dir.clone());
        let ((made, json), held) = holding(held, move || {
            let made = archive_json(&cache, &dir, &json);
            (made, json)
        })
        .await?;
        Ok((made, json, held))
    }
}

/// Runs `work` on a blocking thread, which lets the lock `held` go when the work ends, even when
/// the task that waits for it is aborted: the work cannot be stopped, and another call must not do
/// it again meanwhile (requirement R-WS-6). The lock comes back with the result.
async fn holding<R: Send + 'static>(
    held: PathGuard,
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<(R, PathGuard), ArchiveError> {
    crate::task::spawn_blocking(move || (work(), held))
        .await
        .map_err(|error| {
            let error = crate::task::resume_if_panic(error);
            tracing::debug!(%error, "the work that makes an archive was stopped");
            ArchiveError::Stopped
        })
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

    /// A release of a crate called `c`, which [`JSON`] documents.
    fn c() -> (CrateName, Version) {
        ("c".parse().expect("valid"), Version::new(1, 0, 0))
    }

    /// A deadline that a test does not reach.
    fn later() -> Instant {
        Instant::now()
            .checked_add(std::time::Duration::from_secs(60))
            .expect("in range")
    }

    /// The call that started the work is called off while another waits for it: the work goes on
    /// for the other, and docs.rs is asked once (requirements R-NW-2 and R-WS-6).
    #[tokio::test]
    async fn the_work_goes_on_while_another_call_waits_for_it() {
        let (base_url, requests) =
            data::test_http::serve_slowly(JSON, std::time::Duration::from_millis(300));
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (cache, docs_rs) = (Cache::at(dir.path()), DocsRs::for_local_test(&base_url));
        let flights = Arc::new(Flights::default());
        let call = || {
            let (flights, cache, docs_rs) = (Arc::clone(&flights), cache.clone(), docs_rs.clone());
            tokio::spawn(async move {
                let (name, version) = c();
                docs_rs_archive(&flights, &cache, &docs_rs, &name, &version, None, later()).await
            })
        };
        let first = call();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let second = call();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        first.abort();
        let Joined::Done(Ok(archived)) = second.await.expect("joined") else {
            panic!("archived");
        };
        assert_eq!(
            CrateView::new(archived.mapped.get()).crate_version(),
            Some("1.0.0")
        );
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A call that stops waiting at its deadline is told that the crate is being fetched, and the
    /// work goes on, so that a later call has the archive, which docs.rs was asked for once
    /// (requirements R-ARCH-6 and R-OUT-9).
    #[tokio::test]
    async fn the_work_goes_on_past_the_deadline() {
        let (base_url, requests) =
            data::test_http::serve_slowly(JSON, std::time::Duration::from_millis(300));
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (cache, docs_rs) = (Cache::at(dir.path()), DocsRs::for_local_test(&base_url));
        let flights = Flights::default();
        let (name, version) = c();
        let soon = Instant::now() + std::time::Duration::from_millis(50);
        let first = docs_rs_archive(&flights, &cache, &docs_rs, &name, &version, None, soon).await;
        assert!(matches!(first, Joined::Going(Doing::Fetching)), "{first:?}");
        // Archived by the work that went on, which no call waits for.
        let place = cache.docs_rs_dir(&name, &version, None);
        let deadline = later();
        while open_archived(&cache, &place).expect("no error").is_none() {
            assert!(Instant::now() < deadline, "not archived");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let later =
            docs_rs_archive(&flights, &cache, &docs_rs, &name, &version, None, later()).await;
        assert!(matches!(later, Joined::Done(Ok(_))), "{later:?}");
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// The lock is let go when the blocking work ends, not when the task that waits for it is
    /// aborted, so that another call does not make the archive again meanwhile (requirement
    /// R-WS-6).
    #[tokio::test]
    async fn the_lock_is_held_until_the_blocking_work_ends() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let place = dir.path().join("docs.rs/c/1.0.0/_default");
        let waiting = tokio::spawn(holding(cache.lock(&place).await, || {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        waiting.abort();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            cache.try_lock(&place).is_none(),
            "held by the blocking work"
        );
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert!(cache.try_lock(&place).is_some(), "let go when it ended");
    }
}
