//! Whether the build of a workspace member is fresh: the content digest of what it is built from,
//! taken before the build (requirement R-WS-4, decided in M2's D4).
//!
//! Each file is recorded with its size, its time of change and its SHA-256. A later check reads
//! again only the files whose size or time differ, or whose time is close to when they were
//! taken: a file system may keep the time coarsely.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::cache;

/// The most files that the inputs of a build have. More is taken for a mistake, as a directory of
/// data inside a package, and the build is then not kept fresh.
const MAX_FILES: usize = 50_000;

/// How close to when the inputs were taken a file's time may be for its digest to be taken again:
/// more than the coarsest time that file systems keep (two seconds on FAT).
const CLOSE: Duration = Duration::from_secs(3);

/// What a build is made from: its files, as they were before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inputs {
    /// The files, by path.
    pub files: Vec<InputFile>,
    /// When they were taken, in nanoseconds since the Unix epoch.
    pub taken_at: u128,
}

/// A file among the inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputFile {
    /// Its path.
    pub path: PathBuf,
    /// Its size.
    pub len: u64,
    /// When it changed, in nanoseconds since the Unix epoch, if the OS tells.
    pub modified: Option<u128>,
    /// Its SHA-256, or `None` when it does not exist or cannot be read.
    pub sha256: Option<Box<str>>,
}

/// The inputs of a build: the files `files`, which may not exist, and the files under `dirs`
/// (decided in M2's D4).
///
/// Under a directory, `target` and `.claude` (worktrees) at its top are not walked, nor `.git`,
/// nor another package's directory, nor a cache. A symlink is followed. The digests of `before`,
/// the inputs of the build before, are taken again only for what changed.
///
/// # Errors
///
/// Returns an error when a directory of `dirs` cannot be read, or there are more than 50,000
/// files.
pub fn collect(files: &[PathBuf], dirs: &[PathBuf], before: Option<&Inputs>) -> io::Result<Inputs> {
    let taken_at = nanos(SystemTime::now()).unwrap_or_default();
    let mut paths: Vec<PathBuf> = files.to_vec();
    for dir in dirs {
        walk(dir, dir, &mut paths, &mut HashSet::new())?;
    }
    paths.sort();
    paths.dedup();
    let close = CLOSE.as_nanos();
    let files = paths
        .into_iter()
        .map(|path| {
            let Some(metadata) = fs::metadata(&path).ok().filter(fs::Metadata::is_file) else {
                return InputFile {
                    path,
                    len: 0,
                    modified: None,
                    sha256: None,
                };
            };
            let (len, modified) = (metadata.len(), metadata.modified().ok().and_then(nanos));
            // The digest of the build before, if the file is the same and was not changing then.
            let kept = before.and_then(|before| {
                let index = before
                    .files
                    .binary_search_by(|file| file.path.cmp(&path))
                    .ok()?;
                let file = before.files.get(index)?;
                let settled = modified.is_some_and(|at| at.saturating_add(close) < before.taken_at);
                (file.len == len && file.modified == modified && settled)
                    .then(|| file.sha256.clone())
                    .flatten()
            });
            let sha256 = kept.or_else(|| {
                fs::File::open(&path)
                    .and_then(cache::sha256_hex_read)
                    .ok()
                    .map(Into::into)
            });
            InputFile {
                path,
                len,
                modified,
                sha256,
            }
        })
        .collect();
    Ok(Inputs { files, taken_at })
}

