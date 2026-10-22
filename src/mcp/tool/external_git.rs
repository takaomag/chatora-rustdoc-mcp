//! The builds of `build_external_docs` of a crate of a git repository (requirements R-WS-5 and
//! R-WS-10, decided in M2e's E3).
//!
//! The ref asked for is resolved first, by the key of the ref ([`Key::GitRef`]), in a temporary
//! project of its own that depends on the crate at the ref: cargo fetches the repository and
//! writes the commit in the Cargo.lock. The commit is then documented as a release of crates.io
//! is ([`external_build::make`]), by the key of the commit ([`Key::Git`]), which the refs that
//! resolve to it share, in a temporary project that depends on the crate at the commit. The work
//! holds the locks of both keys, the ref's first (decided in M2e, 13), and resolves the ref anew on
//! every call, as a branch or a tag may move (requirement R-WS-10).

use std::{path::Path, sync::Arc, time::SystemTime};

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    ErrorChain,
    ToolState,
    error_result,
    external_build::{self, cargo_failure, failed_so},
    get_item::{self, GitSpec, building_response},
};
use crate::{
    data::{
        build::{BuildError, Failed},
        cargo,
        crate_file::{self, DocsRsBuild},
        crate_name::CrateName,
        external::{self, Key, Making, Resolved},
        flight::{Doing, Joined, Stage},
        git_source::GitRef,
        metadata::{Metadata, Package, Resolution},
        packages::Source,
        project::workspace_of,
        target::TargetTriple,
        temp_project::{self, Origin, Release},
    },
    query::local::{LocalBuilt, RECORDED_BY},
};

/// What the work for a crate of a git repository made, for the calls that wanted it at once: the
/// crate built at the commit that its ref resolved to, or the body of the error that it ended
/// with, which the records keep.
pub(in crate::mcp) type SharedGit = Arc<Result<GitBuilt, Value>>;

/// A crate of a git repository, built at the commit that its ref resolved to.
#[derive(Debug)]
pub(in crate::mcp) struct GitBuilt {
    /// The build, archived.
    built: Arc<LocalBuilt>,
    /// What the ref resolved to.
    resolved: Resolved,
    /// When it was resolved: a call that came later resolves it again (decided in M2e, 13).
    resolved_then: Instant,
    /// What the response warns about how it was resolved.
    warnings: Vec<Box<str>>,
}

/// Documents the crate `name` of the repository and at the ref of `git`, for `target`, or the
/// host's: the crate is no package of the project (requirement R-WS-5).
///
/// A call that joins the work of another, which resolved the ref before the call came, resolves it
/// again once that ends, as a branch or a tag may have moved since (decided in M2e, 13). Not when
/// the work failed: the call returns the failure, as the next call tries again; nor more than once,
/// so that calls that come again while it builds are answered.
pub(super) async fn crate_of_git(
    state: Arc<ToolState>,
    name: CrateName,
    git: GitSpec,
    target: Option<TargetTriple>,
) -> CallToolResult {
    let called = Instant::now();
    let until = state.until();
    let target = match target {
        Some(target) => target,
        None => state.host().await.triple.clone(),
    };
    let key = Key::GitRef {
        url: git.url.key().into(),
        reference: git.reference.full_name().into(),
        name: name.cache_key().into(),
        target: target.as_str().into(),
        hidden: false,
    };
    let what = what_of(&name, &git);
    let until_built = called
        .checked_add(state.build_wait)
        .map_or(until, |until_built| until_built.min(until));
    let flight = key.lock_key();
    let start = || {
        let (state, key, name, git, target) = (
            Arc::clone(&state),
            key.clone(),
            name.clone(),
            git.clone(),
            target.clone(),
        );
        async move |stage: Arc<Stage>| make(&state, &key, (&name, &git), target, &stage).await
    };
    let mut joined = state
        .git_externals
        .join(&flight, &what, until_built, start())
        .await;
    let before = |made: &SharedGit| {
        made.as_ref()
            .as_ref()
            .is_ok_and(|built| built.resolved_then < called)
    };
    if matches!(&joined, Joined::Done(made) if before(made) && !git.reference.is_hash()) {
        joined = state
            .git_externals
            .join(&flight, &what, until_built, start())
            .await;
    }
    if matches!(joined, Joined::Going(Doing::Indexing)) && until_built < until {
        joined = state
            .git_externals
            .join(&flight, &what, until, start())
            .await;
    }
    match joined {
        Joined::Done(made) => match made.as_ref() {
            Ok(built) => built_response(built, (&name, &git), &target),
            Err(body) => error_result(body),
        },
        Joined::Ended => get_item::stopped(),
        Joined::Going(doing) => going((&name, &git, &target), doing),
    }
}

