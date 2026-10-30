//! The rustdoc JSON of std from a nightly toolchain that rustup installed with the
//! `rust-docs-json` component (requirement R-STD-1).
//!
//! It is used only if it is not older than the project's toolchain, since std of an older nightly
//! lacks what the project may use. Otherwise std comes from the distribution server
//! (requirement R-STD-6).

use std::{
    borrow::Cow,
    fs::File,
    io::{self, Read as _},
    path::{Path, PathBuf},
};

use semver::Version;

use super::{dist::StdCrate, docs_rs::MAX_BYTES, target::TargetTriple, tool};

/// The toolchain that is looked for.
const TOOLCHAIN: &str = "nightly";

/// Largest manifest of the channel read: rustup's is about 1 MB.
const MAX_CHANNEL_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// The JSON of std of the nightly toolchain installed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStd {
    /// The directory of the JSON, `<sysroot>/share/doc/rust/json`.
    pub dir: PathBuf,
    /// What `rustc -vV` of the toolchain says, for the provenance (requirement R-STD-1).
    pub rustc: RustcVersion,
}

/// What `rustc -vV` says about a toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustcVersion {
    /// The release, as `1.101.0-nightly`.
    pub release: Box<str>,
    /// The commit, in full.
    pub commit_hash: Box<str>,
    /// The date of the commit, as `2026-10-17`.
    pub commit_date: Box<str>,
    /// The target the toolchain runs on, which its `rust-docs-json` is for.
    pub host: TargetTriple,
}

impl RustcVersion {
    /// The toolchain, as the first line of `rustc -vV` gives it: `rustc 1.99.1 (0123456789
    /// 2026-10-17)`, with nine digits of the commit (requirement R-OUT-9).
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "rustc {} ({} {})",
            self.release,
            self.commit_hash.get(..9).unwrap_or(&self.commit_hash),
            self.commit_date
        )
    }
}

/// Why the nightly toolchain installed here is not used for std.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotUsed {
    /// There is no nightly toolchain, or rustup or rustc could not be run.
    #[error("no nightly toolchain could be run: {0}")]
    NoNightly(Cow<'static, str>),
    /// The nightly toolchain does not have the `rust-docs-json` component.
    #[error("the nightly toolchain has no rust-docs-json")]
    NoComponent,
    /// The nightly toolchain is older than the project's toolchain.
    #[error("the nightly toolchain {release} is older than the project's {at_least}")]
    TooOld {
        /// The release of the nightly toolchain.
        release: Box<str>,
        /// The release of the project's toolchain.
        at_least: Version,
    },
}

/// The JSON of std of the nightly toolchain installed with rustup (requirement R-STD-1).
///
/// It is used if it has `rust-docs-json` and is not older than `at_least`, the release of the
/// project's toolchain. rustup and rustc run in `neutral_dir`, where no project settings apply,
/// and rustup does not install anything.
///
/// # Errors
///
/// Returns why the nightly toolchain is not used.
pub async fn local_std(
    neutral_dir: &Path,
    at_least: Option<&Version>,
) -> Result<LocalStd, NotUsed> {
    let rustc = async |args: &'static [&'static str]| {
        let mut all = vec!["run", TOOLCHAIN, "rustc"];
        all.extend_from_slice(args);
        tool::output_kept(neutral_dir, tool::Place::Neutral, Path::new("rustup"), &all)
            .await
            .map_err(NotUsed::NoNightly)
    };
    let version = rustc(&["-vV"]).await?;
    let rustc_version = parse_rustc_version(&version).ok_or(NotUsed::NoNightly(Cow::Borrowed(
        "rustc -vV printed no release, commit or host",
    )))?;
    if let Some(at_least) = at_least
        && !is_at_least(&rustc_version.release, at_least)
    {
        return Err(NotUsed::TooOld {
            release: rustc_version.release,
            at_least: at_least.clone(),
        });
    }
    let sysroot = rustc(&["--print", "sysroot"]).await?;
    let dir = Path::new(sysroot.trim()).join("share/doc/rust/json");
    if !dir.join("std.json").is_file() {
        return Err(NotUsed::NoComponent);
    }
    Ok(LocalStd {
        dir,
        rustc: rustc_version,
    })
}