/// The files under `dirs` that git takes for sources, and the directories that git does not
/// tell, to walk (decided in M2's D4).
///
/// In a git work tree, they are the files that git tracks and those that it does not ignore
/// (`git ls-files --cached --others --exclude-standard`): `.gitignore` tells what is not a source,
/// as a working directory of builds. `target` and `.claude` (worktrees) at the top of each
/// directory are left out, as the directories of other packages. What git lists as a directory,
/// as a submodule, is walked, and so is a directory whose package git ignores.
pub async fn sources(dirs: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut files, mut walked) = (Vec::new(), Vec::new());
    for dir in dirs {
        // Without running a program that the user's settings may name, nor writing the index.
        let listed = super::tool::output_with(
            dir,
            super::tool::Place::Project,
            Path::new("git"),
            &[
                "-c",
                "core.fsmonitor=false",
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                ".",
            ],
            &[("GIT_OPTIONAL_LOCKS", "0")],
        )
        .await;
        let listed = match listed {
            Ok(listed) => listed,
            Err(why) => {
                tracing::debug!(dir = %dir.display(), %why, "git does not tell the sources, so the directory is walked");
                walked.push(dir.clone());
                continue;
            },
        };
        let listed: Vec<&str> = listed.split('\0').filter(|path| !path.is_empty()).collect();
        // A package that git ignores, as in a directory of vendored packages, is not listed.
        if !listed.contains(&"Cargo.toml") {
            tracing::debug!(dir = %dir.display(), "git ignores the package, so the directory is walked");
            walked.push(dir.clone());
            continue;
        }
        let mut packages = HashMap::new();
        for path in listed {
            // A name that is not UTF-8, which the output does not keep.
            if path.contains(char::REPLACEMENT_CHARACTER) {
                tracing::debug!(dir = %dir.display(), path, "a file whose name is not UTF-8 is not an input");
                continue;
            }
            if !is_source(dir, Path::new(path), &mut packages) {
                continue;
            }
            let path = dir.join(path);
            // A submodule, a repository inside, or a symlink to a directory.
            if path.is_dir() {
                if !path.join("Cargo.toml").is_file() && !path.join("CACHEDIR.TAG").is_file() {
                    walked.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    (files, walked)
}

/// Whether `path`, relative to `dir`, which git lists, is a source of the package at `dir`: not
/// under `target` or `.claude` at the top, nor in the directory of another package, which
/// `packages` keeps by directory.
fn is_source(dir: &Path, path: &Path, packages: &mut HashMap<PathBuf, bool>) -> bool {
    if path
        .components()
        .next()
        .is_some_and(|top| matches!(top.as_os_str().to_str(), Some("target" | ".claude")))
    {
        return false;
    }
    let mut parent = path.parent();
    while let Some(inside) = parent.filter(|inside| !inside.as_os_str().is_empty()) {
        let is_package = *packages
            .entry(inside.to_path_buf())
            .or_insert_with(|| dir.join(inside).join("Cargo.toml").is_file());
        if is_package {
            return false;
        }
        parent = inside.parent();
    }
    true
}

/// The key of a build made from `inputs` as `facts`, the rest of what it is made from in one text:
/// the SHA-256 of both, without the sizes and times, which do not change what is built.
#[must_use]
pub fn key(inputs: &Inputs, facts: &str) -> String {
    let mut text = String::from(facts);
    for file in &inputs.files {
        text.push('\n');
        text.push_str(&file.path.to_string_lossy());
        text.push('\t');
        text.push_str(file.sha256.as_deref().unwrap_or("-"));
    }
    cache::sha256_hex(text.as_bytes())
}

/// Adds the files under `dir`, in `root`, to `paths`: not `target` and `.claude` at the top of
/// `root`, nor `.git`, nor the directory of another package (one with a `Cargo.toml`), nor a cache
/// (one with `CACHEDIR.TAG`). A symlink is followed, to a directory once: `seen` keeps the
/// directories walked, as the OS names them.
///
/// What cannot be read under `root`, or has a name that is not UTF-8, is left out, with a log
/// line: rustdoc cannot read it either, or the record cannot keep it.
fn walk(
    root: &Path,
    dir: &Path,
    paths: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) -> io::Result<()> {
    if let Ok(real) = dunce::canonicalize(dir)
        && !seen.insert(real)
    {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if dir != root => {
            tracing::debug!(dir = %dir.display(), %error, "a directory that cannot be read is not walked");
            return Ok(());
        },
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!(dir = %dir.display(), %error, "an entry that cannot be read is not an input");
                continue;
            },
        };
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            tracing::debug!(path = %path.display(), "a file whose name is not UTF-8 is not an input");
            continue;
        };
        // Through a symlink. What it does not lead to is recorded as missing.
        if fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            let skipped = name == ".git" || (dir == root && matches!(name, "target" | ".claude"));
            // A cache, as cargo's build directory, says so (https://bford.info/cachedir/).
            if !skipped
                && !path.join("Cargo.toml").is_file()
                && !path.join("CACHEDIR.TAG").is_file()
            {
                walk(root, &path, paths, seen)?;
            }
            continue;
        }
        if paths.len() >= MAX_FILES {
            return Err(io::Error::other(format!(
                "more than {MAX_FILES} files under {}",
                root.display()
            )));
        }
        // What is not a regular file has no digest: `collect` does not read it.
        paths.push(path);
    }
    Ok(())
}

