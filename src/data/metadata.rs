//! The packages of a project as `cargo metadata` resolves them (requirement R-VR-2), kept while
//! the files that decide them do not change, with Cargo.lock read directly when cargo fails.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
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
    flight::{Flights, Joined, Stage},
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

impl PackageId {
    /// The name and the version of the package, from the id of cargo 1.77 and later
    /// (`<source>#<name>@<version>`, or `<source>#<version>` when the name is the last part of
    /// the source's path) or of cargo before it (`<name> <version> (<source>)`).
    #[must_use]
    pub fn name_and_version(&self) -> Option<(&str, &str)> {
        name_and_version(&self.0)
    }

    /// What `cargo rustdoc -p` takes for it: the id, or `<name>@<version>` for an id of cargo
    /// before 1.77, which a newer cargo, as a nightly that the server chose, does not take
    /// (decided in M2's D2). A git source with a query (`?branch=main`) is written without it
    /// and its kind, `file:///r.git#name@1.0.0`: cargo takes a `?` for a glob, which `-p` does not
    /// take (decided in M2's D7, as cargo 1.99.1 does).
    #[must_use]
    pub fn spec(&self) -> Cow<'_, str> {
        if self.0.contains(' ') {
            return self
                .name_and_version()
                .map_or(Cow::Borrowed(&*self.0), |(name, version)| {
                    Cow::Owned(format!("{name}@{version}"))
                });
        }
        match self.0.split_once('#') {
            Some((source, rest)) if source.contains('?') => {
                let url = source.split('?').next().unwrap_or(source);
                let url = url.strip_prefix("git+").unwrap_or(url);
                Cow::Owned(format!("{url}#{rest}"))
            },
            Some(_) | None => Cow::Borrowed(&self.0),
        }
    }

    /// Whether `id`, of either format, is the id of the same package, as far as the name and
    /// the version tell.
    #[must_use]
    pub fn is(&self, id: &str) -> bool {
        *self.0 == *id
            || name_and_version(id).is_some_and(|other| self.name_and_version() == Some(other))
    }
}

/// [`PackageId::name_and_version`] of the text `id`.
fn name_and_version(id: &str) -> Option<(&str, &str)> {
    if let Some((name, rest)) = id.split_once(' ') {
        let version = rest.split_once(' ').map_or(rest, |(version, _)| version);
        return Some((name, version));
    }
    let (source, fragment) = id.rsplit_once('#')?;
    if let Some((name, version)) = fragment.split_once('@') {
        return Some((name, version));
    }
    let path = source.split_once('?').map_or(source, |(path, _)| path);
    let name = path.trim_end_matches('/').rsplit('/').next()?;
    (!name.is_empty()).then_some((name, fragment))
}

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
    /// The names of the features it declares, with those of its optional dependencies.
    #[serde(default, deserialize_with = "feature_names")]
    pub features: BTreeSet<Box<str>>,
}

/// The names of the features of a package: the keys of its `features`.
fn feature_names<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeSet<Box<str>>, D::Error> {
    // Each with what it enables, which is not kept.
    let features: BTreeMap<Box<str>, Vec<Box<str>>> = Deserialize::deserialize(deserializer)?;
    Ok(features.into_keys().collect())
}

/// A target of a package.
#[derive(Debug, Deserialize)]
pub struct Target {
    /// The name: a lib's with `_` for `-`, as its crate's; a bin's as written, with any `-`.
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
    /// The work that resolves the packages ended without a result, as when it panicked.
    #[error("the work that resolves the packages stopped")]
    Stopped,
}

/// The packages of a workspace, resolved for the calls that wanted them at once, or why they
/// could not be.
pub type Resolved = Result<Arc<Resolution>, Arc<ResolveError>>;

