//! The plan of a local build of a workspace member's documentation: the command, its target
//! directory and where the JSON lands (requirements R-WS-1, R-WS-2 and R-WS-8, decided in M2's
//! D2).
//!
//! [`build`] runs a [`Plan`]'s command and publishes the JSON in the cache.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use semver::Version;
use serde::{Deserialize, Serialize};

use super::{
    cache::{self, Cache},
    cargo::{Failure, RunError, classify, run_building, stderr_tail},
    flight::Doing,
    metadata::{Metadata, Package, PackageId, Target},
    process_lock::{self, LockMode, ProcessLock},
    target::TargetTriple,
};
use crate::config::Settings;

/// The longest value of `RUSTC_BOOTSTRAP` that names crates, in bytes: below the 32,767
/// characters of a variable on Windows, on every OS (decided in M2's D2, Q-31).
const MAX_BOOTSTRAP_BYTES: usize = 30_000;

/// The directory of the target directories, in the cache (decided in M2's D2).
pub const TARGET_DIRS: &str = "t";

/// The file that marks a target directory that is not to be used, as what a stopped server's
/// build left running may still hold cargo's lock there (decided in M2's D2).
pub const UNUSABLE: &str = "unusable";

/// How many hex digits of the hash of a workspace's root name its target directory: short, for
/// the paths that cargo makes below it on Windows (requirement R-WS-2).
const HASH_DIGITS: usize = 12;

/// The kinds of a target that make it the package's lib, which `--lib` builds.
const LIB_KINDS: &[&str] = &["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];

/// The jobs of a build, as the setting `build.jobs` gives them.
static JOBS: OnceLock<NonZeroUsize> = OnceLock::new();

/// Whether the documented crate's rustdoc gets `--cfg docsrs`, as the setting `build.cfg_docsrs`
/// says (requirement R-WS-1).
static CFG_DOCSRS: OnceLock<bool> = OnceLock::new();

/// Whether a build passes `--cfg docsrs` first: `build.cfg_docsrs`, which is on unless set off
/// (requirement R-WS-1).
#[must_use]
pub fn cfg_docsrs() -> bool {
    CFG_DOCSRS.get().copied().unwrap_or(true)
}

/// The builds that run at once in this process, unless `limits.concurrent_builds` sets it
/// (requirement R-SEC-7, initial value).
const DEFAULT_CONCURRENT_BUILDS: usize = 2;

/// The build slots of this process, as `limits.concurrent_builds` sets them.
static SLOTS: OnceLock<Slots> = OnceLock::new();

/// Build slots: the builds that run at once (requirement R-SEC-7).
///
/// One is held while cargo builds, and let go between the tries of a build, so that one member's
/// tries do not keep the others waiting (decided in M2's D3). `cargo metadata`, `cargo tree` and
/// the probes take none.
#[derive(Debug)]
pub struct Slots(tokio::sync::Semaphore);

impl Slots {
    /// `count` slots, at least one.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self(tokio::sync::Semaphore::new(
            count.clamp(1, tokio::sync::Semaphore::MAX_PERMITS),
        ))
    }

    /// Takes a slot, telling `on_doing` while it waits for one and when it has it.
    pub async fn take(
        &self,
        on_doing: &mut (impl FnMut(Doing) + Send),
    ) -> Option<tokio::sync::SemaphorePermit<'_>> {
        if let Ok(slot) = self.0.try_acquire() {
            return Some(slot);
        }
        on_doing(Doing::WaitingForSlot);
        // Never closed.
        let slot = self.0.acquire().await.ok();
        on_doing(Doing::Building);
        slot
    }
}

/// This process's build slots.
fn slots() -> &'static Slots {
    SLOTS.get_or_init(|| Slots::new(DEFAULT_CONCURRENT_BUILDS))
}

/// Reads the settings of the builds, once, at startup.
pub fn set_this(settings: &Settings) {
    let jobs = settings.get("build.jobs", |value| {
        value
            .whole_number()
            .and_then(|jobs| usize::try_from(jobs).ok())
            .and_then(NonZeroUsize::new)
            .ok_or("a whole number from 1")
    });
    let slots = settings.get("limits.concurrent_builds", |value| {
        value
            .whole_number()
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count > 0)
            .ok_or("a whole number from 1")
    });
    let run_time = settings.get("build.timeout_secs", |value| {
        value
            .whole_number()
            .filter(|secs| *secs > 0)
            .ok_or("a whole number of seconds from 1")
    });
    if let Some(secs) = run_time {
        super::cargo::set_run_time(Duration::from_secs(secs));
    }
    let docsrs = settings.get("build.cfg_docsrs", |value| {
        value.boolean().ok_or("true or false")
    });
    let docsrs_set = docsrs.is_none_or(|docsrs| CFG_DOCSRS.set(docsrs).is_ok());
    let jobs_set = jobs.is_none_or(|jobs| JOBS.set(jobs).is_ok());
    let slots_set = slots.is_none_or(|count| SLOTS.set(Slots::new(count)).is_ok());
    if !(docsrs_set && jobs_set && slots_set) {
        tracing::debug!("the settings of the builds were read before");
    }
    if let Some(value) = std::env::var_os("RUSTC_BOOTSTRAP") {
        tracing::info!(
            value = %value.to_string_lossy(),
            "RUSTC_BOOTSTRAP is replaced in the builds of documentation by the names of the project's crates"
        );
    }
}

/// The jobs of a build (`-j`): `build.jobs`, or half the CPUs (requirement R-WS-1, decided in
/// M2's D2 until F5 measures it).
#[must_use]
pub fn jobs() -> NonZeroUsize {
    JOBS.get().copied().unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .checked_div(2)
            .and_then(NonZeroUsize::new)
            .unwrap_or(NonZeroUsize::MIN)
    })
}

/// The value of `RUSTC_BOOTSTRAP` of the builds of a workspace (requirement R-WS-8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bootstrap {
    /// The names of the crates, sorted and joined by commas.
    Crates(String),
    /// `1`, as the names are too long for a variable (decided in M2's D2).
    All,
}