/// `time` in nanoseconds since the Unix epoch, if it is after it.
fn nanos(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_package_is_made_from_is_found_and_kept() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let package = dir.path().join("p");
        let write = |path: &str, text: &str| {
            let path = package.join(path);
            fs::create_dir_all(path.parent().expect("a directory")).expect("made");
            fs::write(path, text).expect("written");
        };
        write("Cargo.toml", "[package]\nname = \"p\"\n");
        write("src/lib.rs", "pub fn f() {}\n");
        write("proto/a.proto", "message A {}\n");
        // Below the top, `target` is a module.
        write("src/target/mod.rs", "");
        // Not walked.
        write("target/debug/out", "");
        write(".git/HEAD", "");
        write(".claude/worktrees/w/src/lib.rs", "");
        write("inner/Cargo.toml", "[package]\nname = \"inner\"\n");
        write("inner/src/lib.rs", "");
        let missing = dir.path().join("Cargo.lock");
        let inputs = collect(
            std::slice::from_ref(&missing),
            std::slice::from_ref(&package),
            None,
        )
        .expect("collected");
        let found: Vec<_> = inputs
            .files
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(dir.path())
                    .expect("inside")
                    .to_owned()
            })
            .collect();
        assert_eq!(found, [
            PathBuf::from("Cargo.lock"),
            PathBuf::from("p/Cargo.toml"),
            PathBuf::from("p/proto/a.proto"),
            PathBuf::from("p/src/lib.rs"),
            PathBuf::from("p/src/target/mod.rs"),
        ]);
        assert_eq!(
            inputs.files.first().and_then(|file| file.sha256.as_ref()),
            None
        );
        let facts = "the plan";
        let first = key(&inputs, facts);
        // The same files give the same key; another plan, or an edit, another.
        let again = collect(
            std::slice::from_ref(&missing),
            std::slice::from_ref(&package),
            Some(&inputs),
        )
        .expect("collected");
        assert_eq!(key(&again, facts), first);
        assert_ne!(key(&again, "another plan"), first);
        write("proto/a.proto", "message B {}\n");
        let edited =
            collect(std::slice::from_ref(&missing), &[package], Some(&again)).expect("collected");
        assert_ne!(key(&edited, facts), first);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_followed_and_what_cannot_be_read_is_left_out() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let dir = tempfile::tempdir().expect("a temporary directory");
        let (package, shared) = (dir.path().join("p"), dir.path().join("shared"));
        for made in [package.join("src"), package.join("locked"), shared.clone()] {
            fs::create_dir_all(made).expect("made");
        }
        fs::write(package.join("Cargo.toml"), "").expect("written");
        fs::write(shared.join("x.rs"), "a").expect("written");
        symlink(shared.join("x.rs"), package.join("src/x.rs")).expect("linked");
        symlink(&shared, package.join("src/shared")).expect("linked");
        // A loop: walked once.
        symlink(&package, package.join("src/up")).expect("linked");
        symlink(dir.path().join("nowhere.rs"), package.join("src/gone.rs")).expect("linked");
        fs::write(package.join("locked/secret.rs"), "").expect("written");
        fs::set_permissions(package.join("locked"), fs::Permissions::from_mode(0o000))
            .expect("locked");
        let readable = fs::read_dir(package.join("locked")).is_ok();
        let inputs = collect(&[], std::slice::from_ref(&package), None);
        fs::set_permissions(package.join("locked"), fs::Permissions::from_mode(0o755))
            .expect("unlocked");
        let inputs = inputs.expect("collected");
        let found: Vec<_> = inputs
            .files
            .iter()
            .map(|file| {
                (
                    file.path.strip_prefix(&package).expect("inside").to_owned(),
                    file.sha256.is_some(),
                )
            })
            .collect();
        let mut expected = vec![
            (PathBuf::from("Cargo.toml"), true),
            (PathBuf::from("src/gone.rs"), false),
            (PathBuf::from("src/shared/x.rs"), true),
            (PathBuf::from("src/x.rs"), true),
        ];
        // As root, the directory can be read.
        if readable {
            expected.insert(1, (PathBuf::from("locked/secret.rs"), true));
        }
        assert_eq!(found, expected);
        // An edit through the symlink changes the key.
        let first = key(&inputs, "");
        fs::write(shared.join("x.rs"), "b").expect("written");
        let edited = collect(&[], &[package], Some(&inputs)).expect("collected");
        assert_ne!(key(&edited, ""), first);
    }

    #[test]
    fn a_digest_is_kept_only_for_a_file_that_was_settled() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("lib.rs");
        fs::write(&path, "a").expect("written");
        let now = collect(std::slice::from_ref(&path), &[], None).expect("collected");
        // As if taken long after the file changed, with another digest: it is kept.
        let mut before = now;
        before.taken_at = before.taken_at.saturating_add(CLOSE.as_nanos() * 2);
        if let Some(file) = before.files.first_mut() {
            file.sha256 = Some("kept".into());
        }
        let after = collect(std::slice::from_ref(&path), &[], Some(&before)).expect("collected");
        assert_eq!(
            after.files.first().and_then(|file| file.sha256.as_deref()),
            Some("kept")
        );
        // Taken right after the file changed: read again.
        if let Some(file) = before.files.first_mut() {
            before.taken_at = file.modified.expect("a time");
        }
        let after = collect(std::slice::from_ref(&path), &[], Some(&before)).expect("collected");
        assert_ne!(
            after.files.first().and_then(|file| file.sha256.as_deref()),
            Some("kept")
        );
    }
}