/// What the crate `name` of `git` is, for the messages: the repository as given, which has no
/// password, and the ref.
fn what_of(name: &CrateName, git: &GitSpec) -> String {
    format!(
        "{name} of {} at {}",
        git.url.as_given(),
        git.reference.full_name()
    )
}

/// The work that resolves the ref of `git` for the crate `name`, records it as `key`, and
/// documents the commit for `target`, telling `progress` what it does.
async fn make(
    state: &Arc<ToolState>,
    key: &Key,
    (name, git): (&CrateName, &GitSpec),
    target: TargetTriple,
    progress: &Stage,
) -> SharedGit {
    Arc::new(built(state, key, (name, git), target, progress).await)
}

/// The crate `name` of `git` built for `target` at the commit that its ref resolves to, as
/// [`make`] makes it; or the body of the error.
///
/// It holds the lock of `key` throughout, and the commit is documented ([`external_build::make`])
/// by the work that the calls for the commit share, under its lock: the refs that resolve to it
/// share it (decided in M2e, 13). What failed to resolve is remembered by `key`, for the query
/// tools; an earlier resolution that it fails for good is forgotten.
async fn built(
    state: &Arc<ToolState>,
    key: &Key,
    (name, git): (&CrateName, &GitSpec),
    target: TargetTriple,
    progress: &Stage,
) -> Result<GitBuilt, Value> {
    let what = what_of(name, git);
    let making = external_build::take_lock(state, key, progress)
        .await
        .map_err(|why| failed_so(&what, &why, true))?;
    let (cache, recorded_key) = (state.cache.clone(), key.clone());
    let found = match resolve(state, key, (name, git), progress).await {
        Ok(found) => found,
        Err(body) => {
            let transient = body.get("retryable") == Some(&Value::Bool(true));
            let failed = Failed::now(None, None, transient, body.clone());
            let remembered = crate::task::spawn_blocking(move || {
                external::remember_resolution_failure(&cache, &recorded_key, &failed, &making)
            })
            .await;
            if let Ok(Err(error)) | Err(error) = remembered.map_err(std::io::Error::other) {
                tracing::warn!(%error, "the failure to resolve a git ref could not be remembered");
            }
            return Err(body);
        },
    };
    let resolved_then = Instant::now();
    let Found {
        mut resolved,
        docs_rs,
        offline,
        mut warnings,
    } = found;
    // Offline, the repository was not asked: when it was is kept.
    let keep_when = offline && !git.reference.is_hash();
    let (at, making) = record_resolution(state, key, (resolved.clone(), keep_when), making)
        .await
        .map_err(|error| failed_so(&what, &error.to_string(), true))?;
    resolved.at = at;
    let package: CrateName = resolved
        .package
        .parse()
        .map_err(|error| failed_so(&what, &format!("cargo resolved {error}"), false))?;
    let mut release = Release {
        name: package,
        origin: Origin::Git {
            url: git.url.as_given().into(),
            reference: GitRef::Rev(resolved.commit.clone()),
        },
        features: vec!["default".into()],
        cfgs: Box::default(),
        asks_features: false,
    };
    match docs_rs {
        Ok(build) => external_build::as_docs_rs(&mut release, &build, &mut warnings),
        Err(why) => warnings.push(
            format!(
                "the manifest of {what} could not be read ({why}), so it is built with its \
                 default features, not those of [package.metadata.docs.rs]"
            )
            .into(),
        ),
    }
    let commit = Key::Git {
        url: git.url.key().into(),
        commit: resolved.commit.clone(),
        name: name.cache_key().into(),
        target: target.as_str().into(),
        hidden: false,
    };
    progress.set(Doing::Building);
    let commit_what = release.what();
    let start = {
        let (state, commit) = (Arc::clone(state), commit.clone());
        async move |stage: Arc<Stage>| {
            external_build::make(&state, &commit, release, target, &stage).await
        }
    };
    let built = state
        .externals
        .wait_telling(&commit.lock_key(), &commit_what, Some(progress), start)
        .await
        .ok_or_else(external_build::stopped_body)?
        .map_err(|body| (*body).clone())?;
    drop(making);
    Ok(GitBuilt {
        built,
        resolved,
        resolved_then,
        warnings,
    })
}

