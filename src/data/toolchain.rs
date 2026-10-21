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

use super::{
    cache::{self, Cache},
    cargo_config,
    dist::NightlyDate,
    format_version,
    rustdoc_json,
    target::TargetTriple,
    tool,
};
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

/// The date of the nightly that a project whose toolchain writes too old a format is built with,
/// `build.nightly_date`, or the latest nightly when it is not set (requirement R-WS-8).
static NIGHTLY_DATE: std::sync::OnceLock<Option<NightlyDate>> = std::sync::OnceLock::new();

impl Programs {
    /// Sets this process's programs from `settings`, once: a problem is logged, and the builds
    /// return it (requirement R-WS-1). `RUSTUP_TOOLCHAIN` in the server's environment, which the
    /// children run in a project go without, is logged too.
    pub fn set_this(settings: &Settings) {
        drop(
            NIGHTLY_DATE.set(settings.get("build.nightly_date", |value| {
                value
                    .text()
                    .and_then(|date| date.parse::<NightlyDate>().ok())
                    .ok_or("a date as 2026-07-10")
            })),
        );
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
    pub async fn used_in(&self, kind: Kind, root: &Path, cargo_home: Option<&Path>) -> PathBuf {
        let (set, variables) = match kind {
            Kind::Rustdoc => (&self.rustdoc, ["RUSTDOC", "CARGO_BUILD_RUSTDOC"]),
            Kind::Rustc | Kind::Cargo => (&self.rustc, ["RUSTC", "CARGO_BUILD_RUSTC"]),
        };
        if let Some(set) = set {
            return set.clone();
        }
        // The settings files, with what they include: read off the async threads.
        let (reading, home) = (root.to_path_buf(), cargo_home.map(Path::to_path_buf));
        let config =
            super::blocking(move || Ok(cargo_config::read(&reading, home.as_deref(), None).ok()))
                .await
                .ok()
                .flatten();
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

/// What [`check_target`] found of a toolchain that can build for the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// Its host, which tells whether a build passes `--target` (requirement R-WS-1), if rustc
    /// told it.
    pub host: Option<TargetTriple>,
    /// It, as the first line of `rustc -vV` gives it, for the provenance (requirement R-OUT-9),
    /// if rustc told it.
    pub toolchain: Option<String>,
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
/// `toolchain` is one that the server chose instead of the project's, as `nightly`, which rustc
/// is run with as `+<toolchain>`. Returns what rustc told of its toolchain.
///
/// # Errors
///
/// Returns what is missing.
pub async fn check_target(
    rustc: &Path,
    toolchain: Option<&str>,
    root: &Path,
    target: &TargetTriple,
) -> Result<Checked, Missing> {
    let plus = toolchain.map(|toolchain| format!("+{toolchain}"));
    let run = async |args: &[&str]| {
        let args: Vec<&str> = plus
            .as_deref()
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        tool::output_kept(root, tool::Place::Project, rustc, &args)
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
        Ok(Checked {
            host: host.and_then(|host| host.parse().ok()),
            toolchain: super::local_std::parse_rustc_version(&version)
                .map(|version| version.summary()),
        })
    } else {
        Err(Missing::Target {
            target: target.clone(),
        })
    }
}

/// The nightly that a project whose toolchain writes too old a format is built with: `nightly`, or
/// `nightly-<date>` with `build.nightly_date` (requirement R-WS-8).
fn nightly(date: Option<&NightlyDate>) -> String {
    date.map_or_else(|| "nightly".to_owned(), |date| format!("nightly-{date}"))
}

/// The directory of what the probes of toolchains found, in the cache, for the format of its
/// records (decided in M2's C1).
pub const PROBES_DIR: &str = "toolchains/v1";

/// What the probe of a toolchain found, kept in the cache by the commit of its rustdoc.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Probed {
    /// The format of the rustdoc JSON that it writes.
    format: u32,
    /// The commit of its rustdoc, as `-vV` tells it.
    commit: String,
    /// Its rustdoc.
    rustdoc: PathBuf,
    /// rustup's toolchain that it ran with (`+<toolchain>`), if one.
    toolchain: Option<String>,
}

/// Why the format of a toolchain is not known.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    /// rustup has not installed it.
    #[error(transparent)]
    Missing(Missing),
    /// Its rustdoc did not run, or wrote no JSON that tells its format.
    #[error("{0}")]
    Failed(String),
}

/// The format of the rustdoc JSON that `rustdoc` writes, run in the project at `root` (rustup's
/// `+toolchain` when `toolchain` is given), as cargo would run it.
///
/// It is read from the cache, by the commit that `rustdoc -vV` tells, or else from a run of it on
/// a crate of one line in `work`, and then kept there (decided in M2's D1): a run takes from a
/// tenth of a second (stable) to most of a second (nightly). A rustdoc that tells no commit is
/// run each time.
///
/// # Errors
///
/// Returns an error when the toolchain is not installed, or its rustdoc does not tell its format.
pub async fn format_of(
    rustdoc: &Path,
    toolchain: Option<&str>,
    root: &Path,
    cache: &Cache,
    work: &Path,
) -> Result<u32, ProbeError> {
    let plus = toolchain.map(|toolchain| format!("+{toolchain}"));
    let failed = |why: std::borrow::Cow<'static, str>| {
        not_installed(&why)
            .map_or_else(|| ProbeError::Failed(why.into_owned()), ProbeError::Missing)
    };
    let version_args: Vec<&str> = plus.as_deref().into_iter().chain(["-vV"]).collect();
    let version = tool::output_kept(root, tool::Place::Project, rustdoc, &version_args)
        .await
        .map_err(failed)?;
    let commit = version
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .map(str::trim)
        .filter(|commit| !commit.is_empty() && *commit != "unknown");
    let record = commit.map(|commit| {
        let key = format!(
            "{commit}\n{}\n{}",
            rustdoc.display(),
            toolchain.unwrap_or_default()
        );
        let digest = cache::sha256_hex(key.as_bytes());
        cache
            .dir()
            .join(PROBES_DIR)
            .join(format!("{}.json", digest.get(..32).unwrap_or(&digest)))
    });
    // Files of the cache, read and written off the async threads, as below.
    let reading = record.clone();
    let kept = super::blocking(move || {
        Ok(reading
            .and_then(|record| std::fs::read(record).ok())
            .and_then(|json| serde_json::from_slice::<Probed>(&json).ok()))
    })
    .await
    .ok()
    .flatten();
    if let Some(kept) = kept {
        return Ok(kept.format);
    }
    let format = probe(rustdoc, plus.as_deref(), root, work)
        .await
        .map_err(failed)?;
    if let (Some(record), Some(commit)) = (record, commit) {
        let probed = Probed {
            format,
            commit: commit.to_owned(),
            rustdoc: rustdoc.to_path_buf(),
            toolchain: toolchain.map(str::to_owned),
        };
        let written = super::blocking(move || {
            std::fs::create_dir_all(record.parent().unwrap_or(&record))?;
            let json = serde_json::to_vec(&probed).map_err(std::io::Error::other)?;
            cache::write_atomically(&record, &json)
        })
        .await;
        if let Err(error) = written {
            tracing::warn!(%error, "what the probe of a toolchain found could not be kept");
        }
    }
    Ok(format)
}

