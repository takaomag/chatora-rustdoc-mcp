//! The builds of `build_external_docs` of a release of crates.io that the project does not use:
//! not a dependency, or a version that its Cargo.lock does not have (requirements R-WS-5 and
//! R-WS-10, decided in M2e's E2a).
//!
//! The release is documented in a temporary project of its own, by work that the calls for it
//! share, which holds the lock of its record's key from before the project is written until the
//! record is (decided in M2e, 13). A call waits for it as the query tools wait for a build, and
//! then returns `status: building` while it goes on; the query tools find the record.

use std::{sync::Arc, time::Duration};

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    ErrorChain,
    ToolState,
    error_result,
    get_item::{self, building_response},
    local,
    project,
};
use crate::{
    data::{
        LoadError,
        build::{BuildError, Built, Failed},
        cargo::Failure,
        crate_file,
        crate_name::CrateName,
        docs_rs::FetchError,
        external::{self, Key, Making, Recorded},
        flight::{Doing, Joined, Stage},
        metadata::{CargoError, Resolution},
        packages::{DocsRsCheck, PackageGraph},
        project::workspace_of,
        rustdoc_json::ParseError,
        sparse_index::IndexEntry,
        target::TargetTriple,
        temp_project::{self, Release},
        version::VersionSpec,
    },
    query::{
        load::{self, ArchiveError},
        local::{
            LocalBuilt,
            LocalError,
            LocalWanted,
            RECORDED_BY,
            local_archive_telling,
            recorded,
        },
    },
};

/// The build of a release for the calls that wanted it at once, or the body of the error that it
/// ended with, which its record keeps.
pub(super) type SharedExternal = Result<Arc<LocalBuilt>, Arc<Value>>;

/// How long the work waits for another server that makes the same record: about as long as a
/// build may take (requirement R-WS-7).
const OTHER_MAKES_FOR: Duration = Duration::from_mins(30);

/// How often the work tries the lock of the key again while another holds it.
const TRY_AGAIN_EVERY: Duration = Duration::from_millis(200);

/// Documents the release of `name` that `spec` names, or the latest, for `target`, or the host's
/// (decided in M2e, 11), in a temporary project, unless the project uses it or docs.rs has it.
pub(super) async fn release(
    state: Arc<ToolState>,
    name: CrateName,
    spec: Option<VersionSpec>,
    target: Option<TargetTriple>,
) -> CallToolResult {
    let until = state.until();
    let entry = match get_item::index_entry(&state, &name).await {
        Ok(entry) => entry,
        Err(result) => return *result,
    };
    let spec = spec.unwrap_or(VersionSpec::Latest);
    let resolved = match spec.resolve(&entry.versions) {
        Ok(resolved) => resolved,
        Err(no_version) => {
            return get_item::version_not_found(&state, &entry, &spec, &no_version);
        },
    };
    let version = resolved.version.version.clone();
    // cargo does not resolve a yanked version anew (decided in M2e's E2a).
    if resolved.version.yanked {
        return yanked(&entry.name, &version, &entry.versions);
    }
    let mut warnings: Vec<Box<str>> = entry.stale.iter().cloned().collect();
    warnings.extend(
        resolved
            .warnings
            .iter()
            .map(|warning| Box::from(warning.as_ref())),
    );
    let published: CrateName = entry.name.parse().unwrap_or(name);
    match uses(&state, &entry, &version, until).await {
        Uses::No => {},
        Uses::Yes => {
            return super::build_external_docs::not_yet(
                "a release that the project uses",
                "look it up with get_item, which builds what docs.rs does not have",
            );
        },
        Uses::Response(result) => return *result,
    }
    let target = match target {
        Some(target) => target,
        None => state.host().await.triple.clone(),
    };
    let key = Key::Release {
        name: published.cache_key().into(),
        version: version.to_string().into(),
        target: target.as_str().into(),
        hidden: false,
    };
    // A record is not made again: a call again only forgets a failure (requirement R-WS-10).
    if let Some(built) = whole_record(&state, &key).await {
        return built_response(&built, (&published, &target), &warnings);
    }
    match on_docs_rs(&state, (&published, &version, &target), until).await {
        DocsRs::Lacks => {},
        DocsRs::Unknown(why) => warnings.push(why.into()),
        DocsRs::Response(result) => return *result,
    }
    let release = Release {
        cfgs: Box::default(),
        features: vec!["default".into()],
        name: published,
        version,
    };
    let release =
        match with_docs_rs_features(&state, release, &resolved.version.checksum, &mut warnings)
            .await
        {
            Ok(release) => release,
            Err(result) => return *result,
        };
    built(&state, key, release, target, warnings, until).await
}

