//! The releases of a changelog, and those of a range of versions (decided in M3g, decisions 114,
//! 116 and 117).
//!
//! A release starts at a heading, of any level, that starts with a version once made plain:
//! `## [1.2.3] - 2024-01-01`, `## 1.2.3 (2024-01-01)`, `# Version 1.2.3`, `## v1.2`, or the
//! crate's name and a version (`## parking_lot 0.12.3`). It goes until the next release or
//! `Unreleased`. A heading that names a version further in (`### Migration notes for 2.22.0`) is
//! part of the release it is in.

use semver::{Prerelease, Version};

use super::markdown::{self, Heading};

/// A release of a changelog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its version, `.0` added to one of two parts (`1.2`).
    pub version: Version,
    /// Its heading, made plain.
    pub heading: String,
    /// The line its heading starts at, from 1.
    pub line: usize,
    /// The date that its heading tells, as written (`2024-01-01`).
    pub date: Option<String>,
    /// Whether its heading says that it was yanked.
    pub yanked: bool,
}

/// The releases of a changelog, in its order, and where its sections end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Releases {
    /// The releases.
    pub all: Vec<Release>,
    /// The line of the `Unreleased` heading, from 1.
    pub unreleased: Option<usize>,
    /// The lines, from 1, of the headings that end a section without starting a release: of
    /// another crate's release in the same changelog (`## parking_lot_core 0.9.9`), and the start
    /// of the definitions of the links' labels at its end (`[1.2.3]: https://…`).
    pub ends: Vec<usize>,
}

impl Releases {
    /// The lines of the release at `index`, or of `Unreleased` for `None`: from its heading to the
    /// next release's, `Unreleased`'s or another end, or the end of the `lines` of the changelog.
    /// Lines from 1, the end not included; `None` for a release or an `Unreleased` that is not
    /// there.
    #[must_use]
    pub fn lines_of(&self, index: Option<usize>, lines: usize) -> Option<(usize, usize)> {
        let start = match index {
            Some(index) => self.all.get(index)?.line,
            None => self.unreleased?,
        };
        let end = self
            .all
            .iter()
            .map(|release| release.line)
            .chain(self.unreleased)
            .chain(self.ends.iter().copied())
            .filter(|&line| line > start)
            .min()
            .unwrap_or_else(|| lines.saturating_add(1));
        Some((start, end))
    }
}

/// The headings that start the section of what is not released yet, made plain and in lowercase.
const UNRELEASED: [&str; 4] = ["unreleased", "upcoming", "head", "vnext"];

/// A version that a heading starts with, and what follows it.
#[derive(Debug)]
struct Headed {
    version: Version,
    date: Option<String>,
    yanked: bool,
    /// Whether nothing but a date, a parenthesis, a dash, a link or `yanked` follows the version:
    /// the heading is of the release alone, not a title that starts with a number
    /// (`## 1.2 Introduction`) or a note in a release (`### v1.3 MCP Apps`).
    alone: bool,
    /// Whether it says that it is not released yet (`## [2.16.0] - Unreleased`).
    unreleased: bool,
}

/// The releases of the Markdown changelog `text` of the crate `crate_name`.
///
/// A heading deeper than most of the releases' is one only if its version stands alone, and one
/// of the version of the release it is in is not another (`### pmcp 2.3.0` in `## [2.3.0]`).
#[must_use]
pub fn releases(text: &str, crate_name: &str) -> Releases {
    let outline = markdown::outline(text);
    let headed: Vec<(&Heading, String, Option<Headed>)> = outline
        .iter()
        .map(|heading| {
            let plain = markdown::plain(&heading.text);
            let headed = version_of(&plain, crate_name);
            (heading, plain, headed)
        })
        .collect();
    let level = main_level(headed.iter().filter_map(|(heading, _, headed)| {
        headed
            .as_ref()
            .filter(|headed| headed.alone && !headed.unreleased)
            .map(|_| heading.level)
    }));
    let mut found = Releases::default();
    for (heading, plain, headed) in headed {
        let unreleased = headed
            .as_ref()
            .map_or_else(|| is_unreleased(&plain), |headed| headed.unreleased);
        if unreleased {
            found.unreleased.get_or_insert(heading.line);
            continue;
        }
        match headed {
            Some(headed) if headed.alone || heading.level <= level => {
                let again = found
                    .all
                    .last()
                    .is_some_and(|last| last.version == headed.version);
                if !again {
                    found.all.push(Release {
                        version: headed.version,
                        heading: plain,
                        line: heading.line,
                        date: headed.date,
                        yanked: headed.yanked,
                    });
                }
            },
            None if heading.level <= level && is_another_release(&plain) => {
                found.ends.push(heading.line);
            },
            Some(_) | None => {},
        }
    }
    if let Some(references) = references_at_end(text) {
        found.ends.push(references);
    }
    found
}

