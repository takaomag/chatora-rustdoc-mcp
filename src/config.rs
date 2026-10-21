//! The settings: the user's config file, and environment variables that override it (requirement
//! R-DIST-3).
//!
//! The file is `chatora-rustdoc-mcp/config.toml` in the OS config directory, or the file that
//! `CHATORA_RUSTDOC_MCP_CONFIG` names. A key such as `log.dir` is `[log] dir` in the file, and
//! `CHATORA_RUSTDOC_MCP_LOG__DIR` in the environment. The server reads them once, at startup.
//!
//! Nothing stops the server from starting: a file that cannot be read or parsed, a value that is
//! not valid (replaced by its default), and a key that this version does not read (ignored) are
//! problems to log, so that a misspelled setting is not silently without effect. They are never
//! in responses, which would make the same call answer differently (requirement R-ARCH-5).

use std::{collections::BTreeMap, ffi::OsString, fmt, fs, io, path::PathBuf};

use etcetera::BaseStrategy as _;

/// The prefix of the environment variables of the settings.
const ENV_PREFIX: &str = "CHATORA_RUSTDOC_MCP_";

/// The environment variable that names the config file, which is not a setting itself.
pub const CONFIG_ENV_VAR: &str = "CHATORA_RUSTDOC_MCP_CONFIG";

/// The keys that this version reads. Any other key is reported and ignored.
const KEYS: &[&str] = &[
    "build.cargo_path",
    "build.jobs",
    "build.nightly_date",
    "build.rustc_path",
    "build.rustdoc_path",
    "cache.dir",
    "limits.concurrent_calls",
    "limits.extract_max_size",
    "limits.request_timeout_secs",
    "log.dir",
    "log.file_max_size",
    "log.filter",
    "log.retention_days",
    "log.total_max_size",
    "network.connect_timeout_secs",
    "network.offline",
    "network.read_timeout_secs",
    "std.fetch_from_dist",
];

/// The settings, from the config file and the environment.
#[derive(Debug, Default)]
pub struct Settings {
    /// The values by key. A variable of the environment replaces the file's value.
    values: BTreeMap<String, Setting>,
    /// What to log: problems of the file and of the keys, and values that are not valid.
    problems: parking_lot::Mutex<Vec<String>>,
}

/// A value and where it is from.
#[derive(Debug)]
struct Setting {
    value: Raw,
    /// The environment variable, or the key in the file, for the messages.
    origin: String,
}

#[derive(Debug)]
enum Raw {
    File(toml::Value),
    Env(OsString),
}

/// A value of a setting, to be read as its kind: from the file, it has a TOML type; from the
/// environment, it is text, which is read as the kind asks.
#[derive(Debug, Clone, Copy)]
pub struct Value<'a>(&'a Raw);

