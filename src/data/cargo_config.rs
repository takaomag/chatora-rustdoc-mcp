//! What cargo's settings say for a project, read with cargo's rules without running cargo.
//!
//! Stable cargo cannot print its merged settings (`cargo config get` is unstable), so they are
//! read here (requirements R-TG-1 and R-WS-2, Q-31).
//!
//! The rules, checked against `cargo -Zunstable-options config get` of nightly 1.101 and, where
//! it can be seen, stable cargo 1.99.1 (doc/m2-plan.md, B3):
//!
//! - The files are `.cargo/config` (taken over `.cargo/config.toml` when both exist, with a
//!   warning) in the directory cargo runs in and every one above it, and `config` or
//!   `config.toml` in `$CARGO_HOME`, unless that is one of the former.
//! - From the lowest precedence: `$CARGO_HOME`, the directories above from the top down, and
//!   `CARGO_BUILD_TARGET`. A file that `include` names comes just below the one that names it.
//! - A string of a higher precedence replaces one of a lower; arrays are joined, the lower first.
//!   A string and an array for the same key make cargo fail. `CARGO_BUILD_TARGET` replaces a
//!   string and is added to the end of an array.
//! - `include` is an array of paths, relative to the file, or of tables `{ path, optional }`. A
//!   string, or a missing file that is not optional, makes cargo fail.

use std::{
    fs,
    io,
    path::{Path, PathBuf},
};

/// The most levels of `include` followed, against a loop: cargo fails on one too.
const MAX_INCLUDE_DEPTH: usize = 16;

/// What cargo's settings say for a project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CargoConfig {
    /// Every file that may be read, from the lowest precedence, including those that do not
    /// exist (yet): their contents decide the settings, so they belong in the key of a result
    /// made from them (requirement R-VR-2).
    pub files: Vec<PathBuf>,
    /// `build.target`, after the files and `CARGO_BUILD_TARGET` are merged.
    pub build_target: Option<BuildTarget>,
    /// Whether `[unstable] build-std` is set: the project builds its own std, which this server
    /// does not (`target_unsupported`, requirement R-TG-1).
    pub build_std: bool,
    /// `net.offline` in the files (`CARGO_NET_OFFLINE` is not read here): cargo does not reach
    /// the network, so a crate missing from the cache is `offline_miss` (requirement R-WS-7).
    pub net_offline: bool,
    /// `build.rustc` in the files (`RUSTC` is not read here): the rustc that cargo runs, which
    /// the checks before a build run too (decided in M2's D1).
    pub build_rustc: Option<Program>,
    /// `build.rustdoc` in the files (`RUSTDOC` is not read here), as `build_rustc`.
    pub build_rustdoc: Option<Program>,
}

impl CargoConfig {
    /// The target of its own that the project builds for with its own std, which this server
    /// does not document (`target_unsupported`, requirement R-TG-1): a target JSON of
    /// `build.target`, or with `[unstable] build-std` the target of `build.target`, or else
    /// `build-std` itself. `None` for a project that builds for targets that this server
    /// documents.
    #[must_use]
    pub fn own_target(&self) -> Option<String> {
        let targets = self.build_target.as_ref().map_or(&[][..], BuildTarget::all);
        if let Some(json) = targets.iter().find(|target| is_target_json(target)) {
            return Some(json.clone());
        }
        self.build_std.then(|| {
            targets
                .first()
                .cloned()
                .unwrap_or_else(|| "build-std".to_owned())
        })
    }
}

/// Whether `target`, of `build.target`, is the path of a target JSON.
#[must_use]
pub fn is_target_json(target: &str) -> bool {
    Path::new(target.trim())
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// A program that a setting of cargo names.
///
/// It is a path, which cargo takes relative to the directory that holds the `.cargo` directory of
/// the file, when it has a separator, or else a name that cargo looks for in `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program(pub PathBuf);

impl Program {
    /// The program of the value `value` in the file `file`.
    fn of(value: &str, file: &Path) -> Self {
        if value.contains('/') || value.contains(std::path::MAIN_SEPARATOR) {
            // `<dir>/.cargo/config.toml` names paths relative to `<dir>`.
            let base = file
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new(""));
            Self(base.join(value))
        } else {
            Self(PathBuf::from(value))
        }
    }
}

/// `build.target`: one target, or several, which cargo builds for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildTarget {
    /// A string.
    One(String),
    /// An array.
    Several(Vec<String>),
}

