//! The sources of a crates.io release, unpacked from its `.crate` into the cache (requirements
//! R-SC-2 and R-NW-3, decided in M3b, B1).
//!
//! A `.crate` is a third party's archive. Before anything is written, a first pass over its raw
//! entries checks how much it holds, as the `tar` crate reads a GNU long name or a PAX extension
//! whole into memory, and refuses a PAX extension that would make the second pass read the
//! entries otherwise than the first; then each entry is checked and written into a temporary
//! directory next to the place, which is renamed into it once every file is on the disk.
//!
//! [`sources`] is where they are had: where cargo unpacked them, or else unpacked here.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::read::GzDecoder;
use semver::Version;

use super::{
    cache::{self, Cache, Verified},
    crate_file::{self, CrateError, StaticCrates},
    crate_name::CrateName,
    formats,
    used,
};

/// Most files unpacked from one `.crate` (requirement R-SC-2, initial value; not a setting).
pub const MAX_FILES: usize = 100_000;

/// Largest GNU long name or PAX extension header: a path is far shorter.
const MAX_EXTENSION_BYTES: u64 = 64 * 1024;

/// Most entries of any kind, directories and extension headers too, in one `.crate`.
const MAX_ENTRIES: usize = MAX_FILES.saturating_mul(4);

/// The file that notes in a temporary directory that it is in use: no `.crate` has it.
const IN_USE: &str = ".chatora-rustdoc-mcp.in-use";

/// How many files are written between two notes in the temporary directory that it is in use, so
/// that the removal of stale temporaries, which looks at its time, leaves it.
const FILES_PER_NOTE: usize = 256;

/// Largest `.cargo-ok` read: cargo writes `{"v":1}`.
const MAX_CARGO_OK_BYTES: u64 = 1024;

/// Counts the unpackings of this process, so that no two temporary directories share a name.
static UNPACK_COUNT: AtomicU64 = AtomicU64::new(0);

/// Why a `.crate` was not unpacked.
#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    /// An entry is one that `cargo package` does not make, which a broken or hostile `.crate`
    /// has: the whole `.crate` is refused (decided in M3b, B1-4).
    #[error("the .crate has an entry that is refused, {why}: {entry}")]
    Refused {
        /// The entry's path, as the archive gives it, shortened, with what would not be seen
        /// marked.
        entry: String,
        /// Why it is refused.
        why: &'static str,
    },
    /// It holds more bytes than the limit (`limits.extract_max_size`, decided in M3b, B1-5).
    #[error("the .crate unpacks to more than {limit} bytes")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// It holds more files than [`MAX_FILES`].
    #[error("the .crate holds more than {limit} files")]
    TooManyFiles {
        /// The limit.
        limit: usize,
    },
    /// It is not a gzipped tar that can be read.
    #[error("the .crate could not be read")]
    Archive(#[source] io::Error),
    /// The cache could not be written.
    #[error("the cache could not be written")]
    Cache(#[source] io::Error),
}

/// A tar archive, compressed.
#[derive(Debug, Clone, Copy)]
pub enum Packed<'a> {
    /// With gzip, as a `.crate`.
    Gzip(&'a [u8]),
    /// With xz, as the components of the Rust distribution server (decided in M3b, B3).
    Xz(&'a [u8]),
}

/// The files of a `.crate`, relative to the package's root with `/`, sorted, and their bytes: what
/// is unpacked from it, and what of a directory that cargo unpacked is read (decided in M3b, B1-7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Listing {
    /// The files.
    pub files: Vec<String>,
    /// Their bytes in all.
    pub bytes: u64,
}

/// Where the sources of a release were found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// In `$CARGO_HOME/registry/src`, where cargo unpacked them (decided in M3b, B1-7).
    Cargo,
    /// In the cache, where this server unpacked them.
    Cache,
}

/// The sources of a crates.io release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// The package's root: the directory of its `Cargo.toml`.
    pub root: PathBuf,
    /// The files of its `.crate`, which alone are read under `root` (requirement R-SC-4).
    pub listing: Listing,
    /// Where they were found.
    pub origin: Origin,
}

