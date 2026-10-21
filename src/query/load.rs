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
    dist::{Dist, DistError, StdCrate},
    docs_rs::DocsRs,
    flight::{Doing, Flights, Joined, Stage},
    rustdoc_json::{self, ParseError},
    std_json::{self, Chosen},
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

/// The JSON of std that was chosen for the calls that wanted it at once, or why there is none.
pub type SharedStd = Result<Arc<Chosen>, Arc<DistError>>;

/// What std's JSON is chosen by, besides the cache and the distribution server.
#[derive(Debug, Clone)]
pub struct StdWanted {
    /// The target asked for.
    pub target: TargetTriple,
    /// The host's target, which stands in when the nightly has none for `target`.
    pub host: TargetTriple,
    /// The release of the project's toolchain, which the nightly may not be older than.
    pub at_least: Option<Version>,
    /// Where rustup runs, out of any project.
    pub neutral_dir: PathBuf,
}

/// The JSON of std that [`std_json::choose`] chooses for `wanted`, made ready by work that the
/// calls for it share (requirements R-STD-6, R-NW-2 and R-WS-6).
///
/// The work fetches and unpacks the JSON when it comes from the distribution server, and makes the
/// archives of std, alloc and core.
///
/// A call waits for the work until `until`. The work then goes on, so that a later call finds it
/// done, and [`Joined::Going`] says what it is doing (requirements R-ARCH-6 and R-OUT-9). An
/// archive that cannot be made is left to the lookup, which tells why.
pub async fn std_json(
    flights: &Flights<SharedStd>,
    cache: &Cache,
    dist: &Dist,
    wanted: StdWanted,
    until: Instant,
) -> Joined<SharedStd> {
    let key = format!(
        "std:{}:{}",
        wanted.target,
        wanted
            .at_least
            .as_ref()
            .map_or_else(String::new, ToString::to_string)
    );
    let what = format!("std for {}", wanted.target);
    let (cache, dist) = (cache.clone(), dist.clone());
    flights
        .join(&key, &what, until, async move |stage| {
            let chosen = std_json::choose(
                &cache,
                &dist,
                &wanted.neutral_dir,
                &wanted.target,
                &wanted.host,
                wanted.at_least.as_ref(),
            )
            .await
            .map_err(Arc::new)?;
            stage.set(Doing::Indexing);
            let chosen = Arc::new(chosen);
            let archiving = Arc::clone(&chosen);
            archive_std(&cache, &archiving).await;
            Ok(chosen)
        })
        .await
}

/// Makes the archives of std, alloc and core from `chosen`, which a path of std mostly goes
/// through (requirement R-PR-2). A failure is logged; the lookup that needs the crate tells it,
/// and makes what is missing when the server ended meanwhile.
async fn archive_std(cache: &Cache, chosen: &Arc<Chosen>) {
    for krate in [StdCrate::Std, StdCrate::Alloc, StdCrate::Core] {
        match archive_std_crate(cache, chosen, krate).await {
            Some(Ok(_)) => {},
            Some(Err(error)) => {
                tracing::warn!(krate = krate.as_str(), %error, "the JSON of std could not be archived");
            },
            None => return,
        }
    }
}