/// Records what the ref of `key` resolved to, `resolved`, under its lock (`making`), which it gives
/// back with when it was resolved: that of the record before, if `keep_when` and there is one.
///
/// # Errors
///
/// Returns why the task did not end, which lets the lock go. A record that cannot be written is
/// only logged: the build goes on.
async fn record_resolution(
    state: &ToolState,
    key: &Key,
    (mut resolved, keep_when): (Resolved, bool),
    making: Making,
) -> Result<(Box<str>, Making), tokio::task::JoinError> {
    let (cache, key) = (state.cache.clone(), key.clone());
    crate::task::spawn_blocking(move || {
        if keep_when
            && let external::Resolution::Resolved(before) =
                external::resolution_making(&cache, &key, &making)
        {
            resolved.at = before.at;
        }
        if let Err(error) = external::record_resolution(&cache, &key, &resolved, &making) {
            tracing::warn!(%error, "what a git ref resolved to could not be recorded");
        }
        (resolved.at, making)
    })
    .await
}

/// What a ref resolved to.
struct Found {
    /// The commit and the package.
    resolved: Resolved,
    /// How docs.rs would build the crate, or why its manifest cannot be read.
    docs_rs: Result<DocsRsBuild, String>,
    /// Whether cargo was offline, and did not ask the repository.
    offline: bool,
    /// What the response warns of.
    warnings: Vec<Box<str>>,
}

/// What the ref of `git` resolves to for the crate `name`, in the temporary project in the
/// directory of `key`; or the body of the error.
///
/// cargo matches the name of a package exactly, where the cache key does not tell `-` from `_`:
/// a name that the repository has none of is tried with `-` for `_`, and `_` for `-`.
async fn resolve(
    state: &ToolState,
    key: &Key,
    (name, git): (&CrateName, &GitSpec),
    progress: &Stage,
) -> Result<Found, Value> {
    let dir = key.dir(state.cache.dir()).join("project");
    let mut tried: Vec<String> = Vec::new();
    let mut missing_offline = None;
    for spelling in spellings(name) {
        let release = Release {
            origin: Origin::Git {
                url: git.url.as_given().into(),
                reference: git.reference.clone(),
            },
            features: Vec::new(),
            cfgs: Box::default(),
            asks_features: false,
            name: spelling,
        };
        tried.push(release.name.to_string());
        match resolve_as(state, &dir, &release, progress).await {
            Ok(mut found) => {
                if found.offline && !git.reference.is_hash() {
                    found.warnings.push(
                        format!(
                            "offline, {} is resolved as cargo last fetched the repository, \
                             which may be behind it",
                            git.reference.full_name()
                        )
                        .into(),
                    );
                }
                return Ok(found);
            },
            Err(Resolving::NoSuchPackage(offline)) => missing_offline = offline,
            Err(Resolving::Failed(body)) => return Err(body),
        }
    }
    Err(missing_offline.unwrap_or_else(|| {
        json!({
            "code": "crate_not_found",
            "message": format!(
                "the repository {} has no package named {} at {}",
                git.url.as_given(),
                tried.join(" or "),
                git.reference.full_name()
            ),
            "retryable": false,
            "suggestions": [
                "give the name of the package as the Cargo.toml in the repository names it",
                "check the branch, tag or rev",
            ],
        })
    }))
}

/// `name`, then with `-` for `_` and `_` for `-`, without repeating one.
fn spellings(name: &CrateName) -> Vec<CrateName> {
    let mut spellings = vec![name.clone()];
    for other in [
        name.as_str().replace('_', "-"),
        name.as_str().replace('-', "_"),
    ] {
        if let Ok(other) = other.parse::<CrateName>()
            && !spellings
                .iter()
                .any(|spelling| spelling.as_str() == other.as_str())
        {
            spellings.push(other);
        }
    }
    spellings
}

/// Why a ref was not resolved for a spelling of the crate's name.
enum Resolving {
    /// The repository has no package of the name, or, offline, cargo may not have fetched it: the
    /// body of `offline_miss` then.
    NoSuchPackage(Option<Value>),
    /// It failed otherwise, as the body of the error tells.
    Failed(Value),
}

