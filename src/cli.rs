//! The commands of the binary besides `serve`: what manages the cache (requirement R-DIST-2,
//! decided in M2f's F1, 1). The arguments are read by hand, as there are few.

use std::{
    ffi::OsString,
    path::Path,
    process::ExitCode,
    time::{Duration, SystemTime},
};

use semver::Version;

use crate::{
    config::Settings,
    data::{
        cache::Cache,
        clear::{self, Asked},
        crate_name::CrateName,
        gc::{self, Outcome},
        inventory::{self, Entry, Inventory, Kind},
        layout::{self, Class, Limits},
    },
};

/// The lines of the usage of the binary for the commands of this module.
pub const USAGE: &str =
    "  cache ls              Show what the cache has, and how much of its limits it takes
  cache gc [--dry-run]  Remove what the cache keeps longest unused, and what is over its limits
  cache clear [--dry-run] [<crate>[@<version>]]
                        Remove all that the cache keeps, or what it keeps of a crate";

/// How many of the places removed `cache gc` lists.
const LISTED: usize = 20;

/// How many of the largest places `cache ls` shows (decided in M2f's F1a).
const LARGEST: usize = 10;

/// What a command printed, to stdout and to stderr, and its exit code: the binary prints it, as
/// the library prints nothing itself.
#[derive(Debug)]
pub struct Ran {
    /// The exit code.
    pub code: ExitCode,
    /// What it prints to stdout.
    pub stdout: String,
    /// What it prints to stderr.
    pub stderr: String,
}

impl Ran {
    /// A command that succeeded, printing `stdout`.
    const fn ok(stdout: String) -> Self {
        Self {
            code: ExitCode::SUCCESS,
            stdout,
            stderr: String::new(),
        }
    }

    /// A command that failed with `code`, printing `stderr`.
    const fn failed(code: ExitCode, stderr: String) -> Self {
        Self {
            code,
            stdout: String::new(),
            stderr,
        }
    }
}

/// Runs the command `cache` with `args`, those after it: `usage` and exit code 2 for arguments
/// that it does not take.
#[must_use]
pub fn cache(args: &[OsString], usage: &str) -> Ran {
    match args {
        [command] if command == "ls" => list(),
        [command] if command == "gc" => collect(false),
        [command, flag] if command == "gc" && flag == "--dry-run" => collect(true),
        [command, rest @ ..] if command == "clear" => match clear_args(rest) {
            Ok((asked, dry_run)) => clear(&asked, dry_run),
            Err(why) => Ran::failed(ExitCode::from(2), format!("{why}\n\n{usage}\n")),
        },
        // 2 is the conventional exit code for a usage error.
        _ => Ran::failed(ExitCode::from(2), format!("{usage}\n")),
    }
}

/// The crates of the standard library, which the cache keeps by nightly, not by crate: by their
/// cache keys.
const STD_CRATES: [&str; 5] = ["std", "core", "alloc", "proc-macro", "test"];

/// What `cache clear` with `args` is asked to clear, and whether it is a dry run; or why the
/// arguments are not taken (decided in M2f's F1c, 5 and 6).
fn clear_args(args: &[OsString]) -> Result<(Asked, bool), String> {
    let mut dry_run = false;
    let mut asked = None;
    for arg in args {
        let Some(arg) = arg.to_str() else {
            return Err(format!("{} is not UTF-8", arg.display()));
        };
        if arg == "--dry-run" && !dry_run {
            dry_run = true;
        } else if asked.is_none() && !arg.starts_with('-') {
            asked = Some(crate_asked(arg)?);
        } else {
            return Err(format!("{arg} is not taken here"));
        }
    }
    Ok((asked.unwrap_or(Asked::All), dry_run))
}

