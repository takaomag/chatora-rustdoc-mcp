//! The rustdoc JSON of std from the Rust distribution server, without a nightly toolchain
//! (requirement R-STD-6).
//!
//! The nightly channel publishes `rust-docs-json`: the JSON of std, core, alloc, `proc_macro` and
//! test, as one xz archive per target. The manifest of the channel names the archive and its
//! SHA-256. The archive is fetched, checked against the SHA-256, and unpacked into the cache, once
//! for each nightly.

use std::{
    collections::BTreeMap,
    io::{self, Read as _},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::{
    blocking,
    cache::{self, Cache, Verified},
    docs_rs::MAX_BYTES,
    http::{self, BodyError, Network},
    system_time,
    target::TargetTriple,
    unix_seconds,
};
use crate::config::Settings;

/// Where the Rust distribution server is. Kept in one place so that a mirror can replace it later
/// (requirement R-CA-8).
pub const DEFAULT_BASE_URL: &str = "https://static.rust-lang.org";

/// How long the manifest of the nightly channel is used before it is fetched again (requirement
/// R-STD-6). A new nightly comes out once a day.
const MANIFEST_TTL: Duration = Duration::from_hours(1);

/// Largest manifest held in memory. The nightly one is about 1 MB.
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// Largest archive held in memory. The one for Linux on x86-64 is about 4 MB.
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// `limits.extract_max_size` unless set: the most bytes unpacked from one archive (requirement
/// R-NW-3, initial value). The JSON for Linux on x86-64 comes to about 125 MB, 96 MB of it core.
const EXTRACT_MAX_SIZE: u64 = 4 * 1024 * 1024 * 1024;

/// The package of the manifest that holds the archives.
const PACKAGE: &str = "rust-docs-json-preview";

/// The name of the file, written last, that marks an unpacked archive as complete (requirement
/// R-CA-4).
pub(super) const COMPLETE_FILE: &str = "complete.json";

/// The crates whose JSON the archive has (requirement R-STD-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StdCrate {
    /// `std`.
    Std,
    /// `core`.
    Core,
    /// `alloc`.
    Alloc,
    /// `proc_macro`.
    ProcMacro,
    /// `test`.
    Test,
}

impl StdCrate {
    /// Every one of them.
    pub const ALL: [Self; 5] = [
        Self::Std,
        Self::Core,
        Self::Alloc,
        Self::ProcMacro,
        Self::Test,
    ];

    /// The crate name, which is also the name of its JSON file without `.json`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Std => "std",
            Self::Core => "core",
            Self::Alloc => "alloc",
            Self::ProcMacro => "proc_macro",
            Self::Test => "test",
        }
    }

    /// The crate named `name`, if it is one of them.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|krate| krate.as_str() == name)
    }
}

/// A client for the Rust distribution server.
#[derive(Debug, Clone)]
pub struct Dist {
    client: reqwest::Client,
    // `Arc` so that cloning the client does not copy the URL.
    base_url: Arc<str>,
    settings: DistSettings,
    /// Whether no request may be sent (`network.offline`, requirement R-CA-7).
    offline: bool,
}

/// What the settings say about the JSON of std from the distribution server: `std.fetch_from_dist`
/// and `limits.extract_max_size` (requirements R-STD-1, R-NW-3 and R-DIST-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistSettings {
    /// Whether the JSON may be fetched from the server. Without it, only a nightly toolchain
    /// installed here gives the JSON of std.
    pub fetch: bool,
    /// The most bytes unpacked from one archive.
    pub extract_max_size: u64,
}

impl Default for DistSettings {
    fn default() -> Self {
        Self {
            fetch: true,
            extract_max_size: EXTRACT_MAX_SIZE,
        }
    }
}

impl DistSettings {
    /// The settings that `settings` give, or the defaults.
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        let default = Self::default();
        Self {
            fetch: settings
                .get("std.fetch_from_dist", |value| {
                    value.boolean().ok_or("true or false")
                })
                .unwrap_or(default.fetch),
            extract_max_size: settings
                .get("limits.extract_max_size", |value| {
                    value.size().ok_or("a size such as \"4GiB\"")
                })
                .unwrap_or(default.extract_max_size),
        }
    }
}

