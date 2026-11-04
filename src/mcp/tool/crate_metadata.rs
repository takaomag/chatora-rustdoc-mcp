//! The `crate_metadata` tool: what a crate declares, its releases, how much it is used and the
//! versions where what it declares changed (requirement §5.9's metadata, §3 Q15, decided in
//! M3f, decisions 93 to 109).
//!
//! The release is resolved as `resolve_version` resolves it. What it declares comes from its
//! `.crate`, its dependencies and releases from the crates.io index, and only what neither has
//! from the crates.io API: the downloads, the dependents and the versions' licenses and editions
//! (requirement R-NW-1). When the API does not answer in time, the rest is answered without it,
//! with a warning (decision 93).

use std::{sync::Arc, time::SystemTime};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use semver::Version;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    ErrorChain,
    InvalidArgument,
    ToolState,
    get_item::{self, Paging, ReleaseArguments},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
    resolve_version::{self, Release},
};
#[cfg(feature = "local")]
use super::{project, recorded_git::GitFound};
#[cfg(feature = "local")]
use crate::data::{flight::Joined, metadata::Resolution};
use crate::{
    data::{
        cache::sha256_hex,
        crate_file::{self, Declaration},
        crate_name::CrateName,
        crates_io::{ApiError, Cached, Next, VersionInfo},
        rfc3339,
        sparse_index::{DependencyKind, IndexDependency, IndexEntry},
    },
    query::{
        crate_metadata::{After, Change, changes, parse_time, recency},
        get_item::MAX_RESPONSE_CHARS,
    },
};

/// The name of the tool.
pub(super) const NAME: &str = "crate_metadata";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("crate_metadata.md");

/// The changes of the history in a page unless `limit` says otherwise (decision 96).
const DEFAULT_LIMIT: usize = 20;

/// The most changes of the history in a page (decision 96).
const MAX_LIMIT: usize = 100;

/// The most pages of versions asked of the API in one call, so that a call sends three requests
/// at most (decision 109); the cursor goes on from there.
const MAX_NEW_PAGES: usize = 1;

/// The longest description told, in characters (decision 108).
const MAX_DESCRIPTION: usize = 1_000;

/// The longest URL told, in characters (decision 108).
const MAX_URL: usize = 500;

/// The arguments checked: the release's, for `resolve_version`, and the page of the history.
#[derive(Debug)]
pub(super) struct Metadata {
    release: ReleaseArguments,
    paging: Paging,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (serde), or the crate and its version as name@version.",
            },
            "version": {
                "type": "string",
                "description": const_format::concatcp!(
                    "The version: exact, a semver requirement or latest.",
                    super::VERSION_DEFAULT
                ),
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most versions in a page of the history (default 20).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Read a Rust crate's metadata")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the answer.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let text = json!({ "type": "string" });
    let count = json!({ "type": "integer" });
    let mut provenance = resolve_version::provenance_schema();
    if let Some(Value::Object(properties)) = provenance.get_mut("properties") {
        properties.insert(
            "api_fetched_at".to_owned(),
            json!({
                "type": "string",
                "description": "When the oldest answer of the crates.io API used was fetched \
                                (RFC 3339).",
            }),
        );
    }
    object(json!({
        "type": "object",
        "properties": {
            "provenance": provenance,
            "retry_after_secs": count,
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "declared": {
                        "type": "object",
                        "properties": {
                            "description": text,
                            "license": text,
                            "license_file": text,
                            "repository": text,
                            "homepage": text,
                            "documentation": text,
                            "rust_version": text,
                            "edition": text,
                            "docs_rs_targets": strings,
                            "docs_rs_default_target": text,
                        },
                    },
                    "releases": {
                        "type": "object",
                        "properties": {
                            "latest_stable": text,
                            "latest": text,
                            "versions": count,
                            "yanked": count,
                            "this_version_yanked": { "type": "boolean" },
                            "last_release": text,
                            "last_release_at": text,
                            "days_since_last_release": count,
                            "recency": {
                                "type": "string",
                                "enum": ["within_30_days", "within_90_days", "within_365_days", "over_365_days"],
                            },
                        },
                        "required": ["versions", "yanked"],
                    },
                    "popularity": {
                        "type": "object",
                        "properties": {
                            "downloads": count,
                            "recent_downloads": count,
                            "dependents": count,
                        },
                    },
                    "dependencies": dependencies_schema(),
                    "dependencies_total": count,
                    "dev_dependencies": count,
                    "history": history_schema(),
                },
                "required": ["declared"],
            },
            "page": {
                "type": "object",
                "description": "Of the history.",
                "properties": {
                    "shown": count,
                    "total": count,
                    "has_more": { "type": "boolean" },
                    "next_cursor": text,
                },
                "required": ["shown", "has_more"],
            },
            "hints": strings,
        },
        "required": ["provenance", "data"],
    }))
}

/// The schema of `data.dependencies`.
fn dependencies_schema() -> Value {
    let text = json!({ "type": "string" });
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "name": text,
                "package": text,
                "req": text,
                "kind": { "type": "string", "enum": ["normal", "build", "other"] },
                "optional": { "type": "boolean" },
                "target": text,
                "default_features": { "type": "boolean" },
            },
            "required": ["name", "req", "kind"],
        },
    })
}

