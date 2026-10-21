//! Logging, to stderr and to a file.
//!
//! People read stderr. The file has JSON Lines, with a line for each tool call (requirements
//! R-OBS-1 and R-OBS-2). While serving, stdout carries only JSON-RPC messages (requirement
//! R-SEC-1).

mod file;
mod layer;
mod writer;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::Duration,
};

use etcetera::BaseStrategy as _;
use serde_json::Value;
use tracing_subscriber::{
    Layer as _,
    filter::{EnvFilter, LevelFilter, filter_fn},
    layer::SubscriberExt as _,
    util::{SubscriberInitExt as _, TryInitError},
};

use self::{
    file::{Limits, LogFile},
    writer::Lines,
};
use crate::config::Settings;

/// The name of the span of a tool call. Its `request_id` goes on the warnings in the log file.
pub const CALL_SPAN: &str = "call";

/// The field of [`CALL_SPAN`] and of the log lines that holds the `request_id` (requirement
/// R-ERR-2).
pub const REQUEST_ID_FIELD: &str = "request_id";

/// What a size setting must be.
const SIZE: &str = "a size such as \"20MB\"";

/// The stderr log's filter when none is set, or when the one set is not valid.
const DEFAULT_FILTER: LevelFilter = LevelFilter::INFO;

/// `log.file_max_size` unless set (requirement R-OBS-2, initial value): 20MB.
const DEFAULT_FILE_MAX_SIZE: u64 = 20_000_000;

/// `log.retention_days` unless set (requirement R-OBS-2, initial value).
const DEFAULT_RETENTION_DAYS: u64 = 14;

/// `log.total_max_size` unless set (requirement R-OBS-2, initial value): 50MB.
const DEFAULT_TOTAL_MAX_SIZE: u64 = 50_000_000;

/// The lines of the log file, once [`init`] has opened it.
static FILE: OnceLock<Arc<Lines>> = OnceLock::new();

/// Installs the global subscriber that writes the log to stderr and warnings and errors to the log
/// file, and opens the file for [`write_record`]. The settings are `log.*` (requirements R-OBS-2
/// and R-DIST-3).
///
/// Settings that are not valid are replaced by their defaults, kept as problems of `settings`
/// for [`Settings::report`], and a log file that cannot be opened is done without, which is
/// logged; the server still starts.
///
/// The lines logged so far are written to the file by the time the returned [`Flushing`] is
/// dropped, as when the process ends, or a panic unwinds out of `main`.
///
/// # Errors
///
/// Returns an error when a global subscriber is already installed.
pub fn init(settings: &Settings) -> Result<Flushing, TryInitError> {
    let filter = stderr_filter(settings);
    let limits = limits(settings);
    let file = match log_dir(settings) {
        Ok(dir) => LogFile::open(&dir, limits)
            .and_then(Lines::start)
            .map(Arc::new)
            .map_err(|error| {
                format!(
                    "the log file in {} cannot be written: {error}",
                    dir.display()
                )
            }),
        Err(problem) => Err(problem),
    };
    let stderr = tracing_subscriber::fmt::layer()
        // The default writer is stdout, which would corrupt JSON-RPC.
        .with_writer(std::io::stderr)
        .with_filter(filter);
    let file_layer = file.as_ref().ok().map(|file| {
        layer::FileLayer {
            file: Arc::clone(file),
        }
        .with_filter(filter_fn(layer::enabled))
    });
    tracing_subscriber::registry()
        .with(stderr)
        .with(file_layer)
        .try_init()?;
    match file {
        Ok(file) => {
            tracing::info!(path = %file.path().display(), "logging tool calls");
            // `init` installs the subscriber only once, so the file is set only once.
            FILE.get_or_init(|| file);
        },
        Err(problem) => tracing::warn!("{problem}; tool calls are not logged to a file"),
    }
    Ok(Flushing)
}

/// Writes the lines logged so far to the log file when it is dropped, waiting a second at most.
#[derive(Debug)]
#[must_use = "dropped at once, it waits for the lines at once, and not when the process ends"]
pub struct Flushing;

impl Drop for Flushing {
    fn drop(&mut self) {
        flush();
    }
}

/// Writes `record` as a line of the log file, if there is one. Records are not filtered: each
/// tool call has one (requirements R-OBS-1 and R-DIST-3).
pub fn write_record(record: &Value) {
    if let Some(file) = FILE.get() {
        file.write_line(record.to_string());
    }
}