/// Why the sources of a release could not be had.
#[derive(Debug, thiserror::Error)]
pub enum SourcesError {
    /// Its `.crate` could not be had: offline, [`CrateError::Offline`] when no `.crate` is at hand
    /// (decided in M3b, B2-3).
    #[error(transparent)]
    Crate(#[from] CrateError),
    /// Its `.crate` is not at hand, and without its checksum it is not fetched.
    #[error("the .crate is not at hand")]
    NotCached,
    /// Its `.crate` could not be unpacked or listed.
    #[error(transparent)]
    Unpack(#[from] UnpackError),
    /// The cache could not be used.
    #[error("the cache could not be used")]
    Cache(#[source] io::Error),
}

/// The sources of `name` at `version` from crates.io (requirements R-SC-2 and R-SC-4, decided in
/// M3b, B1): where cargo unpacked them, or else unpacked in the cache.
///
/// Cargo's are in `$CARGO_HOME/registry/src`, if it finished them. Else the `.crate` is unpacked:
/// that of [`StaticCrates::crate_file`], from the cache or cargo's, or else fetched and checked
/// against `checksum`. `name` is spelled as crates.io spells it, as the archive's directory and
/// cargo's are.
///
/// Either way, the files that may be read are those of the `.crate`'s listing, kept in the cache
/// beside the unpacked sources: a build script may have written more where cargo unpacked them.
/// The place in the cache is noted as used.
///
/// # Errors
///
/// Returns an error when the `.crate` cannot be had (offline, [`CrateError::Offline`]), is not at
/// hand without `checksum`, is refused or too large, or the cache cannot be used.
pub async fn sources(
    crates: &StaticCrates,
    cache: &Cache,
    name: &CrateName,
    version: &Version,
    checksum: Option<&str>,
    max_bytes: u64,
) -> Result<Sources, SourcesError> {
    let place = cache.crate_place(name, version);
    let dir = place.join(formats::sources_dir());
    let look = || {
        let (reading, dir) = (cache.clone(), dir.clone());
        let (home, wanted) = (
            crates.cargo_home().map(Path::to_path_buf),
            (name.clone(), version.clone()),
        );
        async move {
            super::blocking(move || {
                let by_cargo = unpacked_by_cargo(home.as_deref(), &wanted.0, &wanted.1);
                Ok(
                    found(&reading, &dir, by_cargo.clone(), max_bytes, Trust::Checked)
                        .map(|found| (found, by_cargo)),
                )
            })
            .await
            .map_err(SourcesError::Cache)?
        }
    };
    if let (Some(found), _) = look().await? {
        used::note_later(&place);
        return Ok(found);
    }
    // Another call that wants the same goes first (requirement R-NW-2), and the GC leaves it. What
    // cargo has may have changed while it waited.
    let held = cache.lock(&dir).await;
    let by_cargo = match look().await? {
        (Some(found), _) => {
            drop(held);
            used::note_later(&place);
            return Ok(found);
        },
        (None, by_cargo) => by_cargo,
    };
    let contents = crates
        .crate_file(cache, name, version, checksum)
        .await?
        .ok_or(SourcesError::NotCached)?;
    let (writing, prefix) = (cache.clone(), format!("{name}-{version}"));
    // With the lock, which goes when the work is done, even when this call is dropped.
    let made = super::blocking(move || {
        let made = make(
            &writing,
            &contents,
            &prefix,
            &dir,
            by_cargo,
            max_bytes,
            Trust::Checked,
        );
        drop(held);
        Ok(made)
    })
    .await
    .map_err(SourcesError::Cache)??;
    used::note_later(&place);
    Ok(made)
}

/// The directory where cargo unpacked `name` at `version` from crates.io, if it finished: its
/// `.cargo-ok` tells a version of at least 1, which cargo writes last (decided in M3b, B1-7).
/// One that is older, empty or cut short is unpacked again by cargo, and is not read here.
///
/// Only the sparse index of crates.io is taken, `index.crates.io-…`, whose name tells its host:
/// that of a git index, `github.com-…`, tells only that it is on GitHub, as another registry may
/// be, and nothing checks what cargo unpacked against the `.crate`. A symlink or a FIFO for
/// `.cargo-ok` is not read.
///
/// It reads the file system, so it is for a blocking thread.
fn unpacked_by_cargo(
    cargo_home: Option<&Path>,
    name: &CrateName,
    version: &Version,
) -> Option<PathBuf> {
    let registries = fs::read_dir(cargo_home?.join("registry/src")).ok()?;
    let package = format!("{name}-{version}");
    registries
        .filter_map(Result::ok)
        .filter(|registry| is_sparse_index(&registry.file_name().to_string_lossy()))
        .map(|registry| registry.path().join(&package))
        .find(|dir| finished_by_cargo(dir))
}

/// Whether `registry`, a directory of `$CARGO_HOME/registry`, is of the sparse index of crates.io.
fn is_sparse_index(registry: &str) -> bool {
    registry.starts_with("index.crates.io-")
}

/// Whether cargo finished unpacking a package into `dir`: its `.cargo-ok`, a regular file, tells
/// a version of at least 1.
fn finished_by_cargo(dir: &Path) -> bool {
    #[derive(serde::Deserialize)]
    struct CargoOk {
        v: u64,
    }
    let mut ok = Vec::new();
    fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.is_dir())
        && cache::open_without_following(&dir.join(".cargo-ok"))
            .and_then(|file| {
                if file.metadata()?.is_file() {
                    file.take(MAX_CARGO_OK_BYTES).read_to_end(&mut ok)
                } else {
                    Err(io::ErrorKind::InvalidData.into())
                }
            })
            .is_ok()
        && serde_json::from_slice::<CargoOk>(&ok).is_ok_and(|ok| ok.v >= 1)
}

/// The sources of the crates.io release that cargo unpacked into `unpacked`, without fetching
/// anything (decided in M3b, B5b).
///
/// `unpacked` is `$CARGO_HOME/registry/src/index.crates.io-…/<name>-<version>` in `cargo_home`,
/// which cargo finished. The listing of the `.crate` is the cache's, or else made now from a
/// `.crate` at hand, of the cache or of cargo's `registry/cache`. `None` when it cannot be had so.
///
/// Cargo's `.crate` is taken without a checksum, as what cargo unpacked from it is: both are
/// cargo's, which checked them when it fetched them. Its listing is kept noted so, which
/// [`sources`], that can check it, makes again. The lock of the sources is not waited for: `None`
/// while another holds it.
///
/// It reads and writes files, so it is for a blocking thread.
#[must_use]
pub fn unpacked_at(
    cache: &Cache,
    cargo_home: &Path,
    unpacked: &Path,
    max_bytes: u64,
) -> Option<Sources> {
    let registry = unpacked.parent()?;
    let index = registry.file_name()?.to_str()?;
    if !is_sparse_index(index)
        || registry.parent()? != cargo_home.join("registry").join("src")
        || !finished_by_cargo(unpacked)
    {
        return None;
    }
    let package = unpacked.file_name()?.to_str()?;
    let (name, version) = package_name(package)?;
    let place = cache.crate_place(&name, &version);
    let dir = place.join(formats::sources_dir());
    let found = |reading: &Cache| {
        found(
            reading,
            &dir,
            Some(unpacked.to_path_buf()),
            max_bytes,
            Trust::Unchecked,
        )
        .ok()
        .flatten()
    };
    if let Some(found) = found(cache) {
        used::note(&place);
        return Some(found);
    }
    // At hand, before the lock: the cache's, checked when it was fetched, or else cargo's.
    let (contents, trust) = match cache.read_verified(&cache.crate_file_path(&name, &version)) {
        Ok(Verified::Found(contents)) => (contents, Trust::Checked),
        Ok(Verified::Mismatch | Verified::None) | Err(_) => {
            let path = cargo_home
                .join("registry")
                .join("cache")
                .join(index)
                .join(format!("{package}.crate"));
            (read_cargos_crate(&path)?, Trust::Unchecked)
        },
    };
    // Not waited for: what holds it may be fetching, which a query does not wait for (decided in
    // M3b, decision 16).
    let held = cache.try_lock(&dir)?;
    if let Some(found) = found(cache) {
        drop(held);
        used::note(&place);
        return Some(found);
    }
    let made = make(
        cache,
        &contents,
        package,
        &dir,
        Some(unpacked.to_path_buf()),
        max_bytes,
        trust,
    );
    drop(held);
    let made = made
        .inspect_err(|error| {
            tracing::debug!(path = %unpacked.display(), %error, "the listing of a .crate could not be made");
        })
        .ok()?;
    used::note(&place);
    Some(made)
}

/// Cargo's `.crate` at `path`, a regular file of at most [`crate_file::MAX_CRATE_BYTES`], read
/// without following a symlink.
fn read_cargos_crate(path: &Path) -> Option<Box<[u8]>> {
    let file = cache::open_without_following(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > crate_file::MAX_CRATE_BYTES {
        return None;
    }
    let mut contents = Vec::new();
    file.take(crate_file::MAX_CRATE_BYTES.saturating_add(1))
        .read_to_end(&mut contents)
        .ok()?;
    (u64::try_from(contents.len()).ok()? <= crate_file::MAX_CRATE_BYTES)
        .then(|| contents.into_boxed_slice())
}

/// Whether the checksum of the `.crate` that a listing is made from was checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    /// Against the crates.io index's, as the cache's `.crate` was when it was fetched.
    Checked,
    /// Not: cargo's, which cargo checked when it fetched it (decided in M3b, decision 19).
    Unchecked,
}

/// A listing as the cache keeps it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    #[serde(flatten)]
    listing: Listing,
    /// Whether it was made from a `.crate` whose checksum was not checked, which only what cargo
    /// unpacked is read by: `sources`, which can check it, makes it again (decided in M3b, B5b).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unchecked: bool,
}

/// The name and the version of a package from its directory's name, `<name>-<version>`, as cargo
/// names it: where the name is valid and the rest is a version.
#[must_use]
pub fn package_name(package: &str) -> Option<(CrateName, Version)> {
    package.match_indices('-').find_map(|(at, _)| {
        let name = package.get(..at)?.parse().ok()?;
        let version = Version::parse(package.get(at.saturating_add(1)..)?).ok()?;
        Some((name, version))
    })
}

/// The sources that are there already, with the listing in the cache beside `dir`: where cargo
/// unpacked them (`by_cargo`), or else in `dir`, which is there only whole. Ones of more than
/// `max_bytes`, as a listing made under a larger limit tells, are refused; and a listing made from
/// a `.crate` whose checksum was not checked, unless `trust` takes it.
///
/// It reads the file system, so it is for a blocking thread.
fn found(
    cache: &Cache,
    dir: &Path,
    by_cargo: Option<PathBuf>,
    max_bytes: u64,
    trust: Trust,
) -> Result<Option<Sources>, SourcesError> {
    let kept = match cache
        .read_verified(&listing_path(dir))
        .map_err(SourcesError::Cache)?
    {
        Verified::Found(json) => serde_json::from_slice::<Kept>(&json).ok(),
        Verified::Mismatch | Verified::None => None,
    };
    let Some(Kept { listing, unchecked }) = kept else {
        return Ok(None);
    };
    if unchecked && trust == Trust::Checked {
        return Ok(None);
    }
    if listing.bytes > max_bytes {
        return Err(UnpackError::TooLarge { limit: max_bytes }.into());
    }
    let (root, origin) = match by_cargo {
        Some(root) => (root, Origin::Cargo),
        None if is_unpacked(dir) => (dir.to_path_buf(), Origin::Cache),
        None => return Ok(None),
    };
    Ok(Some(Sources {
        root,
        listing,
        origin,
    }))
}

/// Whether the sources are unpacked in `dir`: a directory, which is there only whole, and not a
/// symlink, which this server does not make.
fn is_unpacked(dir: &Path) -> bool {
    fs::symlink_metadata(dir).is_ok_and(|metadata| metadata.is_dir())
}

/// Lists the `.crate` `contents` for where cargo unpacked it (`by_cargo`), or for `dir` if it is
/// unpacked there, as when the listing was not written after it, or else unpacks it into `dir`;
/// and keeps its listing beside `dir`, noting if `trust` tells that the checksum of `contents` was
/// not checked. What is unpacked is not unpacked again, which Windows may not let replace while a
/// file in it is open.
///
/// It reads and writes files, so it is for a blocking thread, with the lock of `dir` held.
fn make(
    cache: &Cache,
    contents: &[u8],
    prefix: &str,
    dir: &Path,
    by_cargo: Option<PathBuf>,
    max_bytes: u64,
    trust: Trust,
) -> Result<Sources, SourcesError> {
    let (listing, root, origin) = match by_cargo {
        Some(root) => (
            list(Packed::Gzip(contents), prefix, max_bytes)?,
            root,
            Origin::Cargo,
        ),
        None if is_unpacked(dir) => (
            list(Packed::Gzip(contents), prefix, max_bytes)?,
            dir.to_path_buf(),
            Origin::Cache,
        ),
        None => (
            unpack(Packed::Gzip(contents), prefix, dir, max_bytes)?,
            dir.to_path_buf(),
            Origin::Cache,
        ),
    };
    let kept = Kept {
        listing,
        unchecked: trust == Trust::Unchecked,
    };
    let json = serde_json::to_vec(&kept).map_err(|error| SourcesError::Cache(error.into()))?;
    let Kept { listing, .. } = kept;
    cache
        .write_verified(&listing_path(dir), &json)
        .map_err(SourcesError::Cache)?;
    Ok(Sources {
        root,
        listing,
        origin,
    })
}

/// Where the listing of the `.crate` is kept, beside the directory `dir` of its sources:
/// `src.v<n>.json`.
fn listing_path(dir: &Path) -> PathBuf {
    cache::with_suffix(dir, ".json")
}

/// Unpacks the gzipped tar `contents` into the directory `into`, which is replaced.
///
/// Its entries are all under `prefix/` (`<name>-<version>`), which is left out, so that `into` is
/// the package's root. More than `max_bytes` in all, or more than [`MAX_FILES`] files, is an
/// error, before anything is written. The caller has checked `contents` against its checksum.
///
/// `into` is there only whole: the files are written into a temporary directory beside it, which
/// is renamed to it once each is synced, so that its being there tells that it is complete.
///
/// It reads and writes files, so it is for a blocking thread, with the lock of `into` held.
///
/// # Errors
///
/// Returns an error when an entry is refused, a limit is passed, the archive cannot be read, or the
/// cache cannot be written; what was at `into` is left then, unless it could not be moved away.
pub fn unpack(
    packed: Packed<'_>,
    prefix: &str,
    into: &Path,
    max_bytes: u64,
) -> Result<Listing, UnpackError> {
    check_sizes(packed, max_bytes)?;
    let parent = into.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(UnpackError::Cache)?;
    let temp = temporary(into);
    let unpacked = walk(packed, prefix, Some(&temp), max_bytes)
        .and_then(|unpacked| replace(into, &temp).map(|()| unpacked));
    if unpacked.is_err() {
        remove_quietly(&temp);
    }
    if unpacked.is_ok() {
        cache::remove_stale_temporaries(parent);
    }
    unpacked
}

/// The files of the gzipped tar `contents` under `prefix/`, checked as [`unpack`] checks them,
/// without writing anything.
///
/// It reads the whole archive, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when an entry is refused, a limit is passed, or the archive cannot be read.
pub fn list(packed: Packed<'_>, prefix: &str, max_bytes: u64) -> Result<Listing, UnpackError> {
    check_sizes(packed, max_bytes)?;
    walk(packed, prefix, None, max_bytes)
}

/// A new name next to `into` for a temporary directory: `.<name>.<process>.<count>.tmp`, which the
/// removal of stale temporaries knows.
fn temporary(into: &Path) -> PathBuf {
    into.with_file_name(format!(
        ".{}.{}.{}.tmp",
        into.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        UNPACK_COUNT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Puts the directory `temp` at `into`: what is there is moved away first, and removed after, so
/// that a removal that fails halfway leaves its part under a temporary name.
fn replace(into: &Path, temp: &Path) -> Result<(), UnpackError> {
    let aside = temporary(into);
    let moved = match fs::rename(into, &aside) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(UnpackError::Cache(error)),
    };
    if let Err(error) = fs::rename(temp, into) {
        if moved && let Err(back) = fs::rename(&aside, into) {
            tracing::warn!(path = %into.display(), error = %back, "failed to put back what was unpacked before");
        }
        return Err(UnpackError::Cache(error));
    }
    if moved {
        remove_quietly(&aside);
    }
    Ok(())
}

/// Removes the directory `dir` if it is there, with a warning when it cannot be.
fn remove_quietly(dir: &Path) {
    if let Err(error) = fs::remove_dir_all(dir)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %dir.display(), %error, "failed to remove a temporary directory");
    }
}

/// `packed` decompressed, up to what `max_bytes` of entries with their headers can take: past
/// that, the archive reads as cut short.
fn decompressed(packed: Packed<'_>, max_bytes: u64) -> io::Take<Box<dyn io::Read + '_>> {
    let headers = u64::try_from(MAX_ENTRIES)
        .unwrap_or(u64::MAX)
        .saturating_mul(1024);
    let reader: Box<dyn io::Read + '_> = match packed {
        Packed::Gzip(contents) => Box::new(GzDecoder::new(contents)),
        Packed::Xz(contents) => Box::new(lzma_rust2::XzReader::new(contents, false)),
    };
    reader.take(max_bytes.saturating_add(headers))
}

/// Checks, over the raw entries of `contents`, that no extension header is larger than
/// [`MAX_EXTENSION_BYTES`] nor changes the size of an entry, that there are not too many entries,
/// and that what they hold is within `max_bytes`; before a long name is read into memory, or a
/// file is written.
fn check_sizes(packed: Packed<'_>, max_bytes: u64) -> Result<(), UnpackError> {
    let mut archive = tar::Archive::new(decompressed(packed, max_bytes));
    let mut entries = 0_usize;
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    for entry in archive.entries().map_err(UnpackError::Archive)?.raw(true) {
        let mut entry = entry.map_err(UnpackError::Archive)?;
        let kind = entry.header().entry_type();
        let size = entry.header().entry_size().map_err(UnpackError::Archive)?;
        let pax = kind.is_pax_local_extensions() || kind.is_pax_global_extensions();
        if (pax || kind.is_gnu_longname() || kind.is_gnu_longlink()) && size > MAX_EXTENSION_BYTES {
            return Err(UnpackError::Refused {
                entry: shortened(&entry.path_bytes()),
                why: "an extension header that is too large",
            });
        }
        // The size, read otherwise in the second pass than here, and sparse files, which tar
        // writes with their size apart. Every record is looked at: tar stops at one it cannot read.
        if pax
            && let Some(extensions) = entry.pax_extensions().map_err(UnpackError::Archive)?
            && extensions.filter_map(Result::ok).any(|extension| {
                let key = extension.key_bytes();
                key == b"size" || key.starts_with(b"GNU.sparse")
            })
        {
            return Err(UnpackError::Refused {
                entry: shortened(&entry.path_bytes()),
                why: "a PAX extension that sets a size",
            });
        }
        entries = entries.saturating_add(1);
        if kind.is_file() {
            files = files.saturating_add(1);
        }
        if files > MAX_FILES || entries > MAX_ENTRIES {
            return Err(UnpackError::TooManyFiles { limit: MAX_FILES });
        }
        // The size of every entry, which is read through to get to the next one.
        bytes = bytes.saturating_add(size);
        if bytes > max_bytes {
            return Err(UnpackError::TooLarge { limit: max_bytes });
        }
    }
    Ok(())
}

/// Writes the entries of `contents` under `prefix/` into the new directory `temp`, each file
/// synced to the disk (requirement R-CA-4), or, without `temp`, only checks them. The limits are
/// checked again as it goes.
#[expect(
    clippy::create_dir,
    reason = "the temporary directory is made alone, so that one left there is found"
)]
fn walk(
    packed: Packed<'_>,
    prefix: &str,
    temp: Option<&Path>,
    max_bytes: u64,
) -> Result<Listing, UnpackError> {
    if let Some(temp) = temp
        && let Err(error) = fs::create_dir(temp)
    {
        // Left by a process of the same id that stopped: its name is this unpacking's now.
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(UnpackError::Cache(error));
        }
        fs::remove_dir_all(temp)
            .and_then(|()| fs::create_dir(temp))
            .map_err(UnpackError::Cache)?;
    }
    let mut archive = tar::Archive::new(decompressed(packed, max_bytes));
    let mut written = Written::default();
    // The note's name is not an entry's.
    written
        .claim(Path::new(IN_USE), true)
        .map_err(|why| UnpackError::Refused {
            entry: IN_USE.to_owned(),
            why,
        })?;
    for entry in archive.entries().map_err(UnpackError::Archive)? {
        let mut entry = entry.map_err(UnpackError::Archive)?;
        let raw_path = entry.path_bytes().into_owned();
        let refused = |why| UnpackError::Refused {
            entry: shortened(&raw_path),
            why,
        };
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            continue;
        }
        let relative = relative_path(&raw_path, prefix).map_err(refused)?;
        let is_file = kind.is_file();
        if !(is_file || kind.is_dir()) {
            return Err(refused("not a file nor a directory"));
        }
        let Some(relative) = relative else {
            if is_file {
                return Err(refused("a file in place of the package's directory"));
            }
            continue;
        };
        written.claim(&relative, is_file).map_err(refused)?;
        let size = entry.size();
        if is_file {
            written.count(size, max_bytes)?;
        }
        let Some(temp) = temp else {
            if is_file {
                written.files.push(slashed(&relative));
            }
            continue;
        };
        let path = temp.join(&relative);
        if !is_file {
            make_below(temp, &relative).map_err(UnpackError::Cache)?;
            continue;
        }
        if written.files.len() % FILES_PER_NOTE == 0 {
            note_in_use(temp)?;
        }
        if let Some(parent) = relative.parent() {
            make_below(temp, parent).map_err(UnpackError::Cache)?;
        }
        // A second entry of the same path finds the first there.
        let mut file = match File::create_new(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(refused("a path that another entry has"));
            },
            Err(error) => return Err(UnpackError::Cache(error)),
        };
        io::copy(&mut entry.by_ref().take(size), &mut file).map_err(UnpackError::Archive)?;
        file.sync_all().map_err(UnpackError::Cache)?;
        written.files.push(slashed(&relative));
    }
    // Gone, if the removal of stale temporaries or the GC took it meanwhile: what is in it is not
    // whole.
    if let Some(temp) = temp {
        note_in_use(temp)?;
    }
    let mut files = written.files;
    files.sort_unstable();
    Ok(Listing {
        files,
        bytes: written.bytes,
    })
}

