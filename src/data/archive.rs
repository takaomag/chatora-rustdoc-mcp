//! Archives in rkyv's format, read in place through mmap (requirements R-PF-1, R-CA-2 and
//! R-CA-4).
//!
//! An archive is derived from a source, such as a rustdoc JSON, and is safe to lose: when its
//! header does not match or its bytes do not check out, it is made again. The header says what it
//! holds and what it was made from: a magic number, the version of its schema, the series of rkyv
//! that wrote it, the format version of the source, and the SHA-256 of the source. The bytes after
//! it are checked once, when the archive is opened (bytecheck), and then read without copying.

use std::{fs::File, io, marker::PhantomData, path::Path};

use memmap2::Mmap;
use rkyv::{
    Archive,
    Portable,
    Serialize,
    api::high::{HighSerializer, HighValidator},
    bytecheck::CheckBytes,
    rancor,
    ser::allocator::ArenaHandle,
    util::AlignedVec,
};

use super::cache::{self, Cache};

/// The series of rkyv whose layout the archives have. A test checks it against Cargo.lock, so
/// that an update of rkyv that changes the layout cannot slip by (requirement R-CA-2).
pub const RKYV_SERIES: (u16, u16) = (0, 8);

/// The magic number that starts every archive.
const MAGIC: [u8; 8] = *b"CHRDMCPA";

/// The length of the header. A multiple of 16, so that the archive after it keeps the alignment
/// that mmap gives.
const HEADER_LEN: usize = 128;

/// The layout options of rkyv that the archives use: little endian, 32-bit relative pointers
/// (rkyv's defaults).
const LAYOUT: u32 = 1;

/// Largest archive that is mapped (rkyv's 32-bit relative pointers reach 2 GiB).
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// What an archive holds and was made from, which must match for it to be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The version of the schema of what is archived, changed with its types.
    pub schema: u32,
    /// The format version of the source.
    pub format_version: u32,
    /// The SHA-256 of the source, in lowercase hex.
    pub source_sha256: Box<str>,
}

impl Header {
    /// The bytes of the header.
    fn to_bytes(&self) -> Option<[u8; HEADER_LEN]> {
        let digest = self.source_sha256.as_bytes();
        if digest.len() != 64 {
            return None;
        }
        let mut bytes = [0; HEADER_LEN];
        let (major, minor) = RKYV_SERIES;
        let fields = [
            &MAGIC[..],
            &self.schema.to_le_bytes(),
            &major.to_le_bytes(),
            &minor.to_le_bytes(),
            &LAYOUT.to_le_bytes(),
            &self.format_version.to_le_bytes(),
            digest,
        ];
        let mut at: usize = 0;
        for field in fields {
            let end = at.checked_add(field.len())?;
            bytes.get_mut(at..end)?.copy_from_slice(field);
            at = end;
        }
        Some(bytes)
    }
}

/// Writes `value` to `path` in the cache, after `header`, replacing what was there.
///
/// # Errors
///
/// Returns an error when the header is invalid, `value` cannot be serialized, or the file cannot
/// be written.
pub fn write<T>(cache: &Cache, path: &Path, header: &Header, value: &T) -> io::Result<()>
where
    T: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>,
{
    cache.check_inside(path)?;
    let header = header
        .to_bytes()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "an invalid header"))?;
    let archived = rkyv::to_bytes::<rancor::Error>(value).map_err(io::Error::other)?;
    let mut contents = Vec::with_capacity(HEADER_LEN.saturating_add(archived.len()));
    contents.extend_from_slice(&header);
    contents.extend_from_slice(&archived);
    cache::write_atomically(path, &contents)
}

/// An archive of `T` whose bytes were checked when it was opened: a file mapped into memory, or
/// bytes held in memory when the file could not be written.
pub struct Mapped<T: Archive> {
    bytes: Bytes,
    archived: PhantomData<fn() -> T>,
}

/// Where the bytes of an archive are.
enum Bytes {
    /// A file of the cache, header first.
    File(Mmap),
    /// Memory, without a header.
    Memory(AlignedVec),
}

impl Bytes {
    /// The archived bytes, after the header if there is one.
    fn body(&self) -> &[u8] {
        match self {
            Self::File(mmap) => mmap.get(HEADER_LEN..).unwrap_or_default(),
            Self::Memory(bytes) => bytes,
        }
    }
}

impl<T: Archive> std::fmt::Debug for Mapped<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mapped")
            .field("len", &self.bytes.body().len())
            .field("in_memory", &matches!(self.bytes, Bytes::Memory(_)))
            .finish_non_exhaustive()
    }
}