impl BuildTarget {
    /// The target that a lookup without one is for: the first of several (requirement R-TG-1).
    #[must_use]
    pub fn first(&self) -> Option<&str> {
        match self {
            Self::One(target) => Some(target),
            Self::Several(targets) => targets.first().map(String::as_str),
        }
    }

    /// Every target.
    #[must_use]
    pub fn all(&self) -> &[String] {
        match self {
            Self::One(target) => std::slice::from_ref(target),
            Self::Several(targets) => targets,
        }
    }
}

/// Why the settings cannot be read. Each makes cargo fail too.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A settings file cannot be read.
    #[error("{} cannot be read", .path.display())]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What failed.
        #[source]
        source: io::Error,
    },
    /// A settings file is not TOML, or a value is not of the type cargo takes.
    #[error("{} is not a valid cargo config: {why}", .path.display())]
    Malformed {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        why: String,
    },
    /// A string and an array are given for `build.target`.
    #[error("build.target is a string in one file and an array in another")]
    Mixed,
    /// A file that `include` names, without `optional`, does not exist.
    #[error("{}, which {} includes, does not exist", .path.display(), .by.display())]
    MissingInclude {
        /// The file named.
        path: PathBuf,
        /// The file that names it.
        by: PathBuf,
    },
    /// `include` goes deeper than 16 files, as in a loop.
    #[error("include goes deeper than {MAX_INCLUDE_DEPTH} files at {}", .0.display())]
    TooDeep(PathBuf),
}

/// The settings of cargo run in `dir`, with `cargo_home` as `$CARGO_HOME` and `env_target` as
/// `CARGO_BUILD_TARGET` (unset or empty when `None`).
///
/// # Errors
///
/// Returns an error when a file cannot be read or is not valid, as cargo would.
pub fn read(
    dir: &Path,
    cargo_home: Option<&Path>,
    env_target: Option<&str>,
) -> Result<CargoConfig, ConfigError> {
    let mut places: Vec<PathBuf> = dir.ancestors().map(|dir| dir.join(".cargo")).collect();
    places.reverse();
    if let Some(home) = cargo_home
        && !places.iter().any(|place| place == home)
    {
        places.insert(0, home.to_path_buf());
    }
    let mut config = CargoConfig::default();
    for place in places {
        let (unsuffixed, suffixed) = (place.join("config"), place.join("config.toml"));
        let file = if unsuffixed.is_file() {
            unsuffixed.clone()
        } else {
            suffixed.clone()
        };
        config.files.extend([unsuffixed, suffixed]);
        if file.is_file() {
            merge_file(&mut config, &file, 0)?;
        }
    }
    config.files.dedup();
    if let Some(target) = env_target.filter(|target| !target.is_empty()) {
        config.build_target = Some(match config.build_target.take() {
            Some(BuildTarget::Several(mut targets)) => {
                targets.push(target.to_owned());
                BuildTarget::Several(targets)
            },
            Some(BuildTarget::One(_)) | None => BuildTarget::One(target.to_owned()),
        });
    }
    Ok(config)
}

/// Merges the file `path` into `config`, after the files that it includes.
fn merge_file(config: &mut CargoConfig, path: &Path, depth: usize) -> Result<(), ConfigError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ConfigError::TooDeep(path.to_path_buf()));
    }
    let malformed = |why: String| ConfigError::Malformed {
        path: path.to_path_buf(),
        why,
    };
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    let table: toml::Table =
        toml::from_str(&text).map_err(|error| malformed(error.message().to_owned()))?;
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    for (included, optional) in includes(&table).map_err(malformed)? {
        let included = base.join(included);
        config.files.push(included.clone());
        if included.is_file() {
            merge_file(config, &included, depth.saturating_add(1))?;
        } else if !optional {
            return Err(ConfigError::MissingInclude {
                path: included,
                by: path.to_path_buf(),
            });
        }
    }
    if let Some(target) = table.get("build").and_then(|build| build.get("target")) {
        let target = build_target(target).ok_or_else(|| {
            malformed("build.target is neither a string nor an array of strings".to_owned())
        })?;
        config.build_target = Some(match (config.build_target.take(), target) {
            (None | Some(BuildTarget::One(_)), BuildTarget::One(target)) => {
                BuildTarget::One(target)
            },
            (None, several @ BuildTarget::Several(_)) => several,
            (Some(BuildTarget::Several(mut lower)), BuildTarget::Several(higher)) => {
                lower.extend(higher);
                BuildTarget::Several(lower)
            },
            (Some(BuildTarget::One(_)), BuildTarget::Several(_))
            | (Some(BuildTarget::Several(_)), BuildTarget::One(_)) => {
                return Err(ConfigError::Mixed);
            },
        });
    }
    if table
        .get("unstable")
        .and_then(|unstable| unstable.get("build-std"))
        .is_some()
    {
        config.build_std = true;
    }
    for (key, place) in [
        ("rustc", &mut config.build_rustc),
        ("rustdoc", &mut config.build_rustdoc),
    ] {
        if let Some(value) = table.get("build").and_then(|build| build.get(key)) {
            let value = value
                .as_str()
                .ok_or_else(|| malformed(format!("build.{key} is not a string")))?;
            *place = Some(Program::of(value, path));
        }
    }
    if let Some(offline) = table.get("net").and_then(|net| net.get("offline")) {
        config.net_offline = offline
            .as_bool()
            .ok_or_else(|| malformed("net.offline is not a boolean".to_owned()))?;
    }
    Ok(())
}