impl Bootstrap {
    /// The value for the workspace of `metadata`: the names of the libs and proc-macros of every
    /// package of the resolve and of the members' bins. It is the same for every build in its
    /// target directory, so that what reads it is not built again (requirement R-WS-8).
    #[must_use]
    pub fn of(metadata: &Metadata) -> Self {
        let members: HashSet<&PackageId> = metadata.workspace_members.iter().collect();
        let names: BTreeSet<String> = metadata
            .packages
            .iter()
            .flat_map(|package| {
                let member = members.contains(&package.id);
                package
                    .targets
                    .iter()
                    .filter(move |target| {
                        target.kind.iter().any(|kind| {
                            LIB_KINDS.contains(&&**kind) || (member && &**kind == "bin")
                        })
                    })
                    .map(|target| crate_name(&target.name))
            })
            .collect();
        let value = names.into_iter().collect::<Vec<_>>().join(",");
        if value.len() > MAX_BOOTSTRAP_BYTES {
            Self::All
        } else {
            Self::Crates(value)
        }
    }

    /// The value.
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::Crates(names) => names,
            Self::All => "1",
        }
    }
}

/// The first digits of the hash of a workspace's root, which name its target directory and its
/// published builds.
fn workspace_hash(root: &Path) -> String {
    let mut hash = cache::sha256_hex(root.as_os_str().as_encoded_bytes());
    hash.truncate(HASH_DIGITS);
    hash
}

/// The name of the crate of a target: `cargo metadata` keeps the `-` of a bin's name.
fn crate_name(target: &str) -> String {
    target.replace('-', "_")
}

/// The target directory of a workspace's builds (requirement R-WS-2, decided in M2's D2).
///
/// It is `t/<hash>` in the cache, or `t/<hash>.<n>` after one that is not to be used. Its `target`
/// and `build` are cargo's target directory and build directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetDir(PathBuf);

impl TargetDir {
    /// That of the workspace at `root`, absolute and with symlinks resolved, in the cache at
    /// `cache`: the first that is not marked [`UNUSABLE`].
    #[must_use]
    pub fn of(cache: &Path, root: &Path) -> Self {
        let stem = workspace_hash(root);
        let stem = stem.as_str();
        let dirs = cache.join(TARGET_DIRS);
        let first = dirs.join(stem);
        let dir = (0..=u32::MAX)
            .map(|number| {
                if number == 0 {
                    first.clone()
                } else {
                    dirs.join(format!("{stem}.{number}"))
                }
            })
            .find(|dir| !dir.join(UNUSABLE).exists())
            .unwrap_or(first);
        Self(dir)
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Its name in `t/`, which names its locks.
    fn name(&self) -> String {
        self.0
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// cargo's target directory (`CARGO_TARGET_DIR`).
    #[must_use]
    pub fn target(&self) -> PathBuf {
        self.0.join("target")
    }

    /// cargo's build directory (`CARGO_BUILD_BUILD_DIR`), which a project's `build.build-dir`
    /// would otherwise move (requirement R-WS-2).
    #[must_use]
    pub fn build(&self) -> PathBuf {
        self.0.join("build")
    }

    /// The directory that the JSON of a crate lands in: below the target's with `--target`, else
    /// the host's. A proc-macro is built for the host even with `--target` (requirement R-WS-1).
    #[must_use]
    pub fn doc_dir(&self, target: Option<&TargetTriple>, proc_macro: bool) -> PathBuf {
        match target {
            Some(target) if !proc_macro => self.target().join(target.as_str()).join("doc"),
            _ => self.target().join("doc"),
        }
    }
}

/// The crate of a member that a build documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Documented {
    /// The lib (`--lib`).
    Lib {
        /// Whether it is a proc-macro, which is built for the host.
        proc_macro: bool,
    },
    /// A bin (`--bin <name>`).
    Bin {
        /// The name of the target, as `cargo metadata` gives it.
        name: Box<str>,
    },
}

/// The member whose crate is named `name`, with `_` for `-`, and that crate (decided in M2's D2).
///
/// That is a member's lib, or else a member's bin, or else the lib of the member whose package has
/// the name. A bin of the same name as a lib cannot be reached.
#[must_use]
pub fn member_crate<'a>(metadata: &'a Metadata, name: &str) -> Option<(&'a Package, Documented)> {
    let members: HashSet<&PackageId> = metadata.workspace_members.iter().collect();
    let member_packages = || {
        metadata
            .packages
            .iter()
            .filter(|package| members.contains(&package.id))
    };
    let named = |target: &&Target| crate_name(&target.name) == name;
    let lib = member_packages().find_map(|package| {
        package
            .targets
            .iter()
            .filter(named)
            .find_map(|target| {
                target
                    .kind
                    .iter()
                    .any(|kind| LIB_KINDS.contains(&&**kind))
                    .then(|| Documented::Lib {
                        proc_macro: target.kind.iter().any(|kind| &**kind == "proc-macro"),
                    })
            })
            .map(|documented| (package, documented))
    });
    let bin = || {
        member_packages().find_map(|package| {
            package
                .targets
                .iter()
                .filter(named)
                .find(|target| target.kind.iter().any(|kind| &**kind == "bin"))
                .map(|target| {
                    (package, Documented::Bin {
                        name: target.name.clone(),
                    })
                })
        })
    };
    // The lib of a member asked for by its package's name, as `[lib] name` may differ.
    let by_package = || {
        member_packages()
            .filter(|package| crate_name(&package.name) == name)
            .find_map(|package| {
                let lib = package
                    .targets
                    .iter()
                    .find(|target| target.kind.iter().any(|kind| LIB_KINDS.contains(&&**kind)))?;
                Some((package, Documented::Lib {
                    proc_macro: lib.kind.iter().any(|kind| &**kind == "proc-macro"),
                }))
            })
    };
    lib.or_else(bin).or_else(by_package)
}

