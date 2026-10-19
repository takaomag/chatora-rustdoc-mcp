//! Version resolution against the versions in the crates.io index (requirements R-VR-1, R-VR-3
//! and R-VR-4).
//!
//! Resolution from the project's `Cargo.lock` (R-VR-2) comes with the project context (B5).

use std::{borrow::Cow, fmt, str::FromStr};

use semver::{Version, VersionReq};

use super::sparse_index::IndexVersion;

/// Most versions suggested when the asked one is not found.
const MAX_SUGGESTIONS: usize = 5;

/// A version as it was asked for (requirement R-VR-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// `x.y.z[-pre][+build]`: exactly this version. `0.8.9` is not Cargo's `^0.8.9`.
    Exact(Version),
    /// A semver requirement such as `^1`, `~1.2`, `1` or `>=1, <2`: the largest matching version.
    Requirement(VersionReq),
    /// `latest`, `newest` or `*`: the largest version.
    Latest,
}

/// A version that could not be parsed. It is not taken as `*` (requirement R-VR-1).
#[derive(Debug, thiserror::Error)]
#[error("{spec:?} is not a version, a semver requirement or `latest`")]
pub struct VersionSpecError {
    spec: Box<str>,
    /// Why it is not a semver requirement.
    #[source]
    source: semver::Error,
}

/// The version that a [`VersionSpec`] resolved to.
#[derive(Debug, PartialEq, Eq)]
pub struct Resolved<'a> {
    /// The version in the index, with its build metadata.
    pub version: &'a IndexVersion,
    /// What the response should say about how it was chosen: only pre-release versions, only
    /// yanked ones, or a yanked version asked for exactly.
    pub warnings: Vec<Cow<'static, str>>,
}

/// No version in the index fits the [`VersionSpec`].
#[derive(Debug, PartialEq, Eq)]
pub struct NoVersion {
    /// Versions to suggest instead, largest first: those close to the one asked for, or else the
    /// largest ones that are not yanked.
    pub suggestions: Box<[Version]>,
}

impl FromStr for VersionSpec {
    type Err = VersionSpecError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let spec = spec.trim();
        if spec == "*" || spec.eq_ignore_ascii_case("latest") || spec.eq_ignore_ascii_case("newest")
        {
            return Ok(Self::Latest);
        }
        if let Ok(version) = Version::parse(spec) {
            return Ok(Self::Exact(version));
        }
        VersionReq::parse(spec)
            .map(Self::Requirement)
            .map_err(|source| VersionSpecError {
                spec: spec.into(),
                source,
            })
    }
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(version) => write!(f, "{version}"),
            Self::Requirement(requirement) => write!(f, "{requirement}"),
            Self::Latest => f.write_str("latest"),
        }
    }
}

impl VersionSpec {
    /// The version in `versions` that this spec names.
    ///
    /// # Errors
    ///
    /// Returns [`NoVersion`], with versions to suggest, when none fits.
    pub fn resolve<'a>(&self, versions: &'a [IndexVersion]) -> Result<Resolved<'a>, NoVersion> {
        match self {
            Self::Exact(version) => resolve_exact(version, versions),
            Self::Requirement(requirement) => resolve_requirement(requirement, versions),
            Self::Latest => resolve_requirement(&VersionReq::STAR, versions),
        }
    }
}

/// The version equal to `asked`. Without build metadata, `asked` also names the one version that
/// differs from it only in build metadata (`2.1.0` is `2.1.0+zstd.1.5.7`).
fn resolve_exact<'a>(
    asked: &Version,
    versions: &'a [IndexVersion],
) -> Result<Resolved<'a>, NoVersion> {
    let found = versions
        .iter()
        .find(|entry| entry.version == *asked)
        .or_else(|| {
            if !asked.build.is_empty() {
                return None;
            }
            let mut same = versions
                .iter()
                .filter(|entry| without_build(&entry.version) == *asked);
            match (same.next(), same.next()) {
                (Some(only), None) => Some(only),
                // Several builds of the version: which one is meant is not known.
                _ => None,
            }
        });
    let Some(found) = found else {
        return Err(NoVersion {
            suggestions: suggest(versions, Some(asked)),
        });
    };
    let mut warnings = Vec::new();
    if found.yanked {
        warnings.push(Cow::Owned(format!("{} is yanked", found.version)));
    }
    Ok(Resolved {
        version: found,
        warnings,
    })
}

