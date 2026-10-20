//! The packages of a project as `cargo metadata` resolves them (requirement R-VR-2), kept while
//! the files that decide them do not change, with Cargo.lock read directly when cargo fails.

use std::{
    collections::HashMap,
    fs,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use semver::Version;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::{
    blocking,
    cargo::{self, Failure, RunError},
    cargo_config,
    cargo_lock::CargoLock,
    features::{self, FeatureSets},
    project::{self, Workspace},
    target::TargetTriple,
};

/// The parts of the output of `cargo metadata --format-version 1` that are used.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    /// The directory of the workspace's root `Cargo.toml`.
    pub workspace_root: PathBuf,
    /// The members.
    pub workspace_members: Box<[PackageId]>,
    /// Every package of the resolve, including those of other platforms.
    pub packages: Box<[Package]>,
    /// The dependency graph. Only `--no-deps`, which is not used, leaves it out.
    pub resolve: Option<Resolve>,
    /// The format of the output.
    version: u32,
}

/// A package id, as `registry+https://github.com/rust-lang/crates.io-index#serde@1.0.229`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
pub struct PackageId(pub Box<str>);

/// A package of the resolve.
#[derive(Debug, Deserialize)]
pub struct Package {
    /// The package id.
    pub id: PackageId,
    /// The package name.
    pub name: Box<str>,
    /// The version.
    #[serde(deserialize_with = "super::deserialize_version")]
    pub version: Version,
    /// Where it comes from. `None` for a path dependency and a workspace member.
    pub source: Option<Box<str>>,
    /// Its `Cargo.toml`. For a dependency, the directory of its source: the registry's, a git
    /// checkout's, or the vendor directory's.
    pub manifest_path: PathBuf,
    /// Its targets: the lib, bins, examples, tests, benches and build script.
    pub targets: Box<[Target]>,
}

/// A target of a package.
#[derive(Debug, Deserialize)]
pub struct Target {
    /// The crate name, with `_` for `-`.
    pub name: Box<str>,
    /// The kinds, as `lib`, `proc-macro`, `bin` or `custom-build`.
    pub kind: Box<[Box<str>]>,
}

/// The dependency graph.
#[derive(Debug, Deserialize)]
pub struct Resolve {
    /// One node for each package.
    pub nodes: Box<[Node]>,
}

/// A package in the dependency graph.
#[derive(Debug, Deserialize)]
pub struct Node {
    /// The package.
    pub id: PackageId,
    /// Its dependencies.
    pub deps: Box<[NodeDep]>,
    /// The features enabled on it, over all the kinds and platforms.
    pub features: Box<[Box<str>]>,
}

/// A dependency of a package.
#[derive(Debug, Deserialize)]
pub struct NodeDep {
    /// The name the dependent uses for it in `extern crate`, after a rename.
    pub name: Box<str>,
    /// The package depended on.
    pub pkg: PackageId,
    /// How it is depended on.
    pub dep_kinds: Box<[DepKind]>,
}

/// One way that a package depends on another.
#[derive(Debug, Deserialize)]
pub struct DepKind {
    /// `None` for a normal dependency, else `dev` or `build`.
    pub kind: Option<Box<str>>,
    /// The `cfg` or target that the dependency is limited to, if any.
    pub target: Option<Box<str>>,
}

/// How the packages of a workspace were resolved.
#[derive(Debug)]
pub enum Resolution {
    /// By `cargo metadata`, with Cargo.lock for the checksums, which `cargo metadata` does not
    /// report.
    Metadata {
        /// The output of `cargo metadata`.
        metadata: Metadata,
        /// Cargo.lock.
        lock: CargoLock,
    },
    /// From Cargo.lock alone, because `cargo metadata` failed (`resolved_from: "cargo_lock_raw"`).
    /// Nothing is built locally on this path (requirement R-MCP-3).
    CargoLockRaw {
        /// Cargo.lock.
        lock: CargoLock,
        /// Why `cargo metadata` failed, for the warnings.
        because: CargoError,
    },
}