/// Why the sources of std of the nightly toolchain here are not used (decided in M3b, decision
/// 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NoSourceHere {
    /// rustup or rustc could not be run, or ran too long, which trying again may undo.
    #[error("the nightly toolchain here could not be run")]
    NotRun {
        /// Whether it ran too long.
        timed_out: bool,
    },
    /// The nightly toolchain is of another commit than the JSON of std.
    #[error("the nightly toolchain here is of another commit than the JSON of std")]
    OtherCommit,
    /// It has no `rust-src`, or one of another commit, as rustup's manifests tell.
    #[error("the nightly toolchain here has no rust-src of the JSON's commit")]
    NoComponent,
}

/// The sources of std of the nightly toolchain installed with rustup, if they are of `commit`
/// (decided in M3b, decision 20).
///
/// `commit` is the full commit of the JSON of std. The sources are its `rust-src`, under
/// `<sysroot>/lib/rustlib/src/rust`, whose `library/` the JSON's spans are in.
///
/// It runs the same `rustc -vV` and `rustc --print sysroot` as [`local_std`], in `neutral_dir`,
/// whose outputs are kept for a minute: no more processes within it, with the same directory (the
/// cache's, as the JSON of std is chosen in).
///
/// # Errors
///
/// Returns why they are not used.
pub async fn rust_src(neutral_dir: &Path, commit: &str) -> Result<PathBuf, NoSourceHere> {
    let rustc = async |args: &'static [&'static str]| {
        let mut all = vec!["run", TOOLCHAIN, "rustc"];
        all.extend_from_slice(args);
        tool::output_kept(neutral_dir, tool::Place::Neutral, Path::new("rustup"), &all)
            .await
            .map_err(|why| NoSourceHere::NotRun {
                timed_out: tool::timed_out(&why),
            })
    };
    let version = rustc(&["-vV"]).await?;
    if !of_commit(&version, commit) {
        return Err(NoSourceHere::OtherCommit);
    }
    let sysroot = rustc(&["--print", "sysroot"]).await?;
    rust_src_in(Path::new(sysroot.trim()), commit)
}

/// The sources of std of the nightly toolchain whose JSON of std `local` is, without running it:
/// its sysroot is where the JSON is (`<sysroot>/share/doc/rust/json`).
///
/// # Errors
///
/// Returns why they are not used.
pub fn rust_src_of(local: &LocalStd) -> Result<PathBuf, NoSourceHere> {
    let sysroot = local
        .dir
        .ancestors()
        .nth(4)
        .ok_or(NoSourceHere::NoComponent)?;
    rust_src_in(sysroot, &local.rustc.commit_hash)
}

/// Whether `rustc -vV` printed `version` of the full `commit`.
fn of_commit(version: &str, commit: &str) -> bool {
    parse_rustc_version(version).is_some_and(|version| *version.commit_hash == *commit)
}

/// The sources of std in `sysroot`, if rustup installed its `rust-src` (`manifest-rust-src`) of
/// `commit`, as rustup's manifest of the channel tells: its version names the commit, shortened.
fn rust_src_in(sysroot: &Path, commit: &str) -> Result<PathBuf, NoSourceHere> {
    let rustlib = sysroot.join("lib/rustlib");
    let root = rustlib.join("src/rust");
    let installed = rustlib.join("manifest-rust-src").is_file() && root.join("library").is_dir();
    let of_commit = channel_source_commit(&rustlib.join("multirust-channel-manifest.toml"))
        .is_some_and(|short| short.len() >= 7 && commit.starts_with(&*short));
    if installed && of_commit {
        Ok(root)
    } else {
        Err(NoSourceHere::NoComponent)
    }
}

