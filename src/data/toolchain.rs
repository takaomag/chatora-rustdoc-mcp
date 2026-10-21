//! Which cargo, rustc and rustdoc build the documentation of a project (requirements R-WS-1,
//! R-WS-8 and R-TG-1; `doc/m2-plan.md` §6.5, the decisions of D1).
//!
//! The server's settings `build.cargo_path`, `build.rustc_path` and `build.rustdoc_path` name
//! them. One that is not set is looked for next to one that is, with the same version suffix
//! (`rustc-1.80`, then `rustdoc-1.80`), or else without one; links are not followed. Without the
//! settings, cargo is that of `PATH` (rustup's proxy, which takes the project's toolchain from the
//! directory it runs in), and rustc and rustdoc are those that cargo runs: `RUSTC` and `RUSTDOC`,
//! or else the project's cargo settings, or else the toolchain's.
//!
//! `RUSTUP_TOOLCHAIN`, which would win over the project's `rust-toolchain.toml`, is taken out of
//! the children run in a project.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use super::{cargo_config, target::TargetTriple, tool};
use crate::config::Settings;

/// A program of a toolchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// cargo.
    Cargo,
    /// rustc.
    Rustc,
    /// rustdoc.
    Rustdoc,
}

impl Kind {
    /// Its name, without version suffix or extension.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Rustc => "rustc",
            Self::Rustdoc => "rustdoc",
        }
    }

    /// The key of its setting.
    const fn setting(self) -> &'static str {
        match self {
            Self::Cargo => "build.cargo_path",
            Self::Rustc => "build.rustc_path",
            Self::Rustdoc => "build.rustdoc_path",
        }
    }
}

/// The programs that the server's settings name, with those not set found next to those set.
/// All `None` when nothing is set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Programs {
    cargo: Option<PathBuf>,
    rustc: Option<PathBuf>,
    rustdoc: Option<PathBuf>,
}

/// Why the programs of the settings cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramsError {
    /// A setting names a path that is not a file.
    #[error("{setting} is {}, which is not a file", path.display())]
    Missing {
        /// The setting.
        setting: &'static str,
        /// What it names.
        path: PathBuf,
    },
    /// A program that is not set is not next to one that is.
    #[error(
        "{} is not set, and was not found next to {}: looked for {}",
        kind.setting(),
        next_to.display(),
        looked.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    NotFound {
        /// What was looked for.
        kind: Kind,
        /// The program set that it was looked for next to.
        next_to: PathBuf,
        /// Where.
        looked: Vec<PathBuf>,
    },
}

/// This process's programs, once the server has read its settings.
static THIS: std::sync::OnceLock<Result<Programs, ProgramsError>> = std::sync::OnceLock::new();

impl Programs {
    /// Sets this process's programs from `settings`, once: a problem is logged, and the builds
    /// return it (requirement R-WS-1). `RUSTUP_TOOLCHAIN` in the server's environment, which the
    /// children run in a project go without, is logged too.
    pub fn set_this(settings: &Settings) {
        let read = Self::from_settings(settings);
        if let Err(error) = &read {
            tracing::warn!(%error, "the programs of the build settings cannot be used");
        }
        if let Some(toolchain) = std::env::var_os("RUSTUP_TOOLCHAIN") {
            tracing::info!(
                toolchain = %toolchain.to_string_lossy(),
                "RUSTUP_TOOLCHAIN is not passed to what runs in a project, whose own toolchain applies"
            );
        }
        drop(THIS.set(read));
    }

