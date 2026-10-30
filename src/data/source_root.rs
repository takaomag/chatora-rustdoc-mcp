//! Which files of a crate's sources may be read, and where they are (requirement R-SC-4, decided
//! in M3b, B2): those of its own root only, checked in one place for every tool that reads them.
//!
//! A path asked for is resolved by its text first, and must stay under the root; no component may
//! be a dotfile; the file must be one of the package's, as the root's kind tells; then its real
//! path, through any symlink, must be under the root's real path, by the same rules, and a regular
//! file. In the cache, where this server writes no symlink, one is refused. The real path is what
//! is opened.

use std::{
    fs,
    io,
    path::{Component, Path, PathBuf},
};

use super::crate_source::{Origin, Sources};

/// A root of sources, and which files under it are the package's.
#[derive(Debug, Clone, Copy)]
pub enum Root<'a> {
    /// The sources of a crates.io release: the files of its `.crate`'s listing, where cargo
    /// unpacked them or in the cache (decided in M3b, B2-3).
    Release(&'a Sources),
}

/// A file that may be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Its real path, which is to be opened.
    pub path: PathBuf,
    /// Its path relative to the root, with `/`.
    pub relative: String,
}

/// Why a path is not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// It is not under the root, by its text.
    #[error("the path is outside the package")]
    Outside,
    /// A component of it starts with `.`.
    #[error("the path is a dotfile or in a dot directory")]
    Dotfile,
    /// It is not one of the package's files.
    #[error("the file is not in the package")]
    NotInPackage,
    /// Its real path, through a symlink, is outside the root.
    #[error("the path leads outside the package through a symlink")]
    SymlinkOutside,
    /// The cache has a symlink on its way, which this server does not write.
    #[error("the cache has a symlink on the path")]
    SymlinkInCache,
    /// There is nothing there, or it cannot be looked at, as one that loops through symlinks.
    #[error("the file is not there, or cannot be looked at")]
    Missing,
    /// It is not a regular file, as a directory or a FIFO.
    #[error("the path is not a regular file")]
    NotAFile,
}

impl<'a> Root<'a> {
    /// The directory of the root.
    #[must_use]
    pub fn dir(self) -> &'a Path {
        match self {
            Self::Release(sources) => &sources.root,
        }
    }

    /// Whether the root is in the cache, where no symlink is followed (R-CA-4).
    const fn in_cache(self) -> bool {
        match self {
            Self::Release(sources) => matches!(sources.origin, Origin::Cache),
        }
    }

    /// Whether the file at `relative`, with `/`, is the package's.
    fn holds(self, relative: &str) -> bool {
        match self {
            Self::Release(sources) => sources
                .listing
                .files
                .binary_search_by(|file| file.as_str().cmp(relative))
                .is_ok(),
        }
    }
}

/// The file at `asked` under `root`, if it may be read: `asked` is relative to the root, or
/// absolute under it.
///
/// It reads the file system, so it is for a blocking thread.
///
/// # Errors
///
/// Returns why it may not be read.
pub fn resolve(root: Root<'_>, asked: &Path) -> Result<Resolved, Refusal> {
    // The root as its text resolves, as `$CARGO_HOME` may be relative or have `..`.
    let dir = lexically(&std::path::absolute(root.dir()).map_err(|error| missing(&error))?);
    let full = lexically(&dir.join(asked));
    let inside = full
        .strip_prefix(&dir)
        .map_err(|_outside| Refusal::Outside)?;
    let relative = package_path(root, inside)?;
    let path = if root.in_cache() {
        without_symlinks(&dir, inside)?
    } else {
        // Where a symlink leads is judged as the path asked for is.
        let (real_dir, real) = (real(&dir)?, real(&full)?);
        let real_inside = real
            .strip_prefix(&real_dir)
            .map_err(|_outside| Refusal::SymlinkOutside)?;
        if real_inside != inside {
            package_path(root, real_inside)?;
        }
        real
    };
    let metadata = fs::metadata(&path).map_err(|error| missing(&error))?;
    if !metadata.is_file() {
        return Err(Refusal::NotAFile);
    }
    Ok(Resolved { path, relative })
}

/// `inside`, a path relative to `root`, with `/`, if it names a file of the package by its text:
/// not the root itself, no dotfile, and of the package as the root tells. A name that is not
/// UTF-8 is not the package's, whose files' names are.
fn package_path(root: Root<'_>, inside: &Path) -> Result<String, Refusal> {
    if inside.as_os_str().is_empty() {
        return Err(Refusal::NotAFile);
    }
    let mut names = Vec::new();
    for component in inside.components() {
        let name = component.as_os_str();
        if name.as_encoded_bytes().starts_with(b".") {
            return Err(Refusal::Dotfile);
        }
        names.push(name.to_str().ok_or(Refusal::NotInPackage)?);
    }
    let relative = names.join("/");
    if root.holds(&relative) {
        Ok(relative)
    } else {
        Err(Refusal::NotInPackage)
    }
}