/// What the ref of `release` resolves to, in the temporary project at `dir` written for it.
///
/// Its Cargo.lock is made anew, as a branch or a tag may have moved since, but for a hash, which
/// does not move (requirement R-WS-10).
async fn resolve_as(
    state: &ToolState,
    dir: &Path,
    release: &Release,
    progress: &Stage,
) -> Result<Found, Resolving> {
    let what = release.what();
    let failed = |why: &str| Resolving::Failed(failed_so(&what, why, false));
    let (written, wanted) = (dir.to_path_buf(), release.clone());
    let workspace = crate::task::spawn_blocking(move || {
        temp_project::write(&written, &wanted)?;
        workspace_of(&written).map_err(std::io::Error::other)
    })
    .await
    .map_err(|error| failed(&error.to_string()))?
    .map_err(|error| failed(&error.to_string()))?;
    let moves = !matches!(&release.origin, Origin::Git { reference, .. } if reference.is_hash());
    if moves {
        external_build::remove_lock(dir).await;
    }
    let offline = state.resolver.offline_in(&workspace.root).await;
    progress.set(Doing::Fetching);
    if let Err(error) =
        temp_project::lock(&workspace.root, offline, |doing| progress.set(doing)).await
    {
        let (failure, stderr) = if let BuildError::Fetch { failure, stderr } = &error {
            (Some(*failure), stderr.as_ref())
        } else {
            (None, "")
        };
        let body = cargo_failure(
            &what,
            failure,
            error.is_transient(),
            &format!("{}{stderr}", ErrorChain(&error)),
        );
        // Not a dependency of it that is missing.
        if cargo::no_package_named(stderr, release.name.as_str()) {
            return Err(Resolving::NoSuchPackage(offline.then_some(body)));
        }
        return Err(Resolving::Failed(git_failure(body)));
    }
    let resolution = state
        .resolver
        .resolve(&workspace)
        .await
        .map_err(|error| failed(&ErrorChain(&*error).to_string()))?;
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return Err(failed("cargo metadata did not resolve it"));
    };
    let package = dependency_of_project(metadata, release)
        .ok_or_else(|| failed("cargo did not resolve it from the repository"))?;
    let commit = temp_project::commit_of(package)
        .ok_or_else(|| failed("cargo did not tell the commit that it resolved"))?;
    let manifest = package.manifest_path.clone();
    let docs_rs = crate::task::spawn_blocking(move || crate_file::member_docs_rs(&manifest))
        .await
        .map_err(|error| error.to_string())
        .and_then(|read| read.map_err(|error| error.to_string()));
    let resolved = Resolved {
        commit: commit.into(),
        package: package.name.clone(),
        version: package.version.to_string().into(),
        at: crate::data::rfc3339(SystemTime::now()).into(),
    };
    Ok(Found {
        resolved,
        docs_rs,
        offline,
        warnings: Vec::new(),
    })
}

/// The package of git that the temporary project of `release`, resolved as `metadata`, depends
/// on: not another of the same name that the crate's own dependencies have.
fn dependency_of_project<'m>(metadata: &'m Metadata, release: &Release) -> Option<&'m Package> {
    let project = metadata.workspace_members.first()?;
    let node = metadata
        .resolve
        .as_ref()?
        .nodes
        .iter()
        .find(|node| node.id == *project)?;
    node.deps.iter().find_map(|dependency| {
        metadata.packages.iter().find(|package| {
            package.id == dependency.pkg
                && package
                    .name
                    .parse::<CrateName>()
                    .is_ok_and(|name| name.cache_key() == release.name.cache_key())
                && matches!(Source::parse(package.source.as_deref()), Source::Git(_))
        })
    })
}

/// `body`, an error of cargo that fetched a git repository, with what to do of a failed fetch: the
/// server's cargo fetches it, with the user's git settings and credential helpers.
fn git_failure(mut body: Value) -> Value {
    let suggestion = match body.get("reason").and_then(Value::as_str) {
        Some("auth") => {
            "check the URL first: GitHub asks for credentials for a repository that does not \
             exist, too; then let git fetch it without asking, as cargo runs without a terminal: \
             with the GitHub CLI, `gh auth setup-git` once lets git and cargo use its login"
        },
        Some("no_repository" | "no_rev") => "check the URL and the branch, tag or rev",
        _ => return body,
    };
    if let Some(Value::Array(suggestions)) = body.get_mut("suggestions") {
        suggestions.insert(0, json!(suggestion));
    }
    body
}