/// Why the feature sets of a workspace are not known.
#[derive(Debug, thiserror::Error)]
pub enum FeaturesError {
    /// The packages cannot be resolved.
    #[error(transparent)]
    Resolve(#[from] Arc<ResolveError>),
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
///
/// Calls for the same workspace at the same time share the work, which runs cargo once
/// (requirement R-VR-2). The work goes on when a call stops waiting for it at its deadline, as the
/// first `cargo metadata` of a project may fetch its dependencies for long (requirement R-WS-6).
///
/// The feature sets and the scopes that `cargo tree` tells are work shared the same way, by
/// workspace and target, which a call may stop waiting for at its deadline (requirement
/// R-ARCH-6): cargo may wait for its package cache, which rust-analyzer holds.
#[derive(Debug)]
pub struct Resolver {
    shared: Shared,
    /// The feature sets being read, by workspace root and target.
    featuring: Flights<Featured>,
    /// What [`Self::outside_scope_until`] is finding, by build.
    outsiding: Flights<Outsides>,
}

/// What the work of a [`Resolver`] shares, which it takes along.
#[derive(Debug, Clone)]
struct Shared {
    inner: Arc<Inner>,
    /// The resolutions being made, by workspace root.
    resolving: Arc<Flights<Resolved>>,
    /// The scopes being read, by workspace root and target.
    scoping: Arc<Flights<Scoped>>,
    /// What [`Resolver::outside_scope_until`] found, with the resolution that it is of.
    outside: Arc<parking_lot::Mutex<Outside>>,
}

/// The feature sets of a workspace's build for a target, or why they are not known.
pub type Featured = Result<Arc<FeatureSets>, Arc<FeaturesError>>;

/// The scope of a workspace's local builds for a target, or why it is not known.
pub type Scoped = Result<Arc<BuildScope>, Arc<ScopeError>>;

/// The packages that a member's build would build outside the scope, or why they are not known.
pub type Outsides = Result<Vec<Box<str>>, Arc<ScopeError>>;

/// What [`Resolver::outside_scope_until`] found, by the build it looked at, with the resolution that it
/// is of.
type Outside = HashMap<OutsideKey, (Arc<Resolution>, Vec<Box<str>>)>;

/// A build that [`Resolver::outside_scope_until`] looked at: the workspace's root, the target, the
/// member's package id and the features.
type OutsideKey = (PathBuf, Box<str>, Box<str>, Vec<Box<str>>);

/// What the work of a [`Resolver`] shares.
#[derive(Debug)]
struct Inner {
    /// Whether cargo runs with `--offline` (requirement R-CA-7).
    offline: bool,
    /// `$CARGO_HOME`, whose `config.toml` applies to every project.
    cargo_home: Option<PathBuf>,
    /// The kept results, by workspace root.
    kept: Mutex<HashMap<PathBuf, Arc<Kept>>>,
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
    /// same inputs decide them.
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
        Self {
            shared: Shared {
                inner: Arc::new(Inner {
                    offline,
                    cargo_home: super::cargo_home(),
                    kept: Mutex::new(HashMap::new()),
                }),
                resolving: Arc::default(),
                scoping: Arc::default(),
                outside: Arc::default(),
            },
            featuring: Flights::default(),
            outsiding: Flights::default(),
        }
    }

    /// `$CARGO_HOME`, whose settings apply to every project.
    #[must_use]
    pub fn cargo_home(&self) -> Option<&Path> {
        self.shared.inner.cargo_home.as_deref()
    }

    /// The files that `cargo metadata` of the workspace at `root`, which `metadata` tells, reads:
    /// Cargo.lock, the manifests of the packages that are not from a registry or git, the cargo
    /// settings with what they include, and `rust-toolchain` (requirement R-WS-4): what a member's
    /// build is made from besides its sources (decided in M2's D4). They may not exist.
    pub async fn input_files(&self, root: &Path, metadata: &Metadata) -> Vec<PathBuf> {
        let files = self.shared.inner.input_files(root, Some(metadata));
        with_included(files, root, self.shared.inner.cargo_home.clone()).await
    }

    /// Whether cargo is offline in the project at `root`: run with `--offline`, or with
    /// `net.offline` in the user's cargo settings (requirement R-WS-7).
    pub async fn offline_in(&self, root: &Path) -> bool {
        self.shared.inner.offline_in(root).await
    }

    /// Forgets the failures kept for the workspace at `root`: of `cargo metadata`, and of `cargo
    /// tree` for each target, as the argument `refresh` asks (§6.4, decided in M2's D5). What
    /// worked is kept.
    pub async fn forget_failures(&self, root: &Path) {
        let kept = self.shared.inner.kept.lock().await.get(root).cloned();
        let Some(kept) = kept else {
            return;
        };
        if !matches!(&*kept.resolution, Resolution::Metadata { .. }) {
            tracing::info!(root = %root.display(), "the failure of cargo metadata is forgotten, as asked");
            let mut all = self.shared.inner.kept.lock().await;
            if all.get(root).is_some_and(|now| Arc::ptr_eq(now, &kept)) {
                all.remove(root);
            }
            drop(all);
            return;
        }
        let features = kept
            .features
            .lock()
            .await
            .extract_if(|_, (_, result)| result.is_err())
            .count();
        let scopes = kept
            .scopes
            .lock()
            .await
            .extract_if(|_, (_, result)| result.is_err())
            .count();
        if features > 0 || scopes > 0 {
            tracing::info!(root = %root.display(), features, scopes, "the failures of cargo tree are forgotten, as asked");
        }
    }

    /// The packages of `workspace`, as [`Self::resolve_until`] gives them, waiting for them as
    /// long as it takes.
    ///
    /// # Errors
    ///
    /// Returns an error when Cargo.lock is missing or cannot be read, or the work stopped.
    pub async fn resolve(&self, workspace: &Workspace) -> Resolved {
        self.shared.resolve(workspace).await
    }

    /// The packages of `workspace`, by `cargo metadata`, or from its Cargo.lock when cargo fails
    /// (requirement R-VR-2), waiting for them until `until`. The work then goes on, and
    /// [`Joined::Going`] says what cargo does: fetching the dependencies, or waiting for a lock
    /// (requirements R-WS-6 and R-OUT-9).
    ///
    /// A result of `cargo metadata` is used again while the files it was made from have the same
    /// contents: Cargo.lock, the `Cargo.toml` of every member and path dependency, the cargo and
    /// rustup settings that apply, and what the `members` patterns match.
    pub async fn resolve_until(
        &self,
        workspace: &Workspace,
        until: tokio::time::Instant,
    ) -> Joined<Resolved> {
        let (key, what, start) = self.shared.work(workspace);
        self.shared.resolving.join(&key, &what, until, start).await
    }

    /// The features that the normal build of `workspace` for `target`, the actual build target,
    /// enables on each package (requirements R-FT-2 and R-FT-3), as [`Self::features_until`]
    /// tells, waiting for them as long as it takes.
    ///
    /// # Errors
    ///
    /// Returns an error when the packages cannot be resolved by `cargo metadata`, or `cargo tree`
    /// fails, or the work stopped.
    pub async fn features(&self, workspace: &Workspace, target: &TargetTriple) -> Featured {
        let (key, what, start) = self.features_work(workspace, target);
        self.featuring
            .wait(&key, &what, start)
            .await
            .unwrap_or_else(|| Err(Arc::new(Arc::new(ResolveError::Stopped).into())))
    }

    /// The features that the normal build of `workspace` for `target` enables on each package,
    /// waiting for them until `until`: the work then goes on, as [`Self::resolve_until`]'s does.
    /// They are kept with the result of `cargo metadata`, while the same inputs decide both.
    pub async fn features_until(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
        until: tokio::time::Instant,
    ) -> Joined<Featured> {
        let (key, what, start) = self.features_work(workspace, target);
        self.featuring.join(&key, &what, until, start).await
    }

    /// The key, the name and the start of the work that reads the feature sets of `workspace` for
    /// `target`.
    #[expect(
        clippy::type_complexity,
        reason = "the start of the work is a closure that only its two callers take"
    )]
    fn features_work(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
    ) -> (
        String,
        String,
        impl FnOnce(Arc<Stage>) -> std::pin::Pin<Box<dyn Future<Output = Featured> + Send>>,
    ) {
        let key = format!("features:{}:{target}", workspace.root.display());
        let what = format!(
            "the features of the build of {} for {target}",
            workspace.root.display()
        );
        let (shared, workspace, target) = (self.shared.clone(), workspace.clone(), target.clone());
        let start =
            move |stage: Arc<Stage>| -> std::pin::Pin<Box<dyn Future<Output = Featured> + Send>> {
                Box::pin(async move {
                    cargo::staged(stage, Box::pin(shared.features(&workspace, &target))).await
                })
            };
        (key, what, start)
    }

    /// The packages that a build of the member `package` of `workspace` for `target` with
    /// `--no-default-features --features <features>` would build outside the scope of the
    /// workspace, as `name version` (requirement R-MCP-3, decided in M2's D3), waiting for them
    /// until `until`: the work then goes on, as [`Self::resolve_until`]'s does.
    pub async fn outside_scope_until(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
        (package, features): (&PackageId, &[Box<str>]),
        until: tokio::time::Instant,
    ) -> Joined<Outsides> {
        let key = format!(
            "outside:{}:{target}:{}:{}",
            workspace.root.display(),
            package.0,
            features.join(",")
        );
        let what = format!("the build of {} with its features", package.0);
        let (shared, workspace, target, package, features) = (
            self.shared.clone(),
            workspace.clone(),
            target.clone(),
            package.clone(),
            features.to_vec(),
        );
        let start =
            move |stage: Arc<Stage>| -> std::pin::Pin<Box<dyn Future<Output = Outsides> + Send>> {
                Box::pin(async move {
                    cargo::staged(
                        stage,
                        shared.outside_scope(&workspace, &target, &package, &features),
                    )
                    .await
                })
            };
        self.outsiding.join(&key, &what, until, start).await
    }

    /// Which packages the query tools may build locally for `target`, an actual build target of
    /// `workspace`, the default project (requirement R-MCP-3). It is kept as the feature sets
    /// are.
    ///
    /// # Errors
    ///
    /// Returns an error when the packages cannot be resolved by `cargo metadata`, or `cargo tree`
    /// fails or does not tell the scope (`scope_unknown`).
    pub async fn scope(&self, workspace: &Workspace, target: &TargetTriple) -> Scoped {
        self.shared.scope(workspace, target).await
    }

    /// [`Self::scope`], waiting for it until `until`: the work then goes on, as
    /// [`Self::resolve_until`]'s does (decided in M2's D7, for a dependency's build).
    pub async fn scope_until(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
        until: tokio::time::Instant,
    ) -> Joined<Scoped> {
        let (key, what, start) = self.shared.scope_work(workspace, target);
        self.shared.scoping.join(&key, &what, until, start).await
    }
}