/// Whether the project uses a release.
enum Uses {
    /// It does not, or there is no project to tell.
    No,
    /// It does.
    Yes,
    /// The response to return, while cargo resolves the project.
    Response(Box<CallToolResult>),
}

/// Whether the default project uses the release of `entry` at `version`, as the query tools tell
/// it (decided in M2's D7): a package of crates.io that `cargo metadata` resolves, whose checksum
/// is the index's. Another of the same name and version (a path, git or another registry's
/// package, or a copy) is not it, and a project that cannot be read does not tell: the release of
/// crates.io is documented apart then, which builds nothing of the project.
async fn uses(
    state: &ToolState,
    entry: &IndexEntry,
    version: &semver::Version,
    until: Instant,
) -> Uses {
    let Ok(Some((workspace, project_from))) = project::workspace(state.project.as_ref(), None)
    else {
        return Uses::No;
    };
    match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) if matches!(&*resolution, Resolution::Metadata { .. }) => {
            let uses = PackageGraph::new(&resolution)
                .crates_io_named(&entry.name)
                .into_iter()
                .any(|package| {
                    package.version == *version
                        && package.docs_rs_check(entry) == DocsRsCheck::Allowed
                });
            if uses { Uses::Yes } else { Uses::No }
        },
        Joined::Done(Ok(_) | Err(_)) => Uses::No,
        Joined::Ended => Uses::Response(Box::new(get_item::stopped())),
        Joined::Going(doing) => {
            let mut provenance = Map::new();
            provenance.insert("source".to_owned(), json!("project"));
            provenance.insert(
                "workspace".to_owned(),
                json!(workspace.root.display().to_string()),
            );
            provenance.insert("project_from".to_owned(), json!(project_from.as_str()));
            Uses::Response(Box::new(building_response(
                provenance,
                doing,
                "the project's dependencies are being resolved, to tell whether it uses the \
                 release",
                &[],
            )))
        },
    }
}

/// The build that the record of `key` records, archived, if it is whole.
async fn whole_record(state: &ToolState, key: &Key) -> Option<LocalBuilt> {
    let (cache, key) = (state.cache.clone(), key.clone());
    let read = crate::task::spawn_blocking(move || external::read(&cache, &key))
        .await
        .ok()?;
    let Recorded::Built(built) = read else {
        return None;
    };
    recorded(&state.cache, *built).await.ok()
}

/// What docs.rs has of a release.
enum DocsRs {
    /// No rustdoc JSON that this server reads: it is built here.
    Lacks,
    /// It could not be asked, as this tells: it is built here.
    Unknown(String),
    /// The response to return: docs.rs has it, its fetch failed, or goes on.
    Response(Box<CallToolResult>),
}

