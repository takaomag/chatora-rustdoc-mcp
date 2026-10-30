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
#[cfg(feature = "local")]
use super::{cache::Cache, crate_source};

/// A root of sources, and which files under it are the package's.
#[derive(Debug, Clone, Copy)]
pub enum Root<'a> {
    /// The sources of a crates.io release: the files of its `.crate`'s listing, where cargo
    /// unpacked them or in the cache (decided in M3b, B2-3).
    Release(&'a Sources),
    /// A package built here: a member of the workspace, or a path dependency.
    #[cfg(feature = "local")]
    Package(&'a Package),
    /// The checkout of a git dependency, where cargo checked it out: every file in it but the
    /// dotfiles.
    #[cfg(feature = "local")]
    Checkout(&'a Path),
}

/// A package built here, and what of it may be read (requirement R-SC-4).
///
/// Those are its targets' `.rs` files, the files that rustdoc read, its `Cargo.toml`, its readme
/// and its examples. The paths are absolute: as cargo tells them, and as the dep-info told them,
/// whose paths with `..` were made real; each is compared by its text, and else by its real path.
#[cfg(feature = "local")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    /// The known source root that its paths are judged under: the root of its workspace, where its
    /// inherited readme and the files that rustdoc read may be, a path or git dependency's own
    /// workspace's too; or the package's own directory, if it is in none.
    pub root: PathBuf,
    /// The directory of its `Cargo.toml`.
    pub dir: PathBuf,
    /// Its targets' files, as cargo tells them (`src_path`), its build script's among them: the
    /// `.rs` files under a target's directory are its, but for one directly in the package's
    /// directory, as `build.rs`, that file only, so that nothing else under the package's
    /// directory is taken by it, as its target directory or other packages (decided in M3b).
    pub targets: Vec<PathBuf>,
    /// The files that rustdoc read, as the build's record tells (decided in M3b, B2-2).
    pub source_files: Vec<PathBuf>,
    /// Its readme, as its manifest tells, inherited from the workspace too (decided in M3, P8).
    pub readme: Option<PathBuf>,
    /// Its examples' files.
    pub examples: Vec<PathBuf>,
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
            #[cfg(feature = "local")]
            Self::Package(package) => &package.root,
            #[cfg(feature = "local")]
            Self::Checkout(checkout) => checkout,
        }
    }

    /// Whether the root is in the cache, where no symlink is followed (R-CA-4).
    const fn in_cache(self) -> bool {
        match self {
            Self::Release(sources) => matches!(sources.origin, Origin::Cache),
            #[cfg(feature = "local")]
            Self::Package(_) | Self::Checkout(_) => false,
        }
    }

    /// Whether where a symlink leads is to be one of the package's files too, as a file of a
    /// listing is; else it is enough that it is under the root, as the requirement has it for the
    /// roots of the project and of git, whose files are not listed.
    const fn follows_to_files(self) -> bool {
        match self {
            Self::Release(_) => true,
            #[cfg(feature = "local")]
            Self::Package(_) | Self::Checkout(_) => false,
        }
    }

    /// Whether the file at `relative` under the root, with `/`, whose absolute path is `full`
    /// with its text resolved, is the package's.
    fn holds(self, relative: &str, full: &Path) -> bool {
        match self {
            Self::Release(sources) => {
                let _ = full;
                sources
                    .listing
                    .files
                    .binary_search_by(|file| file.as_str().cmp(relative))
                    .is_ok()
            },
            #[cfg(feature = "local")]
            Self::Package(package) => {
                let _ = relative;
                package.holds(full)
            },
            #[cfg(feature = "local")]
            Self::Checkout(_) => true,
        }
    }
}

#[cfg(feature = "local")]
impl Package {
    /// Whether the file at `full`, absolute with its text resolved, is the package's: by the text
    /// of its paths, or else by the real paths of both, as the dep-info's paths with `..` are.
    fn holds(&self, full: &Path) -> bool {
        self.holds_as(full, |path| Some(lexically(path)))
            || real(full).is_ok_and(|full| self.holds_as(&full, |path| real(path).ok()))
    }

    /// Whether the file at `full` is the package's, with its paths in the `form` of `full`.
    fn holds_as(&self, full: &Path, form: impl Fn(&Path) -> Option<PathBuf>) -> bool {
        let is = |path: &Path| form(path).is_some_and(|path| path == full);
        let dir = form(&self.dir);
        let of_target = |target: &PathBuf| match target.parent().and_then(&form) {
            Some(parent) if Some(&parent) == dir.as_ref() => is(target),
            Some(parent) => {
                full.extension().is_some_and(|extension| extension == "rs")
                    && full.starts_with(parent)
            },
            None => false,
        };
        is(&self.dir.join("Cargo.toml"))
            || self.targets.iter().any(of_target)
            || self.source_files.iter().any(|file| is(file))
            || self.examples.iter().any(|example| is(example))
            || self
                .readme
                .as_deref()
                .is_some_and(|readme| is(readme) && is_document(readme))
    }
}