/// The crate of the member `package` that a lookup of `name`, with `_` for `-`, is in: its lib
/// or bin of that name, else its lib, else its bin if it has one alone (decided in M2's D2).
#[must_use]
pub fn crate_of(package: &Package, name: &str) -> Option<Documented> {
    let is_lib = |target: &&Target| target.kind.iter().any(|kind| LIB_KINDS.contains(&&**kind));
    let is_bin = |target: &&Target| target.kind.iter().any(|kind| &**kind == "bin");
    let lib = |target: &Target| Documented::Lib {
        proc_macro: target.kind.iter().any(|kind| &**kind == "proc-macro"),
    };
    let bin = |target: &Target| Documented::Bin {
        name: target.name.clone(),
    };
    let named = |target: &&Target| crate_name(&target.name) == name;
    if let Some(target) = package.targets.iter().filter(named).find(is_lib) {
        return Some(lib(target));
    }
    if let Some(target) = package.targets.iter().filter(named).find(is_bin) {
        return Some(bin(target));
    }
    if let Some(target) = package.targets.iter().find(is_lib) {
        return Some(lib(target));
    }
    let mut bins = package.targets.iter().filter(is_bin);
    match (bins.next(), bins.next()) {
        (Some(target), None) => Some(bin(target)),
        _ => None,
    }
}

/// The member of `metadata` that is the package `name` at `version`.
#[must_use]
pub fn member_package<'a>(
    metadata: &'a Metadata,
    name: &str,
    version: &Version,
) -> Option<&'a Package> {
    metadata.packages.iter().find(|package| {
        &*package.name == name
            && package.version == *version
            && metadata.workspace_members.contains(&package.id)
    })
}

/// The name of the crate that `documented` of `package` is, with `_` for `-`.
#[must_use]
pub fn documented_name(package: &Package, documented: &Documented) -> String {
    match documented {
        Documented::Bin { name } => crate_name(name),
        Documented::Lib { .. } => package
            .targets
            .iter()
            .find(|target| target.kind.iter().any(|kind| LIB_KINDS.contains(&&**kind)))
            .map_or_else(|| crate_name(&package.name), |lib| crate_name(&lib.name)),
    }
}

/// Whether a build for `target` passes `--target` (requirement R-WS-1).
///
/// It does when the project sets `build.target`, and when `target` is not the host of the
/// toolchain, or the host is not known. Without it, cargo builds for the host, and `RUSTFLAGS`
/// apply to build scripts and proc-macros as in the user's own builds.
#[must_use]
pub fn passes_target(
    target: &TargetTriple,
    host: Option<&TargetTriple>,
    project_sets: bool,
) -> bool {
    project_sets || host != Some(target)
}

/// A build of the documentation of a member's crate.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The workspace's root, where cargo runs.
    pub root: PathBuf,
    /// Whether cargo runs offline.
    pub offline: bool,
    /// The toolchain that the server chose instead of the project's, as `nightly`.
    pub toolchain: Option<String>,
    /// The value of `RUSTC_BOOTSTRAP`.
    pub bootstrap: Bootstrap,
    /// The target directory.
    pub target_dir: TargetDir,
    /// The member.
    pub package: PackageId,
    /// Its crate.
    pub documented: Documented,
    /// The name of the crate, with `_` for `-`: the name of its JSON.
    pub crate_name: Box<str>,
    /// The member's version, which the JSON must have.
    pub version: Version,
    /// The target passed as `--target`, if any.
    pub target: Option<TargetTriple>,
    /// Whether the documented crate's rustdoc gets `--cfg docsrs` (requirement R-WS-1).
    pub cfg_docsrs: bool,
}

impl Plan {
    /// The command (requirement R-WS-1): `cargo [+<toolchain>] --locked [--offline] rustdoc -j
    /// <jobs> -p <package id> (--lib | --bin <name>) [--target <target>] --message-format=json --
    /// -Z unstable-options --output-format json [--cfg docsrs] --document-private-items`, with the
    /// value of `RUSTC_BOOTSTRAP` in place of the user's, and the target directory.
    #[must_use]
    pub fn command(&self) -> tokio::process::Command {
        let mut command =
            super::cargo::project_command(&self.root, self.offline, self.toolchain.as_deref());
        command
            .arg("rustdoc")
            .arg("-j")
            .arg(jobs().to_string())
            .arg("-p")
            .arg(&*self.package.spec());
        match &self.documented {
            Documented::Lib { .. } => command.arg("--lib"),
            Documented::Bin { name } => command.arg("--bin").arg(&**name),
        };
        if let Some(target) = &self.target {
            command.arg("--target").arg(target.as_str());
        }
        command.args([
            "--message-format=json",
            "--",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
        ]);
        // Only the documented crate's rustdoc has it (requirement R-WS-1).
        if self.cfg_docsrs {
            command.args(["--cfg", "docsrs"]);
        }
        command
            // A member's private items too (requirement R-WS-3).
            .arg("--document-private-items")
            .env("RUSTC_BOOTSTRAP", self.bootstrap.value())
            .env("CARGO_TARGET_DIR", self.target_dir.target())
            .env("CARGO_BUILD_BUILD_DIR", self.target_dir.build());
        command
    }

    /// The JSON that the build writes, `<doc dir>/<crate>.json`, where another build of a crate
    /// of the same name writes too (Q-29).
    #[must_use]
    pub fn json(&self) -> PathBuf {
        self.target_dir
            .doc_dir(self.target.as_ref(), self.is_proc_macro())
            .join(format!("{}.json", self.crate_name))
    }

    /// Whether the crate is a proc-macro, which is built for the host.
    const fn is_proc_macro(&self) -> bool {
        matches!(self.documented, Documented::Lib { proc_macro: true })
    }

    /// The directory that the build is published in, until D4 (decided in M2's D2): `local/<the
    /// workspace's hash>/<the package id's hash>/<target>/<lib, or bin-<name>>` in the cache.
    fn published_dir(&self, cache: &Path, target: Option<&TargetTriple>) -> PathBuf {
        let package = cache::sha256_hex(self.package.0.as_bytes());
        let package = package.get(..HASH_DIGITS).unwrap_or(&package);
        let target = target.map_or("_unknown", TargetTriple::as_str);
        let documented = match &self.documented {
            Documented::Lib { .. } => "lib".to_owned(),
            Documented::Bin { name } => format!("bin-{name}"),
        };
        cache
            .join(LOCAL_DIR)
            .join(workspace_hash(&self.root))
            .join(package)
            .join(target)
            .join(documented)
    }
}

/// The directory of the published builds, in the cache, until D4 (decided in M2's D2).
pub const LOCAL_DIR: &str = "local";

/// The record of a published build, next to its JSON.
const RECORD: &str = "record.json";

