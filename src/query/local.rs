//! Loading a workspace member's crate for queries: built locally with `cargo rustdoc` by work
//! that the calls for it share, and archived (requirements R-WS-1, R-WS-6 and R-WS-8, decided in
//! M2's D2).

use std::{
    collections::HashSet,
    fs,
    io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use parking_lot::Mutex;
use tokio::time::Instant;

use super::load::{self, Archived, LoadError};
use crate::data::{
    build::{self, BuildError, BuildRecord, Built, Documented, Plan},
    cache::{self, Cache},
    cargo::Failure,
    flight::{Doing, Flights, Joined, Stage},
    freshness,
    owner::Owner,
    rustdoc_json,
    target::TargetTriple,
    toolchain::{self, ChooseError, Chosen, Kind, Mismatch, Missing, Programs, ProgramsError},
};

/// What a member's build is made for.
#[derive(Debug, Clone)]
pub struct LocalWanted {
    /// The plan of the build. The work decides its toolchain and whether it passes `--target`.
    pub plan: Plan,
    /// The target asked for.
    pub target: TargetTriple,
    /// Whether the project sets `build.target`, so that `--target` is passed even for the host
    /// (requirement R-WS-1).
    pub project_sets_target: bool,
    /// `$CARGO_HOME`, whose settings apply to the project.
    pub cargo_home: Option<PathBuf>,
    /// The features of the member's `[package.metadata.docs.rs]`, built with first, when they
    /// build nothing outside the project's build; the plan's are the project's, to go back to
    /// when they break the build (requirements R-FT-3 and R-MCP-3, decided in M2's D3).
    pub docs_rs_features: Option<Vec<Box<str>>>,
    /// What the response warns about how the build was planned.
    pub warnings: Vec<Box<str>>,
    /// The files that the build is made from, which may not exist: the workspace's manifests,
    /// Cargo.lock and the cargo settings (requirement R-WS-4, decided in M2's D4).
    pub input_files: Vec<PathBuf>,
    /// The directories of the member and of the path dependencies that it builds with, whose
    /// files the build is made from.
    pub input_dirs: Vec<PathBuf>,
}

/// The archive of a member's build, and its record.
#[derive(Debug)]
pub struct LocalBuilt {
    /// The archive, and what the response warns about.
    pub archived: Archived,
    /// The JSON in the cache.
    pub json: PathBuf,
    /// The record of the build, for the provenance (requirement R-OUT-9).
    pub record: BuildRecord,
}

/// Why a member's crate could not be built and archived.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    /// A program of the build settings cannot be used.
    #[error(transparent)]
    Programs(&'static ProgramsError),
    /// There is no work directory, as the cache could not be locked.
    #[error("there is no work directory for the build")]
    Work(#[source] io::Error),
    /// The project's rustc and rustdoc are of different toolchains.
    #[error(transparent)]
    Mismatch(Mismatch),
    /// No toolchain can build it (decided in M2's D1).
    #[error(transparent)]
    Choose(ChooseError),
    /// The toolchain cannot build for the target.
    #[error(transparent)]
    Missing(Missing),
    /// The build failed.
    #[error(transparent)]
    Build(BuildError),
    /// The JSON could not be read, parsed or archived.
    #[error(transparent)]
    Load(LoadError),
    /// The work was stopped, as the server is shutting down.
    #[error("the work that builds the documentation was stopped")]
    Stopped,
}

/// The build of a member for the calls that wanted it at once, or why there is none.
pub type SharedLocal = Result<Arc<LocalBuilt>, Arc<LocalError>>;

/// Whether a member's build that went on past a deadline is handed to the next call for it: only
/// a build without a key, which the next call cannot find fresh, and would make again (decided in
/// M2's D4).
#[must_use]
pub fn hands_over(built: &SharedLocal) -> bool {
    built.as_ref().is_ok_and(|built| built.record.key.is_none())
}

/// The archive of the member's crate that `wanted` names, built and archived by work that the
/// calls for it share (requirements R-WS-6 and R-NW-2).
///
/// A call waits for the work until `until`. The work then goes on, and [`Joined::Going`] says what
/// it is doing (requirements R-ARCH-6 and R-OUT-9). A build that is fresh, as its key tells, is
/// not made again (requirement R-WS-4, decided in M2's D4): a build that went on is found so by
/// the next call, or handed to it, when it has no key ([`hands_over`]).
pub async fn local_archive(
    flights: &Flights<SharedLocal>,
    cache: &Cache,
    wanted: LocalWanted,
    until: Instant,
) -> Joined<SharedLocal> {
    let documented = match &wanted.plan.documented {
        Documented::Lib { .. } => "lib".to_owned(),
        Documented::Bin { name } => format!("bin {name}"),
    };
    let key = format!(
        "local:{}:{}:{}:{documented}",
        wanted.plan.root.display(),
        wanted.plan.package.0,
        wanted.target
    );
    let what = format!("{} {}", wanted.plan.crate_name, wanted.plan.version);
    let cache = cache.clone();
    flights
        .join(&key, &what, until, async move |stage| {
            build_and_archive(&cache, wanted, &stage)
                .await
                .map(Arc::new)
                .map_err(Arc::new)
        })
        .await
}

/// Checks the toolchain, builds, and archives the JSON, telling `stage` what it does.
async fn build_and_archive(
    cache: &Cache,
    wanted: LocalWanted,
    stage: &Stage,
) -> Result<LocalBuilt, LocalError> {
    stage.set(Doing::Building);
    if let Some(problem) = Programs::problem() {
        return Err(LocalError::Programs(problem));
    }
    let work = Owner::this()
        .ok_or_else(|| io::Error::other("this server keeps no work directories"))
        .and_then(Owner::work_dirs)
        .map_err(LocalError::Work)?;
    let LocalWanted {
        mut plan,
        target,
        project_sets_target,
        cargo_home,
        docs_rs_features,
        mut warnings,
        input_files,
        input_dirs,
    } = wanted;
    let root = plan.root.clone();
    let programs = Programs::this();
    let rustc = programs.used_in(Kind::Rustc, &root, cargo_home.as_deref());
    let rustdoc = programs.used_in(Kind::Rustdoc, &root, cargo_home.as_deref());
    toolchain::same_toolchain(&rustc, &rustdoc, &root)
        .await
        .map_err(LocalError::Mismatch)?;
    let choose = async || toolchain::choose(&root, cache, cargo_home.as_deref(), &work).await;
    // Online, cargo installs the project's toolchain as it builds (decided in M2's D1).
    let mut installs = false;
    match choose().await {
        Ok(Chosen::Project { .. }) => {},
        Ok(Chosen::Nightly { toolchain, .. }) => plan.toolchain = Some(toolchain),
        Err(ChooseError::NotInstalled(missing)) if !plan.offline => {
            tracing::info!(%missing, "cargo is to install the project's toolchain as it builds");
            installs = true;
        },
        Err(error) => return Err(LocalError::Choose(error)),
    }
    // As asked, for the build with the nightly too, whatever the first build did.
    let (cfg_docsrs, features) = (plan.cfg_docsrs, plan.features.clone());
    let builder = Builder {
        cache,
        input_files: &input_files,
        input_dirs: &input_dirs,
        rustc: &rustc,
        target: &target,
        project_sets_target,
        work: &work,
        stage,
        docs_rs: docs_rs_features.as_deref(),
    };
    let planned = warnings.len();
    let built = builder.build(&mut plan, installs, &mut warnings).await;
    let mut built = if installs {
        after_install(cache, &rustc, &root, &target, built).await?
    } else {
        built?
    };
    // The toolchain that cargo installed writes too old a format: the nightly builds (decided in
    // M2's D2).
    if installs && built.record.format_version < *rustdoc_json::SUPPORTED.start() {
        built.unpublish();
        match choose().await {
            Ok(Chosen::Nightly { toolchain, .. }) => {
                plan.toolchain = Some(toolchain);
                (plan.cfg_docsrs, plan.features) = (cfg_docsrs, features);
                // What the build that is not kept told.
                warnings.truncate(planned);
                built = builder.build(&mut plan, false, &mut warnings).await?;
            },
            Ok(Chosen::Project { format }) => {
                return Err(LocalError::Choose(ChooseError::TooOld { found: format }));
            },
            Err(error) => return Err(LocalError::Choose(error)),
        }
    }
    stage.set(Doing::Indexing);
    archive(cache, built, warnings).await
}

/// After a build in which cargo was to install the project's toolchain: what rustup said of it
/// before is forgotten, and it is checked now (decided in M2's D2). A failed build of a toolchain
/// that is still missing, or lacks the target, is told as that; a build is recorded with the
/// toolchain, in `cache`.
async fn after_install(
    cache: &Cache,
    rustc: &Path,
    root: &Path,
    target: &TargetTriple,
    built: Result<Built, LocalError>,
) -> Result<Built, LocalError> {
    toolchain::forget_project(root);
    let checked = toolchain::check_target(rustc, None, root, target).await;
    match (built, checked) {
        (Err(LocalError::Build(_)), Err(missing)) => Err(LocalError::Missing(missing)),
        (Ok(mut built), Ok(checked)) => {
            built.record.toolchain = checked.toolchain.map(Into::into);
            if let Err(error) = built.write_record(cache).await {
                tracing::warn!(%error, "the record of a build could not be written again");
            }
            Ok(built)
        },
        (built, _) => built,
    }
}

/// What the builds of a member are made with.
struct Builder<'a> {
    cache: &'a Cache,
    /// The files that the member's build is made from, and the directories of them (decided in
    /// M2's D4).
    input_files: &'a [PathBuf],
    input_dirs: &'a [PathBuf],
    rustc: &'a Path,
    /// The target asked for.
    target: &'a TargetTriple,
    project_sets_target: bool,
    work: &'a Path,
    stage: &'a Stage,
    /// The features of the member's `[package.metadata.docs.rs]`, to build with first.
    docs_rs: Option<&'a [Box<str>]>,
}

impl Builder<'_> {
    /// Builds `plan`, deciding whether it passes `--target` by the host of its toolchain, which
    /// is checked first unless cargo is to install it.
    ///
    /// It builds with the features of `[package.metadata.docs.rs]` first, if any, and `--cfg
    /// docsrs`, unless they broke the build before. When the documented crate fails, it builds
    /// without `--cfg docsrs`; when that fails too, or a dependency fails, with the project's
    /// features: three builds at most (decided in M2's D3). What broke the build is remembered
    /// only once a build works without it, so that an error of the crate's own, which fails
    /// them all, is not taken for it. `warnings` gets what the response tells of it.
    async fn build(
        &self,
        plan: &mut Plan,
        installs: bool,
        warnings: &mut Vec<Box<str>>,
    ) -> Result<Built, LocalError> {
        let checked = if installs {
            None
        } else {
            Some(
                toolchain::check_target(
                    self.rustc,
                    plan.toolchain.as_deref(),
                    &plan.root,
                    self.target,
                )
                .await
                .map_err(LocalError::Missing)?,
            )
        };
        let host = checked.as_ref().and_then(|checked| checked.host.clone());
        plan.target = build::passes_target(self.target, host.as_ref(), self.project_sets_target)
            .then(|| self.target.clone());
        let toolchain = checked.and_then(|checked| checked.toolchain);
        // A build that is fresh is not made again (requirement R-WS-4, decided in M2's D4).
        let fresh = match self.fresh(plan, host.as_ref(), toolchain.as_deref()).await {
            Freshness::Fresh(built) => {
                warnings.extend(built.record.warnings.iter().cloned());
                return Ok(*built);
            },
            Freshness::Stale(fresh) => fresh,
        };
        self.tries(plan, toolchain.as_deref(), fresh.as_ref(), warnings)
            .await
    }

    /// The build of `plan`, as [`Self::build`] tries it, with the toolchain that `rustc -vV`
    /// tells, `toolchain`, and the key and inputs `fresh`, for its record.
    async fn tries(
        &self,
        plan: &mut Plan,
        toolchain: Option<&str>,
        fresh: Option<&build::Fresh>,
        warnings: &mut Vec<Box<str>>,
    ) -> Result<Built, LocalError> {
        let told = warnings.len();
        // With the toolchain that built it: the one that rustc tells, if it did.
        let key = (
            plan.package.0.clone(),
            toolchain
                .map(str::to_owned)
                .or_else(|| plan.toolchain.clone()),
        );
        if plan.cfg_docsrs && DOCSRS_BROKE.lock().contains(&key) {
            plan.cfg_docsrs = false;
        }
        let (projects, cfg_docsrs) = (plan.features.clone(), plan.cfg_docsrs);
        let docs_rs = self
            .docs_rs
            .filter(|_| !DOCS_RS_FEATURES_BROKE.lock().contains(&key));
        if let Some(features) = docs_rs {
            plan.features = Some(features.to_vec());
        } else if self.docs_rs.is_some() {
            warnings.push(DOCS_RS_FEATURES_BROKE_IT.into());
        }
        let mut tell = |doing| self.stage.set(doing);
        let mut built =
            build::build(self.cache, plan, self.work, toolchain, fresh, &mut tell).await;
        // Only the documented crate's rustdoc has it, so only its errors may be of it.
        if matches!(
            built,
            Err(BuildError::Failed {
                in_documented: true,
                ..
            })
        ) && plan.cfg_docsrs
        {
            tracing::info!(package = %plan.package.0, "the build failed with --cfg docsrs, so it is tried without");
            plan.cfg_docsrs = false;
            built = build::build(self.cache, plan, self.work, toolchain, fresh, &mut tell).await;
            if built.is_ok() {
                DOCSRS_BROKE.lock().insert(key.clone());
            }
        }
        // The features of `[package.metadata.docs.rs]` may break it, the documented crate or a
        // dependency, as the compiler's errors tell; not what fetching or the lock tells.
        let compiler_failed = matches!(
            &built,
            Err(BuildError::Failed {
                failure: Failure::Other,
                errors,
                ..
            }) if !errors.is_empty()
        );
        if docs_rs.is_some() && compiler_failed {
            tracing::info!(package = %plan.package.0, "the build failed with the features of [package.metadata.docs.rs], so it is tried with the project's");
            // With `--cfg docsrs` as it began, which did not break it: it failed without it too.
            (plan.features, plan.cfg_docsrs) = (projects, cfg_docsrs);
            built = build::build(self.cache, plan, self.work, toolchain, fresh, &mut tell).await;
            if built.is_ok() {
                DOCS_RS_FEATURES_BROKE.lock().insert(key);
                warnings.push(DOCS_RS_FEATURES_BROKE_IT.into());
            }
        } else if docs_rs.is_some() && built.is_ok() {
            warnings.push(
                "built with the features of [package.metadata.docs.rs], as docs.rs builds it; \
                 enabled_in_project tells those of the project's build"
                    .into(),
            );
        }
        let mut built = built.map_err(LocalError::Build)?;
        // For the builds that find it fresh.
        built.record.warnings = warnings.get(told..).unwrap_or_default().to_vec();
        if let Err(error) = built.write_record(self.cache).await {
            tracing::warn!(%error, "the record of a build could not be written again");
        }
        Ok(built)
    }

    /// The build of `plan` published before, if it is fresh: its key, of what it is made from
    /// as `plan` asks and as its files are now, is the one in its record. Else the key and the
    /// inputs of the build to make, if they could be taken. `host` is the host of the toolchain,
    /// whose `rustc -vV` is `toolchain`; without it, the build is made.
    async fn fresh(
        &self,
        plan: &Plan,
        host: Option<&TargetTriple>,
        toolchain: Option<&str>,
    ) -> Freshness {
        let Some(host) = host else {
            return Freshness::Stale(None);
        };
        // Where the JSON tells it is for: the host's for a proc-macro.
        let target = if plan.is_proc_macro() {
            host
        } else {
            plan.target.as_ref().unwrap_or(host)
        };
        let dir = plan.published_dir(self.cache.dir(), Some(target));
        let facts = plan.facts(toolchain, self.docs_rs);
        let (listed, dirs) = freshness::sources(self.input_dirs).await;
        let files = [self.input_files, &listed].concat();
        let found = crate::task::spawn_blocking(move || {
            let before = build::published(&dir);
            let inputs = freshness::collect(
                &files,
                &dirs,
                before
                    .as_ref()
                    .and_then(|before| before.record.inputs.as_ref()),
            );
            (before, inputs)
        })
        .await;
        let (before, inputs) = match found {
            Ok((before, Ok(inputs))) => (before, inputs),
            Ok((_, Err(error))) => {
                tracing::warn!(%error, "what a build is made from could not be read, so it is made again each time");
                return Freshness::Stale(None);
            },
            Err(error) => {
                let error = crate::task::resume_if_panic(error);
                tracing::debug!(%error, "the check of a build was stopped");
                return Freshness::Stale(None);
            },
        };
        let key = freshness::key(&inputs, &facts);
        match before {
            Some(before) if before.record.key.as_deref() == Some(key.as_str()) => {
                Freshness::Fresh(Box::new(before))
            },
            _ => Freshness::Stale(Some(build::Fresh {
                key: key.into(),
                inputs,
            })),
        }
    }
}