impl Shared {
    /// [`Resolver::resolve`].
    ///
    /// In the work of [`Resolver::features_until`] or [`Resolver::outside_scope_until`], what
    /// cargo does for it is told to their callers.
    async fn resolve(&self, workspace: &Workspace) -> Resolved {
        let (key, what, start) = self.work(workspace);
        let telling = cargo::current_stage();
        self.resolving
            .wait_telling(&key, &what, telling.as_deref(), start)
            .await
            .unwrap_or_else(|| Err(Arc::new(ResolveError::Stopped)))
    }

    /// The key, the name and the start of the work that resolves `workspace`.
    #[expect(
        clippy::type_complexity,
        reason = "the start of the work is a closure that only its two callers take"
    )]
    fn work(
        &self,
        workspace: &Workspace,
    ) -> (
        String,
        String,
        impl FnOnce(Arc<Stage>) -> std::pin::Pin<Box<dyn Future<Output = Resolved> + Send>>,
    ) {
        let key = workspace.root.to_string_lossy().into_owned();
        let what = format!("the dependencies of {}", workspace.root.display());
        let (inner, workspace) = (Arc::clone(&self.inner), workspace.clone());
        let start =
            move |stage: Arc<Stage>| -> std::pin::Pin<Box<dyn Future<Output = Resolved> + Send>> {
                Box::pin(async move { inner.resolve(&workspace, &stage).await.map_err(Arc::new) })
            };
        (key, what, start)
    }

    /// The work of [`Resolver::features`].
    async fn features(&self, workspace: &Workspace, target: &TargetTriple) -> Featured {
        let resolution = self
            .resolve(workspace)
            .await
            .map_err(|error| Arc::new(error.into()))?;
        if let Resolution::CargoLockRaw { .. } = &*resolution {
            return Err(Arc::new(FeaturesError::NoMetadata));
        }
        let kept = self.kept(workspace, &resolution).await;
        let offline = self.inner.offline_in(&workspace.root).await;
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

    /// What is kept with `resolution` of `workspace`, if it is still the one kept.
    async fn kept(&self, workspace: &Workspace, resolution: &Arc<Resolution>) -> Option<Arc<Kept>> {
        self.inner
            .kept
            .lock()
            .await
            .get(&workspace.root)
            .filter(|kept| Arc::ptr_eq(&kept.resolution, resolution))
            .cloned()
    }

    /// The work of [`Resolver::outside_scope_until`].
    async fn outside_scope(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
        package: &PackageId,
        features: &[Box<str>],
    ) -> Outsides {
        let scope = self.scope(workspace, target).await?;
        let resolution = self
            .resolve(workspace)
            .await
            .map_err(|error| Arc::new(error.into()))?;
        let Resolution::Metadata { metadata, .. } = &*resolution else {
            return Err(Arc::new(ScopeError::NoMetadata));
        };
        let Some(package) = metadata.packages.iter().find(|found| found.id == *package) else {
            return Err(Arc::new(ScopeError::Unknown(
                format!("{} is not in the workspace", package.0).into(),
            )));
        };
        // Kept while the resolution is, as the scope is: `cargo tree` runs once for them.
        let key = (
            workspace.root.clone(),
            target.as_str().into(),
            package.id.0.clone(),
            features.to_vec(),
        );
        let found = self
            .outside
            .lock()
            .get(&key)
            .filter(|(of, _)| Arc::ptr_eq(of, &resolution))
            .map(|(_, outside)| outside.clone());
        if let Some(outside) = found {
            return Ok(outside);
        }
        let offline = self.inner.offline_in(&workspace.root).await;
        let outside = build_scope::outside(
            (&workspace.root, offline, target),
            metadata,
            &scope,
            package,
            features,
        )
        .await
        .map_err(Arc::new)?;
        let mut kept = self.outside.lock();
        // What is of a resolution that nothing else holds is of one that changed.
        kept.retain(|_, (of, _)| Arc::strong_count(of) > 1);
        kept.insert(key, (Arc::clone(&resolution), outside.clone()));
        drop(kept);
        Ok(outside)
    }

    /// [`Resolver::scope`], as work shared by workspace and target.
    async fn scope(&self, workspace: &Workspace, target: &TargetTriple) -> Scoped {
        let (key, what, start) = self.scope_work(workspace, target);
        // What cargo does for it is told to the callers of the work that waits for it, as above.
        let telling = cargo::current_stage();
        self.scoping
            .wait_telling(&key, &what, telling.as_deref(), start)
            .await
            .unwrap_or_else(|| Err(Arc::new(Arc::new(ResolveError::Stopped).into())))
    }

    /// The key, the name and the start of the work that reads the scope of the builds of
    /// `workspace` for `target`.
    #[expect(
        clippy::type_complexity,
        reason = "the start of the work is a closure that only its two callers take"
    )]
    fn scope_work(
        &self,
        workspace: &Workspace,
        target: &TargetTriple,
    ) -> (
        String,
        String,
        impl FnOnce(Arc<Stage>) -> std::pin::Pin<Box<dyn Future<Output = Scoped> + Send>>,
    ) {
        let key = format!("scope:{}:{target}", workspace.root.display());
        let what = format!(
            "the scope of the builds of {} for {target}",
            workspace.root.display()
        );
        let (shared, workspace, target) = (self.clone(), workspace.clone(), target.clone());
        let start =
            move |stage: Arc<Stage>| -> std::pin::Pin<Box<dyn Future<Output = Scoped> + Send>> {
                Box::pin(async move {
                    cargo::staged(stage, Box::pin(shared.read_scope(&workspace, &target))).await
                })
            };
        (key, what, start)
    }

    /// The work of [`Self::scope`].
    async fn read_scope(&self, workspace: &Workspace, target: &TargetTriple) -> Scoped {
        let resolution = self
            .resolve(workspace)
            .await
            .map_err(|error| Arc::new(error.into()))?;
        let Resolution::Metadata { metadata, .. } = &*resolution else {
            return Err(Arc::new(ScopeError::NoMetadata));
        };
        let kept = self.kept(workspace, &resolution).await;
        let offline = self.inner.offline_in(&workspace.root).await;
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
}

