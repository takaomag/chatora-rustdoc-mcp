//! Where the rustdoc JSON of std comes from: the nightly toolchain installed here, or the
//! distribution server (requirements R-STD-1 and R-STD-6).

use std::{
    borrow::Cow,
    fs::{self, File},
    io::{self, Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;
use semver::Version;

#[cfg(feature = "local")]
use super::local_std::{self, LocalStd};
use super::{
    cache::{Cache, Verified},
    dist::{self, Dist, DistError, NightlyDate, StdCrate, StdDocs},
    format_version,
    rustdoc_json,
    target::TargetTriple,
};

/// How many days before the latest nightly are looked through for one whose rustdoc JSON this
/// server reads, when the latest one writes a newer format (decided before the first release,
/// 2026-10-20).
const MAX_DAYS_BACK: u32 = 30;

/// How long the std of a nightly is kept after it was last used. A new nightly comes each day,
/// each about 175 MB with its archives (decided before the first release, 2026-10-20; the rest of
/// the GC is R-CA-5, in M2).
const UNUSED_FOR: Duration = Duration::from_hours(7 * 24);

/// The file in the directory of a nightly's std whose time tells when it was last used.
const USED_FILE: &str = "used";

/// How often the use of a nightly is noted, and the old ones looked for, by a process.
const NOTE_EVERY: Duration = Duration::from_hours(1);

/// When this process last noted a use, by directory.
static NOTED: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

/// The JSON of std that was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdJson {
    /// From the nightly toolchain installed here.
    #[cfg(feature = "local")]
    Local(LocalStd),
    /// From the distribution server, unpacked in the cache.
    Dist(StdDocs),
}

/// The JSON of std that was chosen, and why, for the provenance (requirement R-STD-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    /// The JSON.
    pub json: StdJson,
    /// Why it was chosen over the other.
    pub why: Cow<'static, str>,
    /// What the response warns about, such as the target of the JSON differing from the one
    /// asked for.
    pub warnings: Vec<Box<str>>,
}

impl StdJson {
    /// Reads the JSON of `krate`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, or is damaged in the cache.
    pub fn read(&self, cache: &Cache, krate: StdCrate) -> io::Result<Box<[u8]>> {
        match self {
            #[cfg(feature = "local")]
            Self::Local(local) => local_std::read_local_std_json(local, krate),
            Self::Dist(docs) => dist::read_std_json(cache, docs, krate),
        }
    }

    /// Where the archive of `krate` is kept, and the key of its JSON: the SHA-256 that is next to
    /// the JSON unpacked from the distribution server, or, for the nightly installed here, one
    /// made from its commit, which its files do not outlive. So the JSON is read only to make the
    /// archive.
    ///
    /// # Errors
    ///
    /// Returns an error when the digest of the JSON cannot be read.
    pub fn archive_key(&self, cache: &Cache, krate: StdCrate) -> io::Result<(PathBuf, Box<str>)> {
        match self {
            #[cfg(feature = "local")]
            Self::Local(local) => {
                let dir = cache
                    .dir()
                    .join("std")
                    .join("local")
                    .join(&*local.rustc.commit_hash)
                    .join(local.rustc.host.as_str());
                let key = super::cache::sha256_hex(
                    format!("rustc {} {}", local.rustc.commit_hash, krate.as_str()).as_bytes(),
                );
                Ok((dir, key.into()))
            },
            Self::Dist(docs) => Ok((docs.dir.clone(), dist::std_json_sha256(cache, docs, krate)?)),
        }
    }