/// The files that `include` names, with whether each is optional.
fn includes(table: &toml::Table) -> Result<Vec<(String, bool)>, String> {
    let Some(include) = table.get("include") else {
        return Ok(Vec::new());
    };
    let not_valid = || "include is not an array of paths or of tables with a path".to_owned();
    include
        .as_array()
        .ok_or_else(not_valid)?
        .iter()
        .map(|entry| match entry {
            toml::Value::String(path) => Ok((path.clone(), false)),
            toml::Value::Table(entry) => {
                let path = entry
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(not_valid)?;
                let optional = entry
                    .get("optional")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                Ok((path.to_owned(), optional))
            },
            toml::Value::Integer(_)
            | toml::Value::Float(_)
            | toml::Value::Boolean(_)
            | toml::Value::Datetime(_)
            | toml::Value::Array(_) => Err(not_valid()),
        })
        .collect()
}

/// `build.target` as one string or an array of strings.
fn build_target(value: &toml::Value) -> Option<BuildTarget> {
    match value {
        toml::Value::String(target) => Some(BuildTarget::One(target.clone())),
        toml::Value::Array(targets) => targets
            .iter()
            .map(|target| target.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map(BuildTarget::Several),
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Table(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `files` (a path and its contents each) under a new temporary directory, in which
    /// cargo runs in `a/b/c` and `$CARGO_HOME` is `home`.
    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        fs::create_dir_all(dir.path().join("a/b/c")).expect("made");
        for (path, contents) in files {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            fs::write(path, contents).expect("written");
        }
        dir
    }

    fn target_of(
        dir: &tempfile::TempDir,
        env: Option<&str>,
    ) -> Result<Option<BuildTarget>, ConfigError> {
        read(
            &dir.path().join("a/b/c"),
            Some(&dir.path().join("home")),
            env,
        )
        .map(|config| config.build_target)
    }

    fn several(targets: &[&str]) -> BuildTarget {
        BuildTarget::Several(targets.iter().map(|&target| target.to_owned()).collect())
    }

    #[test]
    fn arrays_are_joined_from_the_lowest_precedence() {
        let dir = tree(&[
            (
                "a/.cargo/config.toml",
                "[build]\ntarget = [\"a1\", \"a2\"]\n",
            ),
            ("a/b/.cargo/config.toml", "[build]\ntarget = [\"b1\"]\n"),
            ("home/config.toml", "[build]\ntarget = [\"h1\"]\n"),
        ]);
        let joined = Some(several(&["h1", "a1", "a2", "b1"]));
        assert_eq!(target_of(&dir, None).expect("read"), joined);
        // The environment comes last.
        assert_eq!(
            target_of(&dir, Some("e1")).expect("read"),
            Some(several(&["h1", "a1", "a2", "b1", "e1"]))
        );
        assert_eq!(BuildTarget::Several(vec!["h1".into()]).first(), Some("h1"));
    }

    #[test]
    fn a_string_of_higher_precedence_wins_and_a_mix_fails() {
        let dir = tree(&[
            ("a/.cargo/config.toml", "[build]\ntarget = \"a1\"\n"),
            ("a/b/.cargo/config.toml", "[build]\ntarget = \"b1\"\n"),
        ]);
        assert_eq!(
            target_of(&dir, None).expect("read"),
            Some(BuildTarget::One("b1".into()))
        );
        assert_eq!(
            target_of(&dir, Some("e1")).expect("read"),
            Some(BuildTarget::One("e1".into()))
        );
        let mixed = tree(&[
            ("a/.cargo/config.toml", "[build]\ntarget = [\"a1\"]\n"),
            ("a/b/.cargo/config.toml", "[build]\ntarget = \"b1\"\n"),
        ]);
        assert!(matches!(target_of(&mixed, None), Err(ConfigError::Mixed)));
    }

    #[test]
    fn an_included_file_comes_below_the_one_that_includes_it() {
        let dir = tree(&[
            (
                "a/b/.cargo/config.toml",
                "include = [\"inc.toml\", { path = \"missing.toml\", optional = true }]\n\
                 [build]\ntarget = [\"b1\"]\n",
            ),
            (
                "a/b/.cargo/inc.toml",
                "include = [\"inc2.toml\"]\n[build]\ntarget = [\"i1\"]\n",
            ),
            (
                "a/b/.cargo/inc2.toml",
                "[build]\ntarget = [\"j1\"]\n[unstable]\nbuild-std = [\"core\"]\n",
            ),
        ]);
        let config = read(&dir.path().join("a/b/c"), None, None).expect("read");
        assert_eq!(config.build_target, Some(several(&["j1", "i1", "b1"])));
        assert!(config.build_std);
        // Every file that may be read, to see a change of any of them.
        for file in [
            "a/b/.cargo/inc.toml",
            "a/b/.cargo/inc2.toml",
            "a/b/.cargo/missing.toml",
        ] {
            assert!(
                config.files.contains(&dir.path().join(file)),
                "{file} in {:?}",
                config.files
            );
        }
    }

    #[test]
    fn rustc_and_rustdoc_are_paths_from_the_directory_above_the_settings_or_names() {
        let dir = tree(&[
            ("home/config.toml", "[build]\nrustc = \"rustc-1.80\"\n"),
            (
                "a/b/.cargo/config.toml",
                "[build]\nrustdoc = \"tools/rustdoc\"\n",
            ),
        ]);
        let config = read(
            &dir.path().join("a/b/c"),
            Some(&dir.path().join("home")),
            None,
        )
        .expect("read");
        // A name is looked for in PATH.
        assert_eq!(config.build_rustc, Some(Program("rustc-1.80".into())));
        // A path is relative to the directory that holds `.cargo`.
        assert_eq!(
            config.build_rustdoc,
            Some(Program(dir.path().join("a/b/tools/rustdoc")))
        );
    }

    #[test]
    fn net_offline_of_higher_precedence_wins() {
        let dir = tree(&[
            ("home/config.toml", "[net]\noffline = true\n"),
            ("a/b/.cargo/config.toml", "[net]\noffline = false\n"),
        ]);
        let read = |home: &str| {
            read(
                &dir.path().join("a/b/c"),
                Some(&dir.path().join(home)),
                None,
            )
            .expect("read")
            .net_offline
        };
        assert!(!read("home"), "the project's file wins over the cargo home");
        fs::write(dir.path().join("a/b/.cargo/config.toml"), "").expect("written");
        assert!(read("home"));
    }

    #[test]
    fn what_cargo_refuses_is_an_error() {
        for (contents, missing) in [
            ("include = \"inc.toml\"\n", false),
            ("include = [\"missing.toml\"]\n", true),
            ("[build]\ntarget = 1\n", false),
            ("[build\n", false),
        ] {
            let dir = tree(&[("a/b/.cargo/config.toml", contents)]);
            let error = target_of(&dir, None).expect_err(contents);
            assert_eq!(
                matches!(error, ConfigError::MissingInclude { .. }),
                missing,
                "{contents}: {error}"
            );
        }
    }

    #[test]
    fn the_file_without_extension_wins_and_cargo_home_is_read_once() {
        let dir = tree(&[
            ("a/b/.cargo/config", "[build]\ntarget = \"ext\"\n"),
            ("a/b/.cargo/config.toml", "[build]\ntarget = \"toml\"\n"),
        ]);
        assert_eq!(
            target_of(&dir, None).expect("read"),
            Some(BuildTarget::One("ext".into()))
        );
        // `$CARGO_HOME` that is the `.cargo` of a directory above is read there, once.
        let once = tree(&[
            ("a/.cargo/config.toml", "[build]\ntarget = [\"a1\"]\n"),
            ("a/b/.cargo/config.toml", "[build]\ntarget = [\"b1\"]\n"),
        ]);
        let config = read(
            &once.path().join("a/b/c"),
            Some(&once.path().join("a/.cargo")),
            None,
        )
        .expect("read");
        assert_eq!(config.build_target, Some(several(&["a1", "b1"])));
    }
}
