//! Loading a workspace member's crate for queries: built locally with `cargo rustdoc` by work
//! that the calls for it share, and archived (requirements R-WS-1, R-WS-6 and R-WS-8, decided in
//! M2's D2).

use std::{
    collections::{HashMap, HashSet},
    fs,
    io,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use parking_lot::Mutex;
use tokio::time::Instant;
use tracing::Instrument as _;

use super::load::{self, Archived, LoadError};
use crate::data::{
    build::{self, BuildError, BuildRecord, Built, DepInfoRun, Documented, Plan},
    cache::{self, Cache},
    cargo::Failure,
    child,
    flight::{Doing, Flights, Joined, Stage},
    freshness,
    metadata::PackageId,
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
    /// The member and the path dependencies that it builds with, with their directories, whose
    /// files and build scripts the build is made from.
    pub input_packages: Vec<(PackageId, PathBuf)>,
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
    /// The build before, given while the member is built again, past the deadline of the call
    /// (requirements R-WS-4 and R-OUT-9, decided in M2's D4).
    pub stale: bool,
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

/// Whether a member's build that went on past a deadline is handed to the next call for it
/// (decided in M2's D4).
///
/// It is when it has no key, so that the next call cannot find it fresh, and would make it again,
/// or when it failed, which the calls that stopped waiting did not see: they had the build before.
#[must_use]
pub fn hands_over(built: &SharedLocal) -> bool {
    built
        .as_ref()
        .map_or(true, |built| built.record.key.is_none())
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
    let key = flight_key(&wanted);
    let what = format!("{} {}", wanted.plan.crate_name, wanted.plan.version);
    let cache = cache.clone();
    let table = table_key(&cache, &wanted);
    flights
        .join(&key, &what, until, async move |stage| {
            // What the work found was built before is there while it works.
            let rebuilding = Rebuilding {
                key: table,
                work: NEXT_REBUILD.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            };
            build_and_archive(&cache, wanted, &stage, &rebuilding)
                .await
                .map(Arc::new)
                .map_err(Arc::new)
        })
        .await
}

/// The key of what `wanted` names in [`REBUILDING`] and [`REBUILD_FAILED`]: of the work, in the
/// cache, as one process may have more than one.
fn table_key(cache: &Cache, wanted: &LocalWanted) -> String {
    format!("{}|{}", cache.dir().display(), flight_key(wanted))
}

/// The key of the work that builds what `wanted` names.
fn flight_key(wanted: &LocalWanted) -> String {
    let documented = match &wanted.plan.documented {
        Documented::Lib { .. } => "lib".to_owned(),
        Documented::Bin { name } => format!("bin {name}"),
    };
    format!(
        "local:{}:{}:{}:{documented}",
        wanted.plan.root.display(),
        wanted.plan.package.0,
        wanted.target
    )
}

/// The builds published before, by [`table_key`], with the number of the work that builds them
/// again, which found them so: given while it works (decided in M2's D4).
static REBUILDING: LazyLock<Mutex<HashMap<String, Given>>> = LazyLock::new(Mutex::default);

/// A build given while the work numbered so builds it again.
type Given = (u64, Arc<Built>);

/// The key of the inputs of the last build that failed, by [`table_key`]: while they are the same,
/// the build before is not given while the member is built again, so that the failure is told
/// rather than the build before again and again (until M2's D5 remembers failures).
static REBUILD_FAILED: LazyLock<Mutex<HashMap<String, Box<str>>>> = LazyLock::new(Mutex::default);

/// Numbers the works that build members.
static NEXT_REBUILD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The entry in [`REBUILDING`] under `key` of the work numbered `work`, taken away when it is
/// dropped, as the work ends, however it ends: not that of a later work under the same key.
struct Rebuilding {
    key: String,
    work: u64,
}

impl Rebuilding {
    /// Gives `before` while the work builds it again, unless the build of the same inputs, of
    /// the key `inputs`, failed last.
    fn give(&self, before: Box<Built>, inputs: &str) {
        if REBUILD_FAILED.lock().get(&self.key).map(AsRef::as_ref) == Some(inputs) {
            return;
        }
        REBUILDING
            .lock()
            .insert(self.key.clone(), (self.work, Arc::from(before)));
    }

    /// Notes how the build of the inputs of the key `inputs` ended.
    fn ended(&self, inputs: &str, failed: bool) {
        let mut failures = REBUILD_FAILED.lock();
        if failed {
            failures.insert(self.key.clone(), inputs.into());
        } else {
            failures.remove(&self.key);
        }
    }
}

impl Drop for Rebuilding {
    fn drop(&mut self) {
        let mut rebuilding = REBUILDING.lock();
        if rebuilding
            .get(&self.key)
            .is_some_and(|(work, _)| *work == self.work)
        {
            rebuilding.remove(&self.key);
        }
    }
}

/// The archive of the build of what `wanted` names that was published before, while the work
/// builds it again (decided in M2's D4).
///
/// It is given past the deadline of the call, whatever it was built with. `None` when the work did
/// not find one, or its archive was not made, which is not made here.
pub async fn rebuilding(cache: &Cache, wanted: &LocalWanted) -> Option<LocalBuilt> {
    let before = REBUILDING
        .lock()
        .get(&table_key(cache, wanted))
        .map(|(_, before)| Arc::clone(before))?;
    let Built { json, record, .. } = Built::clone(&before);
    let dir = json.parent().unwrap_or(&json).to_path_buf();
    let mapped = load::archived(
        cache,
        &dir,
        &record.format_version.to_string(),
        &record.sha256,
    )
    .await?;
    // What it warned of when it was built, as when it is found fresh.
    let warnings = warnings_of(&record, record.warnings.clone());
    Some(LocalBuilt {
        archived: Archived { mapped, warnings },
        json,
        record,
        stale: true,
    })
}

/// Checks the toolchain, builds, and archives the JSON, telling `stage` what it does.
async fn build_and_archive(
    cache: &Cache,
    wanted: LocalWanted,
    stage: &Stage,
    rebuilding: &Rebuilding,
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
        input_packages,
    } = wanted;
    let root = plan.root.clone();
    let programs = Programs::this();
    let rustc = programs
        .used_in(Kind::Rustc, &root, cargo_home.as_deref())
        .await;
    let rustdoc = programs
        .used_in(Kind::Rustdoc, &root, cargo_home.as_deref())
        .await;
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
        rebuilding,
        input_files: &input_files,
        input_packages: &input_packages,
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
    /// Where the work that builds it tells the build before (decided in M2's D4).
    rebuilding: &'a Rebuilding,
    /// The files that the member's build is made from, and the directories of them (decided in
    /// M2's D4).
    input_files: &'a [PathBuf],
    input_packages: &'a [(PackageId, PathBuf)],
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
    /// features: three builds at most (decided in M2's D3). What broke the build is told by the
    /// build published before, which ended without them when a build of the same plan worked
    /// only so (decided in M2's D4): an error of the crate's own, which fails them all, publishes
    /// nothing, so it is not taken for it. `warnings` gets what the response tells of it.
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
        let (fresh, earlier) = match self.fresh(plan, host.as_ref(), toolchain.as_deref()).await {
            Freshness::Fresh(built, fresh) => {
                warnings.extend(built.record.warnings.iter().cloned());
                // A server ended before it had the dep-info: with the plan as it was built.
                if !built.record.dep_info {
                    let mut built_with = plan.clone();
                    (built_with.cfg_docsrs, built_with.features) = (
                        built.record.cfg_docsrs,
                        built.record.features_requested.clone(),
                    );
                    if !self.follow_with_dep_info(built_with, (*built).clone(), *fresh) {
                        warnings.push(DEP_INFO_FAILED_IT.into());
                    }
                }
                return Ok(*built);
            },
            Freshness::Stale(fresh, before) => {
                let earlier = before.as_ref().map(|before| before.record.clone());
                if let (Some(before), Some(fresh)) = (before, &fresh) {
                    self.rebuilding.give(before, &fresh.key);
                }
                (fresh, earlier)
            },
        };
        let built = self
            .tries(
                plan,
                toolchain.as_deref(),
                (fresh.as_deref(), earlier.as_ref()),
                warnings,
            )
            .await;
        if let Some(fresh) = &fresh {
            self.rebuilding
                .ended(&fresh.key, matches!(built, Err(LocalError::Build(_))));
        }
        built
    }

    /// The build of `plan`, as [`Self::build`] tries it, with the toolchain that `rustc -vV`
    /// tells, `toolchain`, and the key and inputs `fresh`, for its record.
    async fn tries(
        &self,
        plan: &mut Plan,
        toolchain: Option<&str>,
        (fresh, earlier): (Option<&build::Fresh>, Option<&BuildRecord>),
        warnings: &mut Vec<Box<str>>,
    ) -> Result<Built, LocalError> {
        let told = warnings.len();
        let planned = build::Planned {
            server: env!("CARGO_PKG_VERSION").into(),
            cfg_docsrs: plan.cfg_docsrs,
            cfgs: plan.cfgs.clone(),
            docs_rs_features: self.docs_rs.map(<[_]>::to_vec),
        };
        let (docsrs_broke, features_broke) =
            broke(earlier, &planned, (toolchain, plan.toolchain.as_deref()));
        if docsrs_broke {
            plan.cfg_docsrs = false;
        }
        let (projects, cfg_docsrs) = (plan.features.clone(), plan.cfg_docsrs);
        let docs_rs = self.docs_rs.filter(|_| !features_broke);
        if let Some(features) = docs_rs {
            plan.features = Some(features.to_vec());
        } else if self.docs_rs.is_some() {
            warnings.push(DOCS_RS_FEATURES_BROKE_IT.into());
        }
        let mut tell = |doing| self.stage.set(doing);
        let mut retried = false;
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
            retried = true;
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
            retried = true;
            if built.is_ok() {
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
        // For the builds that find it fresh, and those that plan it so again: unless what failed
        // may have been sources changing between the tries, as while an agent edits them.
        built.record.warnings = warnings.get(told..).unwrap_or_default().to_vec();
        built.record.planned = if retried && !settled(fresh).await {
            tracing::debug!(package = %plan.package.0, "the sources changed between the tries, so what broke the build is not kept");
            None
        } else {
            Some(planned)
        };
        if let Err(error) = built.write_record(self.cache).await {
            tracing::warn!(%error, "the record of a build could not be written again");
        }
        if let Some(fresh) = fresh {
            self.follow_with_dep_info(plan.clone(), built.clone(), fresh.clone());
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
            return Freshness::Stale(None, None);
        };
        // Where the JSON tells it is for: the host's for a proc-macro.
        let target = if plan.is_proc_macro() {
            host
        } else {
            plan.target.as_ref().unwrap_or(host)
        };
        let dir = plan.published_dir(self.cache.dir(), Some(target));
        let facts = plan.facts(toolchain, self.docs_rs);
        let before = match crate::task::spawn_blocking(move || build::published(&dir)).await {
            Ok(before) => before,
            Err(error) => {
                let error = crate::task::resume_if_panic(error);
                tracing::debug!(%error, "the check of a build was stopped");
                return Freshness::Stale(None, None);
            },
        };
        // What the dep-info of the build before and its build scripts told: its key has them.
        let deps = before
            .as_ref()
            .map(|before| before.record.deps.clone())
            .unwrap_or_default();
        let mut package_dirs: Vec<PathBuf> = self
            .input_packages
            .iter()
            .map(|(_, dir)| dir.clone())
            .collect();
        package_dirs.sort();
        package_dirs.dedup();
        let (listed, walked) = freshness::sources(&package_dirs).await;
        let scanned = [self.input_files, &listed].concat();
        let (files, dirs) = (
            [scanned.as_slice(), &deps.files].concat(),
            [walked.as_slice(), &deps.dirs].concat(),
        );
        let found = crate::task::spawn_blocking(move || {
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
            // The build before still tells how a build of the same plan ends.
            Ok((before, Err(error))) => {
                tracing::warn!(%error, "what a build is made from could not be read, so it is made again each time");
                return Freshness::Stale(None, before.map(Box::new));
            },
            Err(error) => {
                let error = crate::task::resume_if_panic(error);
                tracing::debug!(%error, "the check of a build was stopped");
                return Freshness::Stale(None, None);
            },
        };
        let key = freshness::key(
            &inputs,
            &(facts.clone() + &freshness::environment(&deps.env)),
        );
        match before {
            // With the inputs as they were before the build, for its dep-info.
            Some(before) if before.record.key.as_deref() == Some(key.as_str()) => {
                let fresh = build::Fresh {
                    key: key.into(),
                    inputs: before
                        .record
                        .inputs
                        .clone()
                        .unwrap_or_else(|| inputs.clone()),
                    seen: inputs,
                    facts,
                    deps,
                    scanned,
                    walked,
                };
                Freshness::Fresh(Box::new(before), Box::new(fresh))
            },
            before => Freshness::Stale(
                Some(Box::new(build::Fresh {
                    key: key.into(),
                    seen: inputs.clone(),
                    inputs,
                    facts,
                    deps,
                    scanned,
                    walked,
                })),
                before.map(Box::new),
            ),
        }
    }

    /// Runs the build of `plan`, which published `built`, again for its dep-info, in a task of its
    /// own that the calls do not wait for, and adds what it tells to the record ([`follow`]). A
    /// build that is found fresh without it, as when a server ended before, has it then. When it
    /// could not be had for a build, it is not tried again for the life of the process, and this
    /// returns `false`.
    fn follow_with_dep_info(&self, plan: Plan, built: Built, fresh: build::Fresh) -> bool {
        let tried = (built.json.clone(), fresh.key.clone());
        if DEP_INFO_FAILED.lock().contains(&tried) {
            return false;
        }
        let Some(following) = Following::of(tried) else {
            return true;
        };
        let (cache, work) = (self.cache.clone(), self.work.to_path_buf());
        let packages = self.input_packages.to_vec();
        tokio::spawn(
            async move {
                let had = follow(&cache, &work, plan, built, fresh, packages).await;
                if !had {
                    DEP_INFO_FAILED.lock().insert(following.0.clone());
                }
                drop(following);
            }
            .in_current_span(),
        );
        true
    }
}

/// Adds what the dep-info of the build of `plan`, which published `built`, tells to the record
/// (decided in M2's D4), with what its build scripts told cargo to watch ([`deps_of`]), and
/// whether it was had, or need not be tried again.
///
/// The key is taken again with them, and the inputs of `fresh`, taken before the build, but those
/// that the dep-info before told and this one does not. The run reads the sources as they are
/// then: when the files that `fresh` saw changed meanwhile, its dep-info may not be of the build,
/// and the record loses its key, so that the next call builds again. The record is written only
/// over the one that it was found with. When the cache's path has a comma, which `--emit` cannot
/// take, the record notes that it was tried, and warns.
async fn follow(
    cache: &Cache,
    work: &Path,
    plan: Plan,
    built: Built,
    fresh: build::Fresh,
    packages: Vec<(PackageId, PathBuf)>,
) -> bool {
    let key = fresh.key.clone();
    let still = move |now: &BuildRecord| now.key.as_deref() == Some(&*key) && !now.dep_info;
    // Not had meanwhile, by this server or another.
    let dir = built
        .json
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let now = crate::task::spawn_blocking(move || build::published(&dir)).await;
    if !now.ok().flatten().is_some_and(|now| still(&now.record)) {
        return true;
    }
    let mut built = built;
    built.record.dep_info = true;
    let written = async |built: Built, still| {
        built
            .write_record_if(cache, still)
            .await
            .inspect_err(|error| {
                tracing::warn!(%error, "the record of a build could not be written with its dep-info");
            })
            .is_ok()
    };
    if work.to_string_lossy().contains(',') {
        tracing::warn!(work = %work.display(), "the cache's path has a comma, which --emit=dep-info cannot take");
        built.record.warnings.push(NO_DEP_INFO.into());
        return written(built, still).await;
    }
    // Once its turn comes: still the record that it was found with, of sources as they were.
    let (ready_still, ready_seen, ready_dir) = (
        still.clone(),
        fresh.seen.clone(),
        built
            .json
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    );
    let ready = move || {
        build::published(&ready_dir).is_some_and(|now| ready_still(&now.record))
            && freshness::unchanged(&ready_seen)
    };
    let run = match build::dep_info(cache, &plan, work, ready).await {
        Ok(Some(run)) => Ok(run),
        // Another build, or edits: the next call finds what is to be done.
        Ok(None) => return true,
        Err(error) => Err(error),
    };
    let (seen, taken_at, package) = (fresh.seen, fresh.inputs.taken_at, plan.package.0.clone());
    let found = crate::task::spawn_blocking(move || {
        // What the run read is of the build only if nothing changed meanwhile.
        if !freshness::unchanged(&seen) {
            return None;
        }
        Some(run.map(|run| {
            let deps = deps_of(&run, &plan, &packages);
            let more = freshness::collect_after(&deps.files, &deps.dirs, taken_at);
            (deps, more)
        }))
    })
    .await;
    let (deps, more) = match found {
        Ok(None) => {
            tracing::debug!(json = %built.json.display(), "the sources changed since the build, so it is to be made again");
            built.record.key = None;
            return written(built, still).await;
        },
        Ok(Some(Ok((deps, Ok(more))))) => (deps, more),
        Ok(Some(Ok((_, Err(error))))) => {
            tracing::info!(%error, "the files of a dep-info could not be read");
            return false;
        },
        Ok(Some(Err(error))) => {
            tracing::info!(%package, %error, "the dep-info of a build could not be had, so its key has the files of its packages only");
            return child::stopping();
        },
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::debug!(%error, "the reading of a dep-info was stopped");
            return true;
        },
    };
    let mut inputs = fresh.inputs;
    // What only the dep-info before told, and this one does not.
    let scanned: HashSet<&Path> = fresh.scanned.iter().map(PathBuf::as_path).collect();
    let told = |path: &Path, deps: &freshness::Deps| {
        deps.files.iter().any(|file| file == path)
            || deps.dirs.iter().any(|dir| path.starts_with(dir))
    };
    inputs.files.retain(|file| {
        !told(&file.path, &fresh.deps)
            || told(&file.path, &deps)
            || scanned.contains(file.path.as_path())
            || fresh.walked.iter().any(|dir| file.path.starts_with(dir))
    });
    inputs.add(more);
    let facts = fresh.facts + &freshness::environment(&deps.env);
    built.record.key = Some(freshness::key(&inputs, &facts).into());
    (built.record.inputs, built.record.deps) = (Some(inputs), deps);
    written(built, still).await
}

/// What the dep-info of the build of `plan`, `run`, tells it read, with what the build scripts of
/// `packages` told cargo to watch, but what the build wrote in its target directory (decided in
/// M2's D4). Of the variables that rustdoc read, those of the environment that the server passes
/// on: cargo and build scripts set the others from what the key has.
fn deps_of(run: &DepInfoRun, plan: &Plan, packages: &[(PackageId, PathBuf)]) -> freshness::Deps {
    let found = freshness::dep_info(&run.dep_info, &plan.root);
    let mut paths = found.files;
    let mut env: Vec<Box<str>> = found
        .env
        .into_iter()
        .filter(|(name, value)| freshness::from_environment(name, value.as_deref()))
        .map(|(name, _)| name)
        .collect();
    for (id, output) in &run.scripts {
        // The same id first: two path packages may have the same name and version.
        let package = packages
            .iter()
            .find(|(package, _)| *package.0 == **id)
            .or_else(|| packages.iter().find(|(package, _)| package.is(id)));
        if let Some((_, dir)) = package {
            freshness::watched(output, dir, &mut paths, &mut env);
        }
    }
    // As the OS names it too, as `..` in a path is.
    let outputs = plan.target_dir.path();
    let real = dunce::canonicalize(outputs).ok();
    paths.retain(|path| {
        !path.starts_with(outputs) && real.as_ref().is_none_or(|real| !path.starts_with(real))
    });
    let (mut dirs, mut files): (Vec<PathBuf>, Vec<PathBuf>) =
        paths.into_iter().partition(|path| path.is_dir());
    for paths in [&mut files, &mut dirs] {
        paths.sort();
        paths.dedup();
    }
    env.sort();
    env.dedup();
    freshness::Deps { files, dirs, env }
}

/// What the response warns about a member whose build's dep-info could not be had (decided in M2's
/// D4).
const DEP_INFO_FAILED_IT: &str = "the dep-info of this build could not be had: edits to files \
                                  outside its packages that the crate reads do not make it built \
                                  again";

/// What the response warns about a member whose builds cannot have their dep-info.
const NO_DEP_INFO: &str = "the path of the cache has a comma, which rustdoc cannot write its \
                           dep-info to: edits to files outside the packages that the crate reads \
                           do not make it built again";

/// A build, by its JSON and its key.
type Followed = (PathBuf, Box<str>);

/// The builds whose dep-info is had now, so that it is had once at a time.
static FOLLOWING: LazyLock<Mutex<HashSet<Followed>>> = LazyLock::new(Mutex::default);

/// The builds whose dep-info could not be had, for the life of the process: it is not tried
/// again for each call that finds them fresh.
static DEP_INFO_FAILED: LazyLock<Mutex<HashSet<Followed>>> = LazyLock::new(Mutex::default);

/// The dep-info of a build that is had now, until it is dropped.
struct Following(Followed);

impl Following {
    /// The dep-info of the build `followed`, unless it is had now.
    fn of(followed: Followed) -> Option<Self> {
        let inserted = FOLLOWING.lock().insert(followed.clone());
        // Made only when it is had now: dropped, it would take away the one that is.
        if inserted { Some(Self(followed)) } else { None }
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        FOLLOWING.lock().remove(&self.0);
    }
}

/// Whether a build is fresh.
enum Freshness {
    /// It is, as published, with its key and the inputs that it was made from.
    Fresh(Box<Built>, Box<build::Fresh>),
    /// It is to be made, with this key and these inputs, if they could be taken, and was built
    /// before so, if it was.
    Stale(Option<Box<build::Fresh>>, Option<Box<Built>>),
}

/// What the response warns about a member built with the project's features, as the features
/// of its `[package.metadata.docs.rs]` broke its build.
const DOCS_RS_FEATURES_BROKE_IT: &str = "built with the features of the project's build, as those \
                                         of [package.metadata.docs.rs] broke the build";

/// Whether the sources of a build that `fresh` saw before it have not changed since, so that what
/// broke a try was not an edit. Without `fresh`, it cannot be told.
async fn settled(fresh: Option<&build::Fresh>) -> bool {
    let Some(seen) = fresh.map(|fresh| fresh.seen.clone()) else {
        return false;
    };
    crate::task::spawn_blocking(move || freshness::unchanged(&seen))
        .await
        .unwrap_or(false)
}

/// Whether `--cfg docsrs`, and whether the features of `[package.metadata.docs.rs]`, broke the
/// build `earlier`, the one published before, planned as `planned` is now, with the same toolchain:
/// as `rustc -vV` tells it, `toolchain`, and as the server chose it, `chosen` (decided in M2's D4).
/// It ended without them. A build of the same plan then begins so, rather than trying them again
/// at each edit, which a rustdoc run costs.
fn broke(
    earlier: Option<&BuildRecord>,
    planned: &build::Planned,
    (toolchain, chosen): (Option<&str>, Option<&str>),
) -> (bool, bool) {
    let Some(earlier) = earlier.filter(|earlier| {
        earlier.planned.as_ref() == Some(planned)
            && earlier.toolchain.as_deref() == toolchain
            && earlier.chosen_toolchain.as_deref() == chosen
    }) else {
        return (false, false);
    };
    (
        planned.cfg_docsrs && !earlier.cfg_docsrs,
        planned.docs_rs_features.is_some()
            && earlier.features_requested != planned.docs_rs_features,
    )
}

/// The archive of the JSON that `built` published, keyed by its SHA-256, which it must still have,
/// with `warnings` and what else the response warns about.
async fn archive(
    cache: &Cache,
    built: Built,
    warnings: Vec<Box<str>>,
) -> Result<LocalBuilt, LocalError> {
    let Built { json, record, .. } = built;
    let warnings = warnings_of(&record, warnings);
    let (cache, sha256, stem) = (
        cache.clone(),
        record.sha256.clone(),
        record.format_version.to_string(),
    );
    let (path, dir) = (json.clone(), json.parent().unwrap_or(&json).to_path_buf());
    let checked = sha256.clone();
    let mapped = load::archive_from(&cache, &dir, &stem, &sha256, move || {
        read_checked(&path, &checked)
    })
    .await
    .ok_or(LocalError::Stopped)?
    .map_err(LocalError::Load)?;
    Ok(LocalBuilt {
        archived: Archived { mapped, warnings },
        json,
        record,
        stale: false,
    })
}

/// `warnings`, with what the response warns about how the build of `record` was made.
fn warnings_of(record: &BuildRecord, mut warnings: Vec<Box<str>>) -> Vec<Box<str>> {
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
    warnings
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The record of a build planned as `planned`, made with `cfg_docsrs` and the features
    /// `features`, by the toolchain `rustc 1.99.1`.
    fn record(
        planned: &build::Planned,
        cfg_docsrs: bool,
        features: Option<&[&str]>,
    ) -> BuildRecord {
        serde_json::from_value(serde_json::json!({
            "package": "path+file:///w/a#0.1.0",
            "crate_name": "a",
            "crate_version": "0.1.0",
            "format_version": 61,
            "target": null,
            "toolchain": "rustc 1.99.1",
            "chosen_toolchain": null,
            "built_at": "2026-10-21T00:00:00Z",
            "features_requested": features,
            "features_effective": null,
            "bootstrap_all": false,
            "cfg_docsrs": cfg_docsrs,
            "sha256": "0".repeat(64),
            "planned": planned,
        }))
        .expect("a record")
    }

    #[test]
    fn a_build_of_the_same_plan_begins_as_the_one_before_ended() {
        let planned = build::Planned {
            server: env!("CARGO_PKG_VERSION").into(),
            cfg_docsrs: true,
            cfgs: Box::new(["my_cfg".into()]),
            docs_rs_features: Some(vec!["extra".into()]),
        };
        let toolchain = (Some("rustc 1.99.1"), None);
        let ended = |cfg_docsrs, features: &[&str]| {
            broke(
                Some(&record(&planned, cfg_docsrs, Some(features))),
                &planned,
                toolchain,
            )
        };
        // As planned; without `--cfg docsrs`; with the project's features, with `--cfg docsrs`
        // back as the third try puts it; with neither.
        assert_eq!(ended(true, &["extra"]), (false, false));
        assert_eq!(ended(false, &["extra"]), (true, false));
        assert_eq!(ended(true, &[]), (false, true));
        assert_eq!(ended(false, &[]), (true, true));
        let fell_back = record(&planned, false, Some(&[]));
        // Another toolchain, chosen or told, another plan, or another server, tries again.
        for (other, toolchain) in [
            (planned.clone(), (Some("rustc 1.100.0"), None)),
            (
                planned.clone(),
                (Some("rustc 1.99.1"), Some("nightly-2026-10-01")),
            ),
            (
                build::Planned {
                    docs_rs_features: Some(vec!["more".into()]),
                    ..planned.clone()
                },
                toolchain,
            ),
            (
                build::Planned {
                    cfgs: Box::new([]),
                    ..planned.clone()
                },
                toolchain,
            ),
            (
                build::Planned {
                    server: "0.0.1".into(),
                    ..planned.clone()
                },
                toolchain,
            ),
        ] {
            assert_eq!(
                broke(Some(&fell_back), &other, toolchain),
                (false, false),
                "{other:?} {toolchain:?}"
            );
        }
        // Without features of `[package.metadata.docs.rs]`, only `--cfg docsrs` may have broken it.
        let without = build::Planned {
            docs_rs_features: None,
            ..planned.clone()
        };
        assert_eq!(
            broke(Some(&record(&without, false, None)), &without, toolchain),
            (true, false)
        );
        // A record from before plans were kept, or none.
        let mut unplanned = fell_back;
        unplanned.planned = None;
        assert_eq!(broke(Some(&unplanned), &planned, toolchain), (false, false));
        assert_eq!(broke(None, &planned, toolchain), (false, false));
    }

    #[test]
    fn a_record_from_before_plans_were_kept_is_read() {
        let record: BuildRecord = serde_json::from_value(serde_json::json!({
            "package": "path+file:///w/a#0.1.0",
            "crate_name": "a",
            "crate_version": "0.1.0",
            "format_version": 61,
            "target": null,
            "toolchain": "rustc 1.99.1",
            "chosen_toolchain": null,
            "built_at": "2026-10-21T00:00:00Z",
            "features_requested": null,
            "features_effective": null,
            "bootstrap_all": false,
            "cfg_docsrs": false,
            "sha256": "0".repeat(64),
        }))
        .expect("a record");
        assert_eq!(record.planned, None);
    }

    #[test]
    fn the_dep_info_of_a_build_is_had_once_at_a_time() {
        let followed: Followed = (PathBuf::from("/c/local/v1/a/61.json"), "key".into());
        let first = Following::of(followed.clone()).expect("not had");
        // Had now: not again, and the one that is had stays so.
        assert!(Following::of(followed.clone()).is_none());
        assert!(Following::of(followed.clone()).is_none());
        // Another key of the same JSON is another build.
        let other = Following::of((followed.0.clone(), "other".into())).expect("not had");
        drop((first, other));
        assert!(Following::of(followed).is_some());
    }
}