/// The sources of the dependencies of local builds where cargo has them (decided in M3b, B5b).
///
/// They are for the files that the spans of a build tell outside its package, as of code that a
/// derive generated (decisions 17 and 19): a crates.io release that cargo unpacked, by the listing
/// of its `.crate`, made from one at hand if need be but never fetched; and a git checkout.
#[cfg(feature = "local")]
#[derive(Debug)]
pub struct Dependencies {
    cache: Cache,
    cargo_home: PathBuf,
    max_bytes: u64,
    /// The sources of the releases looked at, by the directory that cargo unpacked each into.
    known: parking_lot::Mutex<std::collections::HashMap<PathBuf, Option<std::sync::Arc<Sources>>>>,
}

#[cfg(feature = "local")]
impl Dependencies {
    /// The sources of the dependencies in `cargo_home`, whose listings are kept in `cache`, of at
    /// most `max_bytes` each (`limits.extract_max_size`).
    #[must_use]
    pub fn new(cache: Cache, cargo_home: &Path, max_bytes: u64) -> Self {
        Self {
            cache,
            // As the spans tell it: absolute, with its text resolved.
            cargo_home: lexically(
                &std::path::absolute(cargo_home).unwrap_or_else(|_| cargo_home.to_path_buf()),
            ),
            max_bytes,
            known: parking_lot::Mutex::default(),
        }
    }