    /// The toolchain, as the first line of `rustc -vV` gives it.
    #[must_use]
    pub fn toolchain(&self) -> Cow<'_, str> {
        match self {
            #[cfg(feature = "local")]
            Self::Local(local) => Cow::Owned(format!(
                "rustc {} ({} {})",
                local.rustc.release,
                local
                    .rustc
                    .commit_hash
                    .get(..9)
                    .unwrap_or(&local.rustc.commit_hash),
                local.rustc.commit_date
            )),
            Self::Dist(docs) => Cow::Owned(format!("rustc {}", docs.nightly.version)),
        }
    }

    /// The release of the toolchain (`1.101.0-nightly`).
    #[must_use]
    pub fn release(&self) -> Option<Version> {
        let release = match self {
            #[cfg(feature = "local")]
            Self::Local(local) => &*local.rustc.release,
            Self::Dist(docs) => docs.nightly.version.split(' ').next().unwrap_or_default(),
        };
        Version::parse(release).ok()
    }

    /// `dist` or `local`, for the provenance.
    #[must_use]
    pub const fn source(&self) -> &'static str {
        match self {
            #[cfg(feature = "local")]
            Self::Local(_) => "local",
            Self::Dist(_) => "dist",
        }
    }
}

/// Chooses the JSON of std for `target` (requirement R-STD-1).
///
/// That of the nightly toolchain installed here is taken if it has `rust-docs-json` for `target`
/// and is not older than `at_least`, the release of the project's toolchain; or else that of the
/// latest nightly from the distribution server. If the server has none for `target`, that for
/// `host` stands in, with a warning. rustup runs in `neutral_dir`, where no project settings apply.
///
/// # Errors
///
/// Returns an error when the JSON must come from the distribution server and cannot.
pub async fn choose(
    cache: &Cache,
    dist: &Dist,
    neutral_dir: &Path,
    target: &TargetTriple,
    host: &TargetTriple,
    at_least: Option<&Version>,
) -> Result<Chosen, DistError> {
    let why = match local(neutral_dir, target, at_least).await {
        Ok(chosen) => {
            // The archives of the local nightly are kept in the cache too.
            if let Ok((dir, _)) = chosen.json.archive_key(cache, StdCrate::Std) {
                note_use(cache, dir);
            }
            return Ok(chosen);
        },
        Err(why) => why,
    };
    let (docs, mut warnings) = match dist::std_docs(cache, dist, target, None).await {
        Err(DistError::NotAvailable { .. }) if target != host => {
            let docs = dist::std_docs(cache, dist, host, None).await?;
            let docs = readable_nightly(cache, dist, host, docs).await;
            let warning = format!(
                "the nightly channel publishes no rust-docs-json for {target}, so that for {host} \
                 is used: std differs between targets (std::os and more), and {target} may have no \
                 std at all"
            );
            (docs, vec![warning.into()])
        },
        Err(error) => return Err(error),
        Ok(docs) => (
            readable_nightly(cache, dist, target, docs).await,
            Vec::new(),
        ),
    };
    if docs.nightly.triple != *target && warnings.is_empty() {
        warnings.push(
            format!(
                "the rust-docs-json that the nightly channel gives for {target} is built for {}",
                docs.nightly.triple
            )
            .into(),
        );
    }
    warnings.extend(docs.warnings.iter().cloned());
    note_use(cache, docs.dir.clone());
    Ok(Chosen {
        json: StdJson::Dist(docs),
        why,
        warnings,
    })
}

/// The JSON of the nightly toolchain installed here, or why it is not used.
#[cfg(feature = "local")]
async fn local(
    neutral_dir: &Path,
    target: &TargetTriple,
    at_least: Option<&Version>,
) -> Result<Chosen, Cow<'static, str>> {
    let local = local_std::local_std(neutral_dir, at_least)
        .await
        .map_err(|why| Cow::Owned(why.to_string()))?;
    if local.rustc.host != *target {
        return Err(Cow::Owned(format!(
            "the rust-docs-json of the nightly toolchain here is for {}, not {target}",
            local.rustc.host
        )));
    }
    if let Some(format) = format_in(&local.dir).filter(|format| !readable(*format)) {
        return Err(Cow::Owned(format!(
            "the nightly toolchain here writes rustdoc JSON format {format}, which this server \
             does not read"
        )));
    }
    Ok(Chosen {
        json: StdJson::Local(local),
        why: Cow::Borrowed(
            "the nightly toolchain here has rust-docs-json and is not older than the project's",
        ),
        warnings: Vec::new(),
    })
}