impl<T> Mapped<T>
where
    T: Archive,
    T::Archived: Portable + for<'a> CheckBytes<HighValidator<'a, rancor::Error>>,
{
    /// Maps the archive at `path` in the cache, if there is one with `header` whose bytes check
    /// out. `None` when there is none, or it does not match or check out and is to be made again
    /// (requirement R-CA-4).
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be opened or mapped.
    pub fn open(cache: &Cache, path: &Path, header: &Header) -> io::Result<Option<Self>> {
        let Some(expected) = header.to_bytes() else {
            return Ok(None);
        };
        let Some(file) = cache.open_file(path, MAX_ARCHIVE_BYTES)? else {
            return Ok(None);
        };
        let mmap = map(&file)?;
        if mmap.get(..HEADER_LEN) != Some(&expected[..]) {
            tracing::debug!(path = %path.display(), "an archive with another header is made again");
            return Ok(None);
        }
        let bytes = Bytes::File(mmap);
        if let Err(error) = rkyv::access::<T::Archived, rancor::Error>(bytes.body()) {
            tracing::warn!(path = %path.display(), %error, "an archive that does not check out is made again");
            return Ok(None);
        }
        Ok(Some(Self {
            bytes,
            archived: PhantomData,
        }))
    }

    /// `value` archived in memory, for when its file cannot be written.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` cannot be serialized.
    pub fn in_memory(value: &T) -> io::Result<Self>
    where
        T: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>,
    {
        let bytes = rkyv::to_bytes::<rancor::Error>(value).map_err(io::Error::other)?;
        Ok(Self {
            bytes: Bytes::Memory(bytes),
            archived: PhantomData,
        })
    }

    /// The archived value.
    #[must_use]
    #[expect(
        unsafe_code,
        reason = "checking the bytes again on each access would cost what the archive saves"
    )]
    pub fn get(&self) -> &T::Archived {
        // SAFETY: `open` checked these bytes with `rkyv::access` for this type, or `in_memory`
        // made them for it; they are read-only and live as long as `self`.
        unsafe { rkyv::access_unchecked::<T::Archived>(self.bytes.body()) }
    }
}

/// Maps `file` read-only.
#[expect(unsafe_code, reason = "mapping a file is unsafe in memmap2's API")]
fn map(file: &File) -> io::Result<Mmap> {
    // SAFETY: the file is in the cache, which this server replaces only by renaming a new file
    // over it, so the mapped file is never written in place. Another process that writes to it in
    // place could change what is read, which is then checked by rkyv when the archive is opened,
    // or truncate it, which would end the process (SIGBUS); the cache is this server's own
    // directory (requirement R-SEC-2).
    unsafe { Mmap::map(file) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(format_version: u32) -> Header {
        Header {
            schema: 1,
            format_version,
            source_sha256: "ab".repeat(32).into(),
        }
    }

    #[test]
    fn an_archive_is_read_back_in_place() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let path = dir.path().join("x.rkyv");
        let value: Vec<Box<str>> = vec!["a".into(), "bc".into()];
        write(&cache, &path, &header(61), &value).expect("written");
        let mapped = Mapped::<Vec<Box<str>>>::open(&cache, &path, &header(61))
            .expect("opened")
            .expect("it matches");
        let archived = mapped.get();
        assert_eq!(archived.len(), 2);
        assert_eq!(archived.get(1).map(|text| &**text), Some("bc"));
        let in_memory = Mapped::<Vec<Box<str>>>::in_memory(&value).expect("archived");
        assert_eq!(in_memory.get().len(), 2);
    }

    #[test]
    fn an_archive_of_another_source_or_with_bad_bytes_is_not_used() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let path = dir.path().join("x.rkyv");
        let value: Vec<Box<str>> = vec!["a".into()];
        write(&cache, &path, &header(61), &value).expect("written");
        let open = |header: &Header| {
            Mapped::<Vec<Box<str>>>::open(&cache, &path, header)
                .expect("opened")
                .is_some()
        };
        assert!(!open(&header(60)), "another format");
        let mut other = header(61);
        other.source_sha256 = "cd".repeat(32).into();
        assert!(!open(&other), "another source");
        // The archive is cut short.
        let contents = std::fs::read(&path).expect("read");
        std::fs::write(
            &path,
            contents.get(..contents.len() - 4).expect("long enough"),
        )
        .expect("written");
        assert!(!open(&header(61)), "bytes that do not check out");
        assert!(
            Mapped::<Vec<Box<str>>>::open(&cache, &dir.path().join("none.rkyv"), &header(61))
                .expect("no error")
                .is_none()
        );
    }

    #[test]
    fn the_rkyv_series_is_the_one_in_cargo_lock() {
        let lock: toml::Table =
            toml::from_str(include_str!("../../Cargo.lock")).expect("Cargo.lock");
        let version = lock["package"]
            .as_array()
            .expect("packages")
            .iter()
            .find(|package| package["name"].as_str() == Some("rkyv"))
            .and_then(|package| package["version"].as_str())
            .expect("rkyv is locked");
        let version = semver::Version::parse(version).expect("a version");
        assert_eq!(
            (version.major, version.minor),
            (u64::from(RKYV_SERIES.0), u64::from(RKYV_SERIES.1)),
            "rkyv {version} may lay archives out differently: check, then update RKYV_SERIES"
        );
    }
}