    /// This process's programs: those of the settings, or none when they cannot be used or were
    /// not read, as in the tests.
    #[must_use]
    pub fn this() -> &'static Self {
        static NONE: Programs = Programs {
            cargo: None,
            rustc: None,
            rustdoc: None,
        };
        THIS.get()
            .and_then(|read| read.as_ref().ok())
            .unwrap_or(&NONE)
    }

    /// Why this process's programs cannot be used, if they cannot.
    #[must_use]
    pub fn problem() -> Option<&'static ProgramsError> {
        THIS.get().and_then(|read| read.as_ref().err())
    }

    /// The programs of the settings (requirement R-WS-1).
    ///
    /// # Errors
    ///
    /// Returns an error when a setting names what is not a file, or a program that is not set is
    /// not found next to one that is.
    pub fn from_settings(settings: &Settings) -> Result<Self, ProgramsError> {
        let read = |kind: Kind| {
            settings.get(kind.setting(), |value| {
                value
                    .path()
                    .filter(|path| path.is_absolute())
                    .ok_or("an absolute path")
            })
        };
        Self::find(read(Kind::Cargo), read(Kind::Rustc), read(Kind::Rustdoc))
    }

    /// The programs `cargo`, `rustc` and `rustdoc`, as set, with those not set found next to
    /// those set: rustdoc and rustc next to each other first, as they must be of one toolchain
    /// (`E0786` otherwise), and cargo next to either.
    fn find(
        cargo: Option<PathBuf>,
        rustc: Option<PathBuf>,
        rustdoc: Option<PathBuf>,
    ) -> Result<Self, ProgramsError> {
        if cargo.is_none() && rustc.is_none() && rustdoc.is_none() {
            return Ok(Self::default());
        }
        for (kind, path) in [
            (Kind::Cargo, &cargo),
            (Kind::Rustc, &rustc),
            (Kind::Rustdoc, &rustdoc),
        ] {
            if let Some(path) = path
                && !path.is_file()
            {
                return Err(ProgramsError::Missing {
                    setting: kind.setting(),
                    path: path.clone(),
                });
            }
        }
        let anchors = |order: [(Kind, &Option<PathBuf>); 2]| {
            order
                .into_iter()
                .find_map(|(kind, path)| Some((kind, path.clone()?)))
        };
        let rustc = if let Some(rustc) = rustc {
            rustc
        } else {
            let (kind, set) = anchors([(Kind::Rustdoc, &rustdoc), (Kind::Cargo, &cargo)])
                .ok_or_else(|| unreachable_none(Kind::Rustc))?;
            next_to(&set, kind, Kind::Rustc)?
        };
        let rustdoc = match rustdoc {
            Some(rustdoc) => rustdoc,
            None => next_to(&rustc, Kind::Rustc, Kind::Rustdoc)?,
        };
        let cargo = match cargo {
            Some(cargo) => cargo,
            None => next_to(&rustc, Kind::Rustc, Kind::Cargo)?,
        };
        Ok(Self {
            cargo: Some(cargo),
            rustc: Some(rustc),
            rustdoc: Some(rustdoc),
        })
    }

    /// Whether the settings name the programs: then no other toolchain is switched to
    /// (requirement R-WS-1).
    #[must_use]
    pub const fn are_set(&self) -> bool {
        self.rustc.is_some()
    }

    /// The cargo to run.
    #[must_use]
    pub fn cargo(&self) -> &Path {
        self.cargo
            .as_deref()
            .unwrap_or_else(|| Path::new(Kind::Cargo.name()))
    }

    /// Sets up `command`, cargo run in a project, with the rustc and rustdoc of the settings as
    /// `RUSTC` and `RUSTDOC`, and without `RUSTUP_TOOLCHAIN`.
    pub fn prepare_cargo(&self, command: &mut tokio::process::Command) {
        command.env_remove("RUSTUP_TOOLCHAIN");
        if let Some(rustc) = &self.rustc {
            command.env("RUSTC", rustc);
        }
        if let Some(rustdoc) = &self.rustdoc {
            command.env("RUSTDOC", rustdoc);
        }
    }

    /// The `kind` (rustc or rustdoc) that cargo runs in `root`, for the checks before a build:
    /// that of the settings, or else `RUSTC` or `RUSTDOC`, or else `CARGO_BUILD_RUSTC` or
    /// `CARGO_BUILD_RUSTDOC`, or else the project's cargo settings read with `cargo_home`, or else
    /// the toolchain's (decided in M2's D1).
    #[must_use]
    pub fn used_in(&self, kind: Kind, root: &Path, cargo_home: Option<&Path>) -> PathBuf {
        let (set, variables) = match kind {
            Kind::Rustdoc => (&self.rustdoc, ["RUSTDOC", "CARGO_BUILD_RUSTDOC"]),
            Kind::Rustc | Kind::Cargo => (&self.rustc, ["RUSTC", "CARGO_BUILD_RUSTC"]),
        };
        if let Some(set) = set {
            return set.clone();
        }
        let config = cargo_config::read(root, cargo_home, None).ok();
        let project = config.and_then(|config| match kind {
            Kind::Rustdoc => config.build_rustdoc,
            Kind::Rustc | Kind::Cargo => config.build_rustc,
        });
        // `CARGO_BUILD_RUSTC` sets `build.rustc` over the files, below `RUSTC`.
        let variable = variables
            .into_iter()
            .find_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()));
        chosen(variable, project.map(|program| program.0), root, kind)
    }
}