/// Makes the directory `relative` under `temp`, and those that it is in, but not `temp`: one that
/// was taken away, as by the GC with its place, is not made again, so that what was written in it
/// is not taken for whole.
#[expect(
    clippy::create_dir,
    reason = "each directory is made under one that must be there"
)]
fn make_below(temp: &Path, relative: &Path) -> io::Result<()> {
    let mut dir = temp.to_path_buf();
    for component in relative.components() {
        dir.push(component);
        match fs::create_dir(&dir) {
            Ok(()) => {},
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// What an unpacking has written so far.
#[derive(Default)]
struct Written {
    /// The files, relative with `/`.
    files: Vec<String>,
    /// Their bytes.
    bytes: u64,
    /// Each path written, a directory or a file, by its key: its text, in lowercase where the file
    /// system does not tell case.
    paths: HashMap<String, (String, bool)>,
}

impl Written {
    /// Takes the path `relative` for a file (`is_file`) or a directory, and those of the
    /// directories that it is in. Refused, with why: a path through a file, a file where a
    /// directory or another file is, or a path that differs from another only in case where the
    /// file system does not tell case.
    fn claim(&mut self, relative: &Path, is_file: bool) -> Result<(), &'static str> {
        let mut path = String::new();
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(&component.as_os_str().to_string_lossy());
            let last = components.peek().is_none();
            let file = last && is_file;
            match self.paths.get(&case_key(&path)) {
                None => {
                    self.paths.insert(case_key(&path), (path.clone(), file));
                },
                Some((seen, _)) if *seen != path => {
                    return Err("a path that differs from another only in case");
                },
                Some((_, true)) if !last => return Err("a path through a file"),
                Some((_, seen_file)) if last && (file || *seen_file) => {
                    return Err("a path that another entry has");
                },
                Some(_) => {},
            }
        }
        Ok(())
    }

    /// Counts a file of `size` bytes, within `max_bytes` and [`MAX_FILES`].
    const fn count(&mut self, size: u64, max_bytes: u64) -> Result<(), UnpackError> {
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > max_bytes {
            return Err(UnpackError::TooLarge { limit: max_bytes });
        }
        if self.files.len() >= MAX_FILES {
            return Err(UnpackError::TooManyFiles { limit: MAX_FILES });
        }
        Ok(())
    }
}

/// The key of `path` among those written: in lowercase where the file system does not tell case,
/// as on Windows and macOS by default.
fn case_key(path: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

/// Notes in the temporary directory `temp` that it is in use, by making and removing a file in it,
/// which makes its time new; and checks that it is still there.
fn note_in_use(temp: &Path) -> Result<(), UnpackError> {
    let note = temp.join(IN_USE);
    File::create(&note)
        .and_then(|_| fs::remove_file(&note))
        .map_err(UnpackError::Cache)
}

/// The path of an entry `raw` relative to the package's root, without `prefix/`; `None` for the
/// package's directory itself. Refused, with why: a path that is not UTF-8, absolute, with a NUL,
/// with an empty component, `.` or `..`, outside `prefix/`, or with a name that this OS cannot
/// hold.
fn relative_path(raw: &[u8], prefix: &str) -> Result<Option<PathBuf>, &'static str> {
    let text = std::str::from_utf8(raw).map_err(|_not_utf8| "a path that is not UTF-8")?;
    if text.starts_with('/') {
        return Err("an absolute path");
    }
    if text.contains('\0') {
        return Err("a path with a NUL");
    }
    // A directory's path may end with `/`.
    let text = text.strip_suffix('/').unwrap_or(text);
    let mut components = text.split('/');
    if components.next() != Some(prefix) {
        return Err("a path outside the package's directory");
    }
    let mut relative = PathBuf::new();
    for component in components {
        match component {
            "" => return Err("a path with an empty component"),
            "." | ".." => return Err("a path with . or .."),
            name if !holdable(name) => return Err("a name that this OS cannot hold"),
            name => relative.push(name),
        }
    }
    Ok((!relative.as_os_str().is_empty()).then_some(relative))
}

/// Whether this OS can hold a file named `name` as it is (decided in M3b, B1-4): on Windows, not a
/// reserved device name, nor one with a character that Windows does not take or that ends with a
/// dot or a space; elsewhere, any name.
#[cfg(windows)]
fn holdable(name: &str) -> bool {
    const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let numbered = |device: &str| {
        stem.get(..3)
            .is_some_and(|head| head.eq_ignore_ascii_case(device))
            && stem.get(3..).is_some_and(|digit| {
                matches!(
                    digit,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
    };
    !(RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
        || numbered("COM")
        || numbered("LPT")
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')))
}

/// Whether this OS can hold a file named `name` as it is: any name, here.
#[cfg(not(windows))]
const fn holdable(_name: &str) -> bool {
    true
}

/// `path`, relative, with `/` between its components.
fn slashed(path: &Path) -> String {
    let parts: Vec<_> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect();
    parts.join("/")
}

/// An entry's path for an error: lossy, shortened to 200 characters, and with what would not be
/// seen marked, for the log and the command line as much as for an answer.
fn shortened(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut short: String = text.chars().take(200).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    super::hidden::mark(&short).0.into_owned()
}

#[cfg(test)]
mod tests {
    use flate2::{Compression, write::GzEncoder};

    use super::*;

    /// An entry of a test archive: its path as raw bytes, its type and its contents.
    struct Entry<'a> {
        path: &'a [u8],
        kind: tar::EntryType,
        contents: &'a [u8],
        link: Option<&'a str>,
    }

    /// A regular file at `path` with `contents`.
    const fn file<'a>(path: &'a str, contents: &'a [u8]) -> Entry<'a> {
        Entry {
            path: path.as_bytes(),
            kind: tar::EntryType::Regular,
            contents,
            link: None,
        }
    }

    /// A gzipped tar of `entries`, whose paths are written as they are, not checked by the builder.
    fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(entry.kind);
            header.set_size(entry.contents.len().try_into().expect("small"));
            header.set_mode(0o644);
            if let Some(link) = entry.link {
                header.set_link_name(link).expect("a link name");
            }
            if entry.path.len() < 100 {
                let name = &mut header.as_old_mut().name;
                name[..entry.path.len()].copy_from_slice(entry.path);
                header.set_cksum();
                builder.append(&header, entry.contents).expect("appended");
            } else {
                let path = std::str::from_utf8(entry.path).expect("UTF-8");
                builder
                    .append_data(&mut header, path, entry.contents)
                    .expect("appended");
            }
        }
        builder
            .into_inner()
            .and_then(GzEncoder::finish)
            .expect("finished")
    }

    /// What unpacking `entries` gives, into a new directory.
    fn unpacked(
        entries: &[Entry<'_>],
        max_bytes: u64,
    ) -> (tempfile::TempDir, Result<Listing, UnpackError>) {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let into = dir.path().join("demo/0.1.0/src.v1");
        let result = unpack(
            Packed::Gzip(&archive(entries)),
            "demo-0.1.0",
            &into,
            max_bytes,
        );
        (dir, result)
    }

    /// The names in the place of the test's `.crate`, its temporaries among them.
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir.join("demo/0.1.0"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_crate_is_unpacked_without_its_directory() {
        let (dir, result) = unpacked(
            &[
                Entry {
                    path: b"demo-0.1.0/",
                    kind: tar::EntryType::Directory,
                    contents: b"",
                    link: None,
                },
                file("demo-0.1.0/Cargo.toml", b"[package]\n"),
                file("demo-0.1.0/src/lib.rs", b"pub fn hi() {}\n"),
                file(
                    &format!("demo-0.1.0/{}/deep.rs", "long".repeat(40)),
                    b"// deep\n",
                ),
            ],
            1024 * 1024,
        );
        let unpacked = result.expect("unpacked");
        let long = format!("{}/deep.rs", "long".repeat(40));
        assert_eq!(unpacked.files, [
            "Cargo.toml".to_owned(),
            long.clone(),
            "src/lib.rs".to_owned()
        ]);
        assert_eq!(unpacked.bytes, 10 + 15 + 8);
        let root = dir.path().join("demo/0.1.0/src.v1");
        assert_eq!(
            fs::read_to_string(root.join("src/lib.rs")).expect("read"),
            "pub fn hi() {}\n"
        );
        assert!(root.join(long).is_file());
        assert_eq!(names_in(dir.path()), ["src.v1"]);
    }

    /// What `cargo package` does not make refuses the whole `.crate`, and leaves nothing behind
    /// (decided in M3b, B1-4).
    #[test]
    fn a_crate_with_a_trap_is_refused_whole() {
        let traps: [(&str, Entry<'_>); 8] = [
            ("..", file("demo-0.1.0/../escape.rs", b"x")),
            ("absolute", file("/etc/escape.rs", b"x")),
            ("outside", file("other-0.1.0/lib.rs", b"x")),
            ("empty", file("demo-0.1.0/src//lib.rs", b"x")),
            ("symlink", Entry {
                path: b"demo-0.1.0/link",
                kind: tar::EntryType::Symlink,
                contents: b"",
                link: Some("/etc/passwd"),
            }),
            ("hardlink", Entry {
                path: b"demo-0.1.0/hard",
                kind: tar::EntryType::Link,
                contents: b"",
                link: Some("demo-0.1.0/Cargo.toml"),
            }),
            ("device", Entry {
                path: b"demo-0.1.0/dev",
                kind: tar::EntryType::Char,
                contents: b"",
                link: None,
            }),
            ("not UTF-8", Entry {
                path: b"demo-0.1.0/\xFF.rs",
                kind: tar::EntryType::Regular,
                contents: b"x",
                link: None,
            }),
        ];
        for (what, trap) in traps {
            let (dir, result) =
                unpacked(&[file("demo-0.1.0/Cargo.toml", b"[package]\n"), trap], 1024);
            assert!(
                matches!(result, Err(UnpackError::Refused { .. })),
                "{what}: {result:?}"
            );
            assert_eq!(names_in(dir.path()), Vec::<String>::new(), "{what}");
        }
        let (dir, result) = unpacked(
            &[
                file("demo-0.1.0/Cargo.toml", b"one"),
                file("demo-0.1.0/Cargo.toml", b"two"),
            ],
            1024,
        );
        assert!(
            matches!(result, Err(UnpackError::Refused { .. })),
            "{result:?}"
        );
        assert_eq!(names_in(dir.path()), Vec::<String>::new());
    }

    /// The limits are checked before anything is written.
    #[test]
    fn a_crate_over_the_limits_is_refused_before_it_is_written() {
        let (dir, result) = unpacked(
            &[
                file("demo-0.1.0/a.rs", &[b'a'; 600]),
                file("demo-0.1.0/b.rs", &[b'b'; 600]),
            ],
            1_000,
        );
        assert!(
            matches!(result, Err(UnpackError::TooLarge { limit: 1_000 })),
            "{result:?}"
        );
        assert_eq!(names_in(dir.path()), Vec::<String>::new());
        let long = vec![b'x'; usize::try_from(MAX_EXTENSION_BYTES).expect("small") + 1];
        let (_, result) = unpacked(
            &[Entry {
                path: b"././@LongLink",
                kind: tar::EntryType::GNULongName,
                contents: &long,
                link: None,
            }],
            1024 * 1024,
        );
        assert!(
            matches!(result, Err(UnpackError::Refused { .. })),
            "{result:?}"
        );
    }

    /// A place unpacked before is replaced whole.
    #[test]
    fn unpacking_again_replaces_the_place() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let into = dir.path().join("demo/0.1.0/src.v1");
        fs::create_dir_all(&into).expect("made");
        fs::write(into.join("left.rs"), "").expect("written");
        unpack(
            Packed::Gzip(&archive(&[file("demo-0.1.0/Cargo.toml", b"[package]\n")])),
            "demo-0.1.0",
            &into,
            1024,
        )
        .expect("unpacked");
        assert!(!into.join("left.rs").exists());
        assert!(into.join("Cargo.toml").is_file());
    }

    /// A name that Windows cannot hold is refused there only (decided in M3b, B1-4).
    #[test]
    fn a_name_that_this_os_cannot_hold_is_refused() {
        for name in ["con.rs", "aux", "a.", "a ", "a:b", "a\\b", "a*b"] {
            assert_eq!(holdable(name), !cfg!(windows), "{name}");
        }
        for name in ["lib.rs", "console.rs", "a.b"] {
            assert!(holdable(name), "{name}");
        }
    }

    /// A PAX extension header with the records `records`, as `key=value`.
    fn pax(records: &[(&str, &str)]) -> Vec<u8> {
        let mut data = Vec::new();
        for (key, value) in records {
            let body = format!(" {key}={value}\n");
            // The length counts its own digits.
            let mut length = body.len().saturating_add(1);
            while format!("{length}{body}").len() != length {
                length = length.saturating_add(1);
            }
            data.extend_from_slice(format!("{length}{body}").as_bytes());
        }
        data
    }

    /// A PAX extension that sets a size, which the first pass would not read as the second does,
    /// refuses the `.crate`; one that sets a path goes through the same checks as any path.
    #[test]
    fn a_pax_extension_is_checked() {
        let size = pax(&[("size", "10000000000")]);
        let (_, result) = unpacked(
            &[
                Entry {
                    path: b"././@PaxHeader",
                    kind: tar::EntryType::XHeader,
                    contents: &size,
                    link: None,
                },
                file("demo-0.1.0/a.rs", b""),
            ],
            1024,
        );
        assert!(
            matches!(result, Err(UnpackError::Refused { why, .. }) if why.contains("PAX")),
            "{result:?}"
        );
        let path = pax(&[("path", "demo-0.1.0/../escape.rs")]);
        let (dir, result) = unpacked(
            &[
                Entry {
                    path: b"././@PaxHeader",
                    kind: tar::EntryType::XHeader,
                    contents: &path,
                    link: None,
                },
                file("demo-0.1.0/a.rs", b"x"),
            ],
            1024,
        );
        assert!(
            matches!(result, Err(UnpackError::Refused { ref entry, .. }) if entry.contains("escape")),
            "{result:?}"
        );
        assert!(!dir.path().join("demo/0.1.0/escape.rs").exists());
    }

    /// A GNU long name goes through the same checks as any path.
    #[test]
    fn a_long_name_is_checked() {
        let name = format!("demo-0.1.0/{}/../../../escape.rs\0", "long".repeat(30));
        let (dir, result) = unpacked(
            &[
                Entry {
                    path: b"././@LongLink",
                    kind: tar::EntryType::GNULongName,
                    contents: name.as_bytes(),
                    link: None,
                },
                file("demo-0.1.0/short.rs", b"x"),
            ],
            1024,
        );
        assert!(
            matches!(result, Err(UnpackError::Refused { .. })),
            "{result:?}"
        );
        assert!(!dir.path().join("escape.rs").exists());
    }

    /// A path through a file, or a directory where a file is, refuses the `.crate`.
    #[test]
    fn a_path_through_a_file_is_refused() {
        let directory = Entry {
            path: b"demo-0.1.0/a/",
            kind: tar::EntryType::Directory,
            contents: b"",
            link: None,
        };
        for (what, entries) in [
            ("through", [
                file("demo-0.1.0/a", b"x"),
                file("demo-0.1.0/a/b", b"y"),
            ]),
            ("over", [file("demo-0.1.0/a", b"x"), directory]),
        ] {
            let (_, result) = unpacked(&entries, 1024);
            assert!(
                matches!(result, Err(UnpackError::Refused { .. })),
                "{what}: {result:?}"
            );
        }
    }

    /// Paths that differ only in case are refused where the file system does not tell case, and
    /// a NUL anywhere in a path is refused.
    #[test]
    fn paths_that_the_file_system_would_mix_are_refused() {
        let mut written = Written::default();
        written.claim(Path::new("src/a.rs"), true).expect("claimed");
        let other = written.claim(Path::new("Src/b.rs"), true);
        assert_eq!(other.is_err(), cfg!(any(windows, target_os = "macos")));
        assert_eq!(
            relative_path(b"demo-0.1.0/a\0b.rs", "demo-0.1.0"),
            Err("a path with a NUL")
        );
    }

    /// A client that sends no request, with `cargo_home`.
    fn offline(cargo_home: Option<PathBuf>) -> StaticCrates {
        let network = super::super::http::Network {
            offline: true,
            ..Default::default()
        };
        StaticCrates::with_base_url("http://127.0.0.1:9", &network)
            .expect("a client")
            .with_cargo_home(cargo_home)
    }

    /// The release of the tests.
    fn demo() -> (CrateName, Version) {
        ("demo".parse().expect("a name"), Version::new(0, 1, 0))
    }

    /// The cache at `root`, with `contents` as the `.crate` of the release of the tests.
    fn cache_with(root: &Path, contents: &[u8]) -> Cache {
        let cache = Cache::at(root);
        let (name, version) = demo();
        cache
            .write_verified(&cache.crate_file_path(&name, &version), contents)
            .expect("written");
        cache
    }

    /// The `.crate` of the release of the tests: `Cargo.toml` and `src/lib.rs`.
    fn demo_crate() -> Vec<u8> {
        crate_file::tests::crate_file("demo", "0.1.0", "[package]\n")
    }

    /// The sources of the release of the tests, as `sources` has them.
    async fn sources_of(crates: &StaticCrates, cache: &Cache) -> Result<Sources, SourcesError> {
        let (name, version) = demo();
        sources(crates, cache, &name, &version, None, 1024 * 1024).await
    }

    /// Without cargo's, the `.crate` is unpacked into the cache once, with its listing beside it;
    /// then they are read without the `.crate` (decided in M3b, B1-1).
    #[tokio::test]
    async fn the_sources_are_unpacked_into_the_cache_once() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let cache = cache_with(dir.path(), &demo_crate());
        let crates = offline(None);
        let found = sources_of(&crates, &cache).await.expect("unpacked");
        let place = dir.path().join("static.crates.io/demo/0.1.0");
        assert_eq!(found, Sources {
            root: place.join("src.v1"),
            listing: Listing {
                files: vec!["Cargo.toml".to_owned(), "src/lib.rs".to_owned()],
                bytes: 10,
            },
            origin: Origin::Cache,
        });
        assert!(place.join("src.v1/src/lib.rs").is_file());
        assert!(place.join("src.v1.json").is_file());
        fs::remove_file(place.join("demo-0.1.0.crate")).expect("removed");
        assert_eq!(sources_of(&crates, &cache).await.expect("found"), found);
    }

    /// Two calls at once unpack it once, and both have it.
    #[tokio::test]
    async fn two_calls_at_once_have_the_same_sources() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let cache = cache_with(dir.path(), &demo_crate());
        let crates = offline(None);
        let (one, two) = tokio::join!(sources_of(&crates, &cache), sources_of(&crates, &cache));
        assert_eq!(one.expect("unpacked"), two.expect("unpacked"));
        // The note of its use is written later, on its own.
        let mut names = names_in(&dir.path().join("static.crates.io"));
        names.retain(|name| name != used::USED);
        assert_eq!(names, [
            "demo-0.1.0.crate",
            "demo-0.1.0.crate.sha256",
            "src.v1",
            "src.v1.json",
            "src.v1.json.sha256",
        ]);
    }

    /// What cargo finished unpacking from the sparse index of crates.io is read where it is, by
    /// the listing of the `.crate`, without what a build script wrote there; what it did not
    /// finish, or had from a git index, which may be another registry's, is not read, and the
    /// `.crate` is unpacked into the cache (decided in M3b, B1-7).
    #[tokio::test]
    async fn what_cargo_unpacked_is_read_by_the_listing() {
        const SPARSE: &str = "index.crates.io-1949cf8c6b5b557f";
        for (registry, cargo_ok, origin) in [
            (SPARSE, Some(r#"{"v":1}"#), Origin::Cargo),
            (SPARSE, Some(r#"{"v":2}"#), Origin::Cargo),
            (SPARSE, Some(r#"{"v":0}"#), Origin::Cache),
            (SPARSE, Some("ok"), Origin::Cache),
            (SPARSE, Some(""), Origin::Cache),
            (SPARSE, None, Origin::Cache),
            (
                "github.com-1ecc6299db9ec823",
                Some(r#"{"v":1}"#),
                Origin::Cache,
            ),
        ] {
            let dir = tempfile::TempDir::new().expect("a temporary directory");
            let home = dir.path().join("cargo-home");
            let unpacked = home.join("registry/src").join(registry).join("demo-0.1.0");
            fs::create_dir_all(unpacked.join("src")).expect("made");
            fs::write(unpacked.join("Cargo.toml"), "[package]\n").expect("written");
            fs::write(unpacked.join("src/lib.rs"), "").expect("written");
            fs::write(unpacked.join("written.rs"), "").expect("written");
            if let Some(cargo_ok) = cargo_ok {
                fs::write(unpacked.join(".cargo-ok"), cargo_ok).expect("written");
            }
            let cache = cache_with(&dir.path().join("cache"), &demo_crate());
            let found = sources_of(&offline(Some(home)), &cache)
                .await
                .expect("found");
            assert_eq!(found.origin, origin, "{registry} {cargo_ok:?}");
            assert_eq!(found.listing.files, ["Cargo.toml", "src/lib.rs"]);
            let place = dir.path().join("cache/static.crates.io/demo/0.1.0");
            assert!(place.join("src.v1.json").is_file());
            if origin == Origin::Cargo {
                assert_eq!(found.root, unpacked);
                assert!(!place.join("src.v1").exists());
            } else {
                assert_eq!(found.root, place.join("src.v1"));
            }
        }
    }

    /// A `.cargo-ok` that is a symlink is not followed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_for_cargo_ok_is_not_followed() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let home = dir.path().join("cargo-home");
        let unpacked = home.join("registry/src/index.crates.io-1949cf8c6b5b557f/demo-0.1.0");
        fs::create_dir_all(&unpacked).expect("made");
        fs::write(dir.path().join("ok"), r#"{"v":1}"#).expect("written");
        std::os::unix::fs::symlink(dir.path().join("ok"), unpacked.join(".cargo-ok"))
            .expect("linked");
        let cache = cache_with(&dir.path().join("cache"), &demo_crate());
        let found = sources_of(&offline(Some(home)), &cache)
            .await
            .expect("found");
        assert_eq!(found.origin, Origin::Cache);
    }

    /// What is unpacked without its listing, as when the listing was not written after it, is
    /// listed and not unpacked again; a listing without what is unpacked is unpacked again; one of
    /// more than the limit is refused.
    #[tokio::test]
    async fn the_listing_and_what_is_unpacked_are_had_apart() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let cache = cache_with(dir.path(), &demo_crate());
        let crates = offline(None);
        let place = dir.path().join("static.crates.io/demo/0.1.0");
        let found = sources_of(&crates, &cache).await.expect("unpacked");
        fs::write(place.join("src.v1/kept.rs"), "").expect("written");
        fs::remove_file(place.join("src.v1.json")).expect("removed");
        assert_eq!(sources_of(&crates, &cache).await.expect("listed"), found);
        assert!(place.join("src.v1/kept.rs").is_file(), "not unpacked again");
        assert!(place.join("src.v1.json").is_file());
        fs::remove_dir_all(place.join("src.v1")).expect("removed");
        assert_eq!(sources_of(&crates, &cache).await.expect("unpacked"), found);
        assert!(place.join("src.v1/src/lib.rs").is_file());
        let (name, version) = demo();
        assert!(matches!(
            sources(&crates, &cache, &name, &version, None, 9).await,
            Err(SourcesError::Unpack(UnpackError::TooLarge { limit: 9 }))
        ));
    }

    /// A temporary directory taken away while it is written, as by the GC with its place, is not
    /// made again.
    #[test]
    fn a_temporary_directory_taken_away_is_not_made_again() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let temp = dir.path().join(".src.v1.1.0.tmp");
        let error = make_below(&temp, Path::new("src/bin")).expect_err("not made");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!temp.exists());
        fs::create_dir_all(&temp).expect("made");
        make_below(&temp, Path::new("src/bin")).expect("made");
        make_below(&temp, Path::new("src")).expect("there already");
        assert!(temp.join("src/bin").is_dir());
    }

    /// Cargo's home with the release of the tests unpacked in it, finished, and its `.crate` in
    /// cargo's cache if `cached`; and the directory of what cargo unpacked.
    fn cargo_home_with(root: &Path, cached: bool) -> (PathBuf, PathBuf) {
        let home = root.join("cargo-home");
        let unpacked = home.join("registry/src/index.crates.io-1949cf8c6b5b557f/demo-0.1.0");
        fs::create_dir_all(unpacked.join("src")).expect("made");
        fs::write(unpacked.join("Cargo.toml"), "[package]\n").expect("written");
        fs::write(unpacked.join("src/lib.rs"), "").expect("written");
        fs::write(unpacked.join(".cargo-ok"), r#"{"v":1}"#).expect("written");
        if cached {
            let cache = home.join("registry/cache/index.crates.io-1949cf8c6b5b557f");
            fs::create_dir_all(&cache).expect("made");
            fs::write(cache.join("demo-0.1.0.crate"), demo_crate()).expect("written");
        }
        (home, unpacked)
    }

    /// What cargo unpacked is had with the listing of the `.crate` at hand, cargo's or the
    /// cache's, which is kept; without one at hand, or unfinished, or of another registry, nothing
    /// is had, and nothing is fetched (decided in M3b, B5b and decision 19).
    #[test]
    fn what_cargo_unpacked_is_had_without_fetching() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let (home, unpacked) = cargo_home_with(dir.path(), true);
        let cache = Cache::at(&dir.path().join("cache"));
        let found = unpacked_at(&cache, &home, &unpacked, 1024 * 1024).expect("listed");
        assert_eq!(
            (found.root.as_path(), found.origin),
            (unpacked.as_path(), Origin::Cargo)
        );
        assert_eq!(found.listing.files, ["Cargo.toml", "src/lib.rs"]);
        let listing = dir
            .path()
            .join("cache/static.crates.io/demo/0.1.0/src.v1.json");
        assert!(listing.is_file());
        // Kept: had again without cargo's `.crate`.
        fs::remove_dir_all(home.join("registry/cache")).expect("removed");
        assert_eq!(
            unpacked_at(&cache, &home, &unpacked, 1024 * 1024),
            Some(found)
        );
        // Without a `.crate` at hand; with the cache's.
        let (home, unpacked) = cargo_home_with(&dir.path().join("other"), false);
        let other = Cache::at(&dir.path().join("other-cache"));
        assert_eq!(unpacked_at(&other, &home, &unpacked, 1024 * 1024), None);
        let other = cache_with(&dir.path().join("other-cache"), &demo_crate());
        assert!(unpacked_at(&other, &home, &unpacked, 1024 * 1024).is_some());
        // Unfinished, or of another registry.
        fs::write(unpacked.join(".cargo-ok"), "ok").expect("written");
        let third = Cache::at(&dir.path().join("third"));
        assert_eq!(unpacked_at(&third, &home, &unpacked, 1024 * 1024), None);
        let git = home.join("registry/src/github.com-1ecc6299db9ec823/demo-0.1.0");
        fs::create_dir_all(&git).expect("made");
        fs::write(git.join(".cargo-ok"), r#"{"v":1}"#).expect("written");
        assert_eq!(unpacked_at(&third, &home, &git, 1024 * 1024), None);
    }

    /// A listing made from cargo's `.crate`, whose checksum was not checked, is kept noted so,
    /// and `sources`, that checks it, makes it again; one whose lock another holds is not waited
    /// for (decided in M3b, B5b).
    #[tokio::test]
    async fn a_listing_of_cargos_crate_is_made_again_when_checked() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let (home, unpacked) = cargo_home_with(dir.path(), true);
        let cache = Cache::at(&dir.path().join("cache"));
        let place = dir.path().join("cache/static.crates.io/demo/0.1.0");
        let held = cache.try_lock(&place.join("src.v1")).expect("free");
        assert_eq!(unpacked_at(&cache, &home, &unpacked, 1024 * 1024), None);
        drop(held);
        let had = unpacked_at(&cache, &home, &unpacked, 1024 * 1024).expect("listed");
        let kept = || fs::read_to_string(place.join("src.v1.json")).expect("kept");
        assert!(kept().contains("\"unchecked\":true"), "{}", kept());
        let checksum = cache::sha256_hex(&demo_crate());
        let (name, version) = demo();
        let found = sources(
            &offline(Some(home)),
            &cache,
            &name,
            &version,
            Some(&checksum),
            1024 * 1024,
        )
        .await
        .expect("checked");
        assert_eq!(found, had);
        assert!(!kept().contains("unchecked"), "{}", kept());
    }

    /// Cargo's `.crate` that is a symlink is not read.
    #[cfg(unix)]
    #[test]
    fn cargos_crate_is_not_read_through_a_symlink() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let real = dir.path().join("real.crate");
        fs::write(&real, demo_crate()).expect("written");
        let linked = dir.path().join("demo-0.1.0.crate");
        std::os::unix::fs::symlink(&real, &linked).expect("linked");
        assert_eq!(read_cargos_crate(&linked), None);
        assert_eq!(read_cargos_crate(&real).as_deref(), Some(&*demo_crate()));
    }

    /// The name and the version of a package from its directory's name, as cargo names it.
    #[test]
    fn a_package_is_named_by_its_directory() {
        let named = |package: &str| {
            package_name(package).map(|(name, version)| (name.to_string(), version.to_string()))
        };
        for (package, name, version) in [
            ("demo-0.1.0", "demo", "0.1.0"),
            ("serde-json-1.0.0", "serde-json", "1.0.0"),
            ("a-b-1.0.0-beta.1", "a-b", "1.0.0-beta.1"),
            (
                "windows-sys-0.59.0+build.1",
                "windows-sys",
                "0.59.0+build.1",
            ),
        ] {
            assert_eq!(
                named(package),
                Some((name.to_owned(), version.to_owned())),
                "{package}"
            );
        }
        for package in ["demo", "demo-", "0.1.0", "demo-x.y"] {
            assert_eq!(named(package), None, "{package}");
        }
    }

    /// Without the `.crate` at hand nothing is had, offline or without its checksum; a `.crate`
    /// that is refused leaves nothing (decided in M3b, B1-4 and B2-3).
    #[tokio::test]
    async fn without_a_crate_that_can_be_unpacked_nothing_is_had() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let crates = offline(None);
        let (name, version) = demo();
        assert!(matches!(
            sources_of(&crates, &cache).await,
            Err(SourcesError::NotCached)
        ));
        assert!(matches!(
            sources(&crates, &cache, &name, &version, Some("00"), 1024).await,
            Err(SourcesError::Crate(CrateError::Offline))
        ));
        let cache = cache_with(dir.path(), &archive(&[file("demo-0.1.0/../x.rs", b"")]));
        assert!(matches!(
            sources_of(&crates, &cache).await,
            Err(SourcesError::Unpack(UnpackError::Refused { .. }))
        ));
        assert_eq!(names_in(&dir.path().join("static.crates.io")), [
            "demo-0.1.0.crate",
            "demo-0.1.0.crate.sha256",
        ]);
    }

    /// A temporary directory of the same name, which a stopped process of the same id left, is
    /// emptied, not added to.
    #[test]
    fn a_temporary_directory_left_there_is_emptied() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let temp = dir.path().join(".src.v1.1.0.tmp");
        fs::create_dir_all(&temp).expect("made");
        fs::write(temp.join("left.rs"), "").expect("written");
        let unpacked = walk(
            Packed::Gzip(&archive(&[file("demo-0.1.0/Cargo.toml", b"[package]\n")])),
            "demo-0.1.0",
            Some(&temp),
            1024,
        )
        .expect("unpacked");
        assert_eq!(unpacked.files, ["Cargo.toml"]);
        assert!(!temp.join("left.rs").exists());
        assert!(!temp.join(IN_USE).exists());
    }
}