/// The archive of `krate` of std from `chosen`, made the first time ([`archive_from`]).
pub async fn archive_std_crate(
    cache: &Cache,
    chosen: &Arc<Chosen>,
    krate: StdCrate,
) -> Option<Result<Arc<Mapped<Indexed>>, LoadError>> {
    let (dir, key) = match chosen.json.archive_key(cache, krate) {
        Ok(found) => found,
        Err(error) => return Some(Err(LoadError::Read(error))),
    };
    let (reading, json) = (cache.clone(), Arc::clone(chosen));
    archive_from(cache, &dir, krate.as_str(), &key, move || {
        json.json.read(&reading, krate)
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
        let parsing = Parsing::enter_as_task().await;
        let ((made, json), held) = holding(held, move || {
            let made = archive_json(&cache, &dir, &json, parsing);
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

/// Signalled when a crate has been parsed, to the blocking threads that wait for a place.
static PARSED: Condvar = Condvar::new();

/// Signalled when a crate has been parsed, to the tasks that wait for a place.
static PARSED_TO_TASKS: tokio::sync::Notify = tokio::sync::Notify::const_new();

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

    /// Waits for a place without holding a thread, as a task.
    async fn enter_as_task() -> Self {
        loop {
            // Told before the place is tried, so that a place given back meanwhile is not missed.
            let parsed = PARSED_TO_TASKS.notified();
            let mut parsed = std::pin::pin!(parsed);
            parsed.as_mut().enable();
            if let Some(place) = Self::try_enter() {
                return place;
            }
            parsed.await;
        }
    }

    /// A place, if one is free.
    fn try_enter() -> Option<Self> {
        let mut parsing = PARSING.lock();
        (*parsing < MAX_PARSES).then(|| {
            *parsing = parsing.saturating_add(1);
            Self
        })
    }
}

impl Drop for Parsing {
    fn drop(&mut self) {
        let mut parsing = PARSING.lock();
        *parsing = parsing.saturating_sub(1);
        drop(parsing);
        PARSED.notify_one();
        PARSED_TO_TASKS.notify_waiters();
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
/// archive, in the place among the crates being parsed `parsing`. When the archive cannot be
/// written or read back, it is kept in memory for this time, with a log line.
///
/// # Errors
///
/// Returns an error when the JSON cannot be parsed, or the crate cannot be archived at all.
fn archive_json(
    cache: &Cache,
    dir: &Path,
    json: &[u8],
    parsing: Parsing,
) -> Result<Arc<Mapped<Indexed>>, LoadError> {
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
///
/// It must be called on a blocking thread, not in async code: [`archive_from`] is for async code,
/// which waits for the lock and the place among the crates being parsed without holding a thread.
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
    let path = archive_path_of(dir, stem, source);
    if let Some(mapped) = opened(&path) {
        return Ok(mapped);
    }
    // Another thread that archives the same JSON goes first, and this one then opens its archive
    // (requirement R-WS-6).
    let _held = cache.lock_blocking(&path);
    if let Some(mapped) = archived_at(cache, &path, source) {
        return Ok(mapped);
    }
    make_at(cache, &path, source, json, Parsing::enter())
}

/// The archive of the JSON of the crate `stem` in `dir`, keyed by `source`, if it was made: opened,
/// and not made (decided in M2's D4).
pub async fn archived(
    cache: &Cache,
    dir: &Path,
    stem: &str,
    source: &str,
) -> Option<Arc<Mapped<Indexed>>> {
    let path = archive_path_of(dir, stem, source);
    if let Some(mapped) = opened(&path) {
        return Some(mapped);
    }
    let (cache, source) = (cache.clone(), Box::<str>::from(source));
    crate::task::spawn_blocking(move || archived_at(&cache, &path, &source))
        .await
        .ok()
        .flatten()
}

/// [`archive_of`] for async code: it waits for the lock and for a place among the crates being
/// parsed as a task, and parses on a blocking thread. `None` when the work was stopped, as when
/// the server ends.
pub async fn archive_from(
    cache: &Cache,
    dir: &Path,
    stem: &str,
    source: &str,
    json: impl FnOnce() -> io::Result<Box<[u8]>> + Send + 'static,
) -> Option<Result<Arc<Mapped<Indexed>>, LoadError>> {
    let path = archive_path_of(dir, stem, source);
    if let Some(mapped) = opened(&path) {
        return Some(Ok(mapped));
    }
    // As in `archive_of`, another task or process that archives the same JSON goes first.
    let (looking, source) = ((cache.clone(), path.clone()), Box::<str>::from(source));
    let (found, held) = blocking_holding(cache.lock(&path).await, {
        let source = source.clone();
        move || archived_at(&looking.0, &looking.1, &source)
    })
    .await?;
    if let Some(mapped) = found {
        drop(held);
        return Some(Ok(mapped));
    }
    let parsing = Parsing::enter_as_task().await;
    let cache = cache.clone();
    let (made, _held) =
        blocking_holding(held, move || make_at(&cache, &path, &source, json, parsing)).await?;
    Some(made)
}

/// Runs `work` on a blocking thread, holding `held` until it ends, even when the task that waits
/// for it is dropped. `None` when it was stopped.
async fn blocking_holding<R: Send + 'static>(
    held: PathGuard,
    work: impl FnOnce() -> R + Send + 'static,
) -> Option<(R, PathGuard)> {
    match crate::task::spawn_blocking(move || (work(), held)).await {
        Ok(done) => Some(done),
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::debug!(%error, "the work that makes an archive was stopped");
            None
        },
    }
}

/// Where the archive of the JSON of the crate `stem`, keyed by `source`, is kept in `dir`.
fn archive_path_of(dir: &Path, stem: &str, source: &str) -> PathBuf {
    let short = source.get(..16).unwrap_or(source);
    dir.join(format!("{stem}.{short}.{}.rkyv", archive_version()))
}

/// The archive at `path`, keyed by `source`, if another made it: under its lock.
fn archived_at(cache: &Cache, path: &Path, source: &str) -> Option<Arc<Mapped<Indexed>>> {
    // The format is not compared when an archive is opened.
    match Mapped::open(cache, path, &header(0, source.into())) {
        Ok(Some(mapped)) => Some(keep(path, mapped)),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "an archive could not be read");
            None
        },
    }
}

/// Reads the JSON with `json`, parses it, indexes it and archives it at `path`, keyed by
/// `source`, in the place `parsing`, under the lock of `path`. When the archive cannot be written
/// or read back, it is kept in memory for this time, with a log line.
fn make_at(
    cache: &Cache,
    path: &Path,
    source: &str,
    json: impl FnOnce() -> io::Result<Box<[u8]>>,
    parsing: Parsing,
) -> Result<Arc<Mapped<Indexed>>, LoadError> {
    let json = json().map_err(LoadError::Read)?;
    let parsed = rustdoc_json::parse(&json)?;
    drop(json);
    let header = header(parsed.krate.format_version, source.into());
    let indexed = Indexed::new(parsed).with_source(source);
    match archive::write(cache, path, &header, &indexed)
        .and_then(|()| Mapped::open(cache, path, &header))
    {
        Ok(Some(mapped)) => {
            drop(indexed);
            give_back_memory();
            drop(parsing);
            return Ok(keep(path, mapped));
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
        let made = archive_json(&cache, &place, JSON, Parsing::enter()).expect("archived");
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
            archive_json(
                &cache,
                dir.path(),
                br#"{"format_version":61}"#,
                Parsing::enter()
            ),
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

    /// std's JSON is chosen, fetched from the distribution server and archived by work that goes
    /// on past the deadline of the call that started it, for a later call (requirements R-STD-6,
    /// R-ARCH-6 and R-WS-6).
    #[tokio::test]
    async fn std_is_made_ready_by_work_that_goes_on_past_the_deadline() {
        use crate::data::{
            dist::tests::{HASH, archive, linux, setup_after},
            std_json::StdJson,
        };
        let json = |krate: &str| format!("rust-docs-json-preview/share/doc/rust/json/{krate}.json");
        let (std, core) = (json("std"), json("core"));
        let archive = archive(&[
            ("git-commit-hash", HASH.as_bytes()),
            (&std, JSON),
            (&core, JSON),
        ]);
        let (cache, dist, requests, dir) =
            setup_after(&archive, None, std::time::Duration::from_millis(300));
        let flights = Flights::default();
        let wanted = StdWanted {
            target: linux(),
            host: linux(),
            // No nightly here is as new, so the distribution server gives it.
            at_least: Some(Version::new(999, 0, 0)),
            neutral_dir: dir.path().to_owned(),
        };
        let soon = Instant::now()
            .checked_add(std::time::Duration::from_millis(100))
            .expect("in range");
        let first = std_json(&flights, &cache, &dist, wanted.clone(), soon).await;
        assert!(matches!(first, Joined::Going(Doing::Fetching)), "{first:?}");
        let Joined::Done(Ok(chosen)) = std_json(&flights, &cache, &dist, wanted, later()).await
        else {
            panic!("made ready");
        };
        assert!(matches!(chosen.json, StdJson::Dist(_)), "{chosen:?}");
        // Archived by the work: opened without its JSON.
        let (place, key) = chosen
            .json
            .archive_key(&cache, StdCrate::Std)
            .expect("its key");
        tokio::task::spawn_blocking(move || {
            archive_of(&cache, &place, "std", &key, || {
                panic!("std was not archived by the work")
            })
        })
        .await
        .expect("no panic")
        .expect("archived");
        assert_eq!(
            requests.try_iter().count(),
            2,
            "the manifest and the archive"
        );
    }
}