/// The level of most of `levels`, the highest of those as many; 6 for none.
fn main_level(levels: impl Iterator<Item = u8>) -> u8 {
    let mut counts = [0_usize; 7];
    for level in levels {
        if let Some(count) = counts.get_mut(usize::from(level)) {
            *count = count.saturating_add(1);
        }
    }
    (1..=6_u8)
        .max_by_key(|&level| {
            (
                counts.get(usize::from(level)).copied().unwrap_or_default(),
                std::cmp::Reverse(level),
            )
        })
        .filter(|&level| {
            counts
                .get(usize::from(level))
                .is_some_and(|&count| count > 0)
        })
        .unwrap_or(6)
}

/// Whether the heading `plain` starts the section of what is not released yet.
fn is_unreleased(plain: &str) -> bool {
    let words = plain.trim_start_matches(['[', '(']).to_ascii_lowercase();
    UNRELEASED.iter().any(|name| {
        words
            .strip_prefix(name)
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
    })
}

/// Whether the heading `plain` is of another crate's release: a name and then a version
/// (`parking_lot_core 0.9.9`, `Release rayon-core 1.10.2`).
fn is_another_release(plain: &str) -> bool {
    let rest = without_word_before(plain.trim().trim_start_matches(['[', '(']).trim_start());
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        .unwrap_or(rest.len());
    let (name, after) = rest.split_at(name_end);
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && version_at(after.trim_start_matches(SEPARATORS)).is_some()
}

/// What may come between a crate's name and its version in a heading: `parking_lot - 0.12.5`,
/// `parking_lot: 0.12.5`, `parking_lot@0.12.5`.
const SEPARATORS: [char; 7] = [' ', '\t', '-', ':', '@', '—', '–'];

/// `rest` without a word before a version: `Version`, `Release` or `CHANGES WITH` (as systemd's).
fn without_word_before(rest: &str) -> &str {
    for prefix in ["version", "release", "changes with"] {
        if let Some(after) = strip_prefix_ignoring_case(rest, prefix)
            && after.starts_with(|c: char| c.is_whitespace() || c.is_ascii_digit())
        {
            return after.trim_start();
        }
    }
    rest
}

/// The version that the heading `plain` of `crate_name`'s changelog starts with, and what
/// follows it.
fn version_of(plain: &str, crate_name: &str) -> Option<Headed> {
    let mut rest = without_word_before(plain.trim().trim_start_matches(['[', '(']).trim_start());
    if let Some(after) = strip_crate_name(rest, crate_name) {
        rest = after;
    }
    let (version, two_parts, after) = version_at(rest)?;
    let tail = after.trim_start_matches([']', ')']).trim();
    let lowercase = tail.to_ascii_lowercase();
    let alone = tail.is_empty()
        || tail.starts_with(['(', '[', '-', '—', '–', ':', ',', '/', '|'])
        || tail.starts_with(|c: char| c.is_ascii_digit())
        || lowercase.starts_with("yanked")
        || lowercase.starts_with("unreleased");
    Some(Headed {
        version,
        date: date_in(after),
        yanked: lowercase.contains("yanked"),
        // A version of two parts with words after it is a section's number.
        alone: alone || (!two_parts && tail.is_empty()),
        unreleased: lowercase.contains("unreleased"),
    })
    .filter(|headed| headed.alone || !two_parts)
}