/// What cargo runs in `root`: `variable` (`RUSTC`, relative to the directory it runs in), or else
/// `project` (the project's cargo settings), or else the toolchain's `kind`.
fn chosen(
    variable: Option<OsString>,
    project: Option<PathBuf>,
    root: &Path,
    kind: Kind,
) -> PathBuf {
    variable.filter(|value| !value.is_empty()).map_or_else(
        || project.unwrap_or_else(|| PathBuf::from(kind.name())),
        |value| {
            let value = PathBuf::from(value);
            if value.components().count() > 1 {
                root.join(value)
            } else {
                value
            }
        },
    )
}

/// The error of [`Programs::find`] that cannot happen: something is set when this is called.
const fn unreachable_none(kind: Kind) -> ProgramsError {
    ProgramsError::NotFound {
        kind,
        next_to: PathBuf::new(),
        looked: Vec::new(),
    }
}

/// The `wanted` program next to `set`, a `kind` program: with its version suffix, or else none.
fn next_to(set: &Path, kind: Kind, wanted: Kind) -> Result<PathBuf, ProgramsError> {
    let dir = set.parent().unwrap_or_else(|| Path::new(""));
    let extension = std::env::consts::EXE_SUFFIX;
    let stem = set
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = stem.strip_suffix(extension).unwrap_or(&stem);
    let suffix = stem.strip_prefix(kind.name()).unwrap_or_default();
    let mut looked = Vec::new();
    for suffix in [suffix, ""] {
        let candidate = dir.join(format!("{}{suffix}{extension}", wanted.name()));
        if looked.contains(&candidate) {
            continue;
        }
        if candidate.is_file() {
            return Ok(candidate);
        }
        looked.push(candidate);
    }
    Err(ProgramsError::NotFound {
        kind: wanted,
        next_to: set.to_path_buf(),
        looked,
    })
}

/// rustc and rustdoc of different toolchains, which cargo cannot build with (`E0786`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{} is {rustc}, and {} is {rustdoc}: set build.rustc_path and build.rustdoc_path to those of \
     one toolchain",
    rustc_path.display(),
    rustdoc_path.display()
)]
pub struct Mismatch {
    /// rustc.
    pub rustc_path: PathBuf,
    /// Its version, as `-vV` tells it.
    pub rustc: String,
    /// rustdoc.
    pub rustdoc_path: PathBuf,
    /// Its version.
    pub rustdoc: String,
}

/// Whether `rustc` and `rustdoc`, run in `dir`, are of one toolchain: their `-vV` tell the same
/// commit (decided in M2's D1). Nothing is told when either cannot be run, which the build then
/// tells.
///
/// # Errors
///
/// Returns their versions when they differ.
pub async fn same_toolchain(rustc: &Path, rustdoc: &Path, dir: &Path) -> Result<(), Mismatch> {
    let version = async |program: &Path| {
        tool::output_kept(dir, tool::Place::Project, program, &["-vV"])
            .await
            .ok()
    };
    let (Some(rustc_version), Some(rustdoc_version)) =
        (version(rustc).await, version(rustdoc).await)
    else {
        return Ok(());
    };
    let commit = |version: &str| {
        version
            .lines()
            .find_map(|line| line.strip_prefix("commit-hash: "))
            .map(str::trim)
            .map(str::to_owned)
    };
    if commit(&rustc_version) == commit(&rustdoc_version) {
        return Ok(());
    }
    let first = |version: &str| version.lines().next().unwrap_or_default().to_owned();
    Err(Mismatch {
        rustc_path: rustc.to_path_buf(),
        rustc: first(&rustc_version),
        rustdoc_path: rustdoc.to_path_buf(),
        rustdoc: first(&rustdoc_version),
    })
}

/// Why a toolchain cannot build for a target.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Missing {
    /// rustup has not installed the toolchain that the project names (`toolchain_missing`).
    #[error("the toolchain {toolchain} is not installed")]
    NotInstalled {
        /// Its name, as rustup tells it.
        toolchain: String,
    },
    /// The toolchain has no standard library for the target (`toolchain_missing`, requirement
    /// R-TG-1).
    #[error("the toolchain has no standard library for {target}")]
    Target {
        /// The target.
        target: TargetTriple,
    },
    /// rustc could not be run or did not tell its sysroot.
    #[error("{0}")]
    NotRun(String),
}