/// The most errors of the compiler that a failed build keeps.
const MAX_ERRORS: usize = 5;

/// Numbers the JSON files moved into the work directory.
static NEXT_WORK: AtomicU64 = AtomicU64::new(0);

/// What a build published.
#[derive(Debug, Clone)]
pub struct Built {
    /// The JSON in the cache, next to its `.sha256` and its record (`record.json`).
    pub json: PathBuf,
    /// The record.
    pub record: BuildRecord,
    /// The units that cargo built, rather than found fresh: the documented crate's alone, once
    /// the others are built (requirement R-WS-8).
    pub rebuilt: Vec<Box<str>>,
}

/// What is recorded of a build, `record.json` next to its JSON, for the provenance (requirement
/// R-OUT-9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildRecord {
    /// The package id of the member.
    pub package: Box<str>,
    /// The crate, with `_` for `-`.
    pub crate_name: Box<str>,
    /// Its version.
    pub crate_version: Box<str>,
    /// The format of the JSON.
    pub format_version: u32,
    /// The target that the JSON is of (`target.triple`), if it tells: the host's for a
    /// proc-macro.
    pub target: Option<Box<str>>,
    /// The toolchain, as `rustc -vV` tells it, if known.
    pub toolchain: Option<Box<str>>,
    /// The toolchain that the server chose instead of the project's, as `nightly`.
    pub chosen_toolchain: Option<Box<str>>,
    /// When it was built, in RFC 3339.
    pub built_at: Box<str>,
    /// The features of the crate, as cargo built it (`features_effective`, requirement R-FT-3),
    /// or `None` when no message of cargo told them.
    pub features_effective: Option<Vec<Box<str>>>,
    /// Whether `RUSTC_BOOTSTRAP` was `1`, as the names were too long (decided in M2's D2).
    pub bootstrap_all: bool,
    /// Whether the documented crate's rustdoc had `--cfg docsrs` (requirement R-WS-1).
    pub cfg_docsrs: bool,
    /// The SHA-256 of the JSON.
    pub sha256: Box<str>,
}

/// Why a build published nothing.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// cargo could not be run, or ran too long.
    #[error(transparent)]
    Run(#[from] RunError),
    /// cargo failed.
    #[error("cargo failed to build the documentation")]
    Failed {
        /// How, as its messages tell (requirement R-WS-7).
        failure: Failure,
        /// Whether the documented crate's rustdoc had an error, which `--cfg docsrs` may cause
        /// (decided in M2's D3).
        in_documented: bool,
        /// The first errors of the compiler.
        errors: Vec<Box<str>>,
        /// The end of its stderr.
        stderr: Box<str>,
    },
    /// cargo succeeded, but wrote no JSON where it was looked for.
    #[error("cargo wrote no rustdoc JSON at {}", .0.display())]
    NoJson(PathBuf),
    /// cargo found the documentation fresh and did not run rustdoc, so there is no JSON: cargo
    /// 1.99.1 runs it every time, but another version might not.
    #[error("cargo did not run rustdoc, as it found the documentation fresh, so there is no rustdoc JSON at {}", .0.display())]
    Fresh(PathBuf),
    /// The JSON cannot be read.
    #[error("the rustdoc JSON cannot be read")]
    Unreadable(#[source] serde_json::Error),
    /// The JSON documents another crate, version or target, as another build of the same name
    /// may have written (Q-29).
    #[error("the rustdoc JSON is not that of {expected}, but of {found}")]
    NotTheCrate {
        /// What was built.
        expected: String,
        /// What the JSON documents.
        found: String,
    },
    /// The JSON could not be moved or published.
    #[error("the rustdoc JSON could not be published")]
    Io(#[source] io::Error),
}

/// Runs the build of `plan`, and publishes its JSON in `cache` (requirements R-WS-1 and R-WS-6,
/// decided in M2's D2).
///
/// It uses the target directory with its shared lock, and writes in it with its exclusive one,
/// which it lets go once the JSON is moved to `work`, a directory in the cache: another build of
/// a crate of the same name writes the same file (Q-29). cargo runs with a build slot (decided in
/// M2's D3). Then it checks the JSON and publishes it. `on_doing` is told when it waits for a lock
/// or a slot, and what cargo does. `toolchain` is what `rustc -vV` tells of the toolchain, for the
/// record.
///
/// # Errors
///
/// Returns why nothing was published.
pub async fn build(
    cache: &Cache,
    plan: &Plan,
    work: &Path,
    toolchain: Option<&str>,
    mut on_doing: impl FnMut(Doing) + Send,
) -> Result<Built, BuildError> {
    let name = plan.target_dir.name();
    let using = hold(
        cache.dir(),
        &target_key(&name),
        LockMode::Shared,
        &mut on_doing,
    )
    .await;
    let writing = hold(
        cache.dir(),
        &doc_key(&name),
        LockMode::Exclusive,
        &mut on_doing,
    )
    .await;
    let slot = slots().take(&mut on_doing).await;
    let mut messages = Messages::new(plan);
    let output = run_building(
        plan.command(),
        plan.target_dir.path(),
        |activity| on_doing(activity.of_build()),
        |line| messages.read(line),
    )
    .await;
    drop(slot);
    let output = output?;
    if !output.status.success() {
        return Err(BuildError::Failed {
            failure: classify(&output, plan.offline),
            in_documented: messages.documented_failed,
            errors: messages.errors,
            stderr: stderr_tail(&output),
        });
    }
    let json = plan.json();
    let moved = work.join(format!(
        "build-{}.json",
        NEXT_WORK.fetch_add(1, Ordering::Relaxed)
    ));
    let moving = json.clone();
    // On a blocking thread, which goes on if this future is dropped: it lets the locks go when
    // the file is out of the way, and leaves nothing in `work`.
    let contents = super::blocking(move || {
        fs::create_dir_all(moved.parent().unwrap_or(&moved))?;
        let renamed = fs::rename(&moving, &moved);
        drop(writing);
        drop(using);
        renamed?;
        let read = fs::read(&moved);
        drop(fs::remove_file(&moved));
        read
    })
    .await
    .map_err(|error| match (error.kind(), messages.documented_fresh) {
        (io::ErrorKind::NotFound, Some(true)) => BuildError::Fresh(json),
        (io::ErrorKind::NotFound, _) => BuildError::NoJson(json),
        _ => BuildError::Io(error),
    })?;
    let header = Header::read(&contents).map_err(BuildError::Unreadable)?;
    let target = header.check(plan)?;
    let sha256 = cache::sha256_hex(&contents);
    let record = BuildRecord {
        package: plan.package.0.clone(),
        crate_name: plan.crate_name.clone(),
        crate_version: plan.version.to_string().into(),
        format_version: header.format_version,
        target: target.as_ref().map(|target| target.as_str().into()),
        toolchain: toolchain.map(Into::into),
        chosen_toolchain: plan.toolchain.as_deref().map(Into::into),
        built_at: super::rfc3339(SystemTime::now()).into(),
        features_effective: messages.features,
        bootstrap_all: plan.bootstrap == Bootstrap::All,
        cfg_docsrs: plan.cfg_docsrs,
        sha256: sha256.into(),
    };
    let dir = plan.published_dir(cache.dir(), target.as_ref());
    let path = dir.join(format!("{}.json", header.format_version));
    let record_json =
        serde_json::to_vec_pretty(&record).map_err(|error| BuildError::Io(error.into()))?;
    let publishing = cache.lock(&dir).await;
    let (cache, published) = (cache.clone(), path.clone());
    super::blocking(move || {
        cache.write_verified(&published, &contents)?;
        let written = cache::write_atomically(&dir.join(RECORD), &record_json);
        drop(publishing);
        written
    })
    .await
    .map_err(BuildError::Io)?;
    Ok(Built {
        json: path,
        record,
        rebuilt: messages.rebuilt,
    })
}

impl Built {
    /// Writes the record again, as after more of the toolchain became known.
    ///
    /// # Errors
    ///
    /// Returns an error when it cannot be written.
    pub fn write_record(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.record).map_err(io::Error::from)?;
        cache::write_atomically(&self.record_path(), &json)
    }

    /// Removes the JSON and the record, as for a format that is not read, so that nothing takes
    /// them for a build to use.
    pub fn unpublish(&self) {
        for path in [
            self.json.clone(),
            cache::with_suffix(&self.json, ".sha256"),
            self.record_path(),
        ] {
            if let Err(error) = fs::remove_file(&path)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(path = %path.display(), %error, "a build could not be removed");
            }
        }
    }

    fn record_path(&self) -> PathBuf {
        self.json.with_file_name(RECORD)
    }
}