/// The version that `text` starts with, `v` or not, whether it has two parts only, and what follows
/// it.
fn version_at(text: &str) -> Option<(Version, bool, &str)> {
    let rest = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')))
        .unwrap_or(rest.len());
    let token = rest.get(..end)?.trim_end_matches(['.', '-']);
    let version = parse_version(token)?;
    let two_parts = token
        .split(['-', '+'])
        .next()
        .is_some_and(|core| core.matches('.').count() == 1);
    Some((version, two_parts, rest.get(end..).unwrap_or_default()))
}

/// Where the definitions of the links' labels at the end of `text` start (`[1.2.3]: https://…`),
/// as a line from 1, if it ends with them.
fn references_at_end(text: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let is_reference = |line: &str| {
        let line = line.trim_start();
        line.starts_with('[')
            && line
                .find("]:")
                .is_some_and(|at| at > 1 && !line.get(1..at).unwrap_or_default().contains(']'))
    };
    let mut start = None;
    for (index, line) in lines.iter().enumerate().rev() {
        if line.trim().is_empty() {
            continue;
        }
        if !is_reference(line) {
            break;
        }
        start = Some(index.saturating_add(1));
    }
    start
}

/// `text` without `prefix` at its start, whatever their case.
fn strip_prefix_ignoring_case<'t>(text: &'t str, prefix: &str) -> Option<&'t str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| text.get(prefix.len()..))
        .flatten()
}

/// `text` without `crate_name` and what separates it from a version at its start, whatever their
/// case, `-` and `_` alike.
fn strip_crate_name<'t>(text: &'t str, crate_name: &str) -> Option<&'t str> {
    let head = text.get(..crate_name.len())?;
    let same = head.bytes().zip(crate_name.bytes()).all(|(a, b)| {
        a.eq_ignore_ascii_case(&b) || (matches!(a, b'-' | b'_') && matches!(b, b'-' | b'_'))
    });
    let after = text.get(crate_name.len()..)?;
    (same && after.starts_with(SEPARATORS)).then(|| after.trim_start_matches(SEPARATORS))
}

/// `token` as a version, of three parts or two (`1.2`, read as `1.2.0`).
fn parse_version(token: &str) -> Option<Version> {
    if let Ok(version) = Version::parse(token) {
        return Some(version);
    }
    let (core, pre) = token
        .split_once('-')
        .map_or((token, None), |(core, pre)| (core, Some(pre)));
    let mut parts = core.split('.');
    let (Some(major), Some(minor), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let mut version = Version::new(major.parse().ok()?, minor.parse().ok()?, 0);
    if let Some(pre) = pre {
        version.pre = Prerelease::new(pre).ok()?;
    }
    Some(version)
}

/// The first ISO date (`2024-01-01`) in `text`.
fn date_in(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    (0..bytes.len()).find_map(|at| {
        let end = at.checked_add(10)?;
        let candidate = text.get(at..end)?;
        let shape = candidate
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        let digit_at = |at: Option<usize>| {
            at.and_then(|at| bytes.get(at))
                .is_some_and(u8::is_ascii_digit)
        };
        (shape && !digit_at(at.checked_sub(1)) && !digit_at(Some(end)))
            .then(|| candidate.to_owned())
    })
}

/// An end of a range of versions: a version in full, or only its first parts (`0.8`, `1`), which
/// stands for every version that starts with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bound {
    /// A version in full.
    Exact(Version),
    /// The first one or two parts.
    Parts(Vec<u64>),
}

impl Bound {
    /// `text` as an end of a range (`0.8`, `v0.8.1`, `=1.2.3-rc.1`).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text
            .trim()
            .trim_start_matches('=')
            .trim_start_matches(['v', 'V']);
        if let Ok(version) = Version::parse(text) {
            return Some(Self::Exact(version));
        }
        let parts: Vec<u64> = text
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        (1..=2).contains(&parts.len()).then_some(Self::Parts(parts))
    }

    /// `version`'s parts that `self` has, to compare with its own.
    fn of<'v>(&self, version: &'v Version) -> (Vec<u64>, &'v Version) {
        let parts = match self {
            Self::Exact(_) => Vec::new(),
            Self::Parts(parts) => [version.major, version.minor, version.patch]
                .into_iter()
                .take(parts.len())
                .collect(),
        };
        (parts, version)
    }

    /// Whether `version` is after `self`, as `from` takes it: a version that starts with its
    /// parts is not.
    #[must_use]
    pub fn is_before(&self, version: &Version) -> bool {
        match self {
            Self::Exact(from) => version.cmp_precedence(from).is_gt(),
            Self::Parts(parts) => self.of(version).0 > *parts,
        }
    }

    /// Whether `version` is not after `self`, as `to` takes it: a version that starts with its
    /// parts is not after it.
    #[must_use]
    pub fn is_not_before(&self, version: &Version) -> bool {
        match self {
            Self::Exact(to) => version.cmp_precedence(to).is_le(),
            Self::Parts(parts) => self.of(version).0 <= *parts,
        }
    }
}