/// Runs `rustdoc` on a crate of one line in a new directory in `work`, and reads the format of the
/// JSON that it writes. `plus` is rustup's `+toolchain` argument, if any.
async fn probe(
    rustdoc: &Path,
    plus: Option<&str>,
    root: &Path,
    work: &Path,
) -> Result<u32, std::borrow::Cow<'static, str>> {
    static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let number = PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = work.join(format!("probe-{}-{number}", std::process::id()));
    let making = dir.clone();
    let made = super::blocking(move || {
        std::fs::create_dir_all(&making)?;
        std::fs::write(making.join("probe.rs"), "//! A probe.\n")
    })
    .await;
    if let Err(error) = made {
        return Err(format!("the probe could not be written: {error}").into());
    }
    let (source, out) = (dir.join("probe.rs"), dir.join("out"));
    let (source, out) = (source.to_string_lossy(), out.to_string_lossy());
    let args: Vec<&str> = plus
        .into_iter()
        // No `--edition`: rustdoc before 1.56 refuses 2021, and a line of docs needs none.
        .chain([
            "--crate-type",
            "lib",
            "--crate-name",
            "probe",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
            "-o",
            &out,
            &source,
        ])
        .collect();
    // A stable rustdoc takes `-Z` for the crate that `RUSTC_BOOTSTRAP` names (requirement R-WS-8).
    let ran = tool::output_with(root, tool::Place::Project, rustdoc, &args, &[(
        "RUSTC_BOOTSTRAP",
        "probe",
    )])
    .await;
    let json = super::blocking(move || {
        let json = std::fs::read(dir.join("out/probe.json"));
        drop(std::fs::remove_dir_all(&dir));
        Ok(json)
    })
    .await
    .unwrap_or_else(Err);
    ran?;
    let json = json.map_err(|error| format!("the probe wrote no JSON: {error}"))?;
    format_version::peek(&json)
        .ok_or_else(|| "the JSON of the probe tells no format_version".into())
}