/// The largest version that matches `requirement`, looked for in turn among the versions that
/// are not yanked and not pre-releases, then among the pre-releases that are not yanked, then
/// among all (requirement R-VR-3, decided in M1).
fn resolve_requirement<'a>(
    requirement: &VersionReq,
    versions: &'a [IndexVersion],
) -> Result<Resolved<'a>, NoVersion> {
    // A pre-release matches a requirement only when the requirement names it, so the second and
    // third tiers also match a pre-release by the release it comes before.
    let matches = |version: &Version| {
        requirement.matches(version) || requirement.matches(&without_pre(version))
    };
    let largest = |tier: &dyn Fn(&IndexVersion) -> bool| {
        versions
            .iter()
            .filter(|entry| tier(entry))
            .max_by(|a, b| a.version.cmp(&b.version))
    };
    if let Some(found) = largest(&|entry| {
        !entry.yanked && entry.version.pre.is_empty() && requirement.matches(&entry.version)
    }) {
        return Ok(Resolved {
            version: found,
            warnings: Vec::new(),
        });
    }
    if let Some(found) = largest(&|entry| !entry.yanked && matches(&entry.version)) {
        return Ok(Resolved {
            version: found,
            warnings: vec![Cow::Owned(format!(
                "only pre-release versions match {requirement}; resolved to {}",
                found.version
            ))],
        });
    }
    if let Some(found) = largest(&|entry| matches(&entry.version)) {
        return Ok(Resolved {
            version: found,
            warnings: vec![Cow::Owned(format!(
                "every version that matches {requirement} is yanked; resolved to {}",
                found.version
            ))],
        });
    }
    Err(NoVersion {
        suggestions: suggest(versions, None),
    })
}

/// Up to [`MAX_SUGGESTIONS`] versions that are not yanked, largest first: with the major and
/// minor version of `near` if there are any, else with its major version, else any.
fn suggest(versions: &[IndexVersion], near: Option<&Version>) -> Box<[Version]> {
    let not_yanked = || versions.iter().filter(|entry| !entry.yanked);
    let closest = near.and_then(|near| {
        let same_minor: Vec<&IndexVersion> = not_yanked()
            .filter(|entry| entry.version.major == near.major && entry.version.minor == near.minor)
            .collect();
        if !same_minor.is_empty() {
            return Some(same_minor);
        }
        let same_major: Vec<&IndexVersion> = not_yanked()
            .filter(|entry| entry.version.major == near.major)
            .collect();
        (!same_major.is_empty()).then_some(same_major)
    });
    let mut chosen: Vec<&Version> = closest
        .unwrap_or_else(|| not_yanked().collect())
        .into_iter()
        .map(|entry| &entry.version)
        .collect();
    chosen.sort_unstable_by(|a, b| b.cmp(a));
    chosen.into_iter().take(MAX_SUGGESTIONS).cloned().collect()
}

fn without_build(version: &Version) -> Version {
    Version {
        build: semver::BuildMetadata::EMPTY,
        ..version.clone()
    }
}