/// The indexes of the `releases` after `from` and up to `to`, in their order (decision 114).
#[must_use]
pub fn in_range(releases: &[Release], from: Option<&Bound>, to: Option<&Bound>) -> Vec<usize> {
    releases
        .iter()
        .enumerate()
        .filter(|(_, release)| {
            from.is_none_or(|from| from.is_before(&release.version))
                && to.is_none_or(|to| to.is_not_before(&release.version))
        })
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions_of(text: &str, crate_name: &str) -> Vec<String> {
        releases(text, crate_name)
            .all
            .iter()
            .map(|release| release.version.to_string())
            .collect()
    }

    #[test]
    fn the_forms_of_a_versions_heading_are_read() {
        let text = "# Changelog\n\n## [Unreleased]\n\nnext\n\n\
                    ## [1.2.3] - 2024-01-02\n\n- a\n\n\
                    ## 1.2.2 (2023-12-01)\n\n\
                    ## [1.2.1](https://example.invalid/compare) (2023-11-01)\n\n\
                    # Version 1.2.0 — 2023-10-01\n\n\
                    ### Migration notes for 2.22.0\n\n\
                    ## v1.1 (yanked)\n\n\
                    ## ~~[v1.0.1] - 2023-01-01~~\n\n\
                    1.0.0-rc.1\n----------\n\n\
                    ## demo_crate 0.9.0\n\n\
                    ## CHANGES WITH 0.8.5:\n\n\
                    ## Other-crate 0.8.0\n\n\
                    ## 0.1.x is not a version\n";
        let found = releases(text, "demo-crate");
        assert_eq!(versions_of(text, "demo-crate"), [
            "1.2.3",
            "1.2.2",
            "1.2.1",
            "1.2.0",
            "1.1.0",
            "1.0.1",
            "1.0.0-rc.1",
            "0.9.0",
            "0.8.5"
        ]);
        assert_eq!(found.unreleased, Some(3));
        let first = &found.all[0];
        assert_eq!((first.line, first.date.as_deref()), (7, Some("2024-01-02")));
        assert!(found.all[4].yanked && !first.yanked);
        assert_eq!(found.all[3].date.as_deref(), Some("2023-10-01"));
        // From its heading to the next release's.
        assert_eq!(found.lines_of(Some(0), 40), Some((7, 11)));
        assert_eq!(found.lines_of(None, 40), Some((3, 7)));
        let last = found.all.len() - 1;
        assert_eq!(
            found.lines_of(Some(last), 40).map(|(_, end)| end),
            Some(30),
            "until another crate's release"
        );
        assert_eq!(found.lines_of(Some(last.saturating_add(1)), 40), None);
        assert_eq!(releases("## 1.0.0\n", "demo").lines_of(None, 1), None);
    }

    #[test]
    fn a_heading_without_a_version_first_is_not_a_release() {
        for heading in [
            "# Changelog",
            "## Breaking changes in 2.0.0",
            "## Release process",
            "## Versioning",
            "## 2024-01-01",
            "## Headless mode",
        ] {
            let found = releases(&format!("{heading}\n"), "demo");
            assert!(found.all.is_empty(), "{heading}");
            assert_eq!(found.unreleased, None, "{heading}");
        }
        assert_eq!(releases("## HEAD\n", "demo").unreleased, Some(1));
    }

    /// release-plz's headings of a workspace's changelog, another crate's release in it, and the
    /// definitions of the links' labels at its end.
    #[test]
    fn the_sections_end_at_another_crates_release_and_at_the_links_labels() {
        let text = "## [Unreleased]\n\n\
                    ## `parking_lot` - [0.12.5](https://example.invalid/a) - 2025-09-30\n\n- a\n\n\
                    ## `parking_lot_core` - [0.9.12](https://example.invalid/b) - 2025-09-30\n\n- b\n\n\
                    ## parking_lot 0.12.4\n\n- c\n\n\
                    [0.12.4]: https://example.invalid/c\n[0.12.3]: https://example.invalid/d\n";
        let found = releases(text, "parking_lot");
        assert_eq!(versions_of(text, "parking_lot"), ["0.12.5", "0.12.4"]);
        assert_eq!(found.all[0].date.as_deref(), Some("2025-09-30"));
        assert_eq!(found.lines_of(None, 17), Some((1, 3)));
        assert_eq!(
            found.lines_of(Some(0), 17),
            Some((3, 7)),
            "until parking_lot_core's"
        );
        assert_eq!(
            found.lines_of(Some(1), 17),
            Some((11, 15)),
            "without the labels"
        );
    }

    /// A heading deeper than the releases' is one only if its version stands alone, and not again
    /// one of the release it is in; one that starts with a section's number is none.
    #[test]
    fn a_note_in_a_release_is_not_a_release() {
        let text = "## [2.3.0] - 2026-01-01\n\n### `pmcp` 2.3.0 — what changed\n\n- x\n\n\
                    ## [1.11.0]\n\n### v1.3 MCP Apps Developer Experience\n\n- y\n\n\
                    ### 1.10.1 (2025-12-01)\n\n## 1.2 Introduction\n\n## 2.1. Installation\n\n\
                    ## vNext\n\n## [2.16.0] - Unreleased\n";
        let found = releases(text, "pmcp");
        assert_eq!(versions_of(text, "pmcp"), ["2.3.0", "1.11.0", "1.10.1"]);
        assert_eq!(found.lines_of(Some(0), 30), Some((1, 7)));
        assert_eq!(found.unreleased, Some(19), "vNext");
    }

    fn release(version: &str) -> Release {
        Release {
            version: Version::parse(version).expect("a version"),
            heading: version.to_owned(),
            line: 1,
            date: None,
            yanked: false,
        }
    }

    #[test]
    fn a_range_is_after_from_and_up_to_to() {
        let all: Vec<Release> = [
            "0.9.0",
            "0.8.2",
            "0.8.1",
            "0.8.0",
            "0.7.4",
            "0.7.0",
            "0.8.0-rc.1",
        ]
        .into_iter()
        .map(release)
        .collect();
        let bound = |text: &str| Bound::parse(text).expect("a bound");
        let picked = |from: Option<&str>, to: Option<&str>| -> Vec<String> {
            let (from, to) = (from.map(bound), to.map(bound));
            in_range(&all, from.as_ref(), to.as_ref())
                .into_iter()
                .map(|index| all[index].version.to_string())
                .collect()
        };
        assert_eq!(picked(Some("0.7.4"), Some("0.8.1")), [
            "0.8.1",
            "0.8.0",
            "0.8.0-rc.1"
        ]);
        // `from` of parts: after every version that starts with them; `to`: up to every one.
        assert_eq!(picked(Some("0.7"), Some("0.8")), [
            "0.8.2",
            "0.8.1",
            "0.8.0",
            "0.8.0-rc.1"
        ]);
        assert_eq!(picked(None, Some("v0.7")), ["0.7.4", "0.7.0"]);
        assert_eq!(picked(Some("=0.8.2"), None), ["0.9.0"]);
        // The build metadata is not part of the order.
        let built = [release("1.1.9+zlib.1.3")];
        let to = bound("1.1.9");
        assert_eq!(in_range(&built, None, Some(&to)), [0]);
        assert_eq!(in_range(&built, Some(&to), None), Vec::<usize>::new());
        assert_eq!(Bound::parse("1.x"), None);
        assert_eq!(Bound::parse("1.2.3.4"), None);
        assert_eq!(Bound::parse("1"), Some(Bound::Parts(vec![1])));
    }
}