impl Inner {
    /// The packages of `workspace`, as [`Resolver::resolve_until`] tells, telling `stage` what
    /// cargo does.
    async fn resolve(
        &self,
        workspace: &Workspace,
        stage: &Stage,
    ) -> Result<Arc<Resolution>, ResolveError> {
        let root = workspace.root.clone();
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
        let (lock_root, (lock_text, lock), metadata) = match read_lock(&root).await {
            Ok((text, lock)) => {
                // Cargo.lock that cannot be parsed fails before cargo is run.
                let lock = lock.map_err(ResolveError::Malformed)?;
                let metadata = self.run_metadata(&root, stage).await;
                (root.clone(), (text, Ok(lock)), metadata)
            },
            // An implicit member, such as a path dependency of the root package, has no
            // Cargo.lock of its own: cargo knows its workspace (requirement R-MCP-3).
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let metadata = self.run_metadata(&root, stage).await;
                let above = metadata
                    .as_ref()
                    .ok()
                    .map(|metadata| {
                        dunce::canonicalize(&metadata.workspace_root)
                            .unwrap_or_else(|_| metadata.workspace_root.clone())
                    })
                    .filter(|above| *above != root && root.starts_with(above))
                    .ok_or(ResolveError::NoCargoLock)?;
                let read = read_lock(&above).await.map_err(ResolveError::Unreadable)?;
                (above, read, metadata)
            },
            Err(error) => return Err(ResolveError::Unreadable(error)),
        };
        let lock = lock.map_err(ResolveError::Malformed)?;
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