/// What docs.rs has of the release of `name` at `version` for `target`, read as the query tools
/// read it (decided in M2e's E2a): if it has JSON that this server reads, nothing is built.
async fn on_docs_rs(
    state: &ToolState,
    (name, version, target): (&CrateName, &semver::Version, &TargetTriple),
    until: Instant,
) -> DocsRs {
    let joined = load::docs_rs_archive(
        &state.archives,
        &state.cache,
        &state.docs_rs,
        name,
        version,
        Some(target),
        until,
    )
    .await;
    let release = format!("{name}@{version}");
    let error = match joined {
        Joined::Done(Ok(_)) => {
            return DocsRs::Response(Box::new(on_docs_rs_response(name, version, target)));
        },
        Joined::Done(Err(error)) => error,
        Joined::Ended => return DocsRs::Response(Box::new(get_item::stopped())),
        Joined::Going(doing) => {
            let mut provenance = Map::new();
            provenance.insert("source".to_owned(), json!("docs.rs"));
            provenance.insert("crate".to_owned(), json!(name.as_str()));
            provenance.insert("version".to_owned(), json!(version.to_string()));
            provenance.insert("target".to_owned(), json!(target.as_str()));
            let being = get_item::being(&release, doing, "docs.rs");
            return DocsRs::Response(Box::new(building_response(provenance, doing, &being, &[])));
        },
    };
    match &*error {
        ArchiveError::Load(
            LoadError::NoTargetJson { .. }
            | LoadError::Fetch(FetchError::NotFound | FetchError::BadRequest),
        )
        | ArchiveError::Make(load::LoadError::Parse(ParseError::UnsupportedFormat { .. })) => {
            DocsRs::Lacks
        },
        ArchiveError::Load(LoadError::Fetch(FetchError::Offline)) => DocsRs::Unknown(format!(
            "whether docs.rs has rustdoc JSON of {release} is not known offline, so it is built \
             here"
        )),
        error @ (ArchiveError::Load(_) | ArchiveError::Make(_) | ArchiveError::Stopped) => {
            DocsRs::Response(Box::new(get_item::archive_error_of(
                &state.cache,
                name,
                &release,
                error,
            )))
        },
    }
}

/// `release` with the features and the `--cfg` that docs.rs builds it with, as its `.crate`, of
/// `checksum`, tells (requirement R-FT-1, decided in M2e's E2a); or the error when the `.crate`
/// cannot be had, rather than a record of other features, which would last. A manifest that
/// cannot be read leaves the default features, with a warning.
async fn with_docs_rs_features(
    state: &ToolState,
    mut release: Release,
    checksum: &str,
    warnings: &mut Vec<Box<str>>,
) -> Result<Release, Box<CallToolResult>> {
    let read = state
        .static_crates
        .crate_file(
            &state.cache,
            &release.name,
            &release.version,
            Some(checksum),
        )
        .await;
    let what = format!("{}@{}", release.name, release.version);
    let contents = match read {
        Ok(Some(contents)) => contents,
        Ok(None) => return Err(Box::new(crate_failed(state, &what, "it is not there"))),
        Err(error) => {
            return Err(Box::new(crate_failed(
                state,
                &what,
                &ErrorChain(&error).to_string(),
            )));
        },
    };
    match crate_file::docs_rs_build(&contents, &release.name, &release.version) {
        Ok(build) => {
            release.features = build.features.into_vec();
            release.cfgs = build.rustdoc_cfg;
        },
        Err(error) => warnings.push(
            format!(
                "the manifest in the .crate of {what} could not be read ({}), so it is built \
                 with its default features, not those of [package.metadata.docs.rs]",
                ErrorChain(&error)
            )
            .into(),
        ),
    }
    Ok(release)
}

/// The error when the `.crate` of `what` could not be had from static.crates.io, as `why` tells:
/// `offline_miss` offline, else `network_error`.
fn crate_failed(state: &ToolState, what: &str, why: &str) -> CallToolResult {
    let message = format!("the .crate of {what} could not be had from static.crates.io: {why}");
    if state.index.is_offline() {
        error_result(&json!({
            "code": "offline_miss",
            "message": message,
            "retryable": false,
            "suggestions": ["turn network.offline off in the server's settings to fetch it"],
        }))
    } else {
        error_result(&json!({
            "code": "network_error",
            "message": message,
            "retryable": true,
            "suggestions": ["try again in a while"],
        }))
    }
}