/// Why the JSON of std could not be had from the distribution server.
#[derive(Debug, thiserror::Error)]
pub enum DistError {
    /// The request or the transfer failed, including timeouts.
    #[error("the request to the Rust distribution server failed")]
    Network(#[source] reqwest::Error),
    /// The TLS certificate of the server was rejected.
    #[error("the TLS certificate of the Rust distribution server could not be verified")]
    Certificate(#[source] reqwest::Error),
    /// The server answered with a status other than 200.
    #[error("the Rust distribution server answered {status}")]
    Status {
        /// The HTTP status.
        status: StatusCode,
    },
    /// A body is larger than the limit.
    #[error("a body from the Rust distribution server is larger than {limit} bytes")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// The manifest is not in the form expected.
    #[error("the manifest of the nightly channel could not be read")]
    Manifest(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The nightly channel has no `rust-docs-json` for the target.
    #[error("the nightly channel publishes no rust-docs-json for {target}")]
    NotAvailable {
        /// The target asked for.
        target: TargetTriple,
    },
    /// Fetching the same manifest or archive failed transiently a moment ago (requirement
    /// R-ERR-3).
    #[error("fetching this from the Rust distribution server failed a moment ago; it is tried again in {} seconds", retry_after.as_secs())]
    RecentFailure {
        /// How long until it is fetched again.
        retry_after: Duration,
    },
    /// The archive does not have the SHA-256 that the manifest gives.
    #[error("the archive does not match its SHA-256 in the manifest")]
    ChecksumMismatch,
    /// The archive could not be unpacked, or lacks the JSON of std or core.
    #[error("the archive of rust-docs-json could not be unpacked")]
    Archive(#[source] io::Error),
    /// The cache could not be read or written.
    #[error("the cache of the JSON of std could not be used")]
    Cache(#[source] io::Error),
    /// The JSON may not be fetched from the server (`std.fetch_from_dist`), and the nightly
    /// toolchain here does not give it.
    #[error("std.fetch_from_dist is false, and the nightly toolchain here cannot be used: {why}")]
    Disabled {
        /// Why the nightly here cannot be used.
        why: Box<str>,
    },
    /// No request may be sent (`network.offline`), and the cache does not have it (requirement
    /// R-CA-7).
    #[error("the server is offline (network.offline), and the cache does not have it")]
    Offline,
}

/// The JSON of std of one nightly, unpacked in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdDocs {
    /// The directory that holds `<crate>.json` for each [`StdCrate`].
    pub dir: PathBuf,
    /// What it was built from, for the provenance (requirement R-STD-1).
    pub nightly: Nightly,
    /// What the response warns about, such as a manifest that could not be fetched again.
    pub warnings: Vec<Box<str>>,
}

/// One nightly of `rust-docs-json` for one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nightly {
    /// The date of the nightly channel, as `2026-10-19`.
    pub date: Box<str>,
    /// The version of rustc, as `1.101.0-nightly (b6eeb93c9 2026-10-18)`.
    pub version: Box<str>,
    /// The commit of rustc, in full.
    pub commit_hash: Box<str>,
    /// The target the JSON was built for, which the manifest gives some targets in place of their
    /// own (`x86_64-apple-darwin` gets `aarch64-apple-darwin`, requirement R-STD-6).
    pub triple: TargetTriple,
}

/// The date of a nightly, as `2026-07-10`, checked to be safe in a URL and a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NightlyDate(Box<str>);

/// A string that is not the date of a nightly.
#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a date as 2026-07-10")]
pub struct NightlyDateError(Box<str>);

impl std::str::FromStr for NightlyDate {
    type Err = NightlyDateError;

    fn from_str(date: &str) -> Result<Self, Self::Err> {
        let valid = date.len() == 10
            && date.bytes().enumerate().all(|(at, byte)| match at {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        if valid {
            Ok(Self(date.into()))
        } else {
            Err(NightlyDateError(date.into()))
        }
    }
}

impl std::fmt::Display for NightlyDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What is kept of the manifest: the nightly and its archives, by target.
#[derive(Debug, Serialize, Deserialize)]
struct Summary {
    date: Box<str>,
    version: Box<str>,
    archives: BTreeMap<Box<str>, Archive>,
    /// When the manifest was fetched, in seconds since the Unix epoch.
    fetched_at: u64,
}

/// An archive of `rust-docs-json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Archive {
    url: Box<str>,
    sha256: Box<str>,
}

/// The parts of the manifest that are read.
#[derive(Deserialize)]
struct Manifest {
    date: String,
    pkg: Packages,
}

#[derive(Deserialize)]
struct Packages {
    #[serde(rename = "rust-docs-json-preview")]
    docs: Package,
}

#[derive(Deserialize)]
struct Package {
    version: String,
    target: BTreeMap<String, TargetPackage>,
}

#[derive(Deserialize)]
struct TargetPackage {
    available: bool,
    xz_url: Option<String>,
    xz_hash: Option<String>,
}

impl Dist {
    /// A client for the Rust distribution server itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn new(network: &Network, settings: DistSettings) -> Result<Self, reqwest::Error> {
        Ok(Self {
            settings,
            ..Self::with_base_url(DEFAULT_BASE_URL, network)?
        })
    }

    /// Whether the JSON of std may be fetched from the server (`std.fetch_from_dist`).
    #[must_use]
    pub const fn fetches(&self) -> bool {
        self.settings.fetch
    }

    /// A client for a server at `base_url` that serves the same paths, such as a mirror or a test
    /// server. `base_url` has no trailing `/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn with_base_url(base_url: &str, network: &Network) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: http::client_builder(network).build()?,
            base_url: base_url.into(),
            settings: DistSettings::default(),
            offline: network.offline,
        })
    }