/// The crate, and its version if given, of `arg`: `<crate>` or `<crate>@<version>`.
fn crate_asked(arg: &str) -> Result<Asked, String> {
    let (name, version) = arg
        .split_once('@')
        .map_or((arg, None), |(name, version)| (name, Some(version)));
    let name: CrateName = name
        .parse()
        .map_err(|error| format!("{name} is not the name of a crate: {error}"))?;
    if STD_CRATES.contains(&name.cache_key().as_str()) {
        return Err(format!(
            "{} is of the standard library, which the cache keeps by nightly, not by crate: the \
             std of a nightly unused for a week is removed, and `cache clear` removes all of it",
            name.as_str()
        ));
    }
    let version = version
        .map(|version| {
            Version::parse(version)
                .map_err(|error| format!("{version} is not a version in full, as 1.0.228: {error}"))
        })
        .transpose()?;
    Ok(Asked::Crate { name, version })
}

/// `cache clear`: removes what `asked` names from the cache, as `cache gc` removes it, but even
/// what was just used, and tells what it removed (decided in M2f's F1, 9, and F1c). In a dry run,
/// it removes nothing and writes nothing, and tries no lock: what is in use shows only when it
/// runs, but a target directory that what a stopped server left running uses. It exits with 1 when what it was asked to remove is in
/// use or could not be removed, and stays (decided in M2f's F1c, 7), or when the cache is not this
/// server's.
fn clear(asked: &Asked, dry_run: bool) -> Ran {
    let settings = Settings::load();
    warnings_to_stderr();
    let cache = Cache::open(&settings);
    settings.report();
    let cache = match cache {
        Ok(cache) => cache,
        Err(error) => return Ran::failed(ExitCode::FAILURE, format!("{}\n", error_chain(&error))),
    };
    let root = cache.dir();
    if !root.exists() {
        return Ran::ok(format!("No cache at {}.\n", root.display()));
    }
    match clear::clear(&cache, asked, dry_run) {
        Ok(outcome)
            if outcome.removed.is_empty()
                && outcome.in_use.is_empty()
                && outcome.left.is_empty() =>
        {
            Ran::ok(format!(
                "Cache: {}\nNothing of {asked} to remove.\n",
                root.display()
            ))
        },
        Ok(outcome) => {
            let text = collected(root, &outcome, dry_run);
            if outcome.in_use.is_empty() && outcome.left.is_empty() {
                Ran::ok(text)
            } else {
                Ran {
                    code: ExitCode::FAILURE,
                    stdout: text,
                    stderr: String::new(),
                }
            }
        },
        Err(error) => Ran::failed(
            ExitCode::FAILURE,
            format!("{}: {}\n", root.display(), error_chain(&error)),
        ),
    }
}

/// `cache ls`: what the cache has, by class, with how much of its limits it takes, and its largest
/// places (decided in M2f's F1, 10). It writes nothing: a cache that is not there is not made, nor
/// marked.
fn list() -> Ran {
    let settings = Settings::load();
    warnings_to_stderr();
    let cache = Cache::open(&settings);
    let limits = Limits::from_settings(&settings);
    settings.report();
    let cache = match cache {
        Ok(cache) => cache,
        Err(error) => return Ran::failed(ExitCode::FAILURE, format!("{}\n", error_chain(&error))),
    };
    let root = cache.dir();
    if !root.exists() {
        return Ran::ok(format!("No cache at {}.\n", root.display()));
    }
    let marked = layout::is_owned(root);
    let unknown = if marked {
        Ok(Vec::new())
    } else {
        layout::unknown_names(root)
    };
    match (unknown, inventory::take(root, None)) {
        (Ok(unknown), Ok(inventory)) => {
            let owner = Owner::of(marked, unknown);
            Ran::ok(report(root, &owner, &inventory, &limits, SystemTime::now()))
        },
        (Err(error), _) | (_, Err(error)) => {
            Ran::failed(ExitCode::FAILURE, format!("{}: {error}\n", root.display()))
        },
    }
}

