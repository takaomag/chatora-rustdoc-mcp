//! Loading a workspace member's crate for queries: built locally with `cargo rustdoc` by work
//! that the calls for it share, and archived (requirements R-WS-1, R-WS-6 and R-WS-8, decided in
//! M2's D2).

use std::{
    fs,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::time::Instant;

use super::load::{self, Archived, LoadError};
use crate::data::{
    build::{self, BuildError, BuildRecord, Built, Documented, Plan},
    cache::{self, Cache},
    flight::{Doing, Flights, Joined, Stage},
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
}

/// The archive of a member's build, and its record.
#[derive(Debug)]
pub struct LocalBuilt {
    /// The archive, and what the response warns about.
    pub archived: Archived,
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

/// The archive of the member's crate that `wanted` names, built and archived by work that the
/// calls for it share (requirements R-WS-6 and R-NW-2).
///
/// A call waits for the work until `until`. The work then goes on, and [`Joined::Going`] says what
/// it is doing (requirements R-ARCH-6 and R-OUT-9). Until D4 tells whether the sources changed,
/// every call that does not join one builds again (decided in M2's D2): cargo runs only the
/// documented crate's rustdoc then. So that a crate whose rustdoc takes longer than a call waits
/// is answered, `flights` are [`Flights::handing_over`]: the next call has the result of a build
/// that went on.
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
    let built = build_with(
        cache,
        &mut plan,
        &rustc,
        &target,
        project_sets_target,
        installs,
        &work,
        stage,
    )
    .await;
    let mut built = if installs {
        after_install(&rustc, &root, &target, built).await?
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
                built = build_with(
                    cache,
                    &mut plan,
                    &rustc,
                    &target,
                    project_sets_target,
                    false,
                    &work,
                    stage,
                )
                .await?;
            },
            Ok(Chosen::Project { format }) => {
                return Err(LocalError::Choose(ChooseError::TooOld { found: format }));
            },
            Err(error) => return Err(LocalError::Choose(error)),
        }
    }
    stage.set(Doing::Indexing);
    archive(cache, built).await
}

/// After a build in which cargo was to install the project's toolchain: what rustup said of it
/// before is forgotten, and it is checked now (decided in M2's D2). A failed build of a toolchain
/// that is still missing, or lacks the target, is told as that; a build is recorded with the
/// toolchain.
async fn after_install(
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
            if let Err(error) = built.write_record() {
                tracing::warn!(%error, "the record of a build could not be written again");
            }
            Ok(built)
        },
        (built, _) => built,
    }
}

/// Builds `plan` for `target`, deciding whether it passes `--target` by the host of its toolchain,
/// which is checked first unless cargo is to install it.
#[expect(
    clippy::too_many_arguments,
    reason = "what one build is made from, passed twice"
)]
async fn build_with(
    cache: &Cache,
    plan: &mut Plan,
    rustc: &Path,
    target: &TargetTriple,
    project_sets_target: bool,
    installs: bool,
    work: &Path,
    stage: &Stage,
) -> Result<Built, LocalError> {
    let checked = if installs {
        None
    } else {
        Some(
            toolchain::check_target(rustc, plan.toolchain.as_deref(), &plan.root, target)
                .await
                .map_err(LocalError::Missing)?,
        )
    };
    let host = checked.as_ref().and_then(|checked| checked.host.as_ref());
    plan.target = build::passes_target(target, host, project_sets_target).then(|| target.clone());
    let toolchain = checked.and_then(|checked| checked.toolchain);
    build::build(cache, plan, work, toolchain.as_deref(), |doing| {
        stage.set(doing);
    })
    .await
    .map_err(LocalError::Build)
}

/// The archive of the JSON that `built` published, keyed by its SHA-256, which it must still have.
async fn archive(cache: &Cache, built: Built) -> Result<LocalBuilt, LocalError> {
    let Built { json, record, .. } = built;
    let mut warnings = Vec::new();
    if record.bootstrap_all {
        warnings.push(
            "the names of the project's crates were too many for RUSTC_BOOTSTRAP, so it was 1: \
             build scripts that look for nightly features may have found them"
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
    let mapped = crate::task::spawn_blocking(move || {
        let dir = json.parent().unwrap_or(&json);
        load::archive_of(&cache, dir, &stem, &sha256, || read_checked(&json, &sha256))
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