    /// A client for a test server on this machine, which bypasses any proxy.
    #[cfg(test)]
    pub(super) fn for_local_test(base_url: &str) -> Self {
        Self {
            client: http::client_builder(&Network::default())
                .no_proxy()
                .build()
                .expect("the client builds"),
            base_url: base_url.into(),
            settings: DistSettings::default(),
            offline: false,
        }
    }

    /// The client offline (`network.offline`).
    #[cfg(test)]
    pub(super) const fn offline(mut self) -> Self {
        self.offline = true;
        self
    }

    /// The client with `settings` instead.
    #[cfg(test)]
    pub(super) const fn with_settings(mut self, settings: DistSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Fetches the body at `url`, at most `limit` bytes.
    async fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, DistError> {
        if self.offline {
            return Err(DistError::Offline);
        }
        let mut response = http::send(self.client.get(url))
            .await
            .map_err(network_error)?;
        let status = response.status();
        if status != StatusCode::OK {
            return Err(DistError::Status { status });
        }
        http::read_body(&mut response, limit)
            .await
            .map_err(|error| match error {
                BodyError::Network(error) => network_error(error),
                BodyError::TooLarge { limit } => DistError::TooLarge { limit },
            })
    }
}

/// The error for a failed request.
fn network_error(error: reqwest::Error) -> DistError {
    if http::is_certificate_error(&error) {
        DistError::Certificate(error)
    } else {
        DistError::Network(error)
    }
}

/// The JSON of std of the nightly of `date`, or else the latest, for `target`, unpacked in the
/// cache (requirement R-STD-6).
///
/// The manifest of the latest nightly is fetched when the one kept is older than an hour; that of
/// a date, once. The archive is fetched when that nightly is not unpacked yet. When the manifest cannot be fetched, the one kept is used with a warning, however old.
///
/// # Errors
///
/// Returns an error when the nightly channel has nothing for `target`, or the manifest or the
/// archive cannot be had, or does not check out.
pub async fn std_docs(
    cache: &Cache,
    dist: &Dist,
    target: &TargetTriple,
    date: Option<&NightlyDate>,
) -> Result<StdDocs, DistError> {
    let mut warnings = Vec::new();
    let summary = summary(cache, dist, date, &mut warnings).await?;
    let archive = summary
        .archives
        .get(target.as_str())
        .ok_or_else(|| DistError::NotAvailable {
            target: target.clone(),
        })?;
    let triple = archive_triple(&archive.url).ok_or_else(|| {
        DistError::Manifest(format!("unexpected archive URL {}", archive.url).into())
    })?;
    let short_hash = short_commit_hash(&summary.version).ok_or_else(|| {
        DistError::Manifest(format!("unexpected version {}", summary.version).into())
    })?;
    let dir = cache
        .dir()
        .join("std")
        .join(short_hash)
        .join(triple.as_str());
    // Another call that wants the same nightly goes first, and this one then finds it unpacked
    // instead of fetching the archive again (requirement R-NW-2).
    let held = cache.lock(&dir).await;
    if let Some(nightly) = read_complete(cache, &dir).await? {
        return Ok(StdDocs {
            dir,
            nightly,
            warnings,
        });
    }
    if !archive.url.starts_with(&*dist.base_url) {
        return Err(DistError::Manifest(
            format!("the archive is not on {}: {}", dist.base_url, archive.url).into(),
        ));
    }
    let body = get_remembering(cache, dist, &archive.url, MAX_ARCHIVE_BYTES, &dir).await?;
    let unpacked = {
        let (cache, dir) = (cache.clone(), dir.clone());
        let (date, version) = (summary.date.clone(), summary.version.clone());
        let (max, sha256) = (dist.settings.extract_max_size, archive.sha256.clone());
        // The lock goes with the unpacking, which goes on to its end even when the task that
        // waits for it is aborted, so that another call does not unpack it again meanwhile
        // (requirement R-WS-6). Tens of megabytes are checked there too, off the async threads.
        blocking(move || {
            let unpacked = if cache::sha256_hex(&body).eq_ignore_ascii_case(&sha256) {
                unpack(&cache, &body, &dir, date, version, triple, max)
            } else {
                Err(DistError::ChecksumMismatch)
            };
            drop(held);
            Ok(unpacked)
        })
        .await
        .map_err(DistError::Cache)?
    };
    Ok(StdDocs {
        dir,
        nightly: unpacked?,
        warnings,
    })
}

/// Reads the JSON of `krate` from `docs`.
///
/// # Errors
///
/// Returns an error when the file cannot be read, or does not match its digest.
pub fn read_std_json(cache: &Cache, docs: &StdDocs, krate: StdCrate) -> io::Result<Box<[u8]>> {
    let path = docs.dir.join(format!("{}.json", krate.as_str()));
    match cache.read_verified(&path)? {
        Verified::Found(json) => Ok(json),
        Verified::Mismatch | Verified::None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is missing or damaged", path.display()),
        )),
    }
}