/// `cache gc`: removes what the cache keeps that is old, of a layout of an earlier version, or over
/// its limits, as the server does in the background, and tells what it removed (decided in M2f's
/// F1b, 1). In a dry run, it removes nothing and writes nothing. A cache that is not this server's
/// is not touched, with exit code 1.
fn collect(dry_run: bool) -> Ran {
    let settings = Settings::load();
    warnings_to_stderr();
    let cache = Cache::open(&settings);
    let limits = Limits::from_settings(&settings);
    settings.report();
    let cache = match cache {
        Ok(cache) => cache,
        Err(error) => return Ran::failed(ExitCode::FAILURE, format!("{}\n", error_chain(&error))),
    };
    let root = cache.dir();
    if !root.exists() {
        return Ran::ok(format!("No cache at {}.\n", root.display()));
    }
    match gc::collect(&cache, &limits, None, dry_run) {
        Ok(outcome) => Ran::ok(collected(root, &outcome, dry_run)),
        Err(error) => Ran::failed(
            ExitCode::FAILURE,
            format!("{}: {}\n", root.display(), error_chain(&error)),
        ),
    }
}

/// What `cache gc` prints of the collection of the cache at `root` that did `outcome`.
fn collected(root: &Path, outcome: &Outcome, dry_run: bool) -> String {
    let bytes = outcome
        .removed
        .iter()
        .fold(0_u64, |total, removal| total.saturating_add(removal.bytes));
    let verb = if dry_run { "Would remove" } else { "Removed" };
    let mut lines = vec![
        format!("Cache: {}", root.display()),
        format!(
            "{verb} {} places, {}{}",
            outcome.removed.len(),
            format_size(bytes),
            if outcome.removed.is_empty() { "." } else { ":" }
        ),
    ];
    let mut removed: Vec<&gc::Removal> = outcome.removed.iter().collect();
    removed.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.entry.path.cmp(&b.entry.path))
    });
    for removal in removed.iter().take(LISTED) {
        lines.push(format!(
            "  {:>10}  {:<15} {}  ({})",
            format_size(removal.bytes),
            removal.entry.kind.as_str(),
            slashed(&removal.entry.path),
            removal.reason.as_str()
        ));
    }
    if let Some(more) = removed.len().checked_sub(LISTED).filter(|more| *more > 0) {
        lines.push(format!("  and {more} more"));
    }
    if !outcome.in_use.is_empty() {
        let in_use: Vec<String> = outcome.in_use.iter().map(|path| slashed(path)).collect();
        lines.push(format!("Skipped, as in use: {}", in_use.join(", ")));
    }
    for (path, error) in &outcome.left {
        lines.push(format!(
            "Left for the next time, as it could not be removed: {} ({error})",
            slashed(path)
        ));
    }
    if !outcome.complete {
        lines.push("Not all of the cache could be looked at: only what is old went.".to_owned());
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Logs the warnings, as of the settings, to stderr: not to the server's log file.
fn warnings_to_stderr() {
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .without_time()
        .with_target(false)
        .finish();
    // Only when none is set yet, as in a test of this module.
    drop(tracing::subscriber::set_global_default(subscriber));
}

/// `error` and its causes, on one line.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut cause = error.source();
    while let Some(next) = cause {
        parts.push(next.to_string());
        cause = next.source();
    }
    parts.join(": ")
}

/// Whether the cache directory is this server's, as `cache ls` tells it.
#[derive(Debug, PartialEq, Eq)]
enum Owner {
    /// It has the marker.
    Owned,
    /// It has no marker, and nothing that this server does not put there: the server marks it when
    /// it starts.
    Unmarked,
    /// It has these names that this server does not put there.
    Foreign(Vec<String>),
}

impl Owner {
    /// Of a directory that has the marker if `marked`, with the `unknown` names.
    fn of(marked: bool, unknown: Vec<String>) -> Self {
        if marked {
            Self::Owned
        } else if unknown.is_empty() {
            Self::Unmarked
        } else {
            Self::Foreign(unknown)
        }
    }
}