fn without_pre(version: &Version) -> Version {
    Version {
        pre: semver::Prerelease::EMPTY,
        ..version.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(versions: &[(&str, bool)]) -> Vec<IndexVersion> {
        versions
            .iter()
            .map(|&(version, yanked)| IndexVersion {
                version: Version::parse(version).expect("a version"),
                checksum: "00".into(),
                yanked,
                rust_version: None,
            })
            .collect()
    }

    fn spec(spec: &str) -> VersionSpec {
        spec.parse().expect("a version spec")
    }

    fn resolved(spec_text: &str, versions: &[IndexVersion]) -> (String, Vec<String>) {
        let resolved = spec(spec_text).resolve(versions).expect("resolves");
        (
            resolved.version.version.to_string(),
            resolved.warnings.iter().map(ToString::to_string).collect(),
        )
    }

    #[test]
    fn specs_are_parsed() {
        // An exact version is not Cargo's caret: a regression in other servers.
        assert_eq!(spec("0.8.9"), VersionSpec::Exact(Version::new(0, 8, 9)));
        assert_eq!(spec(" Latest "), VersionSpec::Latest);
        assert_eq!(spec("NEWEST"), VersionSpec::Latest);
        assert_eq!(spec("*"), VersionSpec::Latest);
        for requirement in ["^1", "~1.2", "1", "1.2", ">=1, <2"] {
            assert!(
                matches!(spec(requirement), VersionSpec::Requirement(_)),
                "{requirement}"
            );
        }
        for invalid in ["", "abc", "1.2.3.4", "v1"] {
            assert!(invalid.parse::<VersionSpec>().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn versions_compare_as_semver() {
        let versions = index(&[("0.9.0", false), ("0.10.0", false)]);
        assert_eq!(resolved("latest", &versions).0, "0.10.0");
        assert_eq!(resolved("0.9", &versions).0, "0.9.0");
    }

    #[test]
    fn an_exact_version_without_build_metadata_finds_its_only_build() {
        let versions = index(&[("2.1.0+zstd.1.5.6", false), ("2.1.2+zstd.1.5.7", false)]);
        assert_eq!(resolved("2.1.0", &versions).0, "2.1.0+zstd.1.5.6");
        assert_eq!(
            resolved("2.1.2+zstd.1.5.7", &versions).0,
            "2.1.2+zstd.1.5.7"
        );
        // Two builds of one version: which is meant is not known.
        let two = index(&[("1.0.0+a", false), ("1.0.0+b", false)]);
        spec("1.0.0")
            .resolve(&two)
            .expect_err("which build is meant is not known");
    }

    #[test]
    fn a_yanked_version_asked_for_exactly_is_resolved_with_a_warning() {
        let versions = index(&[("1.0.0", true), ("1.0.1", false)]);
        assert_eq!(
            resolved("1.0.0", &versions),
            ("1.0.0".to_owned(), vec!["1.0.0 is yanked".to_owned()])
        );
    }

    #[test]
    fn latest_skips_yanked_and_pre_release_versions() {
        let versions = index(&[("1.0.0", false), ("1.1.0", true), ("2.0.0-rc.1", false)]);
        assert_eq!(resolved("latest", &versions), ("1.0.0".to_owned(), vec![]));
        assert_eq!(resolved("^1", &versions), ("1.0.0".to_owned(), vec![]));
    }

    #[test]
    fn latest_falls_back_to_pre_releases_then_to_yanked_versions() {
        let only_pre = index(&[("0.1.0-alpha.2", false), ("0.1.0-alpha.3", false)]);
        assert_eq!(
            resolved("latest", &only_pre),
            ("0.1.0-alpha.3".to_owned(), vec![
                "only pre-release versions match *; resolved to 0.1.0-alpha.3".to_owned()
            ])
        );
        assert_eq!(resolved("0.1", &only_pre).0, "0.1.0-alpha.3");
        let all_yanked = index(&[("1.0.0", true), ("1.1.0", true)]);
        assert_eq!(
            resolved("latest", &all_yanked),
            ("1.1.0".to_owned(), vec![
                "every version that matches * is yanked; resolved to 1.1.0".to_owned()
            ])
        );
    }

    #[test]
    fn a_missing_version_suggests_close_ones() {
        let versions = index(&[
            ("1.0.0", false),
            ("1.2.0", false),
            ("1.2.5", false),
            ("1.2.6", true),
            ("2.0.0", false),
        ]);
        assert_eq!(
            spec("1.2.9").resolve(&versions),
            Err(NoVersion {
                suggestions: vec![Version::new(1, 2, 5), Version::new(1, 2, 0)].into(),
            })
        );
        assert_eq!(
            spec("1.7.0").resolve(&versions),
            Err(NoVersion {
                suggestions: vec![
                    Version::new(1, 2, 5),
                    Version::new(1, 2, 0),
                    Version::new(1, 0, 0)
                ]
                .into(),
            })
        );
        assert_eq!(
            spec("^3").resolve(&versions),
            Err(NoVersion {
                suggestions: vec![
                    Version::new(2, 0, 0),
                    Version::new(1, 2, 5),
                    Version::new(1, 2, 0),
                    Version::new(1, 0, 0)
                ]
                .into(),
            })
        );
    }
}