/// Without local features, rustup is not run.
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs rustup"
)]
async fn local(
    _neutral_dir: &Path,
    _target: &TargetTriple,
    _at_least: Option<&Version>,
) -> Result<Chosen, Cow<'static, str>> {
    Err(Cow::Borrowed(
        "this server is built without local features, so it does not look for a nightly toolchain",
    ))
}

/// Notes that the std in `dir` (that of one nightly for one target) is used now, and removes the
/// std of the nightlies unused for [`UNUSED_FOR`], at most once in [`NOTE_EVERY`] for each
/// directory, on a blocking thread that the call does not wait for.
fn note_use(cache: &Cache, dir: PathBuf) {
    {
        let mut noted = NOTED.lock();
        if noted
            .iter()
            .any(|(noted, at)| *noted == dir && at.elapsed() < NOTE_EVERY)
        {
            return;
        }
        noted.retain(|(noted, _)| *noted != dir);
        noted.push((dir.clone(), Instant::now()));
    }
    let cache = cache.clone();
    drop(crate::task::spawn_blocking(move || {
        if let Err(error) = fs::write(dir.join(USED_FILE), b"") {
            tracing::warn!(path = %dir.display(), %error, "the use of a nightly's std could not be noted");
        }
        if let Err(error) = remove_unused(&cache.dir().join("std"), &dir, SystemTime::now()) {
            tracing::warn!(%error, "the std of old nightlies could not all be removed");
        }
    }));
}

/// Removes the directories of a nightly's std under `std_dir` (`<commit>/<target>` and
/// `local/<commit>/<target>`) last used longer than [`UNUSED_FOR`] before `now`, except `keep`,
/// and then their parents left empty. Only directories that this server made (with the mark that
/// the unpacking is complete, or archives) are removed, and symbolic links are not followed.
/// Each that cannot be removed is left (Windows does not remove a file that another process
/// maps), and the last error is returned after the rest are done.
fn remove_unused(std_dir: &Path, keep: &Path, now: SystemTime) -> io::Result<()> {
    let subdirectories = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect()
    };
    let mut nightlies = Vec::new();
    for dir in subdirectories(std_dir) {
        if dir.file_name().is_some_and(|name| name == "local") {
            nightlies.extend(subdirectories(&dir));
        } else {
            nightlies.push(dir);
        }
    }
    let mut failed = None;
    for nightly in nightlies {
        for dir in subdirectories(&nightly) {
            if dir == keep || !made_here(&dir) || !unused(&dir, now) {
                continue;
            }
            if let Err(error) = fs::remove_dir_all(&dir) {
                failed = Some(error);
            }
        }
        // Gone when nothing is left in it; an error means it is not empty.
        drop(fs::remove_dir(&nightly));
    }
    failed.map_or(Ok(()), Err)
}

/// Whether `dir` holds the std of a nightly that this server unpacked or archived.
fn made_here(dir: &Path) -> bool {
    dir.join(dist::COMPLETE_FILE).is_file()
        || fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rkyv")
            })
}

/// Whether the std in `dir` was last used longer than [`UNUSED_FOR`] before `now`: by the time of
/// its [`USED_FILE`], or of the directory when it has none (made before uses were noted).
fn unused(dir: &Path, now: SystemTime) -> bool {
    let used = fs::metadata(dir.join(USED_FILE))
        .or_else(|_| fs::metadata(dir))
        .and_then(|metadata| metadata.modified());
    used.is_ok_and(|used| now.duration_since(used).is_ok_and(|age| age > UNUSED_FOR))
}

/// Whether this server reads rustdoc JSON in `format`.
fn readable(format: u32) -> bool {
    rustdoc_json::SUPPORTED.contains(&format)
}