/// Builds `release` for `target` by the work that the calls for `key` share, waiting for it as
/// the query tools wait for a build (requirement R-WS-10): `build.wait_secs`, and while it is
/// indexed, until the deadline of the call `until`.
async fn built(
    state: &Arc<ToolState>,
    key: Key,
    release: Release,
    target: TargetTriple,
    warnings: Vec<Box<str>>,
    until: Instant,
) -> CallToolResult {
    let what = format!("{}@{}", release.name, release.version);
    let until_built = Instant::now()
        .checked_add(state.build_wait)
        .map_or(until, |until_built| until_built.min(until));
    let flight = key.lock_key();
    let start = || {
        let (state, key, release, target) = (
            Arc::clone(state),
            key.clone(),
            release.clone(),
            target.clone(),
        );
        async move |stage: Arc<Stage>| make(&state, &key, release, target, &stage).await
    };
    let mut joined = state
        .externals
        .join(&flight, &what, until_built, start())
        .await;
    if matches!(joined, Joined::Going(Doing::Indexing)) && until_built < until {
        joined = state.externals.join(&flight, &what, until, start()).await;
    }
    let looked_up = (&release.name, &target);
    match joined {
        Joined::Done(Ok(built)) => built_response(&built, looked_up, &warnings),
        Joined::Done(Err(body)) => error_result(&body),
        Joined::Ended => get_item::stopped(),
        Joined::Going(doing) => going(looked_up, &release.version, doing, &warnings),
    }
}

/// The response while the build of `name` at `version` for `target` goes on, past the deadline:
/// the query tools wait for it, without the confirmation that this tool may ask for (requirement
/// R-WS-10).
fn going(
    (name, target): (&CrateName, &TargetTriple),
    version: &semver::Version,
    doing: Doing,
    warnings: &[Box<str>],
) -> CallToolResult {
    let what = format!("{name}@{version}");
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("version".to_owned(), json!(version.to_string()));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
    let being = get_item::being(&what, doing, "crates.io");
    let mut result = building_response(provenance, doing, &being, warnings);
    if let Some(Value::Object(response)) = &mut result.structured_content {
        response.insert(
            "hints".to_owned(),
            json!([format!(
                "{being}, which goes on without this call: look it up with get_item or \
                 crate_overview, passing {}, which answer once it is built; do not call \
                 build_external_docs again for it",
                json!({ "crate": what, "target": target.as_str() })
            )]),
        );
    }
    result
}

/// Why the work did not build a release.
enum Unmade {
    /// It stopped, as the server shuts down: not remembered.
    Stopped,
    /// It failed, as the body of the error tells, which is remembered.
    Failed(Value),
}

/// The work that documents `release` for `target` and records it as `key`, telling `progress` what
/// it does.
///
/// It holds the lock of the key throughout. A record made meanwhile by another server is taken as
/// it is; a failure or a broken record is forgotten, and so is the failure of the build in the
/// temporary project, which is made again (requirement R-WS-10).
async fn make(
    state: &ToolState,
    key: &Key,
    release: Release,
    target: TargetTriple,
    progress: &Stage,
) -> SharedExternal {
    let what = format!("{}@{}", release.name, release.version);
    let making = take_lock(state, key, progress)
        .await
        .map_err(|why| Arc::new(failed_so(&what, &why, true)))?;
    let refresh = match read_record(state, key).await {
        Some(Recorded::Built(built)) => match recorded(&state.cache, *built).await {
            Ok(built) => return Ok(Arc::new(built)),
            Err(_) => true,
        },
        Some(Recorded::Failed(_) | Recorded::Broken(_)) => true,
        Some(Recorded::Building | Recorded::Missing) | None => false,
    };
    let made = build_in_temporary_project(state, (key, release, target), refresh, progress).await;
    let cache = state.cache.clone();
    let key = key.clone();
    match made {
        Ok(built) => {
            let to_record = Built {
                json: built.json.clone(),
                record: built.record.clone(),
                rebuilt: Vec::new(),
            };
            let written = crate::task::spawn_blocking(move || {
                external::record(&cache, &key, &to_record, &making)
            })
            .await;
            if let Ok(Err(error)) | Err(error) = written.map_err(std::io::Error::other) {
                tracing::warn!(%error, "an external record could not be written");
            }
            recorded(&state.cache, Built {
                json: built.json.clone(),
                record: built.record.clone(),
                rebuilt: Vec::new(),
            })
            .await
            .map(Arc::new)
            .map_err(|error| Arc::new(failed_so(&what, &error.to_string(), true)))
        },
        Err(Unmade::Stopped) => Err(Arc::new(body_of(&get_item::stopped()))),
        Err(Unmade::Failed(body)) => {
            let transient = body.get("retryable") == Some(&Value::Bool(true));
            let failed = Failed::now(None, None, transient, body.clone());
            let remembered = crate::task::spawn_blocking(move || {
                external::remember_failure(&cache, &key, &failed, &making)
            })
            .await;
            if let Ok(Err(error)) | Err(error) = remembered.map_err(std::io::Error::other) {
                tracing::warn!(%error, "the failure of an external build could not be remembered");
            }
            Err(Arc::new(body))
        },
    }
}