impl<'a> Value<'a> {
    /// The text of a string.
    #[must_use]
    pub fn text(self) -> Option<&'a str> {
        match self.0 {
            Raw::File(toml::Value::String(text)) => Some(text),
            Raw::File(_) => None,
            Raw::Env(text) => text.to_str(),
        }
    }

    /// A whole number that is not negative. In the environment, digits.
    #[must_use]
    pub fn whole_number(self) -> Option<u64> {
        match self.0 {
            Raw::File(toml::Value::Integer(number)) => u64::try_from(*number).ok(),
            Raw::File(_) => None,
            Raw::Env(text) => text.to_str()?.trim().parse().ok(),
        }
    }

    /// A boolean. In the environment, `true` or `false` in any case.
    #[must_use]
    pub fn boolean(self) -> Option<bool> {
        match self.0 {
            Raw::File(toml::Value::Boolean(value)) => Some(*value),
            Raw::File(_) => None,
            Raw::Env(text) => match text.to_str()?.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
        }
    }

    /// A size: a string such as `"20MB"`, a whole number with a unit of `B`, `KB`, `MB`, `GB`
    /// (powers of 1000) or `KiB`, `MiB`, `GiB` (powers of 1024), in any case, which is bytes
    /// without one. It is not zero.
    #[must_use]
    pub fn size(self) -> Option<u64> {
        parse_size(self.text()?.trim())
    }

    /// A path. In the environment, it need not be UTF-8.
    #[must_use]
    pub fn path(self) -> Option<PathBuf> {
        match self.0 {
            Raw::File(toml::Value::String(text)) => Some(PathBuf::from(text)),
            Raw::File(_) => None,
            Raw::Env(text) => Some(PathBuf::from(text)),
        }
    }
}

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Raw::File(toml::Value::String(text)) => write!(f, "{text:?}"),
            Raw::File(toml::Value::Integer(number)) => write!(f, "{number}"),
            Raw::File(toml::Value::Float(number)) => write!(f, "{number}"),
            Raw::File(toml::Value::Boolean(value)) => write!(f, "{value}"),
            Raw::File(toml::Value::Datetime(_)) => f.write_str("a date-time"),
            Raw::File(toml::Value::Array(_)) => f.write_str("an array"),
            Raw::File(toml::Value::Table(_)) => f.write_str("a table"),
            Raw::Env(text) => write!(f, "{:?}", text.to_string_lossy()),
        }
    }
}

impl Settings {
    /// The settings of the config file and of this process's environment. Problems are kept for
    /// [`Self::report`].
    #[must_use]
    pub fn load() -> Self {
        let mut problems = Vec::new();
        let file = config_file(std::env::var_os(CONFIG_ENV_VAR), &mut problems);
        let settings = Self::from_sources(file.as_ref(), std::env::vars_os());
        settings.problems.lock().splice(0..0, problems);
        settings
    }

    /// The settings of `file` (its path and contents) and of `env`, the variables of an
    /// environment.
    #[must_use]
    pub fn from_sources(
        file: Option<&(PathBuf, String)>,
        env: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Self {
        let mut values = BTreeMap::new();
        let mut problems = Vec::new();
        if let Some((path, text)) = file {
            match toml::from_str::<toml::Table>(text) {
                Ok(table) => {
                    let mut flat = Vec::new();
                    flatten("", table, &mut flat);
                    for (key, value) in flat {
                        let origin = format!("{key} in {}", path.display());
                        if KEYS.contains(&key.as_str()) {
                            values.insert(key, Setting {
                                value: Raw::File(value),
                                origin,
                            });
                        } else {
                            problems.push(format!(
                                "{origin} is not a setting of this version, so it is ignored"
                            ));
                        }
                    }
                },
                Err(error) => problems.push(format!(
                    "the config file {} is not valid TOML, so it is ignored: {}",
                    path.display(),
                    error.message()
                )),
            }
        }
        let mut env: Vec<_> = env
            .into_iter()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value)))
            // An empty variable is taken as unset, as `CLAUDE_PROJECT_DIR` is (requirement
            // R-MCP-3).
            .filter(|(name, value)| {
                name.starts_with(ENV_PREFIX) && name != CONFIG_ENV_VAR && !value.is_empty()
            })
            .collect();
        env.sort();
        for (name, value) in env {
            let key = env_key(&name);
            if KEYS.contains(&key.as_str()) {
                values.insert(key, Setting {
                    value: Raw::Env(value),
                    origin: name,
                });
            } else {
                problems.push(format!(
                    "{name} is not a setting of this version, so it is ignored"
                ));
            }
        }
        Self {
            values,
            problems: parking_lot::Mutex::new(problems),
        }
    }

    /// The value of `key` that `read` accepts. `None` when it is not set, or when `read` does
    /// not accept it, which is kept as a problem with `read`'s reason, as `"a whole number from
    /// 1"`: the caller uses the default.
    pub fn get<T>(
        &self,
        key: &'static str,
        read: impl FnOnce(Value<'_>) -> Result<T, &'static str>,
    ) -> Option<T> {
        debug_assert!(KEYS.contains(&key), "{key} is not in KEYS");
        let setting = self.values.get(key)?;
        let value = Value(&setting.value);
        read(value)
            .inspect_err(|expected| {
                self.problems.lock().push(format!(
                    "{} is {value}, but it must be {expected}, so the default is used",
                    setting.origin
                ));
            })
            .ok()
    }

    /// Keeps `problem` to log with the others.
    pub fn add_problem(&self, problem: String) {
        self.problems.lock().push(problem);
    }

    /// Logs the problems found so far, once each.
    pub fn report(&self) {
        let problems = std::mem::take(&mut *self.problems.lock());
        for problem in problems {
            tracing::warn!("{problem}");
        }
    }

    /// The problems found so far, for the tests.
    #[cfg(test)]
    pub(crate) fn problems(&self) -> Vec<String> {
        self.problems.lock().clone()
    }
}