/// The key of the lock of the target directory `name` in `t/`: shared by the builds, exclusive
/// for removing it (decided in C1). Other versions find it by this key, which must not change.
fn target_key(name: &str) -> String {
    format!("target:{name}")
}

/// The key of the lock of writing the documentation in the target directory `name` in `t/`,
/// where builds of crates of the same name write the same files (decided in M2's D2, Q-29). It
/// must not change either.
fn doc_key(name: &str) -> String {
    format!("doc:{name}")
}

/// Takes the lock of `key` in the cache at `root` in `mode`, telling `on_doing` while it waits.
/// When the OS cannot lock, it goes on without, with a log line, as [`Cache::lock`] does.
async fn hold(
    root: &Path,
    key: &str,
    mode: LockMode,
    on_doing: &mut (impl FnMut(Doing) + Send),
) -> Option<ProcessLock> {
    let failed = |error: &io::Error| {
        tracing::warn!(%key, %error, "a lock of a build cannot be taken; going on without it");
    };
    match process_lock::try_lock(root, key, mode) {
        Ok(Some(held)) => return Some(held),
        Ok(None) => {},
        Err(error) => {
            failed(&error);
            return None;
        },
    }
    on_doing(Doing::WaitingForLock);
    let held = process_lock::lock(root, key, mode)
        .await
        .inspect_err(failed)
        .ok();
    on_doing(Doing::Building);
    held
}

/// What a build's JSON messages tell (`--message-format=json`).
struct Messages<'a> {
    plan: &'a Plan,
    /// The features of the documented crate, from its `compiler-artifact`.
    features: Option<Vec<Box<str>>>,
    /// Whether cargo found the documented crate fresh, from the same message.
    documented_fresh: Option<bool>,
    /// Whether the compiler had an error in the documented crate.
    documented_failed: bool,
    /// The units that were built rather than fresh.
    rebuilt: Vec<Box<str>>,
    /// The first errors of the compiler, rendered.
    errors: Vec<Box<str>>,
}

/// A JSON message of cargo, as far as it is read.
#[derive(Deserialize)]
struct Message {
    reason: Box<str>,
    #[serde(default)]
    package_id: Option<Box<str>>,
    #[serde(default)]
    target: Option<MessageTarget>,
    #[serde(default)]
    features: Vec<Box<str>>,
    #[serde(default)]
    fresh: bool,
    #[serde(default, rename = "message")]
    diagnostic: Option<Diagnostic>,
}

/// The target of a unit in a message.
#[derive(Deserialize)]
struct MessageTarget {
    name: Box<str>,
    kind: Vec<Box<str>>,
}

/// A message of the compiler.
#[derive(Deserialize)]
struct Diagnostic {
    level: Box<str>,
    #[serde(default)]
    rendered: Option<Box<str>>,
}