/// The record of `key`, as it is now.
async fn read_record(state: &ToolState, key: &Key) -> Option<Recorded> {
    let (cache, key) = (state.cache.clone(), key.clone());
    crate::task::spawn_blocking(move || external::read(&cache, &key))
        .await
        .ok()
}

/// The lock of `key`, waited for while another server makes its record (decided in M2e, 13), at
/// most [`OTHER_MAKES_FOR`]; or why it could not be taken.
async fn take_lock(state: &ToolState, key: &Key, progress: &Stage) -> Result<Making, String> {
    let started = Instant::now();
    loop {
        let (cache, tried) = (state.cache.clone(), key.clone());
        let taken = crate::task::spawn_blocking(move || external::try_make(&cache, &tried))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("the lock of the record cannot be taken: {error}"))?;
        if let Some(making) = taken {
            return Ok(making);
        }
        // A query tool's try holds it for a moment; another server, for its build.
        progress.set(Doing::WaitingForLock);
        if started.elapsed() > OTHER_MAKES_FOR {
            return Err("another server has been making the record for too long".to_owned());
        }
        tokio::time::sleep(TRY_AGAIN_EVERY).await;
    }
}

/// Writes the temporary project of `release` in the directory of the record of `key`, makes its
/// Cargo.lock, and builds and archives the release in it for `target`, forgetting a failure of
/// the build if `refresh`; or why not.
async fn build_in_temporary_project(
    state: &ToolState,
    (key, release, target): (&Key, Release, TargetTriple),
    refresh: bool,
    progress: &Stage,
) -> Result<Arc<LocalBuilt>, Unmade> {
    let what = format!("{}@{}", release.name, release.version);
    let failed = |why: &str| Unmade::Failed(failed_so(&what, why, false));
    let dir = temp_project::dir_in(&key.dir(state.cache.dir()));
    let (written, wanted) = (dir.clone(), release.clone());
    crate::task::spawn_blocking(move || temp_project::write(&written, &wanted))
        .await
        .map_err(|error| failed(&error.to_string()))?
        .map_err(|error| failed(&error.to_string()))?;
    let workspace = workspace_of(&dir).map_err(|error| failed(&error.to_string()))?;
    let root = workspace.root.clone();
    // The user's cargo settings apply to it too, which may build their own std, before cargo runs
    // (decided in M2's D7).
    let project_sets_target = match project::build_targets(state, &root).await {
        project::BuildTargets::These(_) => project::sets_build_target(state, &root).await,
        project::BuildTargets::OwnTarget(own) => {
            return Err(failed(&format!(
                "the cargo settings build their own std ({own}), which this server does not \
                 document"
            )));
        },
    };
    let offline = state.resolver.offline_in(&root).await;
    let resolution = resolved(state, &workspace, (&what, offline), progress)
        .await
        .map_err(Unmade::Failed)?;
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return Err(failed("cargo metadata did not resolve it"));
    };
    let plan = temp_project::plan(
        (&root, metadata),
        &release,
        None,
        (state.cache.dir(), offline),
    )
    .map_err(|error| failed(&error.to_string()))?;
    let wanted = LocalWanted {
        plan,
        target,
        project_sets_target,
        cargo_home: state
            .resolver
            .cargo_home()
            .map(std::path::Path::to_path_buf),
        docs_rs_features: None,
        warnings: Vec::new(),
        refresh,
        input_files: state.resolver.input_files(&root, metadata).await,
        input_packages: Vec::new(),
    };
    progress.set(Doing::Building);
    match local_archive_telling(&state.locals, &state.cache, wanted, progress).await {
        Some(Ok(built)) => Ok(built),
        Some(Err(error)) if matches!(*error, LocalError::Stopped) => Err(Unmade::Stopped),
        Some(Err(error)) => Err(Unmade::Failed(external_body(body_of(&local::local_error(
            &what, &error,
        ))))),
        None => Err(Unmade::Stopped),
    }
}