/// The short commit of `rust-src` in rustup's manifest of the channel at `path`: the `version` of
/// its `[pkg.rust-src]`, as `1.101.0-nightly (41089d07a 2026-10-27)`. Its lines are read alone,
/// as the manifest is large.
fn channel_source_commit(path: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(MAX_CHANNEL_MANIFEST_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let mut lines = text
        .lines()
        .skip_while(|line| line.trim() != "[pkg.rust-src]");
    lines.next()?;
    let version = lines
        .take_while(|line| !line.trim_start().starts_with('['))
        .find_map(|line| line.trim().strip_prefix("version = \""))?;
    let hash = version.split_once('(')?.1.split(' ').next()?;
    hash.bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then(|| hash.to_owned())
}

/// The release of the toolchain that `rustc` is in `dir`, where the project's
/// `rust-toolchain.toml` applies (requirement R-STD-1), or `None` if it cannot be run there without
/// installing anything.
pub async fn rustc_release(dir: &Path, rustc: &Path) -> Option<Version> {
    let version = tool::output_kept(dir, tool::Place::Project, rustc, &["-vV"])
        .await
        .inspect_err(|why| tracing::debug!(%why, "the project's rustc could not be run"))
        .ok()?;
    let release = parse_rustc_version(&version)?.release;
    Version::parse(&release).ok()
}

/// Reads the JSON of `krate` from `local`.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is larger than the JSON held in memory.
pub fn read_local_std_json(local: &LocalStd, krate: StdCrate) -> io::Result<Box<[u8]>> {
    let path = local.dir.join(format!("{}.json", krate.as_str()));
    let mut json = Vec::new();
    File::open(&path)?
        .take(MAX_BYTES.saturating_add(1))
        .read_to_end(&mut json)?;
    if u64::try_from(json.len()).map_or(true, |len| len > MAX_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is larger than {MAX_BYTES} bytes", path.display()),
        ));
    }
    Ok(json.into_boxed_slice())
}

/// The release, commit and host in the output of `rustc -vV`.
pub(super) fn parse_rustc_version(output: &str) -> Option<RustcVersion> {
    let field = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    Some(RustcVersion {
        release: field("release")?.into(),
        commit_hash: field("commit-hash")?.into(),
        commit_date: field("commit-date")?.into(),
        host: field("host")?.parse().ok()?,
    })
}

