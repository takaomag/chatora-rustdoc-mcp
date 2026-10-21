//! Target triples: checked before they go into URLs and paths (requirement R-SEC-4), and the
//! host's, which the documentation is looked up for by default (requirement R-TG-1).

use std::{borrow::Cow, fmt, path::Path, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Longest target triple accepted. The longest built into rustc is about 40 characters.
const MAX_LEN: usize = 100;

/// A target triple, such as `x86_64-unknown-linux-gnu`, made of ASCII letters, digits, `-`, `_`
/// and `.`, which is safe in a URL path and a file name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetTriple(Box<str>);

/// A string that is not a target triple.
#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a target triple")]
pub struct TargetTripleError(Box<str>);

impl FromStr for TargetTriple {
    type Err = TargetTripleError;

    fn from_str(triple: &str) -> Result<Self, Self::Err> {
        let valid = !triple.is_empty()
            && triple.len() <= MAX_LEN
            && triple
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && triple
                .bytes()
                .next()
                .is_some_and(|first| first.is_ascii_alphanumeric())
            && !triple.contains("..");
        if valid {
            Ok(Self(triple.into()))
        } else {
            Err(TargetTripleError(triple.into()))
        }
    }
}

impl fmt::Display for TargetTriple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for TargetTriple {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Read with the same checks as [`FromStr`], so that a file cannot make an unchecked triple.
impl<'de> Deserialize<'de> for TargetTriple {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let triple = <Cow<'de, str>>::deserialize(deserializer)?;
        triple.parse().map_err(de::Error::custom)
    }
}

impl TargetTriple {
    /// The triple as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The host target, and why it is a stand-in if it is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTarget {
    /// The triple.
    pub triple: TargetTriple,
    /// Why `rustc -vV` did not tell the host target, when the triple is the stand-in that
    /// [`build_target`] gives. It goes into the warnings of the responses.
    pub stand_in_because: Option<Cow<'static, str>>,
}

/// The host target, from `rustc -vV`, or else the target this server was built for.
///
/// rustc runs in `neutral_dir`, a directory without project settings (the cache), so that a
/// project's `rust-toolchain.toml` or `.cargo/config.toml` does not apply (requirement R-TG-1).
#[cfg(feature = "local")]
pub async fn host_target(neutral_dir: &Path) -> HostTarget {
    match rustc_host(neutral_dir).await {
        Ok(triple) => HostTarget {
            triple,
            stand_in_because: None,
        },
        Err(why) => {
            tracing::warn!(%why, "rustc -vV did not tell the host target");
            stand_in(why)
        },
    }
}

/// The host target, which a server built without local features cannot ask rustc for: the target
/// this server was built for stands in for it (requirement R-TG-1).
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs rustc"
)]
pub async fn host_target(_neutral_dir: &Path) -> HostTarget {
    stand_in(Cow::Borrowed(
        "this server is built without local features, so it does not run rustc",
    ))
}

/// [`build_target`], with the reason it stands in for the host target.
fn stand_in(why: Cow<'static, str>) -> HostTarget {
    HostTarget {
        triple: build_target(),
        stand_in_because: Some(why),
    }
}

/// The target this server was built for, with a musl environment read as glibc: the server may be
/// distributed as a static musl binary, while the Rust that users write for runs on glibc.
#[must_use]
pub fn build_target() -> TargetTriple {
    let built_for = env!("CHATORA_RUSTDOC_BUILD_TARGET");
    musl_as_gnu(built_for)
        .parse()
        // Cargo always sets the target of a build script, and rustc's targets are valid.
        .unwrap_or_else(|_| TargetTriple("x86_64-unknown-linux-gnu".into()))
}

/// `triple` with an environment that starts with `musl` read as `gnu`: `x86_64-unknown-linux-musl`
/// as `x86_64-unknown-linux-gnu`, `armv7-unknown-linux-musleabihf` as
/// `armv7-unknown-linux-gnueabihf`.
fn musl_as_gnu(triple: &str) -> String {
    match triple.rsplit_once('-') {
        Some((rest, environment)) if environment.starts_with("musl") => {
            format!("{rest}-gnu{}", environment.trim_start_matches("musl"))
        },
        _ => triple.to_owned(),
    }
}

/// Runs `rustc -vV` in `dir` and reads its `host:` line.
#[cfg(feature = "local")]
async fn rustc_host(dir: &Path) -> Result<TargetTriple, Cow<'static, str>> {
    let version = super::tool::output(dir, super::tool::Place::Neutral, Path::new("rustc"), &[
        "-vV",
    ])
    .await?;
    parse_host(&version).ok_or(Cow::Borrowed("rustc -vV printed no valid host line"))
}

/// The triple on the `host:` line of the output of `rustc -vV`.
#[cfg(feature = "local")]
fn parse_host(version: &str) -> Option<TargetTriple> {
    version
        .lines()
        .find_map(|line| line.strip_prefix("host:"))
        .and_then(|host| host.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triples_are_checked() {
        for valid in [
            "x86_64-unknown-linux-gnu",
            "aarch64-apple-darwin",
            "thumbv7em-none-eabihf",
            "wasm32-wasip1",
            "x86_64-pc-windows-msvc",
        ] {
            assert!(valid.parse::<TargetTriple>().is_ok(), "{valid}");
        }
        for invalid in ["", "-x86", "../etc", "x86_64/linux", "a b", "x86..y", "é-x"] {
            assert!(invalid.parse::<TargetTriple>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn musl_is_read_as_gnu() {
        assert_eq!(
            musl_as_gnu("x86_64-unknown-linux-musl"),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            musl_as_gnu("armv7-unknown-linux-musleabihf"),
            "armv7-unknown-linux-gnueabihf"
        );
        assert_eq!(musl_as_gnu("aarch64-apple-darwin"), "aarch64-apple-darwin");
    }

    #[cfg(feature = "local")]
    #[test]
    fn the_host_is_read_from_rustc_vv() {
        let version = "rustc 1.99.1 (abc 2026-10-13)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.1\n";
        assert_eq!(
            parse_host(version)
                .map(|triple| triple.to_string())
                .as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(parse_host("rustc 1.99.1\n"), None);
    }

    #[test]
    fn the_build_target_is_a_triple() {
        assert!(!build_target().as_str().contains("musl"));
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn rustc_tells_the_host_target() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let host = host_target(dir.path()).await;
        assert_eq!(host.stand_in_because, None, "rustc -vV works in the tests");
        assert_ne!(host.triple.as_str(), "");
    }
}