/// The schema of `data.history`.
fn history_schema() -> Value {
    let text = json!({ "type": "string" });
    let changed = json!({
        "type": "object",
        "description": "Left out of from or to: not declared there.",
        "properties": {
            "from": { "type": "string" },
            "to": { "type": "string" },
        },
    });
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "version": text,
                "published": text,
                "yanked": { "type": "boolean" },
                "first": { "type": "boolean" },
                "license": changed,
                "rust_version": changed,
                "edition": changed,
            },
            "required": ["version", "published"],
        },
    })
}

/// `value` as a JSON object, or an empty one.
fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            JsonObject::new()
        },
    }
}

/// Checks the arguments (requirement R-ARG-1).
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<Metadata, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    if get_item::std_crate(&release.name).is_some() {
        return Err(InvalidArgument::new(
            format!("{} is not published on crates.io", release.name),
            &["get_item tells of the standard library's items"],
        ));
    }
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let limit = match argument("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=MAX_LIMIT).contains(value))
            .ok_or_else(|| {
                InvalidArgument::new(
                    format!("`limit` must be a whole number from 1 to {MAX_LIMIT}"),
                    &[r#"pass a whole number, for example {"limit": 20}"#],
                )
            })?,
    };
    let request_digest = list_module::request_digest(NAME, arguments);
    let (offset, cursor_data) = match argument("cursor") {
        None => (0, None),
        Some(Value::String(cursor)) => list_module::parse_cursor(cursor, &request_digest)?,
        Some(_) => {
            return Err(InvalidArgument::new(
                "`cursor` must be a string",
                CURSOR_EXAMPLE,
            ));
        },
    };
    Ok(Metadata {
        release,
        paging: Paging {
            limit,
            offset,
            request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
            cursor_data,
        },
    })
}

/// Resolves the release as `resolve_version` does, and tells what is known of it.
pub(super) async fn execute(metadata: Metadata, state: Arc<ToolState>) -> CallToolResult {
    let Metadata { release, paging } = metadata;
    // One deadline for every step (requirement R-ARCH-6).
    let until = state.until();
    let refresh = release.refresh;
    #[cfg_attr(
        not(feature = "local"),
        expect(
            unused_variables,
            reason = "only a project or a record has more to tell"
        )
    )]
    let (resolved, taken) = resolve_version::resolving(release, Arc::clone(&state), until).await;
    // An error, or the response while the release is resolved, as it is.
    let Some(mut release) = resolved
        .structured_content
        .as_ref()
        .filter(|_| resolved.is_error != Some(true))
        .and_then(Release::of)
    else {
        return resolved;
    };
    #[cfg(feature = "local")]
    if let Some(git) = &taken.git {
        return git_metadata(git, &mut release, &paging).await;
    }
    #[cfg(feature = "local")]
    if let Some(found) = taken.found.as_deref()
        && !release.on_crates_io
    {
        return project_metadata((&state, &mut release), found, (refresh, until), &paging).await;
    }
    if !release.on_crates_io {
        return not_a_release(&release);
    }
    let entry = match state.index.entry(&release.name).await {
        Ok(entry) => entry,
        Err(error) => return get_item::index_error(&state.cache, &release.name, &error),
    };
    if !entry
        .versions
        .iter()
        .any(|listed| listed.version == release.version)
    {
        release.warnings.push(
            format!(
                "the crates.io index as kept does not list {} {}: what it declares and its \
                 dependencies are not known{}",
                release.name,
                release.version,
                if cfg!(feature = "local") {
                    "; call again with refresh to read the index again"
                } else {
                    ""
                }
            )
            .into(),
        );
    }
    if refresh {
        state.crates_io.forget(&release.name).await;
    }
    // A page after the first is read as kept: `refresh` reads the first again.
    let fresh = refresh && paging.offset == 0;
    // The `.crate` and the API at once: neither waits for the other.
    let ((declaration, declaration_warning), api) = tokio::join!(
        declaration(&state, &entry, (&release.name, &release.version), until),
        Api::of(&state, &release.name, (fresh, until), &paging),
    );
    release.warnings.extend(declaration_warning);
    let told = Told {
        declared: declared(
            &entry,
            &release.version,
            declaration.as_ref(),
            &mut release.warnings,
        ),
        dependencies: Some(dependencies(&entry, &release.version)),
        rest_in: "the release's Cargo.toml, which get_source reads (file Cargo.toml)",
    };
    respond(&mut release, told, Some((&entry, &api)), &paging)
}

/// The answer for a package that is not a release of crates.io: what it declares, and its
/// dependencies when they are known (decided in M3f, decision 98). Of the project's copy of a
/// crate of crates.io, what crates.io tells of the crate by its name is told too (decision 110);
/// else that it is not had.
#[cfg(feature = "local")]
async fn declared_here(
    (state, release): (&ToolState, &mut Release),
    told: Told,
    (refresh, until): (bool, Instant),
    paging: &Paging,
) -> CallToolResult {
    let what = format!("{} {}", release.name, release.version);
    let of = match release.source.as_ref() {
        "crates.io" => None,
        "path" => Some("a workspace member or a path dependency"),
        "git" => Some("a crate of git"),
        _ => Some("a crate of another registry"),
    };
    if let Some(of) = of {
        release.warnings.push(
            format!(
                "{what} is {of}: what it declares is told; its releases, downloads, dependents \
                 and history are not, as crates.io has none of them"
            )
            .into(),
        );
        return respond(release, told, None, paging);
    }
    // The crate of crates.io that the project's copy is of.
    let Ok(entry) = state.index.entry(&release.name).await else {
        release.warnings.push(
            format!(
                "{what} is the project's copy of a crate of crates.io, which the crates.io index \
                 could not be read for: what it declares is told, from the copy"
            )
            .into(),
        );
        return respond(release, told, None, paging);
    };
    if refresh {
        state.crates_io.forget(&release.name).await;
    }
    release.warnings.push(
        format!(
            "{what} is the project's copy, which is not known to be the release on crates.io: what \
             it declares and its dependencies are of the copy; its releases, downloads, \
             dependents and history are of the crate on crates.io"
        )
        .into(),
    );
    let fresh = refresh && paging.offset == 0;
    let api = Api::of(state, &release.name, (fresh, until), paging).await;
    respond(release, told, Some((&entry, &api)), paging)
}

