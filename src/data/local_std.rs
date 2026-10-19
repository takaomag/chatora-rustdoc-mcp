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
        tool::output(neutral_dir, "rustup", &all)
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

/// The release of the toolchain that `rustc` is in `dir`, where the project's
/// `rust-toolchain.toml` applies (requirement R-STD-1), or `None` if it cannot be run there without
/// installing anything.
pub async fn rustc_release(dir: &Path) -> Option<Version> {
    let version = tool::output(dir, "rustc", &["-vV"])
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
fn parse_rustc_version(output: &str) -> Option<RustcVersion> {
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
fn is_at_least(release: &str, at_least: &Version) -> bool {
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

    #[tokio::test]
    async fn the_nightly_here_is_looked_for_without_installing_it() {
        // Whatever this machine has, the answer is one of these, and nothing is installed.
        let dir = tempfile::tempdir().expect("a temporary directory");
        match local_std(dir.path(), Some(&Version::new(1, 0, 0))).await {
            Ok(local) => assert!(local.dir.join("std.json").is_file()),
            Err(NotUsed::NoNightly(_) | NotUsed::NoComponent) => {},
            Err(error @ NotUsed::TooOld { .. }) => panic!("{error}"),
        }
    }
}
