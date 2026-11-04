//! What `crate_metadata` makes of what the index and the crates.io API say: the versions where
//! something declared changed, and how long ago the last release was (decided in M3f, decisions
//! 96 and 97).

use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::data::{crates_io::VersionInfo, parse_rfc3339};

/// A version where what it declares changed from the version before it, or that is yanked
/// (decision 96).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    /// The version.
    pub version: Box<str>,
    /// When it was published, as the API says.
    pub published: Box<str>,
    /// Whether it is yanked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub yanked: bool,
    /// Whether it is the first version of all, which nothing came before.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub first: bool,
    /// The license, when it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<Changed>,
    /// The `rust-version`, when it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_version: Option<Changed>,
    /// The edition, when it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition: Option<Changed>,
}

/// A declared value before a version and in it; `None` is not declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Changed {
    /// In the version before.
    pub from: Option<Box<str>>,
    /// In this version.
    pub to: Option<Box<str>>,
}

/// What comes after the last of the versions given, the oldest of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After<'a> {
    /// Nothing: the last is the first version of all.
    Nothing,
    /// This version, the next older one, as the next page of the API begins with it.
    Version(&'a VersionInfo),
    /// Not known: the last is compared with nothing.
    Unknown,
}

/// The versions of `versions` where something declared changed, or that are yanked.
///
/// `versions` are newest first, as the API lists them (by semver), and each is compared with the
/// next older one by semver, the last with `after`: its license, `rust-version` and edition. A
/// backport (`1.38.2` after `1.39.0`) is compared within its line, and the newest of a line with
/// the newest of the line before. `None` is not declared: no `license`, as with `license-file`
/// only, and no `edition`, which is 2015.
#[must_use]
pub fn changes(versions: &[VersionInfo], after: After<'_>) -> Vec<Change> {
    let mut changes = Vec::new();
    for (index, version) in versions.iter().enumerate() {
        let last = match after {
            After::Version(older) => Some(older),
            After::Nothing | After::Unknown => None,
        };
        let older = versions.get(index.saturating_add(1)).or(last);
        let first = older.is_none() && after == After::Nothing;
        let diff = |field: fn(&VersionInfo) -> &Option<Box<str>>| {
            let older = older?;
            (field(older) != field(version)).then(|| Changed {
                from: field(older).clone(),
                to: field(version).clone(),
            })
        };
        let change = Change {
            version: version.num.clone(),
            published: version.created_at.clone(),
            yanked: version.yanked,
            first,
            license: diff(|version| &version.license),
            rust_version: diff(|version| &version.rust_version),
            edition: diff(|version| &version.edition),
        };
        if change.yanked
            || change.first
            || change.license.is_some()
            || change.rust_version.is_some()
            || change.edition.is_some()
        {
            changes.push(change);
        }
    }
    changes
}

/// How long ago the last release was (decision 97). The kinds are only where the days fall; they
/// do not say that a crate is or is not maintained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Recency {
    /// In the last 30 days.
    #[serde(rename = "within_30_days")]
    Month,
    /// In the last 90 days.
    #[serde(rename = "within_90_days")]
    Quarter,
    /// In the last 365 days.
    #[serde(rename = "within_365_days")]
    Year,
    /// Longer ago.
    #[serde(rename = "over_365_days")]
    Older,
}

/// The whole days from `released` to `now`, rounded down, and where they fall; a release after
/// `now` is 0 days ago.
#[must_use]
pub fn recency(released: SystemTime, now: SystemTime) -> (u64, Recency) {
    const DAY: u64 = 24 * 60 * 60;
    let days = now
        .duration_since(released)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        .checked_div(DAY)
        .unwrap_or_default();
    let recency = match days {
        0..=30 => Recency::Month,
        31..=90 => Recency::Quarter,
        91..=365 => Recency::Year,
        _ => Recency::Older,
    };
    (days, recency)
}

