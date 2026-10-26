//! The sources of a crates.io release, unpacked from its `.crate` into the cache (requirements
//! R-SC-2 and R-NW-3, decided in M3b, B1).
//!
//! A `.crate` is a third party's archive. Before anything is written, a first pass over its raw
//! entries checks how much it holds, as the `tar` crate reads a GNU long name or a PAX extension
//! whole into memory, and refuses a PAX extension that would make the second pass read the
//! entries otherwise than the first; then each entry is checked and written into a temporary
//! directory next to the place, which is renamed into it once every file is on the disk.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use flate2::read::GzDecoder;

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

/// What was unpacked: the files, relative to the package's root with `/`, sorted, and their bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Unpacked {
    /// The files.
    pub files: Vec<String>,
    /// Their bytes in all.
    pub bytes: u64,
}

/// Unpacks the gzipped tar `contents` into the directory `into`, which is replaced.
///
/// Its entries are all under `prefix/` (`<name>-<version>`), which is left out, so that `into` is
/// the package's root. More than `max_bytes` in all, or more than [`MAX_FILES`] files, is an
/// error, before anything is written. The caller has checked `contents` against its checksum, and
/// removes its own mark that `into` is complete before this, as `into` may be left part removed
/// when it cannot be replaced.
///
/// It reads and writes files, so it is for a blocking thread, with the lock of the place held.
///
/// # Errors
///
/// Returns an error when an entry is refused, a limit is passed, the archive cannot be read, or the
/// cache cannot be written; what was at `into` is left then, unless it could not be moved away.
pub fn unpack(
    contents: &[u8],
    prefix: &str,
    into: &Path,
    max_bytes: u64,
) -> Result<Unpacked, UnpackError> {
    check_sizes(contents, max_bytes)?;
    let parent = into.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(UnpackError::Cache)?;
    let temp = temporary(into);
    let unpacked = unpack_into(contents, prefix, &temp, max_bytes)
        .and_then(|unpacked| replace(into, &temp).map(|()| unpacked));
    if unpacked.is_err() {
        remove_quietly(&temp);
    }
    if unpacked.is_ok() {
        super::cache::remove_stale_temporaries(parent);
    }
    unpacked
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

/// `contents` decompressed, up to what `max_bytes` of entries with their headers can take: past
/// that, the archive reads as cut short.
fn decompressed(contents: &[u8], max_bytes: u64) -> io::Take<GzDecoder<&[u8]>> {
    let headers = u64::try_from(MAX_ENTRIES)
        .unwrap_or(u64::MAX)
        .saturating_mul(1024);
    GzDecoder::new(contents).take(max_bytes.saturating_add(headers))
}

/// Checks, over the raw entries of `contents`, that no extension header is larger than
/// [`MAX_EXTENSION_BYTES`] nor changes the size of an entry, that there are not too many entries,
/// and that what they hold is within `max_bytes`; before a long name is read into memory, or a
/// file is written.
fn check_sizes(contents: &[u8], max_bytes: u64) -> Result<(), UnpackError> {
    let mut archive = tar::Archive::new(decompressed(contents, max_bytes));
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
/// synced to the disk (requirement R-CA-4). The limits are checked again as it goes.
#[expect(
    clippy::create_dir,
    reason = "the temporary directory is made alone, so that one left there is found"
)]
fn unpack_into(
    contents: &[u8],
    prefix: &str,
    temp: &Path,
    max_bytes: u64,
) -> Result<Unpacked, UnpackError> {
    if let Err(error) = fs::create_dir(temp) {
        // Left by a process of the same id that stopped: its name is this unpacking's now.
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(UnpackError::Cache(error));
        }
        fs::remove_dir_all(temp)
            .and_then(|()| fs::create_dir(temp))
            .map_err(UnpackError::Cache)?;
    }
    let mut archive = tar::Archive::new(decompressed(contents, max_bytes));
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
        let path = temp.join(&relative);
        if !is_file {
            fs::create_dir_all(&path).map_err(UnpackError::Cache)?;
            continue;
        }
        let size = entry.size();
        written.count(size, max_bytes)?;
        if written.files.len() % FILES_PER_NOTE == 0 {
            note_in_use(temp)?;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(UnpackError::Cache)?;
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
    // Gone, if the removal of stale temporaries took it meanwhile: what is in it is not whole.
    note_in_use(temp)?;
    let mut files = written.files;
    files.sort_unstable();
    Ok(Unpacked {
        files,
        bytes: written.bytes,
    })
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
    ) -> (tempfile::TempDir, Result<Unpacked, UnpackError>) {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let into = dir.path().join("demo/0.1.0/src.v1");
        let result = unpack(&archive(entries), "demo-0.1.0", &into, max_bytes);
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
            &archive(&[file("demo-0.1.0/Cargo.toml", b"[package]\n")]),
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

    /// A temporary directory of the same name, which a stopped process of the same id left, is
    /// emptied, not added to.
    #[test]
    fn a_temporary_directory_left_there_is_emptied() {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let temp = dir.path().join(".src.v1.1.0.tmp");
        fs::create_dir_all(&temp).expect("made");
        fs::write(temp.join("left.rs"), "").expect("written");
        let unpacked = unpack_into(
            &archive(&[file("demo-0.1.0/Cargo.toml", b"[package]\n")]),
            "demo-0.1.0",
            &temp,
            1024,
        )
        .expect("unpacked");
        assert_eq!(unpacked.files, ["Cargo.toml"]);
        assert!(!temp.join("left.rs").exists());
        assert!(!temp.join(IN_USE).exists());
    }
}