/// Waits a second at most for the lines logged so far to be written to the log file: for when the
/// process ends.
fn flush() {
    if let Some(file) = FILE.get() {
        file.flush(Duration::from_secs(1));
    }
}

/// The current time for a record, in RFC 3339 and UTC.
#[must_use]
pub fn now() -> String {
    file::now()
}

/// The filter of the stderr log from the value of `log.filter`. When any of its directives is not
/// valid, the whole value is replaced by the default, with a problem to log (requirement
/// R-DIST-3): a filter that is partly applied would hide what was meant to be seen.
fn stderr_filter(settings: &Settings) -> EnvFilter {
    let builder = || EnvFilter::builder().with_default_directive(DEFAULT_FILTER.into());
    settings
        .get("log.filter", |value| {
            value
                .text()
                .and_then(|text| builder().parse(text).ok())
                .ok_or("a filter of tracing's EnvFilter syntax, such as \"debug\"")
        })
        .unwrap_or_else(|| builder().parse_lossy(""))
}

/// The limits of the log files, from their settings or their defaults.
fn limits(settings: &Settings) -> Limits {
    let size = |value: crate::config::Value<'_>| value.size().ok_or(SIZE);
    let file_max_size = settings
        .get("log.file_max_size", size)
        .unwrap_or(DEFAULT_FILE_MAX_SIZE);
    let days = settings
        .get("log.retention_days", |value| {
            value
                .whole_number()
                .filter(|days| *days > 0)
                .ok_or("a whole number from 1")
        })
        .unwrap_or(DEFAULT_RETENTION_DAYS);
    let total_max_size = settings
        .get("log.total_max_size", size)
        .unwrap_or(DEFAULT_TOTAL_MAX_SIZE);
    Limits {
        file_max_size,
        retention: Duration::from_secs(days.saturating_mul(24 * 60 * 60)),
        total_max_size,
    }
}

/// The directory of the logs: `log.dir`, which is absolute, or `logs` in the state directory (requirements R-OBS-2
/// and R-DIST-3): `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs` (`~/.local/state/…` when unset) on
/// Linux and macOS, and `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs` on Windows.
fn log_dir(settings: &Settings) -> Result<PathBuf, String> {
    let dir = settings.get("log.dir", |value| {
        value
            .path()
            .filter(|dir| dir.is_absolute())
            .ok_or("an absolute path")
    });
    if let Some(dir) = dir {
        return Ok(dir);
    }
    let strategy = etcetera::choose_base_strategy()
        .map_err(|error| format!("the home directory could not be found: {error}"))?;
    let app = Path::new(env!("CARGO_PKG_NAME"));
    Ok(strategy
        .state_dir()
        .map_or_else(
            || strategy.cache_dir().join(app).join("state"),
            |state| state.join(app),
        )
        .join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(variables: &[(&str, &str)]) -> Settings {
        Settings::from_sources(
            None,
            variables
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into())),
        )
    }

    #[test]
    fn a_filter_with_any_invalid_directive_is_replaced_by_the_default() {
        let filter = stderr_filter(&settings(&[("CHATORA_RUSTDOC_MCP_LOG__FILTER", "debug")]));
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::DEBUG));
        let invalid = settings(&[("CHATORA_RUSTDOC_MCP_LOG__FILTER", "debug,a=nonsense")]);
        let filter = stderr_filter(&invalid);
        assert_eq!(filter.max_level_hint(), Some(DEFAULT_FILTER));
        let problems = invalid.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("CHATORA_RUSTDOC_MCP_LOG__FILTER"),
            "{problems:?}"
        );
        let filter = stderr_filter(&settings(&[]));
        assert_eq!(filter.max_level_hint(), Some(DEFAULT_FILTER));
    }

    #[test]
    fn the_log_directory_must_be_absolute() {
        let default = log_dir(&settings(&[])).expect("a home directory");
        assert!(
            default.ends_with(Path::new(env!("CARGO_PKG_NAME")).join(if cfg!(windows) {
                "state/logs"
            } else {
                "logs"
            }))
        );
        let relative = settings(&[("CHATORA_RUSTDOC_MCP_LOG__DIR", "logs")]);
        assert_eq!(log_dir(&relative), Ok(default));
        assert_eq!(relative.problems().len(), 1);
        let absolute = std::env::temp_dir().join("logs");
        let set = settings(&[(
            "CHATORA_RUSTDOC_MCP_LOG__DIR",
            absolute.to_str().expect("UTF-8"),
        )]);
        assert_eq!(log_dir(&set), Ok(absolute));
    }
}