/// `dir` joined with `inside`, if no component of `inside` is a symlink.
fn without_symlinks(dir: &Path, inside: &Path) -> Result<PathBuf, Refusal> {
    let mut path = dir.to_path_buf();
    for component in inside.components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|error| missing(&error))?;
        if metadata.file_type().is_symlink() {
            tracing::warn!(path = %path.display(), "ignoring a symlink in the cache");
            return Err(Refusal::SymlinkInCache);
        }
    }
    Ok(path)
}

/// The real path of `path`, through every symlink. Both sides of a comparison are made real by
/// it, so that a `\\?\` that Windows may put before them is on both; `dunce` leaves it on a long
/// path, which a short root would not have.
#[expect(
    clippy::disallowed_methods,
    reason = "both sides of a comparison are made real alike, and the path is only opened"
)]
fn real(path: &Path) -> Result<PathBuf, Refusal> {
    fs::canonicalize(path).map_err(|error| missing(&error))
}

/// The refusal for a path that could not be looked at, as one that is not there, or that cannot
/// be read, or that loops through symlinks: [`Refusal::Missing`].
fn missing(error: &io::Error) -> Refusal {
    if error.kind() != io::ErrorKind::NotFound {
        tracing::debug!(%error, "a path of the sources could not be looked at");
    }
    Refusal::Missing
}

/// `path` with `.` and `..` resolved by its text alone, as cargo writes the spans.
#[must_use]
pub fn lexically(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => {
                resolved.pop();
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                resolved.push(component);
            },
        }
    }
    resolved
}