/// What the crate of git `found` that `build_external_docs` recorded declares, from the
/// `Cargo.toml` of its checkout; what it inherits from its workspace is not read.
#[cfg(feature = "local")]
async fn git_metadata(found: &GitFound, release: &mut Release, paging: &Paging) -> CallToolResult {
    let Some(root) = found.built.record.package_root.clone() else {
        return super::tool_error(
            "docs_unavailable",
            &format!(
                "the record of {} is from before where its package is was kept",
                found.package
            ),
            false,
            &["build it again with build_external_docs"],
        );
    };
    let read = crate::task::spawn_blocking(move || {
        crate_file::manifest_declaration(&root.join("Cargo.toml"))
    })
    .await
    .unwrap_or_else(|error| Err(std::io::Error::other(crate::task::resume_if_panic(error))));
    let (declaration, inherited) = match read {
        Ok(read) => read,
        Err(error) => {
            return super::tool_error(
                "docs_unavailable",
                &format!(
                    "the Cargo.toml of {} could not be read: {}",
                    found.package,
                    ErrorChain(&error)
                ),
                false,
                &["build it again with build_external_docs"],
            );
        },
    };
    if !inherited.is_empty() {
        release.warnings.push(
            format!(
                "{} inherited from its workspace, which is not read: see the workspace's \
                 Cargo.toml",
                inherited.join(", ")
            )
            .into(),
        );
    }
    release.warnings.push(
        format!(
            "{} {} is a crate of git: what it declares is told; its dependencies are in \
             [dependencies] of its Cargo.toml, and its releases, downloads, dependents and history \
             are not, as crates.io has none of them",
            release.name, release.version
        )
        .into(),
    );
    let mut warnings = Vec::new();
    let declared = declared_value(&declaration, &mut warnings);
    release.warnings.extend(warnings);
    let told = Told {
        declared,
        dependencies: None,
        rest_in: "",
    };
    respond(release, told, None, paging)
}

/// What the project's package `found` declares, and its dependencies, from its `cargo metadata`,
/// which resolves what it inherits from its workspace.
#[cfg(feature = "local")]
async fn project_metadata(
    (state, release): (&ToolState, &mut Release),
    found: &project::Found,
    (refresh, until): (bool, Instant),
    paging: &Paging,
) -> CallToolResult {
    let what = format!("{} {}", release.name, release.version);
    let unknown = |why: &str| {
        super::tool_error(
            "docs_unavailable",
            &format!("what {what} declares is not known: {why}"),
            false,
            &["read the [package] of its Cargo.toml"],
        )
    };
    let resolution = match project::resolve_until(state, &found.workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return unknown(&format!(
                "the project could not be read: {}",
                ErrorChain(&error)
            ));
        },
        Joined::Going(doing) => {
            let target = match release.target() {
                Some(target) => target,
                None => state.host().await.triple.clone(),
            };
            return project::resolving_crate(
                &release.name,
                &target,
                (doing, found.project_from),
                &release.warnings,
            );
        },
        Joined::Ended => return get_item::stopped(),
    };
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return unknown("cargo metadata of the project failed");
    };
    let Some(package) = project::metadata_package(metadata, &found.package) else {
        return unknown("it is not in the project's cargo metadata");
    };
    let (dependencies, dev) = project_dependencies(&package.dependencies);
    let mut warnings = Vec::new();
    let declared = declared_value(&package.declaration(), &mut warnings);
    release.warnings.extend(warnings);
    let told = Told {
        declared,
        dependencies: Some((dependencies, dev)),
        rest_in: "its Cargo.toml",
    };
    declared_here((state, release), told, (refresh, until), paging).await
}

/// The normal and build dependencies of a package of the project, as told, and how many dev ones
/// (decision 107).
#[cfg(feature = "local")]
fn project_dependencies(dependencies: &[crate::data::metadata::Dependency]) -> (Vec<Value>, usize) {
    let mut dev = 0_usize;
    let told = dependencies
        .iter()
        .filter_map(|dependency| {
            let kind = match dependency.kind.as_deref() {
                None | Some("normal") => "normal",
                Some("build") => "build",
                Some("dev") => {
                    dev = dev.saturating_add(1);
                    return None;
                },
                Some(_) => "other",
            };
            let mut told = Map::new();
            let given = dependency.rename.as_deref().unwrap_or(&dependency.name);
            told.insert("name".to_owned(), json!(given));
            if dependency.rename.is_some() {
                told.insert("package".to_owned(), json!(dependency.name));
            }
            told.insert("req".to_owned(), json!(dependency.req));
            told.insert("kind".to_owned(), json!(kind));
            if dependency.optional {
                told.insert("optional".to_owned(), json!(true));
            }
            if let Some(target) = &dependency.target {
                told.insert("target".to_owned(), json!(target));
            }
            if !dependency.uses_default_features {
                told.insert("default_features".to_owned(), json!(false));
            }
            Some(Value::Object(told))
        })
        .collect();
    (told, dev)
}