/// What builds the documentation of a project (requirement R-WS-8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen {
    /// Its own toolchain, which writes `format`: one that this server reads, or a newer one,
    /// which is read if it can be, with a warning (decided in M2's D1).
    Project {
        /// The format.
        format: u32,
    },
    /// rustup's `toolchain`, a nightly, as the project's writes too old a format.
    Nightly {
        /// The toolchain, `nightly` or `nightly-<date>`.
        toolchain: String,
        /// The format that it writes.
        format: u32,
        /// The format that the project's toolchain writes, or `None` when its rustdoc could not
        /// write rustdoc JSON at all, as one older than the formats is.
        project_format: Option<u32>,
    },
}

/// Why no toolchain builds the documentation of a project.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChooseError {
    /// rustup has not installed the project's toolchain. Online, cargo installs it in the build;
    /// offline, it is `toolchain_missing` (decided in M2's D1).
    #[error(transparent)]
    NotInstalled(Missing),
    /// The project's toolchain writes too old a format, and the nightly to build with is not
    /// installed (`toolchain_missing`).
    #[error(
        "{missing}, and the project's toolchain writes no rustdoc JSON format that this server reads"
    )]
    NightlyMissing {
        /// What is not installed.
        missing: Missing,
        /// The format of the project's toolchain, or `None` when it writes none.
        found: Option<u32>,
    },
    /// The project's toolchain writes too old a format, and none can be switched to: the
    /// programs are set, or not rustup's (`unsupported_format`).
    #[error("the toolchain writes the rustdoc JSON format {found}, older than {}, the oldest that this server reads", rustdoc_json::SUPPORTED.start())]
    TooOld {
        /// The format.
        found: u32,
    },
    /// A toolchain's rustdoc did not tell its format.
    #[error("{0}")]
    Failed(String),
}

/// What builds the documentation of the project at `root`, with `work` for the probes (decided in
/// M2's D1).
///
/// It is the project's toolchain, as cargo runs it there, unless that writes too old a format:
/// then rustup's nightly (`build.nightly_date`), when rustc and rustdoc are rustup's (`rustc` and
/// `rustdoc` in `PATH`, not ones that a setting, a variable or the project names) and rustup is
/// here.
///
/// # Errors
///
/// Returns an error when no toolchain can build it.
pub async fn choose(
    root: &Path,
    cache: &Cache,
    cargo_home: Option<&Path>,
    work: &Path,
) -> Result<Chosen, ChooseError> {
    let rustdoc = Programs::this()
        .used_in(Kind::Rustdoc, root, cargo_home)
        .await;
    let rustc = Programs::this()
        .used_in(Kind::Rustc, root, cargo_home)
        .await;
    // With rustc or rustdoc of a path, `+nightly` would mix toolchains.
    let switchable = rustdoc == Path::new(Kind::Rustdoc.name())
        && rustc == Path::new(Kind::Rustc.name())
        && tool::output_kept(cache.dir(), tool::Place::Neutral, Path::new("rustup"), &[
            "--version",
        ])
        .await
        .is_ok();
    let nightly = nightly(NIGHTLY_DATE.get().and_then(Option::as_ref));
    choose_with(
        &rustdoc,
        switchable.then_some(nightly.as_str()),
        root,
        cache,
        work,
    )
    .await
}