/// The rustdoc JSON format of `std.json` in `dir`, read from its end, or `None` when it cannot be
/// read there (it is then found out when the JSON is parsed).
fn format_in(dir: &Path) -> Option<u32> {
    let mut file = File::open(dir.join(format!("{}.json", StdCrate::Std.as_str()))).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(256))).ok()?;
    let mut tail = Vec::new();
    file.take(256).read_to_end(&mut tail).ok()?;
    format_version::peek(&tail)
}

/// `latest`, or when the latest nightly writes a format that this server does not read, the
/// newest nightly of the last [`MAX_DAYS_BACK`] days that it reads, with a warning: std is then
/// that of an older nightly until the server is updated, rather than not there at all. The date
/// that was found is kept, so that it is not looked for again on each new nightly.
async fn readable_nightly(
    cache: &Cache,
    dist: &Dist,
    target: &TargetTriple,
    latest: StdDocs,
) -> StdDocs {
    let Some(format) = format_in(&latest.dir).filter(|format| !readable(*format)) else {
        return latest;
    };
    let kept = cache.dir().join("std").join(format!("readable-{target}"));
    let remembered = {
        let (cache, kept) = (cache.clone(), kept.clone());
        tokio::task::spawn_blocking(move || match cache.read_verified(&kept) {
            Ok(Verified::Found(date)) => String::from_utf8(date.into_vec()).ok(),
            Ok(Verified::Mismatch | Verified::None) | Err(_) => None,
        })
        .await
        .ok()
        .flatten()
    };
    let earlier = (1..=MAX_DAYS_BACK).filter_map(|days| days_before(&latest.nightly.date, days));
    for date in remembered.into_iter().chain(earlier) {
        let Ok(date) = date.parse::<NightlyDate>() else {
            continue;
        };
        // A day without a nightly, or one that writes the newer format too, is passed over.
        let Ok(mut docs) = dist::std_docs(cache, dist, target, Some(&date)).await else {
            continue;
        };
        if !format_in(&docs.dir).is_none_or(readable) {
            continue;
        }
        let write = {
            let (cache, kept, date) = (cache.clone(), kept.clone(), date.to_string());
            tokio::task::spawn_blocking(move || cache.write_verified(&kept, date.as_bytes())).await
        };
        if !matches!(write, Ok(Ok(()))) {
            tracing::warn!(path = %kept.display(), "the readable nightly could not be kept");
        }
        docs.warnings.push(
            format!(
                "the latest nightly ({}) writes rustdoc JSON format {format}, which this version \
                 of the server does not read, so std is that of the nightly of {date}; a newer \
                 version of the server reads the latest",
                latest.nightly.date
            )
            .into(),
        );
        return docs;
    }
    let mut latest = latest;
    latest.warnings.push(
        format!(
            "no nightly of the last {MAX_DAYS_BACK} days writes rustdoc JSON that this version of \
             the server reads; a newer version of the server reads it"
        )
        .into(),
    );
    latest
}