/// Whether the release `release` (as `1.101.0-nightly`) is at least `at_least`, comparing only
/// the numbers: a nightly of 1.101 has what 1.100 stable has.
pub(super) fn is_at_least(release: &str, at_least: &Version) -> bool {
    Version::parse(release).is_ok_and(|release| {
        (release.major, release.minor, release.patch)
            >= (at_least.major, at_least.minor, at_least.patch)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NIGHTLY: &str = "rustc 1.101.0-nightly (8006091eb 2026-10-17)
binary: rustc
commit-hash: 8006091eb71ae7d9e9bccb66c6122c9d68af4466
commit-date: 2026-10-17
host: x86_64-unknown-linux-gnu
release: 1.101.0-nightly
LLVM version: 23.1.4
";

    #[test]
    fn rustc_vv_is_read() {
        assert_eq!(
            parse_rustc_version(NIGHTLY),
            Some(RustcVersion {
                release: "1.101.0-nightly".into(),
                commit_hash: "8006091eb71ae7d9e9bccb66c6122c9d68af4466".into(),
                commit_date: "2026-10-17".into(),
                host: "x86_64-unknown-linux-gnu".parse().expect("a triple"),
            })
        );
        assert_eq!(parse_rustc_version("rustc 1.101.0-nightly\n"), None);
    }

    #[test]
    fn a_nightly_is_used_unless_it_is_older_than_the_project() {
        let at_least = |text: &str| Version::parse(text).expect("a version");
        assert!(is_at_least("1.101.0-nightly", &at_least("1.99.1")));
        assert!(is_at_least("1.101.0-nightly", &at_least("1.101.0")));
        assert!(!is_at_least("1.99.0-nightly", &at_least("1.99.1")));
        assert!(!is_at_least("not a version", &at_least("1.0.0")));
    }

    #[test]
    fn the_json_is_read_from_the_sysroot() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(dir.path().join("core.json"), b"{}").expect("written");
        let local = LocalStd {
            dir: dir.path().to_path_buf(),
            rustc: parse_rustc_version(NIGHTLY).expect("parsed"),
        };
        assert_eq!(
            &*read_local_std_json(&local, StdCrate::Core).expect("read"),
            b"{}"
        );
        assert_eq!(
            read_local_std_json(&local, StdCrate::Std)
                .expect_err("no std.json")
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    /// A sysroot with the `rust-src` of the commit that rustup's manifest of the channel gives
    /// as `short`, if `installed`.
    fn sysroot(dir: &Path, short: &str, installed: bool) -> PathBuf {
        let rustlib = dir.join("lib/rustlib");
        std::fs::create_dir_all(rustlib.join("src/rust/library")).expect("made");
        std::fs::write(
            rustlib.join("multirust-channel-manifest.toml"),
            format!(
                "[pkg.rustc]\nversion = \"1.101.0-nightly (000000000 2026-10-17)\"\n\n\
                 [pkg.rust-src]\nversion = \"1.101.0-nightly ({short} 2026-10-17)\"\n\n\
                 [pkg.rust-src.target.\"*\"]\navailable = true\n"
            ),
        )
        .expect("written");
        if installed {
            std::fs::write(rustlib.join("manifest-rust-src"), "").expect("written");
        }
        rustlib.join("src/rust")
    }

    /// The sources of std of the nightly here are used only for its own commit, in full, and only
    /// where rustup installed its `rust-src` of that commit (decided in M3b, decision 20).
    #[test]
    fn the_sources_here_are_of_the_nightlys_commit() {
        const COMMIT: &str = "8006091eb71ae7d9e9bccb66c6122c9d68af4466";
        assert!(of_commit(NIGHTLY, COMMIT));
        assert!(!of_commit(NIGHTLY, "8006091eb"));
        assert!(!of_commit(
            NIGHTLY,
            "0123456789012345678901234567890123456789"
        ));
        assert!(!of_commit("not rustc -vV", COMMIT));
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = sysroot(dir.path(), "8006091eb", true);
        assert_eq!(rust_src_in(dir.path(), COMMIT), Ok(root));
        let other = tempfile::tempdir().expect("a temporary directory");
        sysroot(other.path(), "41089d07a", true);
        assert_eq!(
            rust_src_in(other.path(), COMMIT),
            Err(NoSourceHere::NoComponent)
        );
        let missing = tempfile::tempdir().expect("a temporary directory");
        sysroot(missing.path(), "8006091eb", false);
        assert_eq!(
            rust_src_in(missing.path(), COMMIT),
            Err(NoSourceHere::NoComponent)
        );
        // Of the JSON here, by its sysroot, without running anything.
        let local = LocalStd {
            dir: dir.path().join("share/doc/rust/json"),
            rustc: parse_rustc_version(NIGHTLY).expect("parsed"),
        };
        assert_eq!(
            rust_src_of(&local),
            Ok(dir.path().join("lib/rustlib/src/rust"))
        );
    }

    #[tokio::test]
    async fn the_nightly_here_is_looked_for_without_installing_it() {
        // Whatever this machine has, the answer is one of these, and nothing is installed.
        let dir = tempfile::tempdir().expect("a temporary directory");
        match local_std(dir.path(), Some(&Version::new(1, 0, 0))).await {
            Ok(local) => assert!(local.dir.join("std.json").is_file()),
            Err(NotUsed::NoNightly(_) | NotUsed::NoComponent) => {},
            Err(error @ NotUsed::TooOld { .. }) => panic!("{error}"),
        }
        // Its sources, whatever this machine has: none of a commit that no nightly has.
        rust_src(dir.path(), "0000000000000000000000000000000000000000")
            .await
            .expect_err("no nightly has it");
    }
}