/// The path and contents of the config file: the one that `explicit`
/// ([`CONFIG_ENV_VAR`]) names, or the default one, which need not exist. `None` when there is
/// none to read, with a problem when the file named cannot be read.
fn config_file(
    explicit: Option<OsString>,
    problems: &mut Vec<String>,
) -> Option<(PathBuf, String)> {
    let explicit = explicit
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| {
            path.is_absolute() || {
                problems.push(format!(
                    "{CONFIG_ENV_VAR} must be an absolute path, but it is {}, so the default \
                     config file is read",
                    path.display()
                ));
                false
            }
        });
    let (path, named) = match explicit {
        Some(path) => (path, true),
        None => (default_config_file()?, false),
    };
    match fs::read_to_string(&path) {
        Ok(text) => Some((path, text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !named => None,
        Err(error) => {
            problems.push(format!(
                "the config file {} cannot be read, so it is ignored: {error}",
                path.display()
            ));
            None
        },
    }
}

/// `chatora-rustdoc-mcp/config.toml` in the OS config directory: `$XDG_CONFIG_HOME` (`~/.config`
/// when unset) on Linux and macOS, and `%APPDATA%` on Windows. `None` without a home directory.
fn default_config_file() -> Option<PathBuf> {
    let strategy = etcetera::choose_base_strategy().ok()?;
    Some(
        strategy
            .config_dir()
            .join(env!("CARGO_PKG_NAME"))
            .join("config.toml"),
    )
}

/// Adds the values of `table` to `flat` by dotted key, under `prefix`.
fn flatten(prefix: &str, table: toml::Table, flat: &mut Vec<(String, toml::Value)>) {
    for (key, value) in table {
        let key = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        if let toml::Value::Table(table) = value {
            flatten(&key, table, flat);
        } else {
            flat.push((key, value));
        }
    }
}

/// The key of the environment variable `name`: `CHATORA_RUSTDOC_MCP_LOG__DIR` is `log.dir`.
fn env_key(name: &str) -> String {
    name.get(ENV_PREFIX.len()..)
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace("__", ".")
}

/// A size such as `20MB` (see [`Value::size`]).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn variables(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect()
    }

    fn file(text: &str) -> (PathBuf, String) {
        (PathBuf::from("config.toml"), text.to_owned())
    }

    fn whole_number(value: Value<'_>) -> Result<u64, &'static str> {
        value.whole_number().ok_or("a whole number")
    }

    #[test]
    fn the_environment_wins_over_the_file() {
        let settings = Settings::from_sources(
            Some(&file(
                "[log]\nretention_days = 3\nfile_max_size = \"1KiB\"\n",
            )),
            variables(&[("CHATORA_RUSTDOC_MCP_LOG__RETENTION_DAYS", " 5 ")]),
        );
        assert_eq!(settings.get("log.retention_days", whole_number), Some(5));
        assert_eq!(
            settings.get("log.file_max_size", |value| value.size().ok_or("a size")),
            Some(1024)
        );
        assert_eq!(settings.get("log.total_max_size", whole_number), None);
        assert_eq!(settings.problems(), Vec::<String>::new());
    }

    #[test]
    fn keys_that_this_version_does_not_read_are_problems() {
        let settings = Settings::from_sources(
            Some(&file("[log]\nretention = 3\n[nothing]\nx = 1\n")),
            variables(&[
                ("CHATORA_RUSTDOC_MCP_LOG__RETENTON_DAYS", "1"),
                ("CHATORA_RUSTDOC_MCP_CONFIG", "/elsewhere.toml"),
                ("CHATORA_RUSTDOC_MCP_LOG__DIR", ""),
                ("PATH", "/bin"),
            ]),
        );
        assert_eq!(settings.problems(), [
            "log.retention in config.toml is not a setting of this version, so it is ignored",
            "nothing.x in config.toml is not a setting of this version, so it is ignored",
            "CHATORA_RUSTDOC_MCP_LOG__RETENTON_DAYS is not a setting of this version, so it is \
             ignored",
        ]);
        // An empty variable is unset.
        assert_eq!(
            settings.get("log.dir", |value| value.path().ok_or("")),
            None
        );
    }

    #[test]
    fn a_value_that_is_not_valid_is_a_problem_and_unset() {
        let settings = Settings::from_sources(
            Some(&file("[log]\nretention_days = \"3\"\n")),
            variables(&[("CHATORA_RUSTDOC_MCP_LIMITS__CONCURRENT_CALLS", "many")]),
        );
        assert_eq!(settings.get("log.retention_days", whole_number), None);
        assert_eq!(settings.get("limits.concurrent_calls", whole_number), None);
        assert_eq!(settings.problems(), [
            "log.retention_days in config.toml is \"3\", but it must be a whole number, so the \
             default is used",
            "CHATORA_RUSTDOC_MCP_LIMITS__CONCURRENT_CALLS is \"many\", but it must be a whole \
             number, so the default is used",
        ]);
    }

    #[test]
    fn a_file_that_is_not_toml_is_ignored() {
        let settings = Settings::from_sources(Some(&file("[log\n")), []);
        let problems = settings.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("the config file config.toml is not valid TOML"),
            "{problems:?}"
        );
    }

    #[test]
    fn the_config_file_named_must_be_absolute_and_readable() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[log]\nretention_days = 1\n").expect("written");
        let mut problems = Vec::new();
        assert_eq!(
            config_file(Some(path.clone().into()), &mut problems),
            Some((path, "[log]\nretention_days = 1\n".to_owned()))
        );
        assert_eq!(problems, Vec::<String>::new());

        let missing = dir.path().join("missing.toml");
        assert_eq!(config_file(Some(missing.into()), &mut problems), None);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("cannot be read"), "{problems:?}");

        // A relative path falls back to the default file, which need not exist.
        problems.clear();
        let _default = config_file(Some("config.toml".into()), &mut problems);
        assert!(
            problems
                .first()
                .is_some_and(|problem| problem.contains("must be an absolute path")),
            "{problems:?}"
        );
    }

    #[test]
    fn values_of_the_environment_are_read_as_their_kind() {
        let env = |text: &str| Raw::Env(text.into());
        assert_eq!(Value(&env("TRUE")).boolean(), Some(true));
        assert_eq!(Value(&env("false")).boolean(), Some(false));
        assert_eq!(Value(&env("1")).boolean(), None);
        assert_eq!(Value(&env("12")).whole_number(), Some(12));
        assert_eq!(Value(&env("-1")).whole_number(), None);
        assert_eq!(
            Value(&Raw::File(toml::Value::Integer(-1))).whole_number(),
            None
        );
        assert_eq!(Value(&Raw::File(toml::Value::Integer(7))).boolean(), None);
    }

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
}
