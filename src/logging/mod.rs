//! Logging, to stderr and to a file.
//!
//! People read stderr. The file has JSON Lines, with a line for each tool call (requirements
//! R-OBS-1 and R-OBS-2). While serving, stdout carries only JSON-RPC messages (requirement
//! R-SEC-1).

mod file;
mod layer;

use std::{
    ffi::OsString,
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

use self::file::{Limits, LogFile};

/// The name of the span of a tool call. Its `request_id` goes on the warnings in the log file.
pub const CALL_SPAN: &str = "call";

/// The field of [`CALL_SPAN`] and of the log lines that holds the `request_id` (requirement
/// R-ERR-2).
pub const REQUEST_ID_FIELD: &str = "request_id";

/// Environment variable that filters the stderr log, in the `EnvFilter` syntax (setting
/// `log.filter`, requirement R-DIST-3).
const FILTER_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_LOG__FILTER";

/// Environment variable of the directory of the log files (setting `log.dir`).
pub const DIR_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_LOG__DIR";

/// Environment variable of the size of a log file before the next one (`log.file_max_size`).
const FILE_MAX_SIZE_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_LOG__FILE_MAX_SIZE";

/// Environment variable of the days that logs are kept (`log.retention_days`).
const RETENTION_DAYS_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_LOG__RETENTION_DAYS";

/// Environment variable of the size of all logs together (`log.total_max_size`).
const TOTAL_MAX_SIZE_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_LOG__TOTAL_MAX_SIZE";

/// The stderr log's filter when none is set, or when the one set is not valid.
const DEFAULT_FILTER: LevelFilter = LevelFilter::INFO;

/// `log.file_max_size` unless set (requirement R-OBS-2, initial value): 20MB.
const DEFAULT_FILE_MAX_SIZE: u64 = 20_000_000;

/// `log.retention_days` unless set (requirement R-OBS-2, initial value).
const DEFAULT_RETENTION_DAYS: u64 = 14;

/// `log.total_max_size` unless set (requirement R-OBS-2, initial value): 50MB.
const DEFAULT_TOTAL_MAX_SIZE: u64 = 50_000_000;

/// The log file, once [`init`] has opened it.
static FILE: OnceLock<Arc<LogFile>> = OnceLock::new();

/// Installs the global subscriber that writes the log to stderr and warnings and errors to the log
/// file, and opens the file for [`write_record`].
///
/// Settings that are not valid are replaced by their defaults, and a log file that cannot be
/// opened is done without; both are logged, and the server still starts.
///
/// # Errors
///
/// Returns an error when a global subscriber is already installed.
pub fn init() -> Result<(), TryInitError> {
    let mut problems = Vec::new();
    let filter = stderr_filter(std::env::var_os(FILTER_ENV_VAR), &mut problems);
    let limits = limits(&mut problems);
    let file = match log_dir(std::env::var_os(DIR_ENV_VAR)) {
        Ok(dir) => LogFile::open(&dir, limits).map(Arc::new).map_err(|error| {
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
    for problem in problems {
        tracing::warn!("{problem}");
    }
    match file {
        Ok(file) => {
            tracing::info!(path = %file.path().display(), "logging tool calls");
            // `init` installs the subscriber only once, so the file is set only once.
            FILE.get_or_init(|| file);
        },
        Err(problem) => tracing::warn!("{problem}; tool calls are not logged to a file"),
    }
    Ok(())
}

/// Writes `record` as a line of the log file, if there is one. Records are not filtered: each
/// tool call has one (requirements R-OBS-1 and R-DIST-3).
pub fn write_record(record: &Value) {
    if let Some(file) = FILE.get() {
        file.write_line(&record.to_string());
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
fn stderr_filter(value: Option<OsString>, problems: &mut Vec<String>) -> EnvFilter {
    let builder = EnvFilter::builder().with_default_directive(DEFAULT_FILTER.into());
    let Some(value) = value else {
        return builder.parse_lossy("");
    };
    let parsed = value
        .to_str()
        .ok_or_else(|| "it is not UTF-8".to_owned())
        .and_then(|value| builder.parse(value).map_err(|error| error.to_string()));
    parsed.unwrap_or_else(|why| {
        problems.push(format!(
            "{FILTER_ENV_VAR} is not a valid filter ({why}), so the default `{DEFAULT_FILTER}` is \
             used"
        ));
        builder.parse_lossy("")
    })
}

/// The limits of the log files, from their settings or their defaults.
fn limits(problems: &mut Vec<String>) -> Limits {
    let file_max_size = setting(
        FILE_MAX_SIZE_ENV_VAR,
        DEFAULT_FILE_MAX_SIZE,
        parse_size,
        problems,
    );
    let days = setting(
        RETENTION_DAYS_ENV_VAR,
        DEFAULT_RETENTION_DAYS,
        |value| value.parse::<u64>().ok().filter(|days| *days > 0),
        problems,
    );
    let total_max_size = setting(
        TOTAL_MAX_SIZE_ENV_VAR,
        DEFAULT_TOTAL_MAX_SIZE,
        parse_size,
        problems,
    );
    Limits {
        file_max_size,
        retention: Duration::from_secs(days.saturating_mul(24 * 60 * 60)),
        total_max_size,
    }
}

/// The value of the setting in `env_var` that `parse` reads, or `default` when it is not set or
/// not valid (with a problem to log).
fn setting(
    env_var: &str,
    default: u64,
    parse: impl FnOnce(&str) -> Option<u64>,
    problems: &mut Vec<String>,
) -> u64 {
    let Some(value) = std::env::var_os(env_var) else {
        return default;
    };
    value
        .to_str()
        .map(str::trim)
        .and_then(parse)
        .unwrap_or_else(|| {
            problems.push(format!(
                "{env_var} is not valid ({}), so the default {default} is used",
                value.to_string_lossy()
            ));
            default
        })
}

/// A size such as `20MB`: a whole number, and a unit of `B`, `KB`, `MB`, `GB` (powers of 1000) or
/// `KiB`, `MiB`, `GiB` (powers of 1024), in any case. A number alone is bytes.
fn parse_size(text: &str) -> Option<u64> {
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(digits);
    let number: u64 = number.parse().ok()?;
    let unit: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1000,
        "mb" => 1000 * 1000,
        "gb" => 1000 * 1000 * 1000,
        "kib" => 1024,
        "mib" => 1024 * 1024,
        "gib" => 1024 * 1024 * 1024,
        _ => return None,
    };
    number.checked_mul(unit).filter(|size| *size > 0)
}

/// The directory of the logs: `log.dir`, or `logs` in the state directory (requirements R-OBS-2
/// and R-DIST-3): `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs` (`~/.local/state/…` when unset) on
/// Linux and macOS, and `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs` on Windows.
fn log_dir(value: Option<OsString>) -> Result<PathBuf, String> {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        let dir = PathBuf::from(value);
        return if dir.is_absolute() {
            Ok(dir)
        } else {
            Err(format!(
                "{DIR_ENV_VAR} must be an absolute path, but it is {}",
                dir.display()
            ))
        };
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

    #[test]
    fn sizes() {
        assert_eq!(parse_size("20MB"), Some(20_000_000));
        assert_eq!(parse_size("20 mb"), Some(20_000_000));
        assert_eq!(parse_size("1KiB"), Some(1024));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("0MB"), None);
        assert_eq!(parse_size("MB"), None);
        assert_eq!(parse_size("1.5MB"), None);
        assert_eq!(parse_size("20TB"), None);
        assert_eq!(parse_size("99999999999999GB"), None);
    }

    #[test]
    fn a_filter_with_any_invalid_directive_is_replaced_by_the_default() {
        let mut problems = Vec::new();
        let filter = stderr_filter(Some("debug".into()), &mut problems);
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::DEBUG));
        assert_eq!(problems, Vec::<String>::new());
        let filter = stderr_filter(Some("debug,a=nonsense".into()), &mut problems);
        assert_eq!(filter.max_level_hint(), Some(DEFAULT_FILTER));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(FILTER_ENV_VAR), "{problems:?}");
        let filter = stderr_filter(None, &mut problems);
        assert_eq!(filter.max_level_hint(), Some(DEFAULT_FILTER));
    }

    #[test]
    fn the_log_directory_must_be_absolute() {
        log_dir(Some("logs".into())).expect_err("relative");
        let absolute = std::env::temp_dir().join("logs");
        assert_eq!(log_dir(Some(absolute.clone().into())), Ok(absolute));
        let default = log_dir(None).expect("a home directory");
        assert!(
            default.ends_with(Path::new(env!("CARGO_PKG_NAME")).join(if cfg!(windows) {
                "state/logs"
            } else {
                "logs"
            }))
        );
    }
}