/// The error for a release that is not one of crates.io, without a project or a record to read
/// it from.
fn not_a_release(release: &Release) -> CallToolResult {
    let of = if release.source.as_ref() == "crates.io" {
        "a copy of the project that the crates.io index does not have"
    } else {
        &release.source
    };
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "build_unsupported",
        "message": format!(
            "{} {} is of {of}: crate_metadata reads crates.io releases",
            release.name, release.version
        ),
        "retryable": false,
        "suggestions": ["read the [package] of its Cargo.toml"],
    }))
}

/// What the release declares in its `.crate`, or `None` with why, when the `.crate` cannot be
/// had or read by `until` (requirement R-ARCH-6).
async fn declaration(
    state: &ToolState,
    entry: &IndexEntry,
    (name, version): (&CrateName, &Version),
    until: Instant,
) -> (Option<Declaration>, Option<Box<str>>) {
    let Some(checksum) = entry
        .versions
        .iter()
        .find(|listed| listed.version == *version)
        .map(|listed| listed.checksum.clone())
    else {
        // Told when the index is read.
        return (None, None);
    };
    let fetching = state
        .static_crates
        .crate_file(&state.cache, name, version, Some(&checksum));
    let Ok(contents) = tokio::time::timeout_at(until, fetching).await else {
        let why = format!(
            "what {name} {version} declares is not known yet: its .crate is being fetched; call \
             again for it"
        );
        return (None, Some(why.into()));
    };
    let read = match contents {
        Ok(Some(contents)) => {
            let (name, version) = (name.clone(), version.clone());
            crate::task::spawn_blocking(move || {
                crate_file::declaration(&contents, &name, &version).map(Some)
            })
            .await
            .unwrap_or_else(|error| {
                let error = crate::task::resume_if_panic(error);
                Err(crate_file::CrateError::Cache(std::io::Error::other(error)))
            })
        },
        other => other.map(|_| None),
    };
    match read {
        Ok(Some(declaration)) => (Some(declaration), None),
        Ok(None) => (
            None,
            Some(
                format!("what {name} {version} declares is not known: its .crate is not had")
                    .into(),
            ),
        ),
        Err(error) => (
            None,
            Some(
                format!(
                    "what {name} {version} declares is not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            ),
        ),
    }
}

/// What the crates.io API told, and why what it did not.
struct Api {
    info: Option<Cached<crate::data::crates_io::CrateInfo>>,
    dependents: Option<Cached<u64>>,
    history: Option<History>,
    warnings: Vec<Box<str>>,
}

/// The versions read of the API for the history.
struct History {
    /// The versions, newest first.
    versions: Vec<VersionInfo>,
    /// Whether they reach the first version.
    complete: bool,
    /// How many there are in all, as the first page tells.
    total: u64,
    /// When the oldest page was fetched.
    fetched_at: SystemTime,
    /// Why a page older than its TTL was used, if one was.
    stale: Option<Box<str>>,
    /// Why a page after the first did not come, if one did not.
    cut: Option<Box<str>>,
}

impl Api {
    /// Asks the API, in turn and by `until`, for what it tells of `name`; the history's pages
    /// from the first, at most [`MAX_NEW_PAGES`] that the cache does not have.
    async fn of(
        state: &ToolState,
        name: &CrateName,
        (fresh, until): (bool, Instant),
        paging: &Paging,
    ) -> Self {
        let api = &state.crates_io;
        let mut warnings = Vec::new();
        let info = match api.crate_info(name, fresh, until).await {
            Ok(info) => Some(info),
            Err(error) => {
                left_out(&mut warnings, "the downloads", &error);
                None
            },
        };
        let dependents = match api.dependents(name, fresh, until).await {
            Ok(dependents) => Some(dependents),
            Err(error) => {
                left_out(&mut warnings, "the count of dependents", &error);
                None
            },
        };
        let history = match Self::history(state, name, (fresh, until), paging).await {
            Ok(history) => Some(history),
            Err(error) => {
                left_out(&mut warnings, "the history", &error);
                None
            },
        };
        Self {
            info,
            dependents,
            history,
            warnings,
        }
    }

    /// The versions for the page of the history that `paging` asks for: pages of the API until
    /// enough changes are had past the offset, the last, or [`MAX_NEW_PAGES`] fetched. Only the
    /// first is asked `fresh`.
    async fn history(
        state: &ToolState,
        name: &CrateName,
        (fresh, until): (bool, Instant),
        paging: &Paging,
    ) -> Result<History, ApiError> {
        let started = SystemTime::now();
        let mut versions = Vec::new();
        let mut next: Option<Next> = None;
        let (mut new_pages, mut total, mut oldest) = (0_usize, None, started);
        let (mut stale_reason, mut cut) = (None, None);
        loop {
            let first = versions.is_empty();
            let page = match state
                .crates_io
                .versions(name, next.as_ref(), fresh && first, until)
                .await
            {
                Ok(page) => page,
                // A first page is needed; a later one that does not come ends the history here.
                Err(error) if first => return Err(error),
                Err(error) => {
                    cut = Some(error.to_string().into());
                    break;
                },
            };
            if page.fetched_at >= started {
                new_pages = new_pages.saturating_add(1);
            }
            oldest = oldest.min(page.fetched_at);
            stale_reason = stale_reason.or(page.stale);
            total = total.or(Some(page.value.total));
            versions.extend(page.value.versions);
            next = page.value.next;
            let enough = changes(&versions, After::Unknown).len()
                > paging.offset.saturating_add(paging.limit);
            if next.is_none() || enough || new_pages >= MAX_NEW_PAGES {
                break;
            }
        }
        Ok(History {
            complete: next.is_none(),
            versions,
            total: total.unwrap_or_default(),
            fetched_at: oldest,
            stale: stale_reason,
            cut,
        })
    }
}

/// Warns in `warnings` that `what` is left out, as the API did not tell it.
fn left_out(warnings: &mut Vec<Box<str>>, what: &str, error: &ApiError) {
    let again = matches!(
        error,
        ApiError::NoTurn | ApiError::RecentFailure { .. } | ApiError::Transient { .. }
    );
    warnings.push(
        format!(
            "{what} is left out: {}{}",
            ErrorChain(error),
            if again { "; call again for it" } else { "" }
        )
        .into(),
    );
}

/// `provenance` with when the oldest answer of the API used was fetched, and why one older than
/// its TTL was used, in `warnings`.
fn with_api(
    provenance: &Map<String, Value>,
    api: &Api,
    warnings: &mut Vec<Box<str>>,
) -> Map<String, Value> {
    let mut provenance = provenance.clone();
    let api_fetched = [
        api.info.as_ref().map(|info| info.fetched_at),
        api.dependents
            .as_ref()
            .map(|dependents| dependents.fetched_at),
        api.history.as_ref().map(|history| history.fetched_at),
    ];
    if let Some(oldest) = api_fetched.into_iter().flatten().min() {
        provenance.insert("api_fetched_at".to_owned(), json!(rfc3339(oldest)));
    }
    let stale = [
        (
            "the downloads",
            api.info.as_ref().and_then(|info| info.stale.as_ref()),
        ),
        (
            "the count of dependents",
            api.dependents
                .as_ref()
                .and_then(|dependents| dependents.stale.as_ref()),
        ),
        (
            "the history",
            api.history
                .as_ref()
                .and_then(|history| history.stale.as_ref()),
        ),
    ];
    for (what, why) in stale {
        if let Some(why) = why {
            warnings.push(format!("{what}: {why}").into());
        }
    }
    provenance
}

/// What is told of the package besides what crates.io tells of the crate.
struct Told {
    /// What it declares.
    declared: Value,
    /// Its normal and build dependencies, and how many dev ones, when they are known.
    dependencies: Option<(Vec<Value>, usize)>,
    /// Where the dependencies that do not fit are.
    rest_in: &'static str,
}

/// The answer.
fn respond(
    release: &mut Release,
    told: Told,
    crates_io: Option<(&IndexEntry, &Api)>,
    paging: &Paging,
) -> CallToolResult {
    let mut warnings = std::mem::take(&mut release.warnings);
    let Told {
        declared,
        dependencies,
        rest_in,
    } = told;
    let told_dependencies = dependencies.is_some();
    let (dependencies, dev) = dependencies.unwrap_or_default();
    let total = dependencies.len();
    let releases =
        crates_io.map(|(entry, api)| releases(entry, &release.version, api.history.as_ref()));
    let popularity = crates_io.and_then(|(_, api)| popularity(api));
    let provenance = match crates_io {
        Some((_, api)) => {
            warnings.extend(api.warnings.iter().cloned());
            with_api(&release.provenance, api, &mut warnings)
        },
        None => release.provenance.clone(),
    };
    let page = match crates_io.and_then(|(entry, api)| Some((entry, api.history.as_ref()?))) {
        Some((entry, history)) => {
            if let Some(cut) = &history.cut {
                warnings.push(
                    format!("the history stops here: the next versions did not come ({cut}); call again with the cursor").into(),
                );
            }
            match history_page(history, entry, paging) {
                Ok(page) => Some(page),
                Err(refused) => return *refused,
            }
        },
        None => None,
    };
    let answer = |dependencies: &[Value]| {
        let mut data = Map::new();
        data.insert("declared".to_owned(), declared.clone());
        if let Some(releases) = &releases {
            data.insert("releases".to_owned(), releases.clone());
        }
        if let Some(popularity) = &popularity {
            data.insert("popularity".to_owned(), popularity.clone());
        }
        if told_dependencies {
            data.insert("dependencies".to_owned(), json!(dependencies));
            if dependencies.len() < total {
                data.insert("dependencies_total".to_owned(), json!(total));
            }
            data.insert("dev_dependencies".to_owned(), json!(dev));
        }
        if let Some((shown, _)) = &page {
            data.insert("history".to_owned(), json!(shown));
        }
        let mut answer = Map::new();
        answer.insert("provenance".to_owned(), Value::Object(provenance.clone()));
        if let Some(after) = &release.retry_after {
            answer.insert("retry_after_secs".to_owned(), after.clone());
        }
        answer.insert("data".to_owned(), Value::Object(data));
        if let Some((_, told)) = &page {
            answer.insert("page".to_owned(), told.clone());
        }
        if !release.hints.is_empty() {
            answer.insert("hints".to_owned(), json!(release.hints));
        }
        answer
    };
    // Fewer dependencies while the response is over its budget, with how many there are and
    // where the rest is (requirement R-OUT-3).
    let mut shown = dependencies;
    loop {
        let mut warned = warnings.clone();
        if shown.len() < total {
            warned.push(
                format!(
                    "{} of {total} dependencies are shown, as the answer is long: the rest are in \
                     {rest_in}",
                    shown.len()
                )
                .into(),
            );
        }
        let mut response = Value::Object(answer(&shown));
        get_item::add_warnings(&mut response, &warned);
        let size = response.to_string().chars().count();
        if size <= MAX_RESPONSE_CHARS || shown.is_empty() {
            return CallToolResult::structured(response);
        }
        let fits = shown
            .len()
            .saturating_mul(MAX_RESPONSE_CHARS)
            .checked_div(size)
            .unwrap_or_default();
        shown.truncate(fits.min(shown.len().saturating_sub(1)));
    }
}

/// What the release declares: from its `.crate`, or else only its `rust-version` from the index.
fn declared(
    entry: &IndexEntry,
    version: &Version,
    declaration: Option<&Declaration>,
    warnings: &mut Vec<Box<str>>,
) -> Value {
    let mut declared = Map::new();
    let Some(declaration) = declaration else {
        if let Some(rust_version) = entry
            .versions
            .iter()
            .find(|listed| listed.version == *version)
            .and_then(|listed| listed.rust_version.as_deref())
        {
            declared.insert("rust_version".to_owned(), json!(rust_version));
        }
        return Value::Object(declared);
    };
    declared_value(declaration, warnings)
}

/// What `declaration` tells, with the strings cut (decision 108).
fn declared_value(declaration: &Declaration, warnings: &mut Vec<Box<str>>) -> Value {
    let mut declared = Map::new();
    let mut cut = Vec::new();
    let mut put = |key: &str, value: Option<&str>, max: usize| {
        let Some(value) = value else {
            return;
        };
        let (value, was_cut) = shortened(value, max);
        if was_cut {
            cut.push(key.to_owned());
        }
        declared.insert(key.to_owned(), json!(value));
    };
    put(
        "description",
        declaration.description.as_deref(),
        MAX_DESCRIPTION,
    );
    put("license", declaration.license.as_deref(), MAX_URL);
    put("license_file", declaration.license_file.as_deref(), MAX_URL);
    put("repository", declaration.repository.as_deref(), MAX_URL);
    put("homepage", declaration.homepage.as_deref(), MAX_URL);
    put(
        "documentation",
        declaration.documentation.as_deref(),
        MAX_URL,
    );
    put("rust_version", declaration.rust_version.as_deref(), MAX_URL);
    put("edition", declaration.edition.as_deref(), MAX_URL);
    if let Some(targets) = &declaration.docs_rs_targets {
        let targets: Vec<String> = targets.iter().map(ToString::to_string).collect();
        declared.insert("docs_rs_targets".to_owned(), json!(targets));
    }
    if let Some(target) = &declaration.docs_rs_default_target {
        declared.insert(
            "docs_rs_default_target".to_owned(),
            json!(target.to_string()),
        );
    }
    if !cut.is_empty() {
        warnings.push(
            format!(
                "{} cut, as longer than {MAX_DESCRIPTION} characters for description and \
                 {MAX_URL} for the others",
                cut.join(", ")
            )
            .into(),
        );
    }
    if declaration.docs_rs_targets_left_out > 0 {
        warnings.push(
            format!(
                "{} of the docs.rs targets it declares are left out: not target names",
                declaration.docs_rs_targets_left_out
            )
            .into(),
        );
    }
    Value::Object(declared)
}

/// `text` cut to `max` characters with `…`, and whether it was.
fn shortened(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    (cut, true)
}

/// The releases of the crate, from the index: the newest, how many, and how long ago the last
/// was published (decision 97); from the API when the index does not tell when.
fn releases(entry: &IndexEntry, version: &Version, history: Option<&History>) -> Value {
    let listed = || entry.versions.iter().filter(|listed| !listed.yanked);
    let latest = listed().map(|listed| &listed.version).max();
    let latest_stable = listed()
        .map(|listed| &listed.version)
        .filter(|version| version.pre.is_empty())
        .max();
    let yanked = entry.versions.iter().filter(|listed| listed.yanked).count();
    let mut releases = Map::new();
    if let Some(latest_stable) = latest_stable {
        releases.insert("latest_stable".to_owned(), json!(latest_stable.to_string()));
    }
    if let Some(latest) = latest {
        releases.insert("latest".to_owned(), json!(latest.to_string()));
    }
    releases.insert("versions".to_owned(), json!(entry.versions.len()));
    releases.insert("yanked".to_owned(), json!(yanked));
    if entry
        .versions
        .iter()
        .any(|listed| listed.version == *version && listed.yanked)
    {
        releases.insert("this_version_yanked".to_owned(), json!(true));
    }
    let from_index = listed()
        .filter_map(|listed| {
            let published = listed.published.as_deref()?;
            Some((parse_time(published)?, listed.version.to_string()))
        })
        .max();
    let from_api = || {
        history?
            .versions
            .iter()
            .filter(|version| !version.yanked)
            .filter_map(|version| Some((parse_time(&version.created_at)?, version.num.to_string())))
            .max()
    };
    if let Some((at, version)) = from_index.or_else(from_api) {
        let (days, recency) = recency(at, SystemTime::now());
        releases.insert("last_release".to_owned(), json!(version));
        releases.insert("last_release_at".to_owned(), json!(rfc3339(at)));
        releases.insert("days_since_last_release".to_owned(), json!(days));
        releases.insert("recency".to_owned(), json!(recency));
    }
    Value::Object(releases)
}

/// What the API tells of how much the crate is used, if it told any of it.
fn popularity(api: &Api) -> Option<Value> {
    let mut popularity = Map::new();
    if let Some(info) = &api.info {
        popularity.insert("downloads".to_owned(), json!(info.value.downloads));
        if let Some(recent) = info.value.recent_downloads {
            popularity.insert("recent_downloads".to_owned(), json!(recent));
        }
    }
    if let Some(dependents) = &api.dependents {
        popularity.insert("dependents".to_owned(), json!(dependents.value));
    }
    (!popularity.is_empty()).then_some(Value::Object(popularity))
}

/// The normal and build dependencies that the release declares, and how many dev ones (decision
/// 107).
fn dependencies(entry: &IndexEntry, version: &Version) -> (Vec<Value>, usize) {
    let declared = entry.dependencies(version).unwrap_or_default();
    let dev = declared
        .iter()
        .filter(|dependency| dependency.kind == DependencyKind::Dev)
        .count();
    let told = declared
        .iter()
        .filter(|dependency| dependency.kind != DependencyKind::Dev)
        .map(dependency_value)
        .collect();
    (told, dev)
}

/// A dependency as told.
fn dependency_value(dependency: &IndexDependency) -> Value {
    let mut told = Map::new();
    told.insert("name".to_owned(), json!(dependency.name));
    if let Some(package) = &dependency.package {
        told.insert("package".to_owned(), json!(package));
    }
    told.insert("req".to_owned(), json!(dependency.req));
    told.insert("kind".to_owned(), json!(dependency.kind));
    if dependency.optional {
        told.insert("optional".to_owned(), json!(true));
    }
    if let Some(target) = &dependency.target {
        told.insert("target".to_owned(), json!(target));
    }
    if !dependency.default_features {
        told.insert("default_features".to_owned(), json!(false));
    }
    Value::Object(told)
}

/// The changes of the page that `paging` asks for, and the page as told, with the cursor of the
/// next if any. The cursor is refused when the versions changed since it was made, as one more
/// yanked does, and when it goes past the end (requirement R-OUT-3).
fn history_page(
    history: &History,
    entry: &IndexEntry,
    paging: &Paging,
) -> Result<(Vec<Change>, Value), Box<CallToolResult>> {
    // The last version read is compared with the next one, which is not read yet: it waits.
    let found: Vec<Change> = match history.versions.split_last() {
        Some((last, before)) if !history.complete => changes(before, After::Version(last)),
        _ => changes(&history.versions, After::Nothing),
    };
    let yanked = entry.versions.iter().filter(|listed| listed.yanked).count();
    let newest = history.versions.first().map_or("", |first| &*first.num);
    let digest = sha256_hex(format!("{}:{newest}:{yanked}", history.total).as_bytes());
    let digest = digest.get(..16).unwrap_or_default();
    let refused = |message: &'static str| {
        Box::new(InvalidArgument::new(message, CURSOR_EXAMPLE).into_result())
    };
    if paging
        .cursor_data
        .as_deref()
        .is_some_and(|data| data != digest)
    {
        return Err(refused(
            "the versions changed since the cursor was made; read the history again without the \
             cursor",
        ));
    }
    if history.complete && paging.offset > found.len() {
        return Err(refused(
            "the cursor goes past the end of the history; read it again without the cursor",
        ));
    }
    let shown: Vec<Change> = found
        .iter()
        .skip(paging.offset)
        .take(paging.limit)
        .cloned()
        .collect();
    let after = paging.offset.saturating_add(shown.len());
    let has_more = after < found.len() || !history.complete;
    let mut page = Map::new();
    page.insert("shown".to_owned(), json!(shown.len()));
    if history.complete {
        page.insert("total".to_owned(), json!(found.len()));
    }
    page.insert("has_more".to_owned(), json!(has_more));
    if has_more {
        page.insert(
            "next_cursor".to_owned(),
            json!(format!("{}.{digest}.{after}", paging.request_digest)),
        );
    }
    Ok((shown, Value::Object(page)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::sparse_index::IndexVersion;

    fn entry(versions: &[(&str, bool, Option<&str>)]) -> IndexEntry {
        IndexEntry {
            name: "demo".into(),
            versions: versions
                .iter()
                .map(|&(version, yanked, published)| IndexVersion {
                    version: Version::parse(version).expect("a version"),
                    checksum: "00".into(),
                    yanked,
                    rust_version: None,
                    published: published.map(Into::into),
                    line: 0..0,
                })
                .collect(),
            fetched_at: SystemTime::UNIX_EPOCH,
            stale: None,
            body: Arc::from(&b""[..]),
        }
    }

    fn version(num: &str, license: &str, yanked: bool) -> VersionInfo {
        VersionInfo {
            num: num.into(),
            license: Some(license.into()),
            created_at: "2026-01-01T00:00:00.5Z".into(),
            yanked,
            rust_version: None,
            edition: None,
        }
    }

    fn history(versions: Vec<VersionInfo>, complete: bool) -> History {
        History {
            total: u64::try_from(versions.len()).expect("small"),
            versions,
            complete,
            fetched_at: SystemTime::UNIX_EPOCH,
            stale: None,
            cut: None,
        }
    }

    fn paging(limit: usize, offset: usize, cursor_data: Option<&str>) -> Paging {
        Paging {
            limit,
            offset,
            request_digest: "c1.r".into(),
            cursor_data: cursor_data.map(Into::into),
        }
    }

    #[test]
    fn the_history_is_paged_and_a_cursor_for_other_versions_refused() {
        let versions = vec![
            version("1.2.0", "MIT", false),
            version("1.1.0", "MIT", true),
            version("1.0.0", "Apache-2.0", false),
        ];
        let index = entry(&[
            ("1.0.0", false, None),
            ("1.1.0", true, None),
            ("1.2.0", false, None),
        ]);
        let complete = history(versions.clone(), true);
        let (shown, page) = history_page(&complete, &index, &paging(1, 0, None)).expect("a page");
        assert_eq!(&*shown[0].version, "1.1.0");
        assert_eq!(page["has_more"], true);
        let cursor = page["next_cursor"].as_str().expect("a cursor");
        let digest = cursor.split('.').nth(2).expect("its digest");
        let (shown, page) =
            history_page(&complete, &index, &paging(1, 1, Some(digest))).expect("a page");
        assert_eq!((&*shown[0].version, shown[0].first), ("1.0.0", true));
        assert_eq!(page, json!({ "shown": 1, "total": 2, "has_more": false }));
        // One more yanked: the cursor is of other versions.
        let more_yanked = entry(&[
            ("1.0.0", true, None),
            ("1.1.0", true, None),
            ("1.2.0", false, None),
        ]);
        let refused = |index: &IndexEntry, offset| {
            history_page(&complete, index, &paging(1, offset, Some(digest))).err()
        };
        assert!(refused(&more_yanked, 1).is_some(), "another digest");
        assert!(refused(&index, 3).is_some(), "past the end");
        // Not complete: the last version read waits for the next page.
        let cut = history(versions, false);
        let (shown, page) = history_page(&cut, &index, &paging(10, 0, None)).expect("a page");
        assert_eq!(shown.len(), 1);
        assert_eq!(page["has_more"], true);
        assert!(page.get("total").is_none());
    }

    #[test]
    fn releases_leave_out_the_yanked_and_take_the_api_without_the_index() {
        let index = entry(&[
            ("1.0.0", false, Some("2026-01-01T00:00:00Z")),
            ("1.1.0", true, Some("2026-02-01T00:00:00Z")),
            ("2.0.0-rc.1", false, Some("2026-01-15T00:00:00Z")),
        ]);
        let told = releases(&index, &Version::new(1, 1, 0), None);
        assert_eq!(told["latest_stable"], "1.0.0");
        assert_eq!(told["latest"], "2.0.0-rc.1");
        assert_eq!(
            (told["versions"].as_u64(), told["yanked"].as_u64()),
            (Some(3), Some(1))
        );
        assert_eq!(told["this_version_yanked"], true);
        assert_eq!(told["last_release"], "2.0.0-rc.1");
        assert_eq!(told["last_release_at"], "2026-01-15T00:00:00Z");
        // No `pubtime`: when the API published it.
        let without = entry(&[("1.0.0", false, None)]);
        let api = history(vec![version("1.0.0", "MIT", false)], true);
        let from_api = releases(&without, &Version::new(1, 0, 0), Some(&api));
        assert_eq!(from_api["last_release_at"], "2026-01-01T00:00:00Z");
        assert!(from_api.get("this_version_yanked").is_none());
    }

    /// A package of the project as `cargo metadata` tells it, with what it inherits resolved: its
    /// declaration, and its dependencies by the name it gives them (decided in M3f, decision 98).
    #[cfg(feature = "local")]
    #[test]
    fn a_package_of_the_project_tells_what_it_declares_and_its_dependencies() {
        let package: crate::data::metadata::Package = serde_json::from_value(json!({
            "id": "path+file:///w/front#0.1.0",
            "name": "front",
            "version": "0.1.0",
            "source": null,
            "manifest_path": "/w/front/Cargo.toml",
            "targets": [],
            "license": "MIT",
            "rust_version": "1.85",
            "edition": "2024",
            "description": " ",
            "metadata": { "docs": { "rs": { "targets": ["x86_64-unknown-linux-gnu", "..x"] } } },
            "dependencies": [
                { "name": "serde_json", "rename": "json", "req": "^1", "kind": null,
                  "optional": true, "uses_default_features": false, "target": "cfg(unix)" },
                { "name": "cc", "req": "^1.2", "kind": "build" },
                { "name": "proptest", "req": "^1", "kind": "dev" },
            ],
        }))
        .expect("a package");
        let declaration = package.declaration();
        assert_eq!(declaration.license.as_deref(), Some("MIT"));
        assert_eq!(declaration.description, None, "blank");
        assert_eq!(
            declaration.docs_rs_targets.map(|targets| targets.len()),
            Some(1)
        );
        assert_eq!(declaration.docs_rs_targets_left_out, 1);
        let (dependencies, dev) = project_dependencies(&package.dependencies);
        assert_eq!(dev, 1);
        assert_eq!(
            json!(dependencies),
            json!([
                { "name": "json", "package": "serde_json", "req": "^1", "kind": "normal",
                  "optional": true, "target": "cfg(unix)", "default_features": false },
                { "name": "cc", "req": "^1.2", "kind": "build" },
            ])
        );
    }

    #[test]
    fn a_long_description_is_cut_and_told() {
        let declaration = Declaration {
            description: Some("x".repeat(MAX_DESCRIPTION + 1).into()),
            license: Some("MIT".into()),
            ..Declaration::default()
        };
        let mut warnings = Vec::new();
        let declared = declared(
            &entry(&[]),
            &Version::new(1, 0, 0),
            Some(&declaration),
            &mut warnings,
        );
        let description = declared["description"].as_str().expect("told");
        assert_eq!(description.chars().count(), MAX_DESCRIPTION);
        assert!(description.ends_with('…'));
        assert_eq!(declared["license"], "MIT");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.starts_with("description cut"))
        );
    }
}