/// The arguments that look up the crate `name` of `git` for `target`, as the query tools take them:
/// the ref as asked for, which they resolve as this call did.
fn arguments(name: &str, git: &GitSpec, target: &TargetTriple) -> Value {
    let mut arguments = Map::new();
    arguments.insert("crate".to_owned(), json!(name));
    arguments.insert("git".to_owned(), json!(git.url.key()));
    match &git.reference {
        GitRef::DefaultBranch => {},
        GitRef::Rev(rev) => {
            arguments.insert("rev".to_owned(), json!(rev));
        },
        GitRef::Branch(branch) => {
            arguments.insert("branch".to_owned(), json!(branch));
        },
        GitRef::Tag(tag) => {
            arguments.insert("tag".to_owned(), json!(tag));
        },
    }
    arguments.insert("target".to_owned(), json!(target.as_str()));
    Value::Object(arguments)
}

/// The provenance of the crate of `git`: its repository, the ref asked for unless a hash, and
/// whether it is fetched in plain text (decided in M2e's E3).
fn git_provenance(provenance: &mut Map<String, Value>, git: &GitSpec) {
    provenance.insert("package_source".to_owned(), json!("git"));
    provenance.insert("git".to_owned(), json!(git.url.key()));
    if !git.reference.is_hash() {
        provenance.insert("ref".to_owned(), json!(git.reference.full_name()));
    }
    if git.url.plaintext() {
        provenance.insert("plaintext".to_owned(), json!(true));
    }
}

/// What the response warns of a repository fetched in plain text (requirement R-SEC-4).
const PLAINTEXT: &str = "the repository was fetched in plain text (http or git), so what was \
                         fetched may have been changed on the way";

/// The response for `made`, of the crate `name` of `git`, for `target` (its key): the arguments
/// to look it up by (requirement R-WS-10).
fn built_response(
    made: &GitBuilt,
    (name, git): (&CrateName, &GitSpec),
    target: &TargetTriple,
) -> CallToolResult {
    let record = &made.built.record;
    let package = &*made.resolved.package;
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    git_provenance(&mut provenance, git);
    provenance.insert("crate".to_owned(), json!(package));
    provenance.insert("version".to_owned(), json!(record.crate_version));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    provenance.insert("rev".to_owned(), json!(made.resolved.commit));
    provenance.insert("resolved_at".to_owned(), json!(made.resolved.at));
    external_build::built_provenance(&mut provenance, &made.built);
    let mut warnings = made.warnings.clone();
    if git.url.plaintext() {
        warnings.push(PLAINTEXT.into());
    }
    warnings.extend(made.built.archived.warnings.iter().cloned());
    let mut response = Map::new();
    response.insert("provenance".to_owned(), Value::Object(provenance));
    if !warnings.is_empty() {
        response.insert("warnings".to_owned(), json!(warnings));
    }
    response.insert(
        "data".to_owned(),
        json!({
            "built": true,
            "crate": package,
            "version": record.crate_version,
            "target": target.as_str(),
            "git": git.url.key(),
            "rev": made.resolved.commit,
        }),
    );
    response.insert(
        "hints".to_owned(),
        json!([format!(
            "look it up with get_item, crate_overview or search_items, passing {}",
            arguments(name.as_str(), git, target)
        )]),
    );
    CallToolResult::structured(Value::Object(response))
}