/// The packages of the temporary project `workspace`, by `cargo metadata`, after its Cargo.lock is
/// made, offline if `offline`; or the body of the error, of `what`. A Cargo.lock that cargo
/// cannot read, as one cut short, is made again once, but not for what is missing offline.
async fn resolved(
    state: &ToolState,
    workspace: &crate::data::project::Workspace,
    (what, offline): (&str, bool),
    progress: &Stage,
) -> Result<Arc<Resolution>, Value> {
    let root = &workspace.root;
    let mut body = Value::Null;
    for again in [false, true] {
        if again {
            remove_lock(root).await;
            state.resolver.forget_failures(root).await;
        }
        progress.set(Doing::Fetching);
        if let Err(error) = temp_project::lock(root, offline, |doing| progress.set(doing)).await {
            // Making a Cargo.lock only fetches.
            let (failure, stderr) = if let BuildError::Fetch { failure, stderr } = &error {
                (Some(*failure), stderr.as_ref())
            } else {
                (None, "")
            };
            return Err(cargo_failure(
                what,
                failure,
                error.is_transient(),
                &format!("{}{stderr}", ErrorChain(&error)),
            ));
        }
        match state.resolver.resolve(workspace).await {
            Ok(resolution) => match &*resolution {
                Resolution::Metadata { .. } => return Ok(resolution),
                Resolution::CargoLockRaw { because, .. } => {
                    let (failure, stderr) = if let CargoError::Failed {
                        failure, stderr, ..
                    } = because
                    {
                        (Some(*failure), stderr.as_ref())
                    } else {
                        (None, "")
                    };
                    body = cargo_failure(
                        what,
                        failure,
                        true,
                        &format!("{}: {stderr}", ErrorChain(because)),
                    );
                    if failure == Some(Failure::OfflineMiss) {
                        break;
                    }
                },
            },
            Err(error) => body = failed_so(what, &ErrorChain(&*error).to_string(), true),
        }
    }
    Err(body)
}

/// The body of the error of cargo for `what`, as `failure` classifies it, which may work when
/// tried again if `transient` (requirement R-WS-7), as `why` tells.
fn cargo_failure(what: &str, failure: Option<Failure>, transient: bool, why: &str) -> Value {
    let mut body = failed_so(what, why, transient);
    if let Some(failure) = failure
        && let Some(fields) = body.as_object_mut()
    {
        fields.insert("code".to_owned(), json!(failure.code()));
        if matches!(failure, Failure::OfflineMiss | Failure::LockfileOutdated) {
            fields.insert("retryable".to_owned(), json!(false));
        }
    }
    external_body(body)
}