/// Whether a build is fresh.
enum Freshness {
    /// It is, as published.
    Fresh(Box<Built>),
    /// It is to be made, with this key and these inputs, if they could be taken.
    Stale(Option<build::Fresh>),
}

/// What the response warns about a member built with the project's features, as the features
/// of its `[package.metadata.docs.rs]` broke its build.
const DOCS_RS_FEATURES_BROKE_IT: &str = "built with the features of the project's build, as those \
                                         of [package.metadata.docs.rs] broke the build";

/// The members whose build the features of their `[package.metadata.docs.rs]` broke, by package
/// id and toolchain, for the life of the process (decided in M2's D3).
static DOCS_RS_FEATURES_BROKE: LazyLock<Mutex<HashSet<BuildKey>>> = LazyLock::new(Mutex::default);

/// The members whose build `--cfg docsrs` broke, by package id and toolchain, for the life of the
/// process (decided in M2's D3).
static DOCSRS_BROKE: LazyLock<Mutex<HashSet<BuildKey>>> = LazyLock::new(Mutex::default);

/// A member's build, by its package id and its toolchain: as `rustc -vV` tells it, or else the one
/// that the server chose instead of the project's.
type BuildKey = (Box<str>, Option<String>);

/// The archive of the JSON that `built` published, keyed by its SHA-256, which it must still have,
/// with `warnings` and what else the response warns about.
async fn archive(
    cache: &Cache,
    built: Built,
    mut warnings: Vec<Box<str>>,
) -> Result<LocalBuilt, LocalError> {
    let Built { json, record, .. } = built;
    if record.bootstrap_all {
        warnings.push(
            "the names of the project's crates were too many for RUSTC_BOOTSTRAP, so it was 1: \
             build scripts that look for nightly features may have found them"
                .into(),
        );
    }
    if !record.cfg_docsrs && build::cfg_docsrs() {
        warnings.push(
            "built without --cfg docsrs, nor the --cfg of the rustdoc-args of its \
             [package.metadata.docs.rs], as its build failed with them: what the crate documents \
             only for docs.rs is not shown"
                .into(),
        );
    }
    if record.format_version > *rustdoc_json::SUPPORTED.end() {
        warnings.push(
            format!(
                "the toolchain writes the rustdoc JSON format {}, newer than {}, which this \
                 server was made for: it is read as that, which was not checked",
                record.format_version,
                *rustdoc_json::SUPPORTED.end()
            )
            .into(),
        );
    }
    let (cache, sha256, stem) = (
        cache.clone(),
        record.sha256.clone(),
        record.format_version.to_string(),
    );
    let path = json.clone();
    let mapped = crate::task::spawn_blocking(move || {
        let dir = path.parent().unwrap_or(&path);
        load::archive_of(&cache, dir, &stem, &sha256, || read_checked(&path, &sha256))
    })
    .await
    .map_err(|error| {
        let error = crate::task::resume_if_panic(error);
        tracing::debug!(%error, "the work that archives a build was stopped");
        LocalError::Stopped
    })?
    .map_err(LocalError::Load)?;
    Ok(LocalBuilt {
        archived: Archived { mapped, warnings },
        json,
        record,
    })
}

/// The JSON at `path`, which must have `sha256`: another server may have published another build
/// of the member there meanwhile.
fn read_checked(path: &Path, sha256: &str) -> io::Result<Box<[u8]>> {
    let json = fs::read(path)?;
    if cache::sha256_hex(&json) == sha256 {
        Ok(json.into_boxed_slice())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} changed after it was built", path.display()),
        ))
    }
}