/// The SHA-256 of the JSON of `krate` in `docs`, from the digest written next to it.
///
/// # Errors
///
/// Returns an error when the digest is missing or cannot be read.
pub fn std_json_sha256(cache: &Cache, docs: &StdDocs, krate: StdCrate) -> io::Result<Box<str>> {
    let path = docs.dir.join(format!("{}.json.sha256", krate.as_str()));
    let digest = cache.read_file(&path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is missing", path.display()),
        )
    })?;
    Ok(String::from_utf8_lossy(digest.trim_ascii()).into())
}

/// The summary of the manifest, from the cache if it was fetched within the hour, or else fetched
/// again. If fetching fails, an older one from the cache, with a warning.
async fn summary(
    cache: &Cache,
    dist: &Dist,
    date: Option<&NightlyDate>,
    warnings: &mut Vec<Box<str>>,
) -> Result<Summary, DistError> {
    let path = cache.dir().join("std").join(date.map_or_else(
        || "nightly.json".to_owned(),
        |date| format!("nightly-{date}.json"),
    ));
    let _held = cache.lock(&path).await;
    let kept = {
        let (cache, path) = (cache.clone(), path.clone());
        blocking(move || {
            Ok(match cache.read_verified(&path)? {
                Verified::Found(json) => serde_json::from_slice::<Summary>(&json).ok(),
                Verified::Mismatch | Verified::None => None,
            })
        })
        .await
        .map_err(DistError::Cache)?
    };
    let age = |summary: &Summary| {
        SystemTime::now()
            .duration_since(system_time(summary.fetched_at))
            .unwrap_or(Duration::ZERO)
    };
    // The manifest of a past date does not change.
    if let Some(kept) = kept
        .as_ref()
        .filter(|kept| date.is_some() || age(kept) < MANIFEST_TTL)
    {
        return Ok(Summary {
            date: kept.date.clone(),
            version: kept.version.clone(),
            archives: kept.archives.clone(),
            fetched_at: kept.fetched_at,
        });
    }
    let url = date.map_or_else(
        || format!("{}/dist/channel-rust-nightly.toml", dist.base_url),
        |date| format!("{}/dist/{date}/channel-rust-nightly.toml", dist.base_url),
    );
    let fetched = match get_remembering(cache, dist, &url, MAX_MANIFEST_BYTES, &path).await {
        Ok(body) => blocking(move || Ok(parse_manifest(&body)))
            .await
            .map_err(DistError::Cache)?,
        Err(error) => Err(error),
    };
    match (fetched, kept) {
        (Ok(summary), _) => {
            let json =
                serde_json::to_vec(&summary).map_err(|error| DistError::Cache(error.into()))?;
            let cache = cache.clone();
            let written = blocking(move || cache.write_verified(&path, &json)).await;
            if let Err(error) = written {
                tracing::warn!(%error, "failed to keep the manifest of the nightly channel");
            }
            Ok(summary)
        },
        (Err(error), Some(kept)) => {
            tracing::warn!(error = %error, "using the manifest of the nightly channel kept before");
            warnings.push(
                format!(
                    "the manifest of the nightly channel could not be fetched ({error}), so the one \
                     from {} is used",
                    kept.date
                )
                .into(),
            );
            Ok(kept)
        },
        (Err(error), None) => Err(error),
    }
}