/// The response while the crate `name` of `git` is resolved or built for `target`, past the
/// deadline: the query tools wait for it (requirement R-WS-10).
fn going(
    (name, git, target): (&CrateName, &GitSpec, &TargetTriple),
    doing: Doing,
) -> CallToolResult {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("local"));
    git_provenance(&mut provenance, git);
    provenance.insert("crate".to_owned(), json!(name.as_str()));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
    let being = get_item::being(&what_of(name, git), doing, git.url.key());
    let warnings: Vec<Box<str>> = if git.url.plaintext() {
        vec![PLAINTEXT.into()]
    } else {
        Vec::new()
    };
    let mut result = building_response(provenance, doing, &being, &warnings);
    if let Some(Value::Object(response)) = &mut result.structured_content {
        response.insert(
            "hints".to_owned(),
            json!([format!(
                "{being}, which goes on without this call: look it up with get_item or \
                 crate_overview, passing {}, which answer once it is built; do not call \
                 build_external_docs again for it",
                arguments(name.as_str(), git, target)
            )]),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::git_source::GitUrl;

    fn git(reference: GitRef) -> GitSpec {
        GitSpec {
            url: GitUrl::parse("http://example.invalid/dep.git").expect("taken"),
            reference,
        }
    }

    #[test]
    fn a_name_is_tried_as_given_then_with_dash_and_underscore() {
        let of = |name: &str| -> Vec<String> {
            spellings(&name.parse().expect("a crate name"))
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        assert_eq!(of("git_dep"), ["git_dep", "git-dep"]);
        assert_eq!(of("git-dep"), ["git-dep", "git_dep"]);
        assert_eq!(of("a_b-c"), ["a_b-c", "a-b-c", "a_b_c"]);
        assert_eq!(of("serde"), ["serde"]);
    }

    #[test]
    fn the_query_tools_are_told_the_ref_as_asked_for() {
        let target: TargetTriple = "x86_64-unknown-linux-gnu".parse().expect("a target");
        let told = |reference| arguments("git-dep", &git(reference), &target);
        assert_eq!(
            told(GitRef::Branch("main".into())),
            json!({
                "crate": "git-dep",
                "git": "http://example.invalid/dep",
                "branch": "main",
                "target": "x86_64-unknown-linux-gnu",
            })
        );
        assert_eq!(told(GitRef::Tag("v1".into()))["tag"], "v1");
        assert_eq!(told(GitRef::Rev("abcdef0".into()))["rev"], "abcdef0");
        let default = told(GitRef::DefaultBranch);
        assert!(
            ["rev", "branch", "tag"]
                .iter()
                .all(|key| default.get(*key).is_none())
        );
        let mut provenance = Map::new();
        git_provenance(&mut provenance, &git(GitRef::Branch("main".into())));
        assert_eq!(
            Value::Object(provenance),
            json!({
                "package_source": "git",
                "git": "http://example.invalid/dep",
                "ref": "refs/heads/main",
                "plaintext": true,
            })
        );
        let mut provenance = Map::new();
        git_provenance(&mut provenance, &git(GitRef::Rev("abcdef0".into())));
        assert!(!provenance.contains_key("ref"));
    }

    #[test]
    fn the_package_is_the_one_that_the_temporary_project_depends_on() {
        // A fork whose own dependencies have the upstream of the same name, from git too.
        let fork = "git+https://example.invalid/fork#9a41be9f3db7140981e276cfc1fdb993ed8ae07f";
        let upstream = "git+https://example.invalid/a#0000000000000000000000000000000000000000";
        let project = "path+file:///p#chatora-rustdoc-temporary@0.0.0";
        let package = |source: &str| {
            json!({
                "id": format!("{source}#serde@1.0.0"),
                "name": "serde",
                "version": "1.0.0",
                "source": source,
                "manifest_path": "/r/Cargo.toml",
                "targets": [],
            })
        };
        let metadata: Metadata = serde_json::from_value(json!({
            "workspace_root": "/p",
            "workspace_members": [project],
            "packages": [package(upstream), package(fork)],
            "resolve": { "nodes": [
                {
                    "id": project,
                    "deps": [{
                        "name": "serde",
                        "pkg": format!("{fork}#serde@1.0.0"),
                        "dep_kinds": [{ "kind": null, "target": null }],
                    }],
                    "features": [],
                },
            ] },
            "version": 1,
        }))
        .expect("metadata");
        let release = Release {
            name: "serde".parse().expect("a crate name"),
            origin: Origin::Git {
                url: "https://example.invalid/fork".into(),
                reference: GitRef::DefaultBranch,
            },
            features: Vec::new(),
            cfgs: Box::default(),
            asks_features: false,
        };
        let found = dependency_of_project(&metadata, &release).expect("found");
        assert_eq!(found.source.as_deref(), Some(fork));
    }

    #[test]
    fn a_failed_fetch_is_told_what_to_check() {
        let body = |reason: &str| {
            git_failure(json!({
                "code": "fetch_failed",
                "reason": reason,
                "suggestions": [external_build::CALL_AGAIN],
            }))
        };
        let auth = body("auth");
        assert!(
            auth["suggestions"][0]
                .as_str()
                .is_some_and(|text| text.contains("gh auth setup-git")),
            "{auth}"
        );
        assert_eq!(auth["suggestions"][1], external_build::CALL_AGAIN);
        assert!(
            body("no_rev")["suggestions"][0]
                .as_str()
                .is_some_and(|text| text.contains("branch, tag or rev"))
        );
        assert_eq!(
            body("other")["suggestions"].as_array().map(Vec::len),
            Some(1)
        );
    }
}