/// [`choose`] with `rustdoc`, and the nightly to switch to, if it may.
async fn choose_with(
    rustdoc: &Path,
    nightly: Option<&str>,
    root: &Path,
    cache: &Cache,
    work: &Path,
) -> Result<Chosen, ChooseError> {
    let format = match format_of(rustdoc, None, root, cache, work).await {
        Ok(format) if format >= *rustdoc_json::SUPPORTED.start() => {
            return Ok(Chosen::Project { format });
        },
        Ok(format) => Some(format),
        Err(ProbeError::Missing(missing)) => return Err(ChooseError::NotInstalled(missing)),
        // A rustdoc that cannot write rustdoc JSON at all is older still: the nightly builds, if
        // one may be switched to.
        Err(ProbeError::Failed(why)) if nightly.is_some() => {
            tracing::info!(%why, "the project's rustdoc wrote no rustdoc JSON, so the nightly is tried");
            None
        },
        Err(ProbeError::Failed(why)) => return Err(ChooseError::Failed(why)),
    };
    // Without a nightly, the format is known: a failed probe has returned.
    let Some(nightly) = nightly else {
        return Err(ChooseError::TooOld {
            found: format.unwrap_or_default(),
        });
    };
    let nightly_format = format_of(rustdoc, Some(nightly), root, cache, work)
        .await
        .map_err(|error| match error {
            ProbeError::Missing(missing) => ChooseError::NightlyMissing {
                missing,
                found: format,
            },
            ProbeError::Failed(why) => ChooseError::Failed(why),
        })?;
    if nightly_format < *rustdoc_json::SUPPORTED.start() {
        return Err(ChooseError::TooOld {
            found: nightly_format,
        });
    }
    Ok(Chosen::Nightly {
        toolchain: nightly.to_owned(),
        format: nightly_format,
        project_format: format,
    })
}