    /// Runs `cargo metadata` in `root` and reads its output, telling `stage` whether cargo waits
    /// for a lock or fetches (requirement R-OUT-9).
    async fn run_metadata(&self, root: &Path, stage: &Stage) -> Result<Metadata, CargoError> {
        let mut command = cargo::project_command(root, self.offline, None);
        command.args(["metadata", "--format-version", "1"]);
        let output =
            cargo::run_reporting(command, |activity| stage.set(cargo::doing_of(activity))).await?;
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
/// [`FAILURE_TTL`], or else `read`'s, kept. `read` runs only then, without the lock: the calls at
/// the same time share the work that runs it, by workspace and target.
async fn per_target<T: Send + Sync, E: Send + Sync>(
    results: &PerTarget<T, E>,
    target: &TargetTriple,
    read: impl Future<Output = Result<Arc<T>, Arc<E>>> + Send,
) -> Result<Arc<T>, Arc<E>> {
    let kept = results
        .lock()
        .await
        .get(target)
        .filter(|(at, result)| result.is_ok() || at.elapsed() < FAILURE_TTL)
        .map(|(_, result)| result.clone());
    if let Some(result) = kept {
        return result;
    }
    let result = read.await;
    results
        .lock()
        .await
        .insert(target.clone(), (Instant::now(), result.clone()));
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
                // Only a regular file: a FIFO would not end.
                let contents = fs::metadata(&path)
                    .is_ok_and(|metadata| metadata.is_file())
                    .then(|| fs::read(&path).ok())
                    .flatten()
                    .map(Vec::into_boxed_slice);
                InputFile { path, contents }
            })
            .collect();
        Ok(project::expand_members(&root).map(|members| Inputs { files, members }))
    })
    .await
    .ok()
    .flatten()
}