/// [`Dist::get`] of what goes at `path` in `cache`, unless it failed transiently a moment ago,
/// which is remembered next to `path`: every call, and every process that shares the cache, would
/// wait for the same failure in turn (requirement R-ERR-3).
async fn get_remembering(
    cache: &Cache,
    dist: &Dist,
    url: &str,
    limit: u64,
    path: &Path,
) -> Result<Vec<u8>, DistError> {
    if let Some(retry_after) = super::recent_failure(cache, path).await {
        return Err(DistError::RecentFailure { retry_after });
    }
    let fetched = dist.get(url, limit).await;
    // A status that is not transient, as a 404, is not remembered.
    let transient = match &fetched {
        Err(DistError::Network(_)) => true,
        Err(DistError::Status { status }) => http::is_transient(*status),
        Ok(_) | Err(_) => false,
    };
    if transient {
        super::remember_failure(path).await;
    }
    fetched
}

/// Reads the manifest of the nightly channel.
fn parse_manifest(body: &[u8]) -> Result<Summary, DistError> {
    let text = std::str::from_utf8(body).map_err(|error| DistError::Manifest(error.into()))?;
    let manifest: Manifest =
        toml::from_str(text).map_err(|error| DistError::Manifest(error.into()))?;
    let archives = manifest
        .pkg
        .docs
        .target
        .into_iter()
        .filter_map(|(target, package)| match package {
            TargetPackage {
                available: true,
                xz_url: Some(url),
                xz_hash: Some(sha256),
            } => Some((target.into(), Archive {
                url: url.into(),
                sha256: sha256.into(),
            })),
            TargetPackage { .. } => None,
        })
        .collect();
    Ok(Summary {
        date: manifest.date.into(),
        version: manifest.pkg.docs.version.into(),
        archives,
        fetched_at: unix_seconds(SystemTime::now()),
    })
}

/// The target of an archive from its file name, `rust-docs-json-nightly-<triple>.tar.xz`.
fn archive_triple(url: &str) -> Option<TargetTriple> {
    url.rsplit('/')
        .next()?
        .strip_prefix("rust-docs-json-nightly-")?
        .strip_suffix(".tar.xz")?
        .parse()
        .ok()
}

/// The short commit hash in a version, as `b6eeb93c9` in `1.101.0-nightly (b6eeb93c9 2026-10-18)`.
fn short_commit_hash(version: &str) -> Option<&str> {
    let hash = version.split_once('(')?.1.split(' ').next()?;
    (hash.len() >= 7 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(hash)
}

/// The nightly unpacked in `dir`, if it was unpacked completely.
async fn read_complete(cache: &Cache, dir: &Path) -> Result<Option<Nightly>, DistError> {
    let (cache, path) = (cache.clone(), dir.join(COMPLETE_FILE));
    blocking(move || {
        Ok(match cache.read_verified(&path)? {
            Verified::Found(json) => serde_json::from_slice(&json).ok(),
            Verified::Mismatch | Verified::None => None,
        })
    })
    .await
    .map_err(DistError::Cache)
}

/// Unpacks the JSON files of the xz archive `body` into `dir`, each with its digest, and then the
/// mark that it is complete (requirement R-CA-4). Only the JSON of [`StdCrate`] and the commit
/// hash are read; the names of the files written are made here, never taken from the archive.
/// More than `max_unpacked` bytes in all is an error.
fn unpack(
    cache: &Cache,
    body: &[u8],
    dir: &Path,
    date: Box<str>,
    version: Box<str>,
    triple: TargetTriple,
    max_unpacked: u64,
) -> Result<Nightly, DistError> {
    let mut archive = tar::Archive::new(lzma_rust2::XzReader::new(body, false));
    let mut unpacked: u64 = 0;
    let mut commit_hash = None;
    let mut found = Vec::new();
    for entry in archive.entries().map_err(DistError::Archive)? {
        let mut entry = entry.map_err(DistError::Archive)?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry.path().map_err(DistError::Archive)?.into_owned();
        let Some(file) = archive_file(&path) else {
            continue;
        };
        let size = entry.size();
        unpacked = unpacked.saturating_add(size);
        if size > MAX_BYTES {
            return Err(DistError::TooLarge { limit: MAX_BYTES });
        }
        if unpacked > max_unpacked {
            return Err(DistError::TooLarge {
                limit: max_unpacked,
            });
        }
        let mut contents = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        entry
            .by_ref()
            .take(size)
            .read_to_end(&mut contents)
            .map_err(DistError::Archive)?;
        match file {
            ArchiveFile::CommitHash => {
                let hash = String::from_utf8_lossy(&contents).trim().to_owned();
                if hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    commit_hash = Some(hash);
                }
            },
            ArchiveFile::Json(krate) => {
                cache
                    .write_verified(&dir.join(format!("{}.json", krate.as_str())), &contents)
                    .map_err(DistError::Cache)?;
                found.push(krate);
            },
        }
    }
    if !(found.contains(&StdCrate::Std) && found.contains(&StdCrate::Core)) {
        return Err(DistError::Archive(io::Error::new(
            io::ErrorKind::InvalidData,
            "the archive has no JSON of std or core",
        )));
    }
    let commit_hash = commit_hash.ok_or_else(|| {
        DistError::Archive(io::Error::new(
            io::ErrorKind::InvalidData,
            "the archive has no git-commit-hash",
        ))
    })?;
    let nightly = Nightly {
        date,
        version,
        commit_hash: commit_hash.into(),
        triple,
    };
    let json = serde_json::to_vec(&nightly).map_err(|error| DistError::Cache(error.into()))?;
    cache
        .write_verified(&dir.join(COMPLETE_FILE), &json)
        .map_err(DistError::Cache)?;
    Ok(nightly)
}