/// Whether `rustc`, run in the project at `root` as cargo would, can build for `target`.
///
/// That is, its toolchain is installed, and its sysroot has the standard library of `target` (for
/// the host, its `libcore`), so that cargo is not started to fail (requirement R-TG-1, decided in
/// M2's D1).
///
/// # Errors
///
/// Returns what is missing.
pub async fn check_target(rustc: &Path, root: &Path, target: &TargetTriple) -> Result<(), Missing> {
    let run = async |args: &[&str]| {
        tool::output_kept(root, tool::Place::Project, rustc, args)
            .await
            .map_err(|why| not_installed(&why).unwrap_or_else(|| Missing::NotRun(why.into_owned())))
    };
    let version = run(&["-vV"]).await?;
    let sysroot = run(&["--print", "sysroot"]).await?;
    let sysroot = Path::new(sysroot.trim());
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim);
    let lib = sysroot.join("lib/rustlib").join(target.as_str());
    let found = if host == Some(target.as_str()) {
        // The host's directory has the compiler's own files too: its libcore tells.
        std::fs::read_dir(lib.join("lib"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with("libcore-"))
    } else {
        lib.is_dir()
    };
    if found {
        Ok(())
    } else {
        Err(Missing::Target {
            target: target.clone(),
        })
    }
}