/// `path` with `/` between its components, whatever the platform.
#[must_use]
pub fn slash_separated(path: &Path) -> String {
    let mut joined = String::with_capacity(path.as_os_str().len());
    for (position, component) in path.components().enumerate() {
        if position > 0 {
            joined.push('/');
        }
        joined.push_str(&component.as_os_str().to_string_lossy());
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::crate_source::Listing;

    /// Sources at `root`, from `origin`, whose listing is `files`; the files are written there.
    fn sources(root: &Path, origin: Origin, files: &[&str]) -> Sources {
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            fs::write(path, "x").expect("written");
        }
        let mut files: Vec<String> = files.iter().map(|&file| file.to_owned()).collect();
        files.sort();
        Sources {
            root: root.to_path_buf(),
            listing: Listing { files, bytes: 0 },
            origin,
        }
    }

    fn resolved(sources: &Sources, asked: &str) -> Result<Resolved, Refusal> {
        resolve(Root::Release(sources), Path::new(asked))
    }

    /// A file of the listing is read, asked for relative, through `.` and `..` that stay inside,
    /// or absolute under the root.
    #[test]
    fn a_file_of_the_listing_is_read() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let sources = sources(dir.path(), Origin::Cache, &["Cargo.toml", "src/lib.rs"]);
        let lib = Resolved {
            path: dir.path().join("src/lib.rs"),
            relative: "src/lib.rs".to_owned(),
        };
        assert_eq!(resolved(&sources, "src/lib.rs"), Ok(lib.clone()));
        assert_eq!(resolved(&sources, "./src/../src/lib.rs"), Ok(lib.clone()));
        assert_eq!(resolved(&sources, "src//lib.rs"), Ok(lib.clone()));
        assert_eq!(resolved(&sources, "src/lib.rs/"), Ok(lib.clone()));
        let absolute = dir.path().join("src/lib.rs");
        assert_eq!(
            resolved(&sources, absolute.to_str().expect("UTF-8")),
            Ok(lib.clone())
        );
        // A root whose text has `..`, as `$CARGO_HOME` may.
        let mut around = sources;
        around.root = dir.path().join("src").join("..");
        assert_eq!(resolved(&around, "src/lib.rs"), Ok(lib));
    }

    /// What is outside the root, a dotfile, not in the listing, not there, or not a file is not
    /// read.
    #[test]
    fn what_is_not_the_packages_is_refused() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().join("root");
        let mut sources = sources(&root, Origin::Cache, &[
            "Cargo.toml",
            ".cargo_vcs_info.json",
            ".github/x.rs",
            "gone.rs",
        ]);
        fs::write(root.join("extra.rs"), "x").expect("written");
        fs::write(dir.path().join("secret.rs"), "x").expect("written");
        fs::remove_file(root.join("gone.rs")).expect("removed");
        fs::create_dir_all(root.join("src/dir")).expect("made");
        sources.listing.files.push("src/dir".to_owned());
        sources.listing.files.sort();
        let secret = dir.path().join("secret.rs");
        for (asked, refusal) in [
            ("../secret.rs", Refusal::Outside),
            ("src/../../secret.rs", Refusal::Outside),
            (secret.to_str().expect("UTF-8"), Refusal::Outside),
            (".cargo_vcs_info.json", Refusal::Dotfile),
            (".github/x.rs", Refusal::Dotfile),
            ("extra.rs", Refusal::NotInPackage),
            ("CARGO.toml", Refusal::NotInPackage),
            ("src", Refusal::NotInPackage),
            ("gone.rs", Refusal::Missing),
            ("src/dir", Refusal::NotAFile),
            ("", Refusal::NotAFile),
            (".", Refusal::NotAFile),
        ] {
            assert_eq!(resolved(&sources, asked), Err(refusal), "{asked}");
        }
    }

    /// On Windows, `\` separates as `/` does; a path with a drive, of a share or verbatim, or from
    /// the drive's root, is not under the root.
    #[cfg(windows)]
    #[test]
    fn a_path_of_windows_is_resolved_as_windows_does() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let sources = sources(dir.path(), Origin::Cache, &["Cargo.toml", "src/lib.rs"]);
        assert_eq!(
            resolved(&sources, r"src\lib.rs").map(|resolved| resolved.relative),
            Ok("src/lib.rs".to_owned())
        );
        for asked in [
            r"C:\x.rs",
            r"C:x.rs",
            r"\x.rs",
            r"\\server\share\x.rs",
            r"\\?\C:\x.rs",
        ] {
            assert_eq!(resolved(&sources, asked), Err(Refusal::Outside), "{asked}");
        }
    }

    /// A name that is not UTF-8 is not the package's, whatever its lossy text would match.
    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_utf8_is_not_the_packages() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt as _};

        let dir = tempfile::tempdir().expect("a temporary directory");
        let sources = sources(dir.path(), Origin::Cache, &["a\u{fffd}.rs"]);
        let asked = Path::new(OsStr::from_bytes(b"a\xff.rs"));
        assert_eq!(
            resolve(Root::Release(&sources), asked),
            Err(Refusal::NotInPackage)
        );
    }

    /// In the cache, a symlink is refused, wherever it leads, a directory's too; where cargo
    /// unpacked, one is followed to its real path if that is under the root's, which may be a
    /// symlink itself, and is a file of the package.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_followed_only_where_cargo_unpacked() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("a temporary directory");
        let outside = dir.path().join("outside.rs");
        fs::write(&outside, "x").expect("written");
        let in_cache = sources(&dir.path().join("cache"), Origin::Cache, &[
            "a.rs",
            "real/b.rs",
            "linked/b.rs",
        ]);
        fs::remove_file(dir.path().join("cache/a.rs")).expect("removed");
        symlink(&outside, dir.path().join("cache/a.rs")).expect("linked");
        assert_eq!(resolved(&in_cache, "a.rs"), Err(Refusal::SymlinkInCache));
        fs::remove_dir_all(dir.path().join("cache/linked")).expect("removed");
        symlink(
            dir.path().join("cache/real"),
            dir.path().join("cache/linked"),
        )
        .expect("linked");
        assert_eq!(
            resolved(&in_cache, "linked/b.rs"),
            Err(Refusal::SymlinkInCache)
        );
        let cargo = dir.path().join("cargo");
        let mut by_cargo = sources(&cargo, Origin::Cargo, &[
            "a.rs",
            "real.rs",
            "out.rs",
            "to_extra.rs",
            "to_dot.rs",
        ]);
        fs::write(cargo.join("extra.rs"), "x").expect("written");
        fs::write(cargo.join(".cargo-ok"), "x").expect("written");
        for (link, target) in [("to_extra.rs", "extra.rs"), ("to_dot.rs", ".cargo-ok")] {
            fs::remove_file(cargo.join(link)).expect("removed");
            symlink(cargo.join(target), cargo.join(link)).expect("linked");
        }
        // Where a symlink leads is judged as the path asked for is.
        assert_eq!(
            resolved(&by_cargo, "to_extra.rs"),
            Err(Refusal::NotInPackage)
        );
        assert_eq!(resolved(&by_cargo, "to_dot.rs"), Err(Refusal::Dotfile));
        fs::remove_file(cargo.join("a.rs")).expect("removed");
        symlink(cargo.join("real.rs"), cargo.join("a.rs")).expect("linked");
        fs::remove_file(cargo.join("out.rs")).expect("removed");
        symlink(&outside, cargo.join("out.rs")).expect("linked");
        let real = real(&cargo.join("real.rs")).expect("there");
        assert_eq!(
            resolved(&by_cargo, "a.rs").map(|resolved| resolved.path),
            Ok(real.clone())
        );
        assert_eq!(resolved(&by_cargo, "out.rs"), Err(Refusal::SymlinkOutside));
        let linked = dir.path().join("linked");
        symlink(&cargo, &linked).expect("linked");
        by_cargo.root = linked;
        assert_eq!(
            resolved(&by_cargo, "a.rs").map(|resolved| resolved.path),
            Ok(real)
        );
    }

    #[test]
    fn a_path_is_resolved_by_its_text_and_told_with_slashes() {
        assert_eq!(lexically(Path::new("a/./b/../c")), Path::new("a").join("c"));
        assert_eq!(
            slash_separated(&Path::new("a").join("b").join("c")),
            "a/b/c"
        );
    }
}