/// What `cache ls` prints of the cache at `root`, whose owner is `owner`, which has `inventory`,
/// with `limits`, at `now`.
fn report(
    root: &Path,
    owner: &Owner,
    inventory: &Inventory,
    limits: &Limits,
    now: SystemTime,
) -> String {
    let whose = match owner {
        Owner::Owned => "owned".to_owned(),
        Owner::Unmarked => "not marked yet: the server marks it when it starts".to_owned(),
        Owner::Foreign(names) => format!(
            "not this server's: it has no marker, and has {}; nothing in it is removed",
            names.join(", ")
        ),
    };
    let mut lines = vec![format!("Cache: {} ({whose})", root.display())];
    if !inventory.complete {
        lines.push("(not all of it could be looked at)".to_owned());
    }
    let days = limits
        .target_max_age
        .as_secs()
        .checked_div(24 * 60 * 60)
        .unwrap_or_default();
    lines.push(String::new());
    lines.push(format!(
        "{:<8} {:>10} {:>10} {:>7}  unused over {days} days",
        "class", "used", "limit", "places"
    ));
    let rows = [
        ("docs", Some(Class::Docs), Some(limits.docs_max_size)),
        (
            "targets",
            Some(Class::Targets),
            Some(limits.target_max_size),
        ),
        ("kept", Some(Class::Kept), None),
        ("unknown", None, None),
    ];
    for (name, class, limit) in rows {
        let entries: Vec<&Entry> = inventory
            .entries
            .iter()
            .filter(|entry| entry.class == class)
            .collect();
        let bytes = entries
            .iter()
            .fold(0_u64, |total, entry| total.saturating_add(entry.bytes));
        let unused = (class == Some(Class::Targets)).then(|| {
            entries
                .iter()
                .filter(|entry| unused_for(entry, now, limits.target_max_age))
                .count()
                .to_string()
        });
        lines.push(
            format!(
                "{name:<8} {:>10} {:>10} {:>7}  {}",
                if class.is_some() {
                    format_size(bytes)
                } else {
                    "-".to_owned()
                },
                limit.map_or_else(|| "-".to_owned(), format_size),
                entries.len(),
                unused.unwrap_or_default()
            )
            .trim_end()
            .to_owned(),
        );
    }
    let mut largest: Vec<&Entry> = inventory
        .entries
        .iter()
        .filter(|entry| entry.class.is_some())
        .collect();
    largest.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    largest.truncate(LARGEST);
    if !largest.is_empty() {
        lines.push(String::new());
        lines.push("largest:".to_owned());
    }
    for entry in largest {
        let used = entry
            .last_used
            .map_or_else(|| "use not known".to_owned(), |at| format_use(now, at));
        let about = entry
            .about
            .as_ref()
            .map_or_else(String::new, |about| format!("  ({about})"));
        lines.push(format!(
            "  {:>10}  {:<15} {}  {used}{about}",
            format_size(entry.bytes),
            entry.kind.as_str(),
            slashed(&entry.path),
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// `path`, relative to the cache, with `/` between its parts on every OS.
fn slashed(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `entry` was last used longer than `age` before `now`.
fn unused_for(entry: &Entry, now: SystemTime, age: Duration) -> bool {
    entry.kind != Kind::Toolchains
        && entry
            .last_used
            .is_some_and(|used| now.duration_since(used).is_ok_and(|since| since > age))
}

/// `bytes` with a unit of powers of 1000, as the settings write sizes (decided in M2f's F1a).
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes;
    let mut unit = 0_usize;
    while value >= 1_000_000 && unit < UNITS.len().saturating_sub(1) {
        value = value.checked_div(1000).unwrap_or_default();
        unit = unit.saturating_add(1);
    }
    // In tenths of the unit, rounded down.
    let tenths = value.checked_div(100).unwrap_or_default();
    format!(
        "{}.{} {}",
        tenths.checked_div(10).unwrap_or_default(),
        tenths.checked_rem(10).unwrap_or_default(),
        UNITS.get(unit).copied().unwrap_or("TB")
    )
}

/// When something was last used, at `at`, as of `now`.
fn format_use(now: SystemTime, at: SystemTime) -> String {
    let Ok(since) = now.duration_since(at) else {
        return "used just now".to_owned();
    };
    let minutes = since.as_secs().checked_div(60).unwrap_or_default();
    let (count, unit) = match minutes {
        0 => return "used just now".to_owned(),
        1..60 => (minutes, "minute"),
        60..1440 => (minutes.checked_div(60).unwrap_or_default(), "hour"),
        1440.. => (minutes.checked_div(1440).unwrap_or_default(), "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("used {count} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn sizes_are_told_in_powers_of_1000() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1000), "1.0 kB");
        assert_eq!(format_size(1_550), "1.5 kB");
        assert_eq!(format_size(686_400_000), "686.4 MB");
        assert_eq!(format_size(5_000_000_000), "5.0 GB");
        assert_eq!(format_size(20_000_000_000_000), "20.0 TB");
    }

    #[test]
    fn uses_are_told_by_how_long_ago() {
        let now = SystemTime::now();
        let ago = |secs| now.checked_sub(Duration::from_secs(secs)).expect("a time");
        assert_eq!(format_use(now, ago(10)), "used just now");
        assert_eq!(format_use(now, ago(5 * 60)), "used 5 minutes ago");
        assert_eq!(format_use(now, ago(3 * 3600)), "used 3 hours ago");
        assert_eq!(format_use(now, ago(2 * 86_400)), "used 2 days ago");
        assert_eq!(format_use(now, ago(86_400)), "used 1 day ago");
    }

    #[test]
    fn the_report_tells_each_class_its_limit_and_the_largest() {
        let now = SystemTime::now();
        let old = now.checked_sub(Duration::from_hours(40 * 24));
        let entry = |kind, class, path: &str, bytes, last_used| Entry {
            kind,
            class,
            path: PathBuf::from(path),
            bytes,
            last_used,
            about: (kind == Kind::TargetDir).then(|| "/w x86_64-unknown-linux-gnu".to_owned()),
        };
        let inventory = Inventory {
            entries: vec![
                entry(
                    Kind::DocsRs,
                    Some(Class::Docs),
                    "docs.rs/a/1.0.0/_default",
                    2_000,
                    Some(now),
                ),
                entry(
                    Kind::TargetDir,
                    Some(Class::Targets),
                    "t/0123",
                    500_000_000,
                    old,
                ),
                entry(Kind::Unknown, None, "config.toml", 10, Some(now)),
            ],
            complete: true,
        };
        let limits = Limits {
            docs_max_size: 5_000_000_000,
            target_max_size: 20_000_000_000,
            target_max_age: Duration::from_hours(30 * 24),
        };
        let owner = Owner::of(false, vec!["config.toml".to_owned()]);
        let text = report(Path::new("/c"), &owner, &inventory, &limits, now);
        assert!(
            text.contains("Cache: /c (not this server's: it has no marker, and has config.toml"),
            "{text}"
        );
        assert!(text.contains("unused over 30 days"), "{text}");
        let line = |name: &str| {
            text.lines()
                .find(|line| line.starts_with(name))
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<&str>>()
                .join(" ")
        };
        assert_eq!(line("docs"), "docs 2.0 kB 5.0 GB 1", "{text}");
        assert_eq!(line("targets"), "targets 500.0 MB 20.0 GB 1 1", "{text}");
        assert_eq!(line("unknown"), "unknown - - 1", "{text}");
        let largest = text
            .lines()
            .skip_while(|line| *line != "largest:")
            .nth(1)
            .unwrap_or_default();
        assert!(largest.contains("t/0123"), "{text}");
        assert!(largest.contains("used 40 days ago"), "{text}");
        assert!(largest.contains("(/w x86_64-unknown-linux-gnu)"), "{text}");
    }
}
