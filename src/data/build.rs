//! The plan of a local build of a workspace member's documentation: the command, its target
//! directory and where the JSON lands (requirements R-WS-1, R-WS-2 and R-WS-8, decided in M2's
//! D2).
//!
//! Nothing here runs: the build itself runs the [`Plan`]'s command.

use std::{
    collections::{BTreeSet, HashSet},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use super::{
    cache,
    metadata::{Metadata, Package, PackageId},
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

/// Reads the settings of the builds, once, at startup.
pub fn set_this(settings: &Settings) {
    let jobs = settings.get("build.jobs", |value| {
        value
            .whole_number()
            .and_then(|jobs| usize::try_from(jobs).ok())
            .and_then(NonZeroUsize::new)
            .ok_or("a whole number from 1")
    });
    if let Some(jobs) = jobs
        && JOBS.set(jobs).is_err()
    {
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
        let hash = cache::sha256_hex(root.as_os_str().as_encoded_bytes());
        let stem = hash.get(..HASH_DIGITS).unwrap_or(&hash);
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

/// The member whose crate is named `name`, with `_` for `-`, and that crate: a member's lib, or
/// else a member's bin. A bin of the same name as a lib cannot be reached (decided in M2's D2).
#[must_use]
pub fn member_crate<'a>(metadata: &'a Metadata, name: &str) -> Option<(&'a Package, Documented)> {
    let members: HashSet<&PackageId> = metadata.workspace_members.iter().collect();
    let member_packages = || {
        metadata
            .packages
            .iter()
            .filter(|package| members.contains(&package.id))
    };
    let named = |target: &&super::metadata::Target| crate_name(&target.name) == name;
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
    lib.or_else(|| {
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
    })
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
    /// The target passed as `--target`, if any.
    pub target: Option<TargetTriple>,
}

impl Plan {
    /// The command (requirement R-WS-1): `cargo [+<toolchain>] --locked [--offline] rustdoc -j
    /// <jobs> -p <package id> (--lib | --bin <name>) [--target <target>] --message-format=json --
    /// -Z unstable-options --output-format json --cfg docsrs --document-private-items`, with the
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
            .arg(&*self.package.0);
        match &self.documented {
            Documented::Lib { .. } => command.arg("--lib"),
            Documented::Bin { name } => command.arg("--bin").arg(&**name),
        };
        if let Some(target) = &self.target {
            command.arg("--target").arg(target.as_str());
        }
        command
            .args([
                "--message-format=json",
                "--",
                "-Z",
                "unstable-options",
                "--output-format",
                "json",
                // Only the documented crate's rustdoc has it (requirement R-WS-1).
                "--cfg",
                "docsrs",
                // A member's private items too (requirement R-WS-3).
                "--document-private-items",
            ])
            .env("RUSTC_BOOTSTRAP", self.bootstrap.value())
            .env("CARGO_TARGET_DIR", self.target_dir.target())
            .env("CARGO_BUILD_BUILD_DIR", self.target_dir.build());
        command
    }

    /// The JSON that the build writes, `<doc dir>/<crate>.json`, where another build of a crate
    /// of the same name writes too (Q-29).
    #[must_use]
    pub fn json(&self, crate_name: &str) -> PathBuf {
        let proc_macro = matches!(self.documented, Documented::Lib { proc_macro: true });
        self.target_dir
            .doc_dir(self.target.as_ref(), proc_macro)
            .join(format!("{crate_name}.json"))
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
        package.targets = vec![super::super::metadata::Target {
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
        std::fs::create_dir_all(first.path()).expect("made");
        std::fs::write(first.path().join(UNUSABLE), "").expect("marked");
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
            target: Some(other),
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
            plan.json("my_app"),
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
            target: None,
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
            plan.json("macros"),
            Path::new("/c/t/0123456789ab/target/doc/macros.json")
        );
    }
}
