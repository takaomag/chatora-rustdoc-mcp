//! The packages of a project as `cargo metadata` resolves them (requirement R-VR-2), kept while
//! the files that decide them do not change, with Cargo.lock read directly when cargo fails.

use std::{
    collections::HashMap,
    fs,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use semver::Version;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::{
    blocking,
    build_scope::{self, BuildScope, ScopeError},
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

/// How long a failure of `cargo metadata` or `cargo tree` is given again without running cargo,
/// while its inputs stay the same (the TTL of transient failures, requirement R-WS-7).
const FAILURE_TTL: Duration = Duration::from_secs(30);

/// Resolves the packages of workspaces, and keeps each result while its inputs stay the same.
/// Calls for the same workspace at the same time run cargo once (requirement R-VR-2).
#[derive(Debug)]
pub struct Resolver {
    /// Whether cargo runs with `--offline` (requirement R-CA-7).
    offline: bool,
    /// `$CARGO_HOME`, whose `config.toml` applies to every project.
    cargo_home: Option<PathBuf>,
    /// The kept results, by workspace root.
    kept: Mutex<HashMap<PathBuf, Arc<Kept>>>,
    /// A lock for each workspace root, held while it is resolved.
    flights: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

/// A result of `cargo metadata`, or of Cargo.lock when it failed, and the inputs it was made from.
#[derive(Debug)]
struct Kept {
    inputs: Inputs,
    /// The workspace root that the inputs are of: cargo's, which is above the one asked for when
    /// that is an implicit member (requirement R-MCP-3).
    inputs_root: PathBuf,
    resolution: Arc<Resolution>,
    /// When it was made: a failure is given again only for [`FAILURE_TTL`].
    at: Instant,
    /// The feature sets from `cargo tree`, or why they are not known, by target, and when. The
    /// same inputs decide them. Held while `cargo tree` runs, so that it runs once.
    features: PerTarget<FeatureSets, FeaturesError>,
    /// The scopes of the local builds from `cargo tree`, as the feature sets.
    scopes: PerTarget<BuildScope, ScopeError>,
}

/// Results by target, and when each was made.
type PerTarget<T, E> = Mutex<HashMap<TargetTriple, (Instant, Result<Arc<T>, Arc<E>>)>>;

impl Kept {
    /// Whether the result may be given again: a success while its inputs stay the same, and a
    /// failure for [`FAILURE_TTL`].
    fn current(&self) -> bool {
        matches!(&*self.resolution, Resolution::Metadata { .. }) || self.at.elapsed() < FAILURE_TTL
    }
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
        let cargo_home = super::cargo_home();
        Self {
            offline,
            cargo_home,
            kept: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
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
        let flight = Arc::clone(self.flights.lock().await.entry(root.clone()).or_default());
        let _flying = flight.lock().await;
        let kept = self.kept.lock().await.get(&root).cloned();
        if let Some(kept) = kept.filter(|kept| kept.current()) {
            let files = kept.inputs.files.iter().map(|file| file.path.clone());
            if read_inputs(&kept.inputs_root, files.collect())
                .await
                .as_ref()
                == Some(&kept.inputs)
            {
                return Ok(Arc::clone(&kept.resolution));
            }
        }
        let (lock_root, lock_text, metadata) = match read_lock(&root).await {
            Ok(text) => {
                // Cargo.lock that cannot be parsed fails before cargo is run.
                CargoLock::parse(&text).map_err(ResolveError::Malformed)?;
                let metadata = self.run_metadata(&root).await;
                (root.clone(), text, metadata)
            },
            // An implicit member, such as a path dependency of the root package, has no
            // Cargo.lock of its own: cargo knows its workspace (requirement R-MCP-3).
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let metadata = self.run_metadata(&root).await;
                let above = metadata
                    .as_ref()
                    .ok()
                    .map(|metadata| {
                        dunce::canonicalize(&metadata.workspace_root)
                            .unwrap_or_else(|_| metadata.workspace_root.clone())
                    })
                    .filter(|above| *above != root && root.starts_with(above))
                    .ok_or(ResolveError::NoCargoLock)?;
                let text = read_lock(&above).await.map_err(ResolveError::Unreadable)?;
                (above, text, metadata)
            },
            Err(error) => return Err(ResolveError::Unreadable(error)),
        };
        let lock = CargoLock::parse(&lock_text).map_err(ResolveError::Malformed)?;
        let (resolution, files) = match metadata {
            Ok(metadata) => {
                let files = self.input_files(&lock_root, Some(&metadata));
                (Resolution::Metadata { metadata, lock }, files)
            },
            Err(because) => (
                Resolution::CargoLockRaw { lock, because },
                self.input_files(&lock_root, None),
            ),
        };
        let files = with_included(files, &lock_root, self.cargo_home.clone()).await;
        let resolution = Arc::new(resolution);
        // Kept only if Cargo.lock did not change while cargo ran, so that a result is never kept
        // under inputs newer than it.
        match read_inputs(&lock_root, files).await {
            Some(inputs) if lock_is(&inputs, &lock_root, &lock_text) => {
                self.kept.lock().await.insert(
                    root,
                    Arc::new(Kept {
                        inputs,
                        inputs_root: lock_root,
                        resolution: Arc::clone(&resolution),
                        at: Instant::now(),
                        features: Mutex::new(HashMap::new()),
                        scopes: Mutex::new(HashMap::new()),
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
    ) -> Result<Arc<FeatureSets>, Arc<FeaturesError>> {
        let resolution = self
            .resolve(workspace)
            .await
            .map_err(|error| Arc::new(error.into()))?;
        if let Resolution::CargoLockRaw { .. } = &*resolution {
            return Err(Arc::new(FeaturesError::NoMetadata));
        }
        let kept = self
            .kept
            .lock()
            .await
            .get(&workspace.root)
            .filter(|kept| Arc::ptr_eq(&kept.resolution, &resolution))
            .cloned();
        let offline = self.offline_in(&workspace.root).await;
        let read = async {
            features::read(&workspace.root, offline, target)
                .await
                .map(Arc::new)
                .map_err(|error| Arc::new(FeaturesError::Cargo(error)))
        };
        match kept {
            Some(kept) => per_target(&kept.features, target, read).await,
            None => read.await,
        }
    }

    /// Which packages the query tools may build locally for `target`, an actual build target of
    /// `workspace`, the default project (requirement R-MCP-3). It is kept as the feature sets
    /// are.
    ///
    /// # Errors
    ///
    /// Returns an error when the packages cannot be resolved by `cargo metadata`, or `cargo tree`
    /// fails or does not tell the scope (`scope_unknown`).
    pub async fn scope(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
    ) -> Result<Arc<BuildScope>, Arc<ScopeError>> {
        let resolution = self
            .resolve(workspace)
            .await
            .map_err(|error| Arc::new(error.into()))?;
        let Resolution::Metadata { metadata, .. } = &*resolution else {
            return Err(Arc::new(ScopeError::NoMetadata));
        };
        let kept = self
            .kept
            .lock()
            .await
            .get(&workspace.root)
            .filter(|kept| Arc::ptr_eq(&kept.resolution, &resolution))
            .cloned();
        let offline = self.offline_in(&workspace.root).await;
        let read = async {
            build_scope::read(&workspace.root, offline, target, metadata)
                .await
                .map(Arc::new)
                .map_err(Arc::new)
        };
        match kept {
            Some(kept) => per_target(&kept.scopes, target, read).await,
            None => read.await,
        }
    }

    /// Whether cargo is offline in `root`: run with `--offline` (requirement R-CA-7), or with
    /// `net.offline` in the user's cargo settings, read with cargo's rules (requirement R-WS-7).
    /// Offline, `no matching package named` is a crate missing from the cache, not a misspelled
    /// one.
    async fn offline_in(&self, root: &Path) -> bool {
        if self.offline {
            return true;
        }
        // The environment wins over the files, as in cargo.
        if let Some(value) = std::env::var_os("CARGO_NET_OFFLINE").filter(|value| !value.is_empty())
        {
            return value.eq_ignore_ascii_case("true");
        }
        let (root, home) = (root.to_path_buf(), self.cargo_home.clone());
        blocking(move || {
            Ok(cargo_config::read(&root, home.as_deref(), None)
                .is_ok_and(|config| config.net_offline))
        })
        .await
        .unwrap_or(false)
    }

    /// Runs `cargo metadata` in `root` and reads its output.
    async fn run_metadata(&self, root: &Path) -> Result<Metadata, CargoError> {
        let mut command = cargo::project_command(root, self.offline);
        command.args(["metadata", "--format-version", "1"]);
        let output = cargo::run(command).await?;
        if !output.status.success() {
            return Err(CargoError::Failed {
                command: "metadata",
                failure: cargo::classify(&output, self.offline_in(root).await),
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
    /// `metadata` tells, if it ran: Cargo.lock, the manifests of the root and of every package
    /// that is not from a registry or git, and the cargo and rustup settings that cargo finds from
    /// `root`. A file that `include` in a cargo setting names is not among them.
    fn input_files(&self, root: &Path, metadata: Option<&Metadata>) -> Vec<PathBuf> {
        let mut files = vec![root.join("Cargo.lock"), root.join("Cargo.toml")];
        files.extend(
            metadata
                .into_iter()
                .flat_map(|metadata| &metadata.packages)
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

/// The result for `target` in `results`, kept while it is a success, and a failure for
/// [`FAILURE_TTL`], or else `read`'s, kept. `read` runs only then, and the lock is held while it
/// does, so that the calls at the same time run cargo once.
async fn per_target<T: Send + Sync, E: Send + Sync>(
    results: &PerTarget<T, E>,
    target: &TargetTriple,
    read: impl Future<Output = Result<Arc<T>, Arc<E>>> + Send,
) -> Result<Arc<T>, Arc<E>> {
    let mut results = results.lock().await;
    if let Some((at, result)) = results.get(target)
        && (result.is_ok() || at.elapsed() < FAILURE_TTL)
    {
        return result.clone();
    }
    let result = read.await;
    results.insert(target.clone(), (Instant::now(), result.clone()));
    result
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

/// The text of the Cargo.lock of the workspace at `root`.
async fn read_lock(root: &Path) -> io::Result<String> {
    let path = root.join("Cargo.lock");
    blocking(move || fs::read_to_string(path)).await
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
        // The failure is given again for a while, without running cargo.
        let again = resolver.resolve(&workspace).await.expect("resolved");
        assert!(Arc::ptr_eq(&resolution, &again), "the failure is kept");
    }

    #[tokio::test]
    async fn calls_at_the_same_time_run_cargo_once() {
        let (_dir, workspace) = workspace();
        let resolver = Resolver::new(true);
        let results = resolve_four_at_once(&resolver, &workspace).await;
        let first = &results[0];
        for other in &results[1..] {
            assert!(Arc::ptr_eq(first, other), "one result");
        }
        let target = crate::data::target::build_target();
        let (one, two) = tokio::join!(
            resolver.features(&workspace, &target),
            resolver.features(&workspace, &target),
        );
        assert!(
            Arc::ptr_eq(&one.expect("read"), &two.expect("read")),
            "one cargo tree"
        );
    }

    /// Four resolutions of `workspace` at the same time.
    async fn resolve_four_at_once(
        resolver: &Resolver,
        workspace: &Workspace,
    ) -> Vec<Arc<Resolution>> {
        let all: [_; 4] = tokio::join!(
            resolver.resolve(workspace),
            resolver.resolve(workspace),
            resolver.resolve(workspace),
            resolver.resolve(workspace),
        )
        .into();
        all.into_iter()
            .map(|resolved| resolved.expect("resolved"))
            .collect()
    }

    /// A path dependency of the root package is a member without being listed, and has no
    /// Cargo.lock of its own: cargo's `workspace_root` tells where it is (requirement R-MCP-3).
    #[tokio::test]
    async fn an_implicit_member_is_resolved_in_its_workspace() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path();
        fs::create_dir_all(root.join("src")).expect("made");
        fs::create_dir_all(root.join("sub/src")).expect("made");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"top\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
             [dependencies]\nsub = { path = \"sub\" }\n[workspace]\n",
        )
        .expect("written");
        fs::write(root.join("src/lib.rs"), "").expect("written");
        fs::write(
            root.join("sub/Cargo.toml"),
            MEMBER.replace("\"a\"", "\"sub\""),
        )
        .expect("written");
        fs::write(root.join("sub/src/lib.rs"), "").expect("written");
        let status = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(root)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("cargo runs")
            .status;
        assert!(status.success(), "cargo generate-lockfile");
        // As it is found without cargo: a package of its own.
        let sub = project::workspace_of(&root.join("sub")).expect("a workspace");
        assert!(sub.root.ends_with("sub"));
        let resolution = Resolver::new(true).resolve(&sub).await.expect("resolved");
        let Resolution::Metadata { metadata, lock } = &*resolution else {
            panic!("cargo resolves it: {resolution:?}");
        };
        assert_eq!(
            dunce::canonicalize(&metadata.workspace_root).expect("there"),
            dunce::canonicalize(root).expect("there")
        );
        assert_eq!(lock.packages.len(), 2, "the Cargo.lock of the workspace");
    }

    #[tokio::test]
    async fn net_offline_in_the_cargo_settings_is_offline() {
        let (dir, workspace) = workspace();
        let resolver = Resolver::new(false);
        // Unless the environment of the tests sets it.
        if std::env::var_os("CARGO_NET_OFFLINE").is_some() {
            return;
        }
        assert!(!resolver.offline_in(&workspace.root).await);
        fs::create_dir_all(dir.path().join(".cargo")).expect("made");
        fs::write(
            dir.path().join(".cargo/config.toml"),
            "[net]\noffline = true\n",
        )
        .expect("written");
        assert!(resolver.offline_in(&workspace.root).await);
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