    /// The file at `full`, absolute with its text resolved, if it is a dependency's that may be
    /// read: under `$CARGO_HOME/registry/src/index.crates.io-…/<name>-<version>/`, or under
    /// `$CARGO_HOME/git/checkouts/<repository>/<revision>/` that cargo finished.
    ///
    /// It reads and writes files, so it is for a blocking thread. A release whose sources another
    /// holds the lock of is not waited for: it has no path then.
    #[must_use]
    pub fn resolve(&self, full: &Path) -> Option<Resolved> {
        let inside = full.strip_prefix(&self.cargo_home).ok()?;
        let mut parts = inside.components().map(Component::as_os_str);
        let (Some(kind), Some(of), Some(first), Some(second)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let root = self.cargo_home.join(kind).join(of).join(first).join(second);
        if kind == "registry" && of == "src" {
            let known = self.known.lock().get(&root).cloned();
            let sources = known.unwrap_or_else(|| {
                // Made without the lock, which is for a moment only.
                let made =
                    crate_source::unpacked_at(&self.cache, &self.cargo_home, &root, self.max_bytes)
                        .map(std::sync::Arc::new);
                self.known.lock().insert(root.clone(), made.clone());
                made
            })?;
            resolve(Root::Release(&sources), full).ok()
        } else if kind == "git"
            && of == "checkouts"
            && fs::symlink_metadata(root.join(".cargo-ok")).is_ok_and(|ok| ok.is_file())
        {
            resolve(Root::Checkout(&root), full).ok()
        } else {
            None
        }
    }
}

/// Whether the readme at `path` is a document (requirement R-SC-4, decided in M3, P8): of the
/// extensions of one, or named `README` without one, in any case.
#[cfg(feature = "local")]
fn is_document(path: &Path) -> bool {
    path.extension().map_or_else(
        || {
            path.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("README"))
        },
        |extension| {
            ["md", "markdown", "txt", "rst", "adoc"]
                .iter()
                .any(|document| extension.eq_ignore_ascii_case(document))
        },
    )
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
    let relative = package_path(root, inside, &full)?;
    let path = if root.in_cache() {
        without_symlinks(&dir, inside)?
    } else {
        // Where a symlink leads is judged as the path asked for is.
        let (real_dir, real) = (real(&dir)?, real(&full)?);
        let real_inside = real
            .strip_prefix(&real_dir)
            .map_err(|_outside| Refusal::SymlinkOutside)?;
        if real_inside != inside {
            if root.follows_to_files() {
                package_path(root, real_inside, &real)?;
            } else if has_dotfile(real_inside) {
                return Err(Refusal::Dotfile);
            }
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
/// not the root itself, no dotfile, and of the package as the root tells by it or by `full`, its
/// absolute path. A name that is not UTF-8 is not the package's, whose files' names are.
fn package_path(root: Root<'_>, inside: &Path, full: &Path) -> Result<String, Refusal> {
    if inside.as_os_str().is_empty() {
        return Err(Refusal::NotAFile);
    }
    if has_dotfile(inside) {
        return Err(Refusal::Dotfile);
    }
    let names: Option<Vec<&str>> = inside
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect();
    let relative = names.ok_or(Refusal::NotInPackage)?.join("/");
    if root.holds(&relative, full) {
        Ok(relative)
    } else {
        Err(Refusal::NotInPackage)
    }
}

/// Whether a component of `inside` is a dotfile, or a dot directory.
fn has_dotfile(inside: &Path) -> bool {
    inside
        .components()
        .any(|component| component.as_os_str().as_encoded_bytes().starts_with(b"."))
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

    /// Writes each of `files` under `root`.
    #[cfg(feature = "local")]
    fn write_all(root: &Path, files: &[&str]) {
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            fs::write(path, "x").expect("written");
        }
    }

    /// Of a member, its targets' `.rs` files, the files that rustdoc read, its `Cargo.toml`, its
    /// readme of a document's name, inherited from the workspace too, and its examples are read,
    /// under the workspace's root; nothing else of the workspace (requirement R-SC-4, decided in
    /// M3, P8, and M3b, B2-2).
    #[cfg(feature = "local")]
    #[test]
    fn of_a_package_its_sources_are_read() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let workspace = dir.path().join("ws");
        write_all(&workspace, &[
            "Cargo.toml",
            "README.md",
            "notes.md",
            "secret.txt",
            "one/Cargo.toml",
            "one/build.rs",
            "one/src/lib.rs",
            "one/src/deep/mod.rs",
            "one/src/data.json",
            "one/examples/hi.rs",
            "one/README",
            "one/other.rs",
            "one/target/debug/x.rs",
            "two/src/lib.rs",
        ]);
        let one = workspace.join("one");
        let mut package = Package {
            root: workspace.clone(),
            dir: one.clone(),
            targets: vec![one.join("src/lib.rs"), one.join("build.rs")],
            source_files: vec![one.join("src/lib.rs"), workspace.join("notes.md")],
            readme: Some(one.join("../README.md")),
            examples: vec![one.join("examples/hi.rs")],
        };
        let read = |package: &Package, asked: &str| {
            resolve(Root::Package(package), Path::new(asked)).map(|resolved| resolved.relative)
        };
        for asked in [
            "one/Cargo.toml",
            "one/build.rs",
            "one/src/lib.rs",
            "one/src/deep/mod.rs",
            "one/examples/hi.rs",
            "notes.md",
            "README.md",
        ] {
            assert_eq!(read(&package, asked), Ok(asked.to_owned()), "{asked}");
        }
        for (asked, refusal) in [
            ("one/src/data.json", Refusal::NotInPackage),
            ("one/README", Refusal::NotInPackage),
            ("one/other.rs", Refusal::NotInPackage),
            ("one/target/debug/x.rs", Refusal::NotInPackage),
            ("secret.txt", Refusal::NotInPackage),
            ("Cargo.toml", Refusal::NotInPackage),
            ("two/src/lib.rs", Refusal::NotInPackage),
            ("../ws/secret.txt", Refusal::NotInPackage),
            ("../outside.rs", Refusal::Outside),
        ] {
            assert_eq!(read(&package, asked), Err(refusal), "{asked}");
        }
        // A readme of no document's name is not read, but `README` without an extension is.
        package.readme = Some(workspace.join("secret.txt").with_extension("json"));
        assert_eq!(read(&package, "secret.json"), Err(Refusal::NotInPackage));
        package.readme = Some(one.join("README"));
        assert_eq!(read(&package, "one/README"), Ok("one/README".to_owned()));
        // A package at the workspace's root, with its build script there: not its members, nor
        // its target directory.
        let root = Package {
            root: workspace.clone(),
            dir: workspace.clone(),
            targets: vec![workspace.join("build.rs"), workspace.join("src/lib.rs")],
            ..Package::default()
        };
        write_all(&workspace, &["build.rs", "src/lib.rs", "target/debug/x.rs"]);
        for asked in ["build.rs", "src/lib.rs", "Cargo.toml"] {
            assert_eq!(read(&root, asked), Ok(asked.to_owned()), "{asked}");
        }
        for asked in ["one/src/lib.rs", "two/src/lib.rs", "target/debug/x.rs"] {
            assert_eq!(read(&root, asked), Err(Refusal::NotInPackage), "{asked}");
        }
    }

    /// The files that rustdoc read through `..` are kept as their real paths, which a workspace
    /// under a symlink, as macOS's `/var` is, does not have: they are read by their real paths.
    #[cfg(all(feature = "local", unix))]
    #[test]
    fn a_file_kept_by_its_real_path_is_read_under_a_symlink() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let workspace = dir.path().join("real");
        write_all(&workspace, &["notes.md", "one/src/lib.rs"]);
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&workspace, &linked).expect("linked");
        let package = Package {
            root: linked.clone(),
            dir: linked.join("one"),
            targets: vec![linked.join("one/src/lib.rs")],
            source_files: vec![real(&workspace.join("notes.md")).expect("there")],
            ..Package::default()
        };
        assert_eq!(
            resolve(Root::Package(&package), Path::new("notes.md"))
                .map(|resolved| resolved.relative),
            Ok("notes.md".to_owned())
        );
    }

    /// Where a symlink in a package or a checkout leads, under the root, is read, as a file of the
    /// workspace outside the package is, but not a dotfile, and outside the root nothing is; a
    /// checkout's files are all read but dotfiles.
    #[cfg(all(feature = "local", unix))]
    #[test]
    fn a_symlink_of_a_package_leads_under_its_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("a temporary directory");
        let workspace = dir.path().join("ws");
        write_all(&workspace, &[
            "Cargo.toml",
            "shared/util.rs",
            ".env",
            "src/lib.rs",
        ]);
        fs::write(dir.path().join("outside.rs"), "x").expect("written");
        for (link, target) in [
            ("src/util.rs", workspace.join("shared/util.rs")),
            ("src/env.rs", workspace.join(".env")),
            ("src/out.rs", dir.path().join("outside.rs")),
        ] {
            symlink(target, workspace.join(link)).expect("linked");
        }
        let package = Package {
            root: workspace.clone(),
            dir: workspace.clone(),
            targets: vec![workspace.join("src/lib.rs")],
            ..Package::default()
        };
        let read = |root: Root<'_>, asked: &str| resolve(root, Path::new(asked));
        assert_eq!(
            read(Root::Package(&package), "src/util.rs").map(|resolved| resolved.path),
            Ok(real(&workspace.join("shared/util.rs")).expect("there"))
        );
        assert_eq!(
            read(Root::Package(&package), "src/env.rs"),
            Err(Refusal::Dotfile)
        );
        assert_eq!(
            read(Root::Package(&package), "src/out.rs"),
            Err(Refusal::SymlinkOutside)
        );
        let checkout = Root::Checkout(&workspace);
        for asked in ["shared/util.rs", "src/util.rs"] {
            assert_eq!(
                read(checkout, asked).map(|resolved| resolved.relative),
                Ok(asked.to_owned())
            );
        }
        assert_eq!(read(checkout, ".env"), Err(Refusal::Dotfile));
        assert_eq!(read(checkout, "src/out.rs"), Err(Refusal::SymlinkOutside));
    }

    /// A dependency's file where cargo has it is read: of a crates.io release that cargo
    /// unpacked, by its `.crate`'s listing, made from the `.crate` at hand; of a git checkout that
    /// cargo finished, but its dotfiles; nothing elsewhere (decided in M3b, B5b).
    #[cfg(feature = "local")]
    #[test]
    fn a_dependencys_file_is_read_where_cargo_has_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let home = dir.path().join("cargo-home");
        let unpacked = home.join("registry/src/index.crates.io-1949cf8c6b5b557f/demo-0.1.0");
        write_all(&unpacked, &["Cargo.toml", "src/lib.rs", "written.rs"]);
        fs::write(unpacked.join(".cargo-ok"), r#"{"v":1}"#).expect("written");
        let cached = home.join("registry/cache/index.crates.io-1949cf8c6b5b557f");
        fs::create_dir_all(&cached).expect("made");
        fs::write(
            cached.join("demo-0.1.0.crate"),
            crate::data::crate_file::tests::crate_file("demo", "0.1.0", "[package]\n"),
        )
        .expect("written");
        let checkout = home.join("git/checkouts/repo-0123/abcdef0");
        write_all(&checkout, &["Cargo.toml", "src/lib.rs", ".env"]);
        fs::write(checkout.join(".cargo-ok"), "").expect("written");
        let unfinished = home.join("git/checkouts/repo-0123/1234567");
        write_all(&unfinished, &["src/lib.rs"]);
        write_all(&home, &["config.toml"]);
        let dependencies =
            Dependencies::new(Cache::at(&dir.path().join("cache")), &home, 1024 * 1024);
        let read = |path: &Path| dependencies.resolve(path).map(|resolved| resolved.relative);
        assert_eq!(
            read(&unpacked.join("src/lib.rs")),
            Some("src/lib.rs".to_owned())
        );
        assert_eq!(
            read(&checkout.join("src/lib.rs")),
            Some("src/lib.rs".to_owned())
        );
        for path in [
            unpacked.join("written.rs"),
            checkout.join(".env"),
            unfinished.join("src/lib.rs"),
            home.join("config.toml"),
            dir.path().join("elsewhere.rs"),
        ] {
            assert_eq!(read(&path), None, "{}", path.display());
        }
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