/// The time that `text` writes in RFC 3339 and UTC, to the second or with a fraction of it, as the
/// index (`2026-01-02T03:04:05Z`) and the API (`2026-07-18T23:05:13.266456Z`) write it. The
/// fraction is dropped.
#[must_use]
pub fn parse_time(text: &str) -> Option<SystemTime> {
    let whole = match (text.get(..19), text.get(19..)) {
        (Some(whole), Some(rest)) if rest.len() > 1 => {
            let fraction = rest.strip_prefix('.')?.strip_suffix('Z')?;
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            format!("{whole}Z")
        },
        _ => text.to_owned(),
    };
    parse_rfc3339(&whole)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(num: &str, license: Option<&str>, msrv: Option<&str>, yanked: bool) -> VersionInfo {
        VersionInfo {
            num: num.into(),
            license: license.map(Into::into),
            created_at: "2026-01-01T00:00:00Z".into(),
            yanked,
            rust_version: msrv.map(Into::into),
            edition: Some("2021".into()),
        }
    }

    #[test]
    fn only_versions_that_changed_or_are_yanked_are_told() {
        let versions = [
            version("1.0.3", Some("MIT OR Apache-2.0"), Some("1.70"), false),
            version("1.0.2", Some("MIT OR Apache-2.0"), Some("1.70"), true),
            version("1.0.1", Some("MIT OR Apache-2.0"), Some("1.70"), false),
            version("1.0.0", Some("MIT/Apache-2.0"), None, false),
            version("0.9.0", Some("MIT/Apache-2.0"), None, false),
        ];
        let told = changes(&versions, After::Nothing);
        let told: Vec<(&str, bool, bool, bool, bool)> = told
            .iter()
            .map(|change| {
                (
                    &*change.version,
                    change.yanked,
                    change.first,
                    change.license.is_some(),
                    change.rust_version.is_some(),
                )
            })
            .collect();
        assert_eq!(told, [
            ("1.0.2", true, false, false, false),
            ("1.0.1", false, false, true, true),
            ("0.9.0", false, true, false, false),
        ]);
        let license = changes(&versions, After::Nothing)
            .into_iter()
            .find_map(|change| change.license)
            .expect("a change");
        assert_eq!(license, Changed {
            from: Some("MIT/Apache-2.0".into()),
            to: Some("MIT OR Apache-2.0".into()),
        });
        // A page that does not reach the first version: its last is compared with the first of the
        // next page, or with nothing when that is not known.
        assert_eq!(changes(&versions[..3], After::Unknown).len(), 1);
        let across = changes(&versions[..3], After::Version(&versions[3]));
        assert_eq!(across.len(), 2);
        assert_eq!(&*across[1].version, "1.0.1");
        // Not declared, before or after.
        let without = [
            version("2.0.0", None, None, false),
            version("1.0.0", Some("MIT"), None, false),
        ];
        let told = changes(&without, After::Nothing);
        assert_eq!(
            told[0].license,
            Some(Changed {
                from: Some("MIT".into()),
                to: None,
            })
        );
        // One version of all: the first, and nothing changed.
        let one = changes(&without[1..], After::Nothing);
        assert!(one.len() == 1 && one[0].first && one[0].license.is_none());
    }

    #[test]
    fn the_days_since_the_last_release_fall_in_kinds() {
        let day = Duration::from_hours(24);
        let now = SystemTime::UNIX_EPOCH
            .checked_add(day.saturating_mul(1_000))
            .expect("a time");
        let ago = |days: u32| {
            now.checked_sub(day.saturating_mul(days))
                .and_then(|time| time.checked_sub(Duration::from_secs(5)))
                .expect("a time")
        };
        assert_eq!(recency(ago(0), now), (0, Recency::Month));
        assert_eq!(recency(ago(30), now), (30, Recency::Month));
        assert_eq!(recency(ago(31), now), (31, Recency::Quarter));
        assert_eq!(recency(ago(365), now), (365, Recency::Year));
        assert_eq!(recency(ago(366), now), (366, Recency::Older));
        let exactly = now.checked_sub(day.saturating_mul(31)).expect("a time");
        assert_eq!(recency(exactly, now), (31, Recency::Quarter));
        assert_eq!(
            recency(
                now.checked_add(Duration::from_secs(60)).expect("a time"),
                now
            ),
            (0, Recency::Month)
        );
    }

    #[test]
    fn times_of_the_index_and_of_the_api_are_read() {
        let second = parse_time("2026-07-18T23:05:13Z").expect("a time");
        assert_eq!(parse_time("2026-07-18T23:05:13.266456Z"), Some(second));
        assert_eq!(parse_time("2026-07-18T23:05:13.Z"), None);
        assert_eq!(parse_time("2026-07-18T23:05:13.12x4Z"), None);
        assert_eq!(parse_time("2026-07-18T23:05:13+09:00"), None);
        assert_eq!(parse_time("2026-07-18"), None);
        assert_eq!(parse_time("2026-07-18T23:05:1\u{e9}.5Z"), None);
    }
}