/// The date `days` before `date` (`2026-10-20`), or `None` when it is not a date.
fn days_before(date: &str, days: u32) -> Option<String> {
    let mut parts = date.splitn(3, '-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (year, month, day) =
        civil_from_days(days_from_civil(year, month, day)?.checked_sub(i64::from(days))?)?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// The days since 1970-01-01 of a date in the proleptic Gregorian calendar (Howard Hinnant's
/// algorithm), with checked arithmetic.
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year.checked_div_euclid(400)?;
    let year_of_era = year.checked_sub(era.checked_mul(400)?)?;
    let shifted_month = if month > 2 {
        month.checked_sub(3)?
    } else {
        month.checked_add(9)?
    };
    let day_of_year = shifted_month
        .checked_mul(153)?
        .checked_add(2)?
        .checked_div(5)?
        .checked_add(day)?
        .checked_sub(1)?;
    let day_of_era = year_of_era
        .checked_mul(365)?
        .checked_add(year_of_era.checked_div(4)?)?
        .checked_sub(year_of_era.checked_div(100)?)?
        .checked_add(day_of_year)?;
    era.checked_mul(146_097)?
        .checked_add(day_of_era)?
        .checked_sub(719_468)
}

/// The date of `days` since 1970-01-01, the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> Option<(i64, i64, i64)> {
    let days = days.checked_add(719_468)?;
    let era = days.checked_div_euclid(146_097)?;
    let day_of_era = days.checked_sub(era.checked_mul(146_097)?)?;
    let year_of_era = day_of_era
        .checked_sub(day_of_era.checked_div(1460)?)?
        .checked_add(day_of_era.checked_div(36_524)?)?
        .checked_sub(day_of_era.checked_div(146_096)?)?
        .checked_div(365)?;
    let day_of_year = day_of_era.checked_sub(
        year_of_era
            .checked_mul(365)?
            .checked_add(year_of_era.checked_div(4)?)?
            .checked_sub(year_of_era.checked_div(100)?)?,
    )?;
    let shifted_month = day_of_year
        .checked_mul(5)?
        .checked_add(2)?
        .checked_div(153)?;
    let day = day_of_year
        .checked_sub(
            shifted_month
                .checked_mul(153)?
                .checked_add(2)?
                .checked_div(5)?,
        )?
        .checked_add(1)?;
    let month = if shifted_month < 10 {
        shifted_month.checked_add(3)?
    } else {
        shifted_month.checked_sub(9)?
    };
    let year = year_of_era
        .checked_add(era.checked_mul(400)?)?
        .checked_add(i64::from(month <= 2))?;
    Some((year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{
        cache,
        dist::tests::{archive, json_files, linux, setup},
        test_http,
    };

    #[test]
    fn the_std_of_nightlies_unused_for_a_week_goes() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let std_dir = temp.path();
        let now = SystemTime::now();
        let make = |path: &str, file: &str, days_ago: u64| {
            let dir = std_dir.join(path);
            fs::create_dir_all(&dir).expect("made");
            fs::write(dir.join(file), b"{}").expect("written");
            let used = dir.join(USED_FILE);
            fs::write(&used, b"").expect("written");
            File::options()
                .write(true)
                .open(&used)
                .and_then(|file| file.set_modified(now - Duration::from_hours(days_ago * 24)))
                .expect("set");
            dir
        };
        let old = make(
            "aaaaaaaaa/x86_64-unknown-linux-gnu",
            dist::COMPLETE_FILE,
            30,
        );
        let recent = make("bbbbbbbbb/x86_64-unknown-linux-gnu", dist::COMPLETE_FILE, 1);
        let in_use = make(
            "ccccccccc/x86_64-unknown-linux-gnu",
            dist::COMPLETE_FILE,
            30,
        );
        let local = make(
            "local/0123abcd/x86_64-unknown-linux-gnu",
            "std.0123.v5t61.rkyv",
            30,
        );
        let not_ours = make("ddddddddd/x86_64-unknown-linux-gnu", "notes.txt", 30);
        remove_unused(std_dir, &in_use, now).expect("no error");
        assert!(!old.exists() && !std_dir.join("aaaaaaaaa").exists());
        assert!(!local.exists() && !std_dir.join("local/0123abcd").exists());
        assert!(recent.exists() && in_use.exists() && not_ours.exists());
    }

    #[test]
    fn days_before_a_date_cross_months_years_and_leap_days() {
        assert_eq!(days_before("2026-10-20", 1).as_deref(), Some("2026-10-19"));
        assert_eq!(days_before("2026-03-01", 1).as_deref(), Some("2026-02-28"));
        assert_eq!(days_before("2028-03-01", 1).as_deref(), Some("2028-02-29"));
        assert_eq!(days_before("2027-01-01", 1).as_deref(), Some("2026-12-31"));
        assert_eq!(days_before("2026-10-20", 30).as_deref(), Some("2026-09-20"));
        assert_eq!(days_before("2026-13-01", 1), None);
        assert_eq!(days_before("nightly", 1), None);
    }

    /// A manifest of the nightly of `date` built from the commit `short`, with `archive` for Linux.
    fn manifest(base_url: &str, date: &str, short: &str, archive: &[u8]) -> Vec<u8> {
        format!(
            r#"manifest-version = "2"
date = "{date}"

[pkg.rust-docs-json-preview]
version = "1.101.0-nightly ({short} {date})"

[pkg.rust-docs-json-preview.target.x86_64-unknown-linux-gnu]
available = true
xz_url = "{base_url}/dist/{date}/rust-docs-json-nightly-x86_64-unknown-linux-gnu.tar.xz"
xz_hash = "{}"
"#,
            cache::sha256_hex(archive)
        )
        .into_bytes()
    }

    fn ok(body: &[u8]) -> (String, Vec<u8>) {
        (
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ),
            body.to_vec(),
        )
    }

    #[tokio::test]
    async fn an_older_nightly_stands_in_when_the_latest_writes_a_newer_format() {
        let newer = archive(&[
            ("git-commit-hash", b"a".repeat(40).as_slice()),
            (
                "rust-docs-json-preview/share/doc/rust/json/std.json",
                br#"{"format_version":999}"#,
            ),
            (
                "rust-docs-json-preview/share/doc/rust/json/core.json",
                br#"{"format_version":999}"#,
            ),
        ]);
        let older = archive(&json_files());
        let (base_url, _requests) = test_http::serve_sequence_with(|base_url| {
            vec![
                ok(&manifest(base_url, "2026-10-19", "aaaaaaaaa", &newer)),
                ok(&newer),
                // The day before, also a newer format: passed over.
                ok(&manifest(base_url, "2026-10-18", "bbbbbbbbb", &newer)),
                ok(&newer),
                ok(&manifest(base_url, "2026-10-17", "ccccccccc", &older)),
                ok(&older),
            ]
        });
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let dist = Dist::for_local_test(&base_url);
        let latest = dist::std_docs(&cache, &dist, &linux(), None)
            .await
            .expect("the latest");
        assert_eq!(format_in(&latest.dir), Some(999));
        let docs = readable_nightly(&cache, &dist, &linux(), latest).await;
        assert_eq!(&*docs.nightly.date, "2026-10-17");
        assert!(
            docs.warnings
                .iter()
                .any(|warning| warning.contains("2026-10-19") && warning.contains("2026-10-17")),
            "{:?}",
            docs.warnings
        );
        let kept = fs::read_to_string(dir.path().join("std/readable-x86_64-unknown-linux-gnu"))
            .expect("kept");
        assert_eq!(kept, "2026-10-17");
    }

    #[tokio::test]
    async fn the_host_stands_in_for_a_target_without_rust_docs_json() {
        let archive = archive(&json_files());
        let (cache, dist, _requests, dir) = setup(&archive, None);
        let wasm: TargetTriple = "wasm32-unknown-unknown".parse().expect("a triple");
        let chosen = choose(&cache, &dist, dir.path(), &wasm, &linux(), None)
            .await
            .expect("the host's JSON");
        #[cfg_attr(
            not(feature = "local"),
            expect(
                clippy::infallible_destructuring_match,
                reason = "without local features, the JSON comes only from the distribution server"
            )
        )]
        let docs = match &chosen.json {
            StdJson::Dist(docs) => docs,
            #[cfg(feature = "local")]
            StdJson::Local(_) => panic!("from the distribution server: {chosen:?}"),
        };
        assert_eq!(docs.nightly.triple, linux());
        assert_eq!(chosen.warnings.len(), 1, "{:?}", chosen.warnings);
        assert!(chosen.warnings[0].contains("wasm32-unknown-unknown"));
        assert_eq!(
            &*chosen.json.read(&cache, StdCrate::Std).expect("std"),
            br#"{"format_version":61}"#
        );
    }
}