impl<'a> Messages<'a> {
    const fn new(plan: &'a Plan) -> Self {
        Self {
            plan,
            features: None,
            documented_fresh: None,
            documented_failed: false,
            rebuilt: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Reads a line of stdout. What is not a message is skipped.
    fn read(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<Message>(line) else {
            return;
        };
        match &*message.reason {
            "compiler-artifact" => {
                let Some(target) = &message.target else {
                    return;
                };
                if !message.fresh {
                    self.rebuilt.push(target.name.clone());
                }
                if message
                    .package_id
                    .as_deref()
                    .is_some_and(|id| self.plan.package.is(id))
                    && self.is_documented(target)
                {
                    self.features = Some(message.features);
                    self.documented_fresh = Some(message.fresh);
                }
            },
            "compiler-message" => {
                let Some(Diagnostic { level, rendered }) = message.diagnostic else {
                    return;
                };
                if &*level != "error" {
                    return;
                }
                if message
                    .package_id
                    .as_deref()
                    .is_some_and(|id| self.plan.package.is(id))
                    && message
                        .target
                        .as_ref()
                        .is_some_and(|target| self.is_documented(target))
                {
                    self.documented_failed = true;
                }
                if let Some(rendered) = rendered
                    && self.errors.len() < MAX_ERRORS
                {
                    self.errors.push(rendered);
                }
            },
            _ => {},
        }
    }

    /// Whether `target` is the documented crate's, not the lib that a bin of the same name
    /// needs.
    fn is_documented(&self, target: &MessageTarget) -> bool {
        let has = |wanted: &str| target.kind.iter().any(|kind| &**kind == wanted);
        match &self.plan.documented {
            Documented::Lib { .. } => {
                crate_name(&target.name) == *self.plan.crate_name
                    && target.kind.iter().any(|kind| LIB_KINDS.contains(&&**kind))
            },
            Documented::Bin { name } => target.name == *name && has("bin"),
        }
    }
}

/// The top-level members of a build's JSON that tell what it documents.
#[derive(Deserialize)]
struct Header {
    format_version: u32,
    #[serde(default)]
    crate_version: Option<Box<str>>,
    root: serde_json::Value,
    index: HashMap<Box<str>, Named>,
    #[serde(default)]
    target: Option<HeaderTarget>,
}

/// An item of the index, as far as its name.
#[derive(Deserialize)]
struct Named {
    #[serde(default)]
    name: Option<Box<str>>,
}

/// The target of the JSON (format 35 and later).
#[derive(Deserialize)]
struct HeaderTarget {
    triple: Box<str>,
}

impl Header {
    /// Reads the header of `json`, skipping all but the names of the items.
    fn read(json: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(json)
    }

    /// Checks that the JSON documents the crate, the version and, with `--target`, the target of
    /// `plan`, and returns its target, if it tells.
    fn check(&self, plan: &Plan) -> Result<Option<TargetTriple>, BuildError> {
        // A number in the newer formats and text in the older; the keys of the index are text.
        let root = if let serde_json::Value::String(id) = &self.root {
            id.clone()
        } else {
            self.root.to_string()
        };
        let name = self
            .index
            .get(root.as_str())
            .and_then(|item| item.name.as_deref());
        let target = self
            .target
            .as_ref()
            .and_then(|target| target.triple.parse::<TargetTriple>().ok());
        let version = plan.version.to_string();
        let target_differs = match (&plan.target, &target) {
            (Some(wanted), Some(found)) => !plan.is_proc_macro() && wanted != found,
            _ => false,
        };
        if name == Some(&*plan.crate_name)
            && self.crate_version.as_deref() == Some(version.as_str())
            && !target_differs
        {
            return Ok(target);
        }
        let wanted = plan
            .target
            .as_ref()
            .map_or(String::new(), |target| format!(" for {target}"));
        Err(BuildError::NotTheCrate {
            expected: format!("{} {version}{wanted}", plan.crate_name),
            found: format!(
                "{} {}{}",
                name.unwrap_or("?"),
                self.crate_version.as_deref().unwrap_or("?"),
                target
                    .as_ref()
                    .map_or(String::new(), |target| format!(" for {target}"))
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo metadata` of a workspace of the members `app` (a lib `app` and a bin `my-app`),
    /// `macros` (a proc-macro) and `tool` (only a bin `tool`), with the dependencies `serde`
    /// (a lib, and a bin of its own) and `cdy` (only a cdylib).
    fn metadata() -> Metadata {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "workspace_root": "/ws",
            "workspace_members": ["path+file:///ws/app#0.1.0", "path+file:///ws/macros#0.1.0", "path+file:///ws/tool#0.1.0"],
            "packages": [
                {"id": "path+file:///ws/app#0.1.0", "name": "app", "version": "0.1.0", "source": null,
                 "manifest_path": "/ws/app/Cargo.toml", "targets": [
                    {"name": "app", "kind": ["lib"]},
                    {"name": "my-app", "kind": ["bin"]},
                    {"name": "app", "kind": ["bin"]},
                    {"name": "build-script-build", "kind": ["custom-build"]},
                    {"name": "it", "kind": ["test"]}]},
                {"id": "path+file:///ws/macros#0.1.0", "name": "macros", "version": "0.1.0", "source": null,
                 "manifest_path": "/ws/macros/Cargo.toml", "targets": [{"name": "macros", "kind": ["proc-macro"]}]},
                {"id": "path+file:///ws/tool#0.1.0", "name": "tool", "version": "0.1.0", "source": null,
                 "manifest_path": "/ws/tool/Cargo.toml", "targets": [{"name": "tool", "kind": ["bin"]}]},
                {"id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0", "name": "serde",
                 "version": "1.0.0", "source": "registry+https://github.com/rust-lang/crates.io-index",
                 "manifest_path": "/r/serde/Cargo.toml", "targets": [
                    {"name": "serde", "kind": ["lib"]}, {"name": "serde-cli", "kind": ["bin"]}]},
                {"id": "registry+https://github.com/rust-lang/crates.io-index#cdy@1.0.0", "name": "cdy",
                 "version": "1.0.0", "source": "registry+https://github.com/rust-lang/crates.io-index",
                 "manifest_path": "/r/cdy/Cargo.toml", "targets": [{"name": "cdy", "kind": ["cdylib"]}]}
            ],
            "resolve": null
        }))
        .expect("metadata")
    }

    #[test]
    fn the_value_names_every_lib_and_the_members_bins() {
        // Not the bin of a dependency, nor a build script or a test (requirement R-WS-8).
        assert_eq!(
            Bootstrap::of(&metadata()).value(),
            "app,cdy,macros,my_app,serde,tool"
        );
    }

    #[test]
    fn too_long_a_value_is_one() {
        let mut metadata = metadata();
        let package = metadata.packages.first_mut().expect("a package");
        let long = "a".repeat(MAX_BOOTSTRAP_BYTES.saturating_add(1));
        package.targets = vec![Target {
            name: long.into(),
            kind: vec!["lib".into()].into(),
        }]
        .into();
        assert_eq!(Bootstrap::of(&metadata), Bootstrap::All);
        assert_eq!(Bootstrap::All.value(), "1");
    }

    #[test]
    fn a_members_lib_comes_before_its_bins() {
        let metadata = metadata();
        let found = |name| {
            member_crate(&metadata, name).map(|(package, documented)| (&*package.name, documented))
        };
        // The lib, though a bin has its name too.
        assert_eq!(
            found("app"),
            Some(("app", Documented::Lib { proc_macro: false }))
        );
        assert_eq!(
            found("my_app"),
            Some(("app", Documented::Bin {
                name: "my-app".into()
            }))
        );
        assert_eq!(
            found("macros"),
            Some(("macros", Documented::Lib { proc_macro: true }))
        );
        assert_eq!(
            found("tool"),
            Some(("tool", Documented::Bin {
                name: "tool".into()
            }))
        );
        // Not a dependency's, nor a build script.
        assert_eq!(found("serde"), None);
        assert_eq!(found("build_script_build"), None);
    }

    #[test]
    fn the_crate_of_a_member_package_is_the_one_named_or_its_lib() {
        let metadata = metadata();
        let package =
            |name: &str| member_package(&metadata, name, &Version::new(0, 1, 0)).expect("a member");
        // The bin named, though the package has a lib.
        assert_eq!(
            crate_of(package("app"), "my_app"),
            Some(Documented::Bin {
                name: "my-app".into()
            })
        );
        // The lib, when the name is not a target's, as a dependency's renamed name.
        assert_eq!(
            crate_of(package("app"), "renamed"),
            Some(Documented::Lib { proc_macro: false })
        );
        // Its bin alone, without a lib.
        assert_eq!(
            crate_of(package("tool"), "renamed"),
            Some(Documented::Bin {
                name: "tool".into()
            })
        );
        // Not a dependency, nor another version.
        assert!(member_package(&metadata, "serde", &Version::new(1, 0, 0)).is_none());
        assert!(member_package(&metadata, "app", &Version::new(0, 2, 0)).is_none());
    }

    #[test]
    fn a_target_directory_marked_unusable_is_passed_over() {
        let cache = tempfile::tempdir().expect("a temporary directory");
        let root = Path::new("/ws");
        let first = TargetDir::of(cache.path(), root);
        let name = first
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name")
            .to_owned();
        assert_eq!(name.len(), HASH_DIGITS);
        assert_eq!(
            first.path().parent(),
            Some(&*cache.path().join(TARGET_DIRS))
        );
        assert_eq!(TargetDir::of(cache.path(), root), first, "the same");
        assert_ne!(TargetDir::of(cache.path(), Path::new("/other")), first);
        fs::create_dir_all(first.path()).expect("made");
        fs::write(first.path().join(UNUSABLE), "").expect("marked");
        let second = TargetDir::of(cache.path(), root);
        assert_eq!(
            second.path().file_name(),
            Some(format!("{name}.1").as_ref())
        );
        assert_eq!(second.target(), second.path().join("target"));
        assert_eq!(second.build(), second.path().join("build"));
    }

    #[test]
    fn the_target_is_passed_when_it_is_not_the_hosts_or_the_project_sets_one() {
        let host: TargetTriple = "x86_64-unknown-linux-gnu".parse().expect("a triple");
        let other: TargetTriple = "aarch64-apple-darwin".parse().expect("a triple");
        assert!(!passes_target(&host, Some(&host), false));
        assert!(passes_target(&host, Some(&host), true));
        assert!(passes_target(&other, Some(&host), false));
        assert!(passes_target(&host, None, false));
    }

    /// A plan of the lib `app` 0.1.0, for `target` if any.
    fn app_plan(target: Option<&str>, proc_macro: bool) -> Plan {
        Plan {
            root: PathBuf::from("/ws"),
            offline: false,
            toolchain: None,
            bootstrap: Bootstrap::All,
            target_dir: TargetDir(PathBuf::from("/c/t/0123456789ab")),
            package: PackageId("path+file:///ws/app#0.1.0".into()),
            documented: Documented::Lib { proc_macro },
            crate_name: "app".into(),
            version: Version::new(0, 1, 0),
            target: target.map(|target| target.parse().expect("a triple")),
            cfg_docsrs: true,
        }
    }

    fn header(json: &serde_json::Value) -> Header {
        Header::read(&serde_json::to_vec(json).expect("JSON")).expect("a header")
    }

    #[test]
    fn the_json_is_checked_against_the_plan() {
        let linux = "x86_64-unknown-linux-gnu";
        // The root is a number in the newer formats, and text in the older.
        let newer = header(&serde_json::json!({
            "format_version": 61, "crate_version": "0.1.0", "root": 0,
            "index": {"0": {"name": "app"}, "1": {"name": "other"}},
            "target": {"triple": linux},
        }));
        assert_eq!(
            newer
                .check(&app_plan(None, false))
                .expect("app")
                .as_ref()
                .map(TargetTriple::as_str),
            Some(linux)
        );
        let older = header(&serde_json::json!({
            "format_version": 30, "crate_version": "0.1.0", "root": "0:0",
            "index": {"0:0": {"name": "app"}},
        }));
        assert_eq!(older.check(&app_plan(None, false)).expect("app"), None);
        let not = |json: serde_json::Value| {
            matches!(
                header(&json).check(&app_plan(None, false)),
                Err(BuildError::NotTheCrate { .. })
            )
        };
        assert!(
            not(serde_json::json!({
                "format_version": 61, "root": 0, "index": {"0": {"name": "app"}},
            })),
            "without a version"
        );
        assert!(
            not(serde_json::json!({
                "format_version": 61, "crate_version": "0.2.0", "root": 0,
                "index": {"0": {"name": "app"}},
            })),
            "another version"
        );
        assert!(
            not(serde_json::json!({
                "format_version": 61, "crate_version": "0.1.0", "root": 1,
                "index": {"0": {"name": "app"}, "1": {"name": "other"}},
            })),
            "another crate"
        );
        // With `--target`, the target must be it, but a proc-macro's is the host's.
        let built_for_linux = header(&serde_json::json!({
            "format_version": 61, "crate_version": "0.1.0", "root": 0,
            "index": {"0": {"name": "app"}}, "target": {"triple": linux},
        }));
        let other = Some("aarch64-apple-darwin");
        assert!(matches!(
            built_for_linux.check(&app_plan(other, false)),
            Err(BuildError::NotTheCrate { .. })
        ));
        let found = |plan: &Plan| {
            built_for_linux
                .check(plan)
                .expect("app")
                .map(|target| target.as_str().to_owned())
        };
        assert_eq!(found(&app_plan(other, true)).as_deref(), Some(linux));
        assert_eq!(found(&app_plan(Some(linux), false)).as_deref(), Some(linux));
    }

    #[tokio::test]
    async fn a_build_waits_for_a_slot_when_all_are_taken() {
        let slots = std::sync::Arc::new(Slots::new(1));
        let held = slots.take(&mut |_| {}).await.expect("a slot");
        let told = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let waiting = {
            let (slots, told) = (std::sync::Arc::clone(&slots), std::sync::Arc::clone(&told));
            tokio::spawn(async move {
                let mut tell = |doing| told.lock().push(doing);
                slots.take(&mut tell).await.is_some()
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*told.lock(), [Doing::WaitingForSlot]);
        drop(held);
        assert!(waiting.await.expect("joined"));
        assert_eq!(*told.lock(), [Doing::WaitingForSlot, Doing::Building]);
        // At least one.
        assert!(Slots::new(0).take(&mut |_| {}).await.is_some());
    }

    #[test]
    fn only_an_error_of_the_documented_crate_is_told_apart() {
        let plan = app_plan(None, false);
        let message = |package: &str, target: &str, level: &str| {
            serde_json::json!({
                "reason": "compiler-message",
                "package_id": package,
                "target": {"name": target, "kind": ["lib"]},
                "message": {"level": level, "rendered": "error[E0557]: feature has been removed"},
            })
            .to_string()
        };
        let app = "path+file:///ws/app#0.1.0";
        let mut messages = Messages::new(&plan);
        messages.read(&message(
            "registry+https://github.com/rust-lang/crates.io-index#dep@1.0.0",
            "dep",
            "error",
        ));
        messages.read(&message(app, "app", "warning"));
        assert!(!messages.documented_failed);
        assert_eq!(messages.errors.len(), 1);
        messages.read(&message(app, "app", "error"));
        assert!(messages.documented_failed);
    }

    #[test]
    fn the_keys_of_the_locks_do_not_change() {
        assert_eq!(target_key("0123456789ab.1"), "target:0123456789ab.1");
        assert_eq!(doc_key("0123456789ab.1"), "doc:0123456789ab.1");
    }

    #[test]
    fn a_proc_macro_lands_in_the_hosts_doc_directory() {
        let dir = TargetDir(PathBuf::from("/c/t/0123456789ab"));
        let other: TargetTriple = "aarch64-apple-darwin".parse().expect("a triple");
        let docs = Path::new("/c/t/0123456789ab/target");
        assert_eq!(dir.doc_dir(None, false), docs.join("doc"));
        assert_eq!(
            dir.doc_dir(Some(&other), false),
            docs.join("aarch64-apple-darwin/doc")
        );
        assert_eq!(dir.doc_dir(Some(&other), true), docs.join("doc"));
    }

    #[test]
    fn the_command_is_that_of_the_requirements() {
        let other: TargetTriple = "aarch64-apple-darwin".parse().expect("a triple");
        let plan = Plan {
            root: PathBuf::from("/ws"),
            offline: true,
            toolchain: Some("nightly-2026-07-10".to_owned()),
            bootstrap: Bootstrap::Crates("app,my_app".to_owned()),
            target_dir: TargetDir(PathBuf::from("/c/t/0123456789ab")),
            package: PackageId("path+file:///ws/app#0.1.0".into()),
            documented: Documented::Bin {
                name: "my-app".into(),
            },
            crate_name: "my_app".into(),
            version: Version::new(0, 1, 0),
            target: Some(other),
            cfg_docsrs: true,
        };
        let command = plan.command();
        let command = command.as_std();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let jobs = jobs().to_string();
        assert_eq!(args, [
            "+nightly-2026-07-10",
            "--locked",
            "--offline",
            "rustdoc",
            "-j",
            &jobs,
            "-p",
            "path+file:///ws/app#0.1.0",
            "--bin",
            "my-app",
            "--target",
            "aarch64-apple-darwin",
            "--message-format=json",
            "--",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
            "--cfg",
            "docsrs",
            "--document-private-items",
        ]);
        let env = |name: &str| {
            command
                .get_envs()
                .find(|(key, _)| *key == name)
                .and_then(|(_, value)| value)
                .map(|value| value.to_string_lossy().into_owned())
        };
        assert_eq!(env("RUSTC_BOOTSTRAP").as_deref(), Some("app,my_app"));
        assert_eq!(env("RUSTUP_AUTO_INSTALL").as_deref(), Some("0"));
        assert_eq!(
            env("CARGO_TARGET_DIR").map(PathBuf::from),
            Some(PathBuf::from("/c/t/0123456789ab/target"))
        );
        assert_eq!(
            env("CARGO_BUILD_BUILD_DIR").map(PathBuf::from),
            Some(PathBuf::from("/c/t/0123456789ab/build"))
        );
        assert_eq!(
            plan.json(),
            Path::new("/c/t/0123456789ab/target/aarch64-apple-darwin/doc/my_app.json")
        );
    }

    #[test]
    fn a_lib_without_a_target_is_built_for_the_host() {
        let plan = Plan {
            root: PathBuf::from("/ws"),
            offline: false,
            toolchain: None,
            bootstrap: Bootstrap::All,
            target_dir: TargetDir(PathBuf::from("/c/t/0123456789ab")),
            package: PackageId("path+file:///ws/macros#0.1.0".into()),
            documented: Documented::Lib { proc_macro: true },
            crate_name: "macros".into(),
            version: Version::new(0, 1, 0),
            target: None,
            cfg_docsrs: false,
        };
        let command = plan.command();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.first().map(String::as_str), Some("--locked"));
        assert!(args.iter().any(|arg| arg == "--lib"));
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--target" || arg == "--offline")
        );
        assert!(
            !command
                .as_std()
                .get_envs()
                .any(|(key, _)| key == "RUSTUP_AUTO_INSTALL"),
            "rustup may install the project's toolchain online (decided in M2's D1)"
        );
        assert_eq!(
            plan.json(),
            Path::new("/c/t/0123456789ab/target/doc/macros.json")
        );
    }
}