/// Why a cargo command gave no result.
#[derive(Debug, thiserror::Error)]
pub enum CargoError {
    /// cargo did not finish.
    #[error(transparent)]
    Run(#[from] RunError),
    /// cargo failed.
    #[error("cargo {command} failed")]
    Failed {
        /// The subcommand.
        command: &'static str,
        /// What kind of failure its messages tell.
        failure: Failure,
        /// The end of its stderr.
        stderr: Box<str>,
    },
    /// The output is not in the form asked for.
    #[error("the output of cargo could not be read")]
    Unreadable(#[source] io::Error),
}

/// Why a workspace's packages cannot be resolved at all.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// There is no Cargo.lock, and cargo may not write one (`lockfile_outdated`, requirement
    /// R-VR-2).
    #[error("the workspace has no Cargo.lock")]
    NoCargoLock,
    /// Cargo.lock could not be read.
    #[error("Cargo.lock could not be read")]
    Unreadable(#[source] io::Error),
    /// Cargo.lock is not in the form of a Cargo.lock.
    #[error("Cargo.lock is not valid")]
    Malformed(#[source] toml::de::Error),
}

/// Why the feature sets of a workspace are not known.
#[derive(Debug, thiserror::Error)]
pub enum FeaturesError {
    /// The packages cannot be resolved.
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    /// `cargo metadata` failed, so `cargo tree`, which resolves the same way, would fail, too.
    /// Only Cargo.lock is known.
    #[error("cargo metadata failed, so the features are not known")]
    NoMetadata,
    /// `cargo tree` failed.
    #[error(transparent)]
    Cargo(#[from] CargoError),
}

/// Resolves the packages of workspaces, and keeps each result while its inputs stay the same.
#[derive(Debug)]
pub struct Resolver {
    /// Whether cargo runs with `--offline` (requirement R-CA-7).
    offline: bool,
    /// `$CARGO_HOME`, whose `config.toml` applies to every project.
    cargo_home: Option<PathBuf>,
    /// The kept results, by workspace root.
    kept: Mutex<HashMap<PathBuf, Arc<Kept>>>,
}

/// A result of `cargo metadata`, and the inputs it was made from.
#[derive(Debug)]
struct Kept {
    inputs: Inputs,
    resolution: Arc<Resolution>,
    /// The feature sets from `cargo tree`, by target. The same inputs decide them.
    features: Mutex<HashMap<TargetTriple, Arc<FeatureSets>>>,
}

/// The inputs that decide the output of `cargo metadata`, by content (requirement R-VR-2).
#[derive(Debug, PartialEq, Eq)]
struct Inputs {
    /// The files, in the order of their paths.
    files: Box<[InputFile]>,
    /// The directories that the `members` patterns match.
    members: Vec<PathBuf>,
}

/// A file among the inputs.
#[derive(Debug, PartialEq, Eq)]
struct InputFile {
    path: PathBuf,
    /// The contents, `None` when the file does not exist or cannot be read.
    contents: Option<Box<[u8]>>,
}

impl Resolver {
    /// A resolver that runs cargo offline when `offline` is set.
    #[must_use]
    pub fn new(offline: bool) -> Self {
        let cargo_home = std::env::var_os("CARGO_HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| etcetera::home_dir().ok().map(|home| home.join(".cargo")));
        Self {
            offline,
            cargo_home,
            kept: Mutex::new(HashMap::new()),
        }
    }

    /// `$CARGO_HOME`, whose settings apply to every project.
    #[must_use]
    pub fn cargo_home(&self) -> Option<&Path> {
        self.cargo_home.as_deref()
    }

    /// The packages of `workspace`, by `cargo metadata`, or from its Cargo.lock when cargo fails
    /// (requirement R-VR-2). A result of `cargo metadata` is used again while the files it was
    /// made from have the same contents: Cargo.lock, the `Cargo.toml` of every member and path
    /// dependency, the cargo and rustup settings that apply, and what the `members` patterns
    /// match.
    ///
    /// # Errors
    ///
    /// Returns an error when Cargo.lock is missing or cannot be read.
    pub async fn resolve(&self, workspace: &Workspace) -> Result<Arc<Resolution>, ResolveError> {
        let root = workspace.root.clone();
        let lock_path = root.join("Cargo.lock");
        let lock_text = blocking(move || fs::read_to_string(lock_path))
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    ResolveError::NoCargoLock
                } else {
                    ResolveError::Unreadable(error)
                }
            })?;
        let kept = self.kept.lock().await.get(&root).cloned();
        if let Some(kept) = kept {
            let files = kept.inputs.files.iter().map(|file| file.path.clone());
            if read_inputs(&root, files.collect()).await.as_ref() == Some(&kept.inputs) {
                return Ok(Arc::clone(&kept.resolution));
            }
        }
        let lock = CargoLock::parse(&lock_text).map_err(ResolveError::Malformed)?;
        let metadata = match self.run_metadata(&root).await {
            Ok(metadata) => metadata,
            Err(because) => return Ok(Arc::new(Resolution::CargoLockRaw { lock, because })),
        };
        let files = with_included(
            self.input_files(&root, &metadata),
            &root,
            self.cargo_home.clone(),
        )
        .await;
        let resolution = Arc::new(Resolution::Metadata { metadata, lock });
        // Kept only if Cargo.lock did not change while cargo ran, so that a result is never kept
        // under inputs newer than it.
        match read_inputs(&root, files).await {
            Some(inputs) if lock_is(&inputs, &root, &lock_text) => {
                self.kept.lock().await.insert(
                    root,
                    Arc::new(Kept {
                        inputs,
                        resolution: Arc::clone(&resolution),
                        features: Mutex::new(HashMap::new()),
                    }),
                );
            },
            Some(_) | None => {},
        }
        Ok(resolution)
    }

    /// The features that the normal build of `workspace` for `target`, the actual build target,
    /// enables on each package (requirements R-FT-2 and R-FT-3). They are kept with the result of
    /// `cargo metadata`, while the same inputs decide both.
    ///
    /// # Errors
    ///
    /// Returns an error when the packages cannot be resolved by `cargo metadata`, or `cargo tree`
    /// fails.
    pub async fn features(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
    ) -> Result<Arc<FeatureSets>, FeaturesError> {
        let resolution = self.resolve(workspace).await?;
        if let Resolution::CargoLockRaw { .. } = &*resolution {
            return Err(FeaturesError::NoMetadata);
        }
        let kept = self
            .kept
            .lock()
            .await
            .get(&workspace.root)
            .filter(|kept| Arc::ptr_eq(&kept.resolution, &resolution))
            .cloned();
        if let Some(kept) = &kept
            && let Some(sets) = kept.features.lock().await.get(target)
        {
            return Ok(Arc::clone(sets));
        }
        let sets = Arc::new(features::read(&workspace.root, self.offline, target).await?);
        if let Some(kept) = kept {
            kept.features
                .lock()
                .await
                .insert(target.clone(), Arc::clone(&sets));
        }
        Ok(sets)
    }

    /// Runs `cargo metadata` in `root` and reads its output.
    async fn run_metadata(&self, root: &Path) -> Result<Metadata, CargoError> {
        let mut command = cargo::project_command(root, self.offline);
        command.args(["metadata", "--format-version", "1"]);
        let output = cargo::run(command).await?;
        if !output.status.success() {
            return Err(CargoError::Failed {
                command: "metadata",
                failure: cargo::classify(&output, self.offline),
                stderr: cargo::stderr_tail(&output),
            });
        }
        // Megabytes for a large workspace: read off the async threads.
        let metadata: Metadata =
            blocking(move || serde_json::from_slice(&output.stdout).map_err(io::Error::other))
                .await
                .map_err(CargoError::Unreadable)?;
        if metadata.version == 1 {
            Ok(metadata)
        } else {
            Err(CargoError::Unreadable(io::Error::other(format!(
                "format version {} instead of 1",
                metadata.version
            ))))
        }
    }

    /// The files whose contents decide the output of `cargo metadata` in `root`, as far as
    /// `metadata` tells: Cargo.lock, the manifests of the root and of every package that is not
    /// from a registry or git, and the cargo and rustup settings that cargo finds from `root`. A
    /// file that `include` in a cargo setting names is not among them.
    fn input_files(&self, root: &Path, metadata: &Metadata) -> Vec<PathBuf> {
        let mut files = vec![root.join("Cargo.lock"), root.join("Cargo.toml")];
        files.extend(
            metadata
                .packages
                .iter()
                .filter(|package| package.source.is_none())
                .map(|package| package.manifest_path.clone()),
        );
        for dir in root.ancestors() {
            for name in [
                ".cargo/config.toml",
                ".cargo/config",
                "rust-toolchain.toml",
                "rust-toolchain",
            ] {
                files.push(dir.join(name));
            }
        }
        if let Some(home) = &self.cargo_home {
            files.extend([home.join("config.toml"), home.join("config")]);
        }
        files.sort();
        files.dedup();
        files
    }
}

/// `files` with those that cargo's settings for the workspace at `root` include (Q-31), sorted.
/// Settings that cannot be read make cargo fail as well; the settings files in `files` still
/// tell when they change.
async fn with_included(
    files: Vec<PathBuf>,
    root: &Path,
    cargo_home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let root = root.to_path_buf();
    let base = files.clone();
    blocking(move || {
        let mut files = files;
        if let Ok(config) = cargo_config::read(&root, cargo_home.as_deref(), None) {
            files.extend(config.files);
        }
        files.sort();
        files.dedup();
        Ok(files)
    })
    .await
    .unwrap_or(base)
}

/// Reads the contents of `files` and expands the `members` patterns of the workspace at `root`.
/// `None` when the patterns cannot be expanded, so that nothing is kept or reused.
async fn read_inputs(root: &Path, files: Vec<PathBuf>) -> Option<Inputs> {
    let root = root.to_path_buf();
    blocking(move || {
        let files = files
            .into_iter()
            .map(|path| {
                let contents = fs::read(&path).ok().map(Vec::into_boxed_slice);
                InputFile { path, contents }
            })
            .collect();
        Ok(project::expand_members(&root).map(|members| Inputs { files, members }))
    })
    .await
    .ok()
    .flatten()
}

/// Whether `inputs` holds `text` as the Cargo.lock of the workspace at `root`.
fn lock_is(inputs: &Inputs, root: &Path, text: &str) -> bool {
    let lock_path = root.join("Cargo.lock");
    inputs
        .files
        .iter()
        .any(|file| file.path == lock_path && file.contents.as_deref() == Some(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    const MEMBER: &str = "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

    /// A workspace whose members are `crates/*`, with one member `a` and a Cargo.lock.
    fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n",
        )
        .expect("written");
        add_member(root, "a");
        let status = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(root)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("cargo runs")
            .status;
        assert!(status.success(), "cargo generate-lockfile");
        let workspace = project::workspace_of(root).expect("a workspace");
        (dir, workspace)
    }

    fn add_member(root: &Path, name: &str) {
        let dir = root.join("crates").join(name);
        fs::create_dir_all(dir.join("src")).expect("made");
        fs::write(
            dir.join("Cargo.toml"),
            MEMBER.replace("\"a\"", &format!("{name:?}")),
        )
        .expect("written");
        fs::write(dir.join("src/lib.rs"), "").expect("written");
    }

    #[tokio::test]
    async fn a_result_is_kept_while_its_inputs_stay_the_same() {
        let (dir, workspace) = workspace();
        let resolver = Resolver::new(true);
        let first = resolver.resolve(&workspace).await.expect("resolved");
        let Resolution::Metadata { metadata, lock } = &*first else {
            panic!("cargo metadata works offline here: {first:?}");
        };
        assert_eq!(metadata.workspace_root, workspace.root);
        assert_eq!(metadata.workspace_members.len(), 1);
        assert_eq!(&*lock.packages[0].name, "a");
        let again = resolver.resolve(&workspace).await.expect("resolved");
        assert!(Arc::ptr_eq(&first, &again), "kept");

        let manifest = dir.path().join("crates/a/Cargo.toml");
        fs::write(&manifest, format!("{MEMBER}description = \"changed\"\n")).expect("written");
        let changed = resolver.resolve(&workspace).await.expect("resolved");
        assert!(
            !Arc::ptr_eq(&first, &changed),
            "a member manifest is an input"
        );
    }

    #[tokio::test]
    async fn a_file_that_the_cargo_settings_include_is_an_input() {
        let (dir, workspace) = workspace();
        let settings = dir.path().join(".cargo");
        fs::create_dir_all(&settings).expect("made");
        fs::write(settings.join("config.toml"), "include = [\"more.toml\"]\n").expect("written");
        fs::write(settings.join("more.toml"), "[alias]\nprobe = \"version\"\n").expect("written");
        let resolver = Resolver::new(true);
        let first = resolver.resolve(&workspace).await.expect("resolved");
        assert!(matches!(&*first, Resolution::Metadata { .. }), "{first:?}");
        let again = resolver.resolve(&workspace).await.expect("resolved");
        assert!(Arc::ptr_eq(&first, &again), "kept");
        fs::write(settings.join("more.toml"), "[alias]\nprobe = \"help\"\n").expect("written");
        let changed = resolver.resolve(&workspace).await.expect("resolved");
        assert!(
            !Arc::ptr_eq(&first, &changed),
            "an included file is an input"
        );
    }

    #[tokio::test]
    async fn a_new_member_outdates_cargo_lock() {
        let (dir, workspace) = workspace();
        let resolver = Resolver::new(true);
        let first = resolver.resolve(&workspace).await.expect("resolved");
        assert!(matches!(&*first, Resolution::Metadata { .. }));
        add_member(dir.path(), "b");
        let resolution = resolver.resolve(&workspace).await.expect("resolved");
        let Resolution::CargoLockRaw { lock, because } = &*resolution else {
            panic!("--locked refuses to add b: {resolution:?}");
        };
        assert!(matches!(because, CargoError::Failed {
            failure: Failure::LockfileOutdated,
            ..
        }));
        assert_eq!(lock.packages.len(), 1);
    }

    #[tokio::test]
    async fn features_are_those_of_the_normal_build_for_the_target() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        let package = |name: &str, rest: &str| {
            fs::create_dir_all(root.join(name).join("src")).expect("made");
            fs::write(root.join(name).join("src/lib.rs"), "").expect("written");
            fs::write(
                root.join(name).join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{rest}"
                ),
            )
            .expect("written");
        };
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = [\"a\"]\n",
        )
        .expect("written");
        package("dep", "[features]\nx = []\ny = []\nz = []\n");
        package(
            "pm",
            "[lib]\nproc-macro = true\n[dependencies]\ndep = { path = \"../dep\", features = [\"x\"] }\n",
        );
        package(
            "a",
            "[dependencies]\npm = { path = \"../pm\" }\ndep = { path = \"../dep\", features = [\"y\"] }\n\
             [dev-dependencies]\ndep = { path = \"../dep\", features = [\"z\"] }\n",
        );
        let status = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(root)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("cargo runs")
            .status;
        assert!(status.success(), "cargo generate-lockfile");
        let workspace = project::workspace_of(root).expect("a workspace");
        let target = crate::data::target::build_target();
        let resolver = Resolver::new(true);
        let sets = resolver
            .features(&workspace, &target)
            .await
            .expect("cargo tree works offline here");
        let version = Version::new(0, 1, 0);
        let names = |set: Option<&std::collections::BTreeSet<Box<str>>>| {
            set.expect("built")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        // `pm` is a member as a path dependency, so building the whole workspace enables its
        // `x` on `dep` for the target, too, as `cargo check --workspace` does (cargo 1.99.1).
        // `z` is only for the tests.
        assert_eq!(names(sets.normal("dep", &version)), ["x", "y"]);
        assert_eq!(names(sets.with_dev("dep", &version)), ["x", "y", "z"]);
        let again = resolver.features(&workspace, &target).await.expect("kept");
        assert!(Arc::ptr_eq(&sets, &again), "kept");
    }

    #[tokio::test]
    async fn a_workspace_without_cargo_lock_cannot_be_resolved() {
        let (dir, workspace) = workspace();
        fs::remove_file(dir.path().join("Cargo.lock")).expect("removed");
        assert!(matches!(
            Resolver::new(true).resolve(&workspace).await,
            Err(ResolveError::NoCargoLock)
        ));
    }
}