/// The toolchain that rustup says is not installed, in why a command failed:
/// `error: toolchain '1.80.0-x86_64-unknown-linux-gnu' is not installed` (rustup 1.29.1). One of
/// the few messages that are matched (requirement R-WS-7).
fn not_installed(why: &str) -> Option<Missing> {
    let (_, rest) = why.split_once("toolchain '")?;
    let (toolchain, rest) = rest.split_once('\'')?;
    rest.trim_start()
        .starts_with("is not installed")
        .then(|| Missing::NotInstalled {
            toolchain: toolchain.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes files named `names` in a new directory.
    fn programs(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for name in names {
            std::fs::write(dir.path().join(name), "").expect("written");
        }
        dir
    }

    /// A program's file name on this OS.
    fn exe(name: &str) -> String {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    }

    #[test]
    fn nothing_set_is_the_toolchain_of_path() {
        let found = Programs::find(None, None, None).expect("found");
        assert!(!found.are_set());
        assert_eq!(found.cargo(), Path::new("cargo"));
    }

    /// Those not set are found next to the one set, with its version suffix first (Q-31, decided
    /// in M2's D1).
    #[test]
    fn those_not_set_are_found_next_to_the_one_set() {
        let dir = programs(&[
            &exe("rustc-1.80"),
            &exe("rustdoc-1.80"),
            &exe("rustdoc"),
            &exe("cargo"),
        ]);
        let found =
            Programs::find(None, Some(dir.path().join(exe("rustc-1.80"))), None).expect("found");
        assert_eq!(found.rustdoc, Some(dir.path().join(exe("rustdoc-1.80"))));
        assert_eq!(
            found.cargo,
            Some(dir.path().join(exe("cargo"))),
            "without suffix"
        );
        // rustc is looked for next to rustdoc before cargo.
        let found = Programs::find(Some(dir.path().join(exe("cargo"))), None, None);
        assert!(
            matches!(
                found,
                Err(ProgramsError::NotFound {
                    kind: Kind::Rustc,
                    ..
                })
            ),
            "{found:?}"
        );
    }

    #[test]
    fn a_setting_that_names_no_file_is_an_error() {
        let dir = programs(&[]);
        let found = Programs::find(None, Some(dir.path().join(exe("rustc"))), None);
        assert!(
            matches!(
                found,
                Err(ProgramsError::Missing {
                    setting: "build.rustc_path",
                    ..
                })
            ),
            "{found:?}"
        );
    }

    #[test]
    fn cargo_runs_with_the_programs_set_and_without_rustup_toolchain() {
        let dir = programs(&[&exe("rustc"), &exe("rustdoc"), &exe("cargo")]);
        let found = Programs::find(None, Some(dir.path().join(exe("rustc"))), None).expect("found");
        let mut command = tokio::process::Command::new(found.cargo());
        found.prepare_cargo(&mut command);
        let envs: Vec<_> = command
            .as_std()
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect();
        assert!(envs.contains(&("RUSTUP_TOOLCHAIN".into(), None)));
        assert!(envs.contains(&("RUSTC".into(), Some(dir.path().join(exe("rustc")).into()))));
        assert!(envs.contains(&(
            "RUSTDOC".into(),
            Some(dir.path().join(exe("rustdoc")).into())
        )));
    }

    /// Without the settings, what cargo runs: `RUSTC` before the project's settings (decided in
    /// M2's D1).
    #[test]
    fn rustc_is_the_variable_or_else_the_projects_or_else_the_toolchains() {
        let root = Path::new("/project");
        let tools = PathBuf::from("/project/tools/rustc");
        let project = Some(tools.clone());
        assert_eq!(
            chosen(Some("my-rustc".into()), project.clone(), root, Kind::Rustc),
            PathBuf::from("my-rustc")
        );
        assert_eq!(
            chosen(Some("bin/rustc".into()), project.clone(), root, Kind::Rustc),
            PathBuf::from("/project/bin/rustc")
        );
        assert_eq!(chosen(None, project, root, Kind::Rustc), tools);
        assert_eq!(
            chosen(None, None, root, Kind::Rustc),
            PathBuf::from("rustc")
        );
    }

    #[test]
    fn rustups_message_of_a_missing_toolchain_is_read() {
        let why = "rustc -vV failed (exit status: 1): error: toolchain \
                   '1.80.0-x86_64-unknown-linux-gnu' is not installed";
        assert_eq!(
            not_installed(why),
            Some(Missing::NotInstalled {
                toolchain: "1.80.0-x86_64-unknown-linux-gnu".to_owned()
            })
        );
        assert_eq!(not_installed("rustc -vV failed: something else"), None);
    }

    /// The host's toolchain builds for the host.
    #[tokio::test]
    async fn the_toolchain_here_builds_for_its_host() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let host = crate::data::target::host_target(dir.path()).await;
        check_target(Path::new("rustc"), dir.path(), &host.triple)
            .await
            .expect("the host's standard library");
    }

    /// A project whose `rust-toolchain.toml` names a toolchain that is not installed is told so,
    /// without installing it (fixture (e), decided in M2's D1).
    #[tokio::test]
    async fn a_toolchain_that_is_not_installed_is_missing() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        // No release of this name will ever be installed here.
        std::fs::write(
            dir.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.0.1\"\n",
        )
        .expect("written");
        let target: TargetTriple = "x86_64-unknown-linux-gnu".parse().expect("a triple");
        let missing = check_target(Path::new("rustc"), dir.path(), &target).await;
        assert!(
            matches!(&missing, Err(Missing::NotInstalled { toolchain }) if toolchain.starts_with("1.0.1-")),
            "{missing:?}"
        );
    }

    /// A target whose standard library the toolchain lacks is missing (requirement R-TG-1).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_target_without_its_standard_library_is_missing() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a temporary directory");
        let sysroot = dir.path().join("sysroot");
        std::fs::create_dir_all(sysroot.join("lib/rustlib/x86_64-unknown-linux-gnu/lib"))
            .expect("made");
        std::fs::write(
            sysroot.join("lib/rustlib/x86_64-unknown-linux-gnu/lib/libcore-0123.rlib"),
            "",
        )
        .expect("written");
        let rustc = dir.path().join("rustc");
        std::fs::write(
            &rustc,
            format!(
                "#!/bin/sh\nif [ \"$1\" = -vV ]; then echo 'host: x86_64-unknown-linux-gnu'; \
                 else echo '{}'; fi\n",
                sysroot.display()
            ),
        )
        .expect("written");
        std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755))
            .expect("made runnable");
        let host: TargetTriple = "x86_64-unknown-linux-gnu".parse().expect("a triple");
        check_target(&rustc, dir.path(), &host)
            .await
            .expect("the host's libcore");
        let other: TargetTriple = "aarch64-apple-darwin".parse().expect("a triple");
        assert_eq!(
            check_target(&rustc, dir.path(), &other).await,
            Err(Missing::Target { target: other })
        );
    }

    /// rustc and rustdoc of different toolchains are told apart before a build (decided in M2's
    /// D1).
    #[cfg(unix)]
    #[tokio::test]
    async fn rustc_and_rustdoc_of_different_toolchains_are_told() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a temporary directory");
        let fake = |name: &str, commit: &str| {
            let path = dir.path().join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho '{name} 1.0.0'\necho 'commit-hash: {commit}'\n"),
            )
            .expect("written");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("made runnable");
            path
        };
        let (rustc, rustdoc, other) = (
            fake("rustc", "aaa"),
            fake("rustdoc", "aaa"),
            fake("rustdoc-2", "bbb"),
        );
        same_toolchain(&rustc, &rustdoc, dir.path())
            .await
            .expect("one toolchain");
        let mismatch = same_toolchain(&rustc, &other, dir.path())
            .await
            .expect_err("two toolchains");
        assert_eq!(mismatch.rustc, "rustc 1.0.0");
        assert_eq!(mismatch.rustdoc, "rustdoc-2 1.0.0");
    }
}