/// The text of the Cargo.lock of the workspace at `root`, and what it reads as.
async fn read_lock(root: &Path) -> io::Result<(String, Result<CargoLock, toml::de::Error>)> {
    let path = root.join("Cargo.lock");
    // Megabytes for a large workspace: read and parsed off the async threads.
    blocking(move || {
        let text = fs::read_to_string(path)?;
        let lock = CargoLock::parse(&text);
        Ok((text, lock))
    })
    .await
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

    #[test]
    fn a_package_id_of_either_format_tells_its_name_and_version() {
        let id = |id: &str| PackageId(id.into());
        let registry = id("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228");
        assert_eq!(registry.name_and_version(), Some(("serde", "1.0.228")));
        assert_eq!(registry.spec(), registry.0.as_ref());
        // The name is the last part of the path when it is the same.
        let path = id("path+file:///ws/one#0.1.0");
        assert_eq!(path.name_and_version(), Some(("one", "0.1.0")));
        let git = id("git+https://github.com/o/repo?branch=main#tool@0.2.0");
        assert_eq!(git.name_and_version(), Some(("tool", "0.2.0")));
        // `-p` takes it without its query, which cargo takes for a glob, and its kind.
        assert_eq!(git.spec(), "https://github.com/o/repo#tool@0.2.0");
        // Without a query, as with no branch, tag or rev, it is taken as it is.
        let plain = id("git+https://github.com/o/repo#tool@0.2.0");
        assert_eq!(plain.spec(), plain.0.as_ref());
        // That of cargo before 1.77, which `-p` of a newer cargo does not take.
        let old = id("one 0.1.0 (path+file:///ws/one)");
        assert_eq!(old.name_and_version(), Some(("one", "0.1.0")));
        assert_eq!(old.spec(), "one@0.1.0");
        assert!(old.is("path+file:///ws/one#0.1.0"));
        assert!(path.is(&path.0));
        assert!(!path.is("path+file:///ws/one#0.2.0"));
        assert!(!path.is("registry+https://github.com/rust-lang/crates.io-index#two@0.1.0"));
    }

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
        // `refresh` keeps what worked (decided in M2's D5).
        resolver.forget_failures(&workspace.root).await;
        let refreshed = resolver.resolve(&workspace).await.expect("resolved");
        assert!(Arc::ptr_eq(&first, &refreshed), "still kept");

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
        // Until `refresh` forgets it (decided in M2's D5): cargo runs again.
        resolver.forget_failures(&workspace.root).await;
        let refreshed = resolver.resolve(&workspace).await.expect("resolved");
        assert!(!Arc::ptr_eq(&resolution, &refreshed), "run again");
        assert!(matches!(&*refreshed, Resolution::CargoLockRaw { .. }));
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
        assert!(!resolver.shared.inner.offline_in(&workspace.root).await);
        fs::create_dir_all(dir.path().join(".cargo")).expect("made");
        fs::write(
            dir.path().join(".cargo/config.toml"),
            "[net]\noffline = true\n",
        )
        .expect("written");
        assert!(resolver.shared.inner.offline_in(&workspace.root).await);
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
        let names = |set: Option<&BTreeSet<Box<str>>>| {
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
        let resolved = Resolver::new(true).resolve(&workspace).await;
        assert!(
            matches!(
                resolved.as_ref().map_err(|error| &**error),
                Err(ResolveError::NoCargoLock)
            ),
            "{resolved:?}"
        );
    }

    /// The environment variable that tells [`resolve_while_cargo_waits_for_a_lock`] the cargo
    /// home that its cargo uses.
    const HOME: &str = "CHATORA_RUSTDOC_TEST_CARGO_HOME";

    /// Not a test by itself: a process that
    /// [`a_call_that_stops_waiting_tells_that_cargo_waits_for_a_lock`] starts with `CARGO_HOME`
    /// set, which holds the lock of the package cache while it resolves a workspace.
    #[tokio::test]
    #[ignore = "started by a_call_that_stops_waiting_tells_that_cargo_waits_for_a_lock"]
    async fn resolve_while_cargo_waits_for_a_lock() {
        let Some(home) = std::env::var_os(HOME) else {
            return;
        };
        let (_dir, workspace) = workspace();
        // Another cargo, such as rust-analyzer's, holds the package cache.
        let lock = fs::File::create(Path::new(&home).join(".package-cache")).expect("made");
        lock.lock().expect("locked");
        let resolver = Resolver::new(true);
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .expect("in range");
        loop {
            let soon = tokio::time::Instant::now()
                .checked_add(Duration::from_millis(200))
                .expect("in range");
            match resolver.resolve_until(&workspace, soon).await {
                Joined::Going(crate::data::flight::Doing::WaitingForLock) => break,
                // cargo has not told it yet.
                Joined::Going(_) => {},
                other @ (Joined::Done(_) | Joined::Ended) => panic!("{other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "cargo did not tell that it waits"
            );
        }
        lock.unlock().expect("unlocked");
        // The work that went on gives the result.
        let resolution = resolver.resolve(&workspace).await.expect("resolved");
        assert!(matches!(*resolution, Resolution::Metadata { .. }));
        // So does `cargo tree`, for the features.
        let target = crate::data::target::build_target();
        lock.lock().expect("locked");
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .expect("in range");
        loop {
            let soon = tokio::time::Instant::now()
                .checked_add(Duration::from_millis(200))
                .expect("in range");
            match resolver.features_until(&workspace, &target, soon).await {
                Joined::Going(crate::data::flight::Doing::WaitingForLock) => break,
                Joined::Going(_) => {},
                other @ (Joined::Done(_) | Joined::Ended) => panic!("{other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "cargo did not tell that it waits"
            );
        }
        lock.unlock().expect("unlocked");
        resolver.features(&workspace, &target).await.expect("read");
    }

    /// While cargo waits for the lock of the package cache past the deadline of a call, the call
    /// tells so, and cargo goes on to resolve the workspace once the lock is free (requirements
    /// R-WS-2, R-WS-6 and R-OUT-9).
    #[test]
    fn a_call_that_stops_waiting_tells_that_cargo_waits_for_a_lock() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let env = [
            ("CARGO_HOME", home.path().as_os_str()),
            (HOME, home.path().as_os_str()),
        ];
        let child = crate::test_process::start(
            "data::metadata::tests::resolve_while_cargo_waits_for_a_lock",
            &env,
        );
        crate::test_process::finish(child);
    }
}