/// `body`, an error of a build, with what to do of this tool: not what the query tools tell, as
/// `refresh` or the project's terminal (requirement R-WS-10).
fn external_body(mut body: Value) -> Value {
    let offline = body.get("code") == Some(&json!("offline_miss"));
    if let Some(fields) = body.as_object_mut() {
        let mut suggestions: Vec<Value> = match fields.remove("suggestions") {
            Some(Value::Array(suggestions)) => suggestions
                .into_iter()
                .filter(|suggestion| {
                    suggestion.as_str().is_some_and(|text| {
                        !text.contains("refresh") && !text.contains("project's terminal")
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        if offline {
            suggestions.push(json!(
                "turn network.offline off in the server's settings, so that cargo fetches it"
            ));
        }
        suggestions.push(json!(CALL_AGAIN));
        fields.insert("suggestions".to_owned(), Value::Array(suggestions));
    }
    body
}

/// What calls this tool again, which builds again what failed (requirement R-WS-10).
const CALL_AGAIN: &str = "call build_external_docs again with the same arguments to try again";

/// Removes the Cargo.lock of the temporary project at `root`, which cargo could not read.
async fn remove_lock(root: &std::path::Path) {
    let lock = root.join("Cargo.lock");
    let removed = crate::task::spawn_blocking(move || std::fs::remove_file(lock)).await;
    if let Ok(Err(error)) = removed {
        tracing::warn!(%error, "the Cargo.lock of a temporary project could not be removed");
    }
}

/// The JSON body of the tool error `result`.
fn body_of(result: &CallToolResult) -> Value {
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str(&text.text).ok())
        .unwrap_or_else(|| json!({ "code": "build_failed", "message": "the build failed" }))
}

/// The body of `build_failed` for `what`, as `why` tells, which may work when tried again if
/// `retryable`.
fn failed_so(what: &str, why: &str, retryable: bool) -> Value {
    json!({
        "code": "build_failed",
        "message": format!("the documentation of {what} could not be built: {why}"),
        "retryable": retryable,
        "suggestions": [CALL_AGAIN],
    })
}

/// The response for `built`, of `name` for `target` (its key, which the JSON of a proc macro,
/// built for the host, does not tell): the keys to look it up by (requirement R-WS-10).
fn built_response(
    built: &LocalBuilt,
    (name, target): (&CrateName, &TargetTriple),
    warnings: &[Box<str>],
) -> CallToolResult {
    let record = &built.record;
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    provenance.insert("package_source".to_owned(), json!("crates.io"));
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("version".to_owned(), json!(record.crate_version));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    if let Some(toolchain) = record.toolchain.as_deref().and_then(|t| t.lines().next()) {
        provenance.insert("toolchain".to_owned(), json!(toolchain));
    }
    provenance.insert("built_at".to_owned(), json!(record.built_at));
    if let Some(features) = &record.features_effective {
        provenance.insert("features_effective".to_owned(), json!(features));
    }
    provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
    let mut warnings: Vec<Box<str>> = warnings.to_vec();
    warnings.extend(built.archived.warnings.iter().cloned());
    let release = format!("{name}@{}", record.crate_version);
    let mut response = Map::new();
    response.insert("provenance".to_owned(), Value::Object(provenance));
    if !warnings.is_empty() {
        response.insert("warnings".to_owned(), json!(warnings));
    }
    response.insert(
        "data".to_owned(),
        json!({
            "built": true,
            "crate": name.as_str(),
            "version": record.crate_version,
            "target": target.as_str(),
        }),
    );
    response.insert(
        "hints".to_owned(),
        json!([format!(
            "look it up with get_item, crate_overview or search_items, passing {}",
            json!({ "crate": release, "target": target.as_str() })
        )]),
    );
    CallToolResult::structured(Value::Object(response))
}

/// The response when docs.rs has rustdoc JSON of `name` at `version` for `target` that this
/// server reads: nothing is built (requirement R-WS-10).
fn on_docs_rs_response(
    name: &CrateName,
    version: &semver::Version,
    target: &TargetTriple,
) -> CallToolResult {
    let release = format!("{name}@{version}");
    CallToolResult::structured(json!({
        "provenance": {
            "source": "docs.rs",
            "crate": name.as_str(),
            "version": version.to_string(),
            "target": target.as_str(),
        },
        "data": {
            "built": false,
            "reason": "docs_rs",
            "crate": name.as_str(),
            "version": version.to_string(),
            "target": target.as_str(),
        },
        "hints": [format!(
            "nothing was built: docs.rs has it; look it up with get_item or crate_overview, \
             passing {}",
            json!({ "crate": release, "target": target.as_str() })
        )],
    }))
}

/// The error for the yanked `version` of `name`, which cargo does not resolve anew: with the
/// versions near it that are not yanked (decided in M2e's E2a).
fn yanked(
    name: &str,
    version: &semver::Version,
    versions: &[crate::data::sparse_index::IndexVersion],
) -> CallToolResult {
    let mut near: Vec<&semver::Version> = versions
        .iter()
        .filter(|other| !other.yanked && other.version.major == version.major)
        .map(|other| &other.version)
        .collect();
    near.sort_by_key(|other| std::cmp::Reverse(*other));
    let candidates: Vec<String> = near.iter().take(5).map(ToString::to_string).collect();
    error_result(&json!({
        "code": "invalid_argument",
        "message": format!(
            "{name} {version} is yanked, and cargo does not resolve a yanked version anew, so it \
             cannot be built here; docs.rs may have it: look it up with get_item"
        ),
        "retryable": false,
        "suggestions": ["ask for one of the candidates, which are not yanked"],
        "candidates": candidates,
    }))
}