/// A file of the archive that is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFile {
    /// `<top>/git-commit-hash`.
    CommitHash,
    /// `<top>/rust-docs-json-preview/share/doc/rust/json/<crate>.json`.
    Json(StdCrate),
}

/// Which file `path` of the archive is, if it is read.
fn archive_file(path: &Path) -> Option<ArchiveFile> {
    let parts: Vec<&str> = path
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str(),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => None,
        })
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [_, "git-commit-hash"] => Some(ArchiveFile::CommitHash),
        [_, PACKAGE, "share", "doc", "rust", "json", file] => file
            .strip_suffix(".json")
            .and_then(StdCrate::from_name)
            .map(ArchiveFile::Json),
        _ => None,
    }
}

/// Tests, and an archive and a server for the tests of the modules next to this one.
#[cfg(test)]
pub(crate) mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::data::test_http;

    pub(crate) const HASH: &str = "b6eeb93c9a1b2c3d4e5f60718293a4b5c6d7e8f9";

    /// An xz archive with `files`, under a top directory, as the distribution server makes it.
    pub(crate) fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(contents.len()).expect("small"));
            header.set_mode(0o644);
            if path.contains("..") {
                // Written as it is, without the checks of `append_data`, as an attacker could.
                let field = &mut header.as_old_mut().name;
                field
                    .get_mut(..path.len())
                    .expect("a short name")
                    .copy_from_slice(path.as_bytes());
                header.set_cksum();
                tar.append(&header, *contents).expect("appended");
            } else {
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("rust-docs-json-nightly-x86_64-unknown-linux-gnu/{path}"),
                    *contents,
                )
                .expect("appended");
            }
        }
        let tar = tar.into_inner().expect("a tar");
        let mut xz = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1))
            .expect("an encoder");
        io::Write::write_all(&mut xz, &tar).expect("compressed");
        xz.finish().expect("finished")
    }

    pub(crate) fn json_files() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("git-commit-hash", HASH.as_bytes()),
            (
                "rust-docs-json-preview/share/doc/rust/json/std.json",
                br#"{"format_version":61}"#,
            ),
            (
                "rust-docs-json-preview/share/doc/rust/json/core.json",
                br#"{"format_version":61,"core":true}"#,
            ),
            (
                "rust-docs-json-preview/share/doc/rust/json/../evil.json",
                b"{}",
            ),
            ("README.md", b"readme"),
        ]
    }

    /// The manifest for an archive at `url` with `sha256`, for Linux on x86-64 and, as the real one
    /// does, the same archive for FreeBSD.
    fn manifest(url: &str, sha256: &str) -> Vec<u8> {
        format!(
            r#"manifest-version = "2"
date = "2026-10-19"

[pkg.rustc]
version = "1.101.0-nightly (b6eeb93c9 2026-10-18)"

[pkg.rust-docs-json-preview]
version = "1.101.0-nightly (b6eeb93c9 2026-10-18)"

[pkg.rust-docs-json-preview.target.x86_64-unknown-linux-gnu]
available = true
url = "{url}.gz"
hash = "0"
xz_url = "{url}"
xz_hash = "{sha256}"

[pkg.rust-docs-json-preview.target.x86_64-unknown-freebsd]
available = true
xz_url = "{url}"
xz_hash = "{sha256}"

[pkg.rust-docs-json-preview.target.wasm32-unknown-unknown]
available = false
"#
        )
        .into_bytes()
    }

    fn ok(body: &[u8]) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body.to_vec(),
        )
    }

    /// A cache, and a distribution server that answers the manifest and then `archive`, whose
    /// SHA-256 in the manifest is `sha256` (or its own).
    pub(in crate::data) fn setup(
        archive: &[u8],
        sha256: Option<&str>,
    ) -> (Cache, Dist, mpsc::Receiver<String>, tempfile::TempDir) {
        setup_after(archive, sha256, Duration::ZERO)
    }

    /// [`setup`] whose server answers each request after `delay`.
    pub(crate) fn setup_after(
        archive: &[u8],
        sha256: Option<&str>,
        delay: Duration,
    ) -> (Cache, Dist, mpsc::Receiver<String>, tempfile::TempDir) {
        // The URL is known only once the server runs, so the manifest names a path that the
        // server answers whatever is asked.
        let (base_url, requests) = test_http::serve_sequence_after(
            |base_url| {
                let url = format!(
                    "{base_url}/dist/2026-10-19/rust-docs-json-nightly-x86_64-unknown-linux-gnu.tar.xz"
                );
                let sha256 = sha256.map_or_else(|| cache::sha256_hex(archive), str::to_owned);
                vec![ok(&manifest(&url, &sha256)), ok(archive)]
            },
            delay,
        );
        let dir = tempfile::tempdir().expect("a temporary directory");
        (
            Cache::at(dir.path()),
            Dist::for_local_test(&base_url),
            requests,
            dir,
        )
    }

    pub(crate) fn linux() -> TargetTriple {
        "x86_64-unknown-linux-gnu".parse().expect("a triple")
    }

    /// A transient failure to fetch the manifest is remembered for a while in the cache, so that
    /// the next call, in this process or another, does not ask again (requirement R-ERR-3).
    #[tokio::test]
    async fn a_transient_failure_is_not_asked_again_for_a_while() {
        let unavailable = || {
            (
                "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n".to_owned(),
                Vec::new(),
            )
        };
        // Each fetch is tried 3 times.
        let (base_url, requests) =
            test_http::serve_sequence(vec![unavailable(), unavailable(), unavailable()]);
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let dist = Dist::for_local_test(&base_url);
        assert!(matches!(
            std_docs(&cache, &dist, &linux(), None).await,
            Err(DistError::Status { .. })
        ));
        assert!(matches!(
            std_docs(&cache, &dist, &linux(), None).await,
            Err(DistError::RecentFailure { .. })
        ));
        assert_eq!(requests.try_iter().count(), 3, "not asked again");
    }

    #[tokio::test]
    async fn the_archive_is_fetched_checked_and_unpacked_once() {
        let archive = archive(&json_files());
        let (cache, dist, requests, _dir) = setup(&archive, None);
        let docs = std_docs(&cache, &dist, &linux(), None)
            .await
            .expect("unpacked");
        assert_eq!(docs.nightly.commit_hash.as_ref(), HASH);
        assert_eq!(docs.nightly.date.as_ref(), "2026-10-19");
        assert_eq!(docs.nightly.triple, linux());
        assert!(docs.dir.ends_with("std/b6eeb93c9/x86_64-unknown-linux-gnu"));
        assert_eq!(
            &*read_std_json(&cache, &docs, StdCrate::Core).expect("core"),
            br#"{"format_version":61,"core":true}"#
        );
        // Only the JSON of the crates and the mark were written; nothing outside the directory.
        let mut names: Vec<String> = std::fs::read_dir(&docs.dir)
            .expect("the directory")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(names, [
            "complete.json",
            "complete.json.sha256",
            "core.json",
            "core.json.sha256",
            "std.json",
            "std.json.sha256",
        ]);
        // `top/../../std.json` was not written anywhere.
        for outside in [
            cache.dir().join("std.json"),
            cache.dir().join("std/std.json"),
        ] {
            assert!(!outside.exists(), "{}", outside.display());
        }
        // Another target with the same archive, within the hour: no request at all.
        let freebsd: TargetTriple = "x86_64-unknown-freebsd".parse().expect("a triple");
        let again = std_docs(&cache, &dist, &freebsd, None)
            .await
            .expect("cached");
        assert_eq!(again.dir, docs.dir);
        assert_eq!(again.nightly.triple, linux(), "the archive's own target");
        assert_eq!(requests.iter().count(), 2);
    }

    #[tokio::test]
    async fn an_archive_that_does_not_match_its_sha256_is_not_unpacked() {
        let archive = archive(&json_files());
        let (cache, dist, _requests, _dir) = setup(&archive, Some("00"));
        assert!(matches!(
            std_docs(&cache, &dist, &linux(), None).await,
            Err(DistError::ChecksumMismatch)
        ));
    }

    #[tokio::test]
    async fn a_target_without_rust_docs_json_is_told() {
        let archive = archive(&json_files());
        let (cache, dist, _requests, _dir) = setup(&archive, None);
        let wasm: TargetTriple = "wasm32-unknown-unknown".parse().expect("a triple");
        assert!(matches!(
            std_docs(&cache, &dist, &wasm, None).await,
            Err(DistError::NotAvailable { target }) if target == wasm
        ));
    }

    #[tokio::test]
    async fn an_archive_that_unpacks_to_more_than_the_limit_is_not_unpacked() {
        let archive = archive(&json_files());
        let (cache, dist, _requests, _dir) = setup(&archive, None);
        let dist = dist.with_settings(DistSettings {
            extract_max_size: 10,
            ..DistSettings::default()
        });
        assert!(matches!(
            std_docs(&cache, &dist, &linux(), None).await,
            Err(DistError::TooLarge { limit: 10 })
        ));
    }

    /// Offline, the server is not asked, and without the manifest in the cache there is nothing
    /// to unpack (requirement R-CA-7).
    #[tokio::test]
    async fn offline_nothing_is_fetched() {
        let archive = archive(&json_files());
        let (cache, dist, requests, _dir) = setup(&archive, None);
        assert!(matches!(
            std_docs(&cache, &dist.offline(), &linux(), None).await,
            Err(DistError::Offline)
        ));
        assert!(requests.try_recv().is_err(), "nothing was asked");
    }

    #[test]
    fn the_settings_of_the_dist_are_read() {
        let settings = Settings::from_sources(
            None,
            [
                ("CHATORA_RUSTDOC_MCP_STD__FETCH_FROM_DIST", "false"),
                ("CHATORA_RUSTDOC_MCP_LIMITS__EXTRACT_MAX_SIZE", "1GiB"),
            ]
            .map(|(name, value)| (name.into(), value.into())),
        );
        assert_eq!(DistSettings::from_settings(&settings), DistSettings {
            fetch: false,
            extract_max_size: 1024 * 1024 * 1024,
        });
        assert_eq!(
            DistSettings::from_settings(&Settings::default()),
            DistSettings::default()
        );
    }

    #[tokio::test]
    async fn an_archive_without_std_is_an_error() {
        let archive = archive(&[("git-commit-hash", HASH.as_bytes())]);
        let (cache, dist, _requests, _dir) = setup(&archive, None);
        assert!(matches!(
            std_docs(&cache, &dist, &linux(), None).await,
            Err(DistError::Archive(_))
        ));
    }

    #[test]
    fn nightly_dates_are_checked() {
        "2026-07-10".parse::<NightlyDate>().expect("a date");
        for invalid in ["2026-7-10", "2026/07/10", "../07-10x", "2026-07-1a", ""] {
            assert!(invalid.parse::<NightlyDate>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn versions_and_urls_are_read() {
        assert_eq!(
            short_commit_hash("1.101.0-nightly (b6eeb93c9 2026-10-18)"),
            Some("b6eeb93c9")
        );
        assert_eq!(short_commit_hash("1.101.0-nightly"), None);
        assert_eq!(
            archive_triple(
                "https://static.rust-lang.org/dist/2026-10-19/rust-docs-json-nightly-aarch64-apple-darwin.tar.xz"
            ),
            "aarch64-apple-darwin".parse().ok()
        );
        assert_eq!(archive_triple("https://example.com/other.tar.xz"), None);
    }
}