/// Forgets what was kept of the toolchain of the project at `root`, as rustup's saying that it is
/// not installed, once cargo has installed it in a build (decided in M2's D2).
pub fn forget_project(root: &Path) {
    tool::forget_project(root);
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
        let told = check_target(Path::new("rustc"), None, dir.path(), &host.triple)
            .await
            .expect("the host's standard library");
        assert_eq!(told.host, Some(host.triple));
        assert!(
            told.toolchain
                .as_deref()
                .is_some_and(|toolchain| toolchain.starts_with("rustc 1.")),
            "{told:?}"
        );
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
        let missing = check_target(Path::new("rustc"), None, dir.path(), &target).await;
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
        let told = check_target(&rustc, None, dir.path(), &host)
            .await
            .expect("the host's libcore");
        assert_eq!(told.host.as_ref(), Some(&host));
        let other: TargetTriple = "aarch64-apple-darwin".parse().expect("a triple");
        assert_eq!(
            check_target(&rustc, None, dir.path(), &other).await,
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

    #[test]
    fn the_nightly_is_the_latest_or_that_of_the_date_set() {
        assert_eq!(nightly(None), "nightly");
        let date: NightlyDate = "2026-06-11".parse().expect("a date");
        assert_eq!(nightly(Some(&date)), "nightly-2026-06-11");
    }

    /// The rustdoc here writes a format, which is kept by its commit (decided in M2's D1).
    #[tokio::test]
    async fn the_format_of_the_rustdoc_here_is_probed_and_kept() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let format = format_of(Path::new("rustdoc"), None, dir.path(), &cache, dir.path())
            .await
            .expect("probed");
        assert!(format >= *rustdoc_json::SUPPORTED.start(), "{format}");
        let kept: Vec<_> = std::fs::read_dir(dir.path().join(PROBES_DIR))
            .expect("kept")
            .collect();
        assert_eq!(kept.len(), 1);
    }

    /// A rustdoc that writes `format`, and 61 with rustup's `+<toolchain>`, unless `nightly` says
    /// that the nightly is not installed. It notes each probe in `probes`.
    #[cfg(unix)]
    fn fake_rustdoc(dir: &Path, format: u32, nightly: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = dir.join("rustdoc");
        let missing = if nightly {
            ""
        } else {
            "echo \"error: toolchain 'nightly-x86_64-unknown-linux-gnu' is not installed\" >&2; exit 1"
        };
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n\
                 plus=''\n\
                 case \"$1\" in +*) plus=\"$1\"; shift; {missing};; esac\n\
                 if [ \"$1\" = -vV ]; then echo 'rustdoc 1.0.0'; echo \"commit-hash: abc$plus\"; exit 0; fi\n\
                 echo probe >> '{probes}'\n\
                 if [ -z \"$plus\" ] && [ {format} = 0 ]; then echo 'error: Unrecognized option' >&2; exit 1; fi\n\
                 out=''\n\
                 while [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then out=\"$2\"; fi; shift; done\n\
                 mkdir -p \"$out\"\n\
                 if [ -n \"$plus\" ]; then f=61; else f={format}; fi\n\
                 echo \"{{\\\"format_version\\\":$f}}\" > \"$out/probe.json\"\n",
                probes = dir.join("probes").display(),
            ),
        )
        .expect("written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("made runnable");
        path
    }

    /// How many times the fake rustdoc in `dir` ran a probe.
    #[cfg(unix)]
    fn probes(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("probes")).map_or(0, |probes| probes.lines().count())
    }

    /// A toolchain that writes a format that this server reads, or a newer one, builds the
    /// documentation; its format is probed once (decided in M2's D1).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_projects_toolchain_builds_unless_its_format_is_too_old() {
        for format in [61, 62] {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let (cache, rustdoc) = (
                Cache::at(dir.path()),
                fake_rustdoc(dir.path(), format, true),
            );
            for _ in 0..2 {
                let chosen =
                    choose_with(&rustdoc, Some("nightly"), dir.path(), &cache, dir.path()).await;
                assert_eq!(chosen, Ok(Chosen::Project { format }));
            }
            assert_eq!(probes(dir.path()), 1, "kept by its commit");
        }
    }

    /// Too old a format is built with the nightly, if one may be switched to: else it is too old,
    /// and a nightly that is not installed is missing (requirement R-WS-8).
    #[cfg(unix)]
    #[tokio::test]
    async fn too_old_a_format_is_built_with_the_nightly() {
        // One too old to write rustdoc JSON at all, as before 1.48.
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (cache, rustdoc) = (Cache::at(dir.path()), fake_rustdoc(dir.path(), 0, true));
        assert_eq!(
            choose_with(&rustdoc, Some("nightly"), dir.path(), &cache, dir.path()).await,
            Ok(Chosen::Nightly {
                toolchain: "nightly".to_owned(),
                format: 61,
                project_format: None,
            })
        );
        assert!(
            matches!(
                choose_with(&rustdoc, None, dir.path(), &cache, dir.path()).await,
                Err(ChooseError::Failed(_))
            ),
            "without a nightly, the failure is told"
        );
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (cache, rustdoc) = (Cache::at(dir.path()), fake_rustdoc(dir.path(), 40, true));
        assert_eq!(
            choose_with(&rustdoc, Some("nightly"), dir.path(), &cache, dir.path()).await,
            Ok(Chosen::Nightly {
                toolchain: "nightly".to_owned(),
                format: 61,
                project_format: Some(40),
            })
        );
        assert_eq!(
            choose_with(&rustdoc, None, dir.path(), &cache, dir.path()).await,
            Err(ChooseError::TooOld { found: 40 })
        );
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (cache, rustdoc) = (Cache::at(dir.path()), fake_rustdoc(dir.path(), 40, false));
        assert!(
            matches!(
                choose_with(&rustdoc, Some("nightly"), dir.path(), &cache, dir.path()).await,
                Err(ChooseError::NightlyMissing {
                    missing: Missing::NotInstalled { .. },
                    found: Some(40),
                })
            ),
            "the nightly is not installed"
        );
    }
}
