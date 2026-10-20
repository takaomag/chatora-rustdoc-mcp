//! The `get_item` tool (`doc/m0-plan.md`, A8): one item of a crate from its docs.rs rustdoc JSON.
//!
//! M0a takes only an exact version, reads only docs.rs, and parses the JSON on every call.

#[cfg(feature = "local")]
use std::borrow::Cow;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use semver::Version;
use serde_json::{Value, json};

use super::{
    ErrorChain,
    InvalidArgument,
    ToolState,
    error_result,
    project::{self, InProject},
};
#[cfg(feature = "local")]
use crate::data::{
    features::FeatureSets,
    packages::{DocsRsCheck, Lookup as PackageLookup, PackageGraph, ProjectPackage, Source},
    project::Workspace,
};
use crate::{
    data::{
        self,
        LoadError,
        archive::Mapped,
        cache::Cache,
        crate_file::{self, DocsRsBuild},
        crate_name::CrateName,
        dist::{DistError, StdCrate},
        docs_rs::FetchError,
        rustdoc_json::ParseError,
        sparse_index::IndexError,
        std_json::{self, Chosen},
        target::TargetTriple,
        version::{NoVersion, VersionSpec},
    },
    query::{
        across::{self, Indexed},
        get_item::{
            Destination,
            Detail,
            GetItemError,
            ItemRequest,
            Origin,
            ProjectFeatures,
            ResolvedFrom,
            get_item,
        },
        load,
        view::CrateView,
    },
};

/// Name of the tool in `tools/list` and `tools/call`.
pub(super) const NAME: &str = "get_item";

/// Description for `tools/list` (§6.2), at most 2,048 characters.
const DESCRIPTION: &str = include_str!("get_item.md");

/// How to pass `crate`, for `invalid_argument`.
const CRATE_EXAMPLE: &[&str] = &[
    r#"pass the crate, with a version if it matters, for example {"crate": "tokio"} or {"crate": "tokio@1.47.1"}"#,
];

/// How to pass `version`, for `invalid_argument`.
const VERSION_EXAMPLE: &[&str] = &[
    r#"pass an exact version (1.47.1), a semver requirement (^1.47) or latest, for example {"version": "1.47.1"}"#,
];

/// How to pass `target`, for `invalid_argument`.
const TARGET_EXAMPLE: &[&str] =
    &[r#"pass a target triple, for example {"target": "x86_64-unknown-linux-gnu"}"#];

/// How to pass `project`, for `invalid_argument`.
const PROJECT_EXAMPLE: &[&str] = &[
    r#"pass the directory or the Cargo.toml of a workspace, for example {"project": "/home/me/app"}"#,
];

/// How to pass `path`, for `invalid_argument`.
const PATH_EXAMPLE: &[&str] =
    &[r#"pass the path of the item, for example {"path": "tokio::sync::Mutex::lock"}"#];

/// How to pass `detail`, for `invalid_argument`.
const DETAIL_EXAMPLE: &[&str] = &[r#"pass brief, normal or full, for example {"detail": "full"}"#];

/// The most candidates an `ambiguous_path` error lists (requirement R-OUT-3).
const MAX_CANDIDATES: usize = 20;

/// Checked arguments.
#[derive(Debug)]
pub(super) struct Arguments {
    name: CrateName,
    /// The version asked for, in `crate` or `version`. Without one, the project's is used.
    version: Option<VersionSpec>,
    /// The project whose Cargo.lock decides the version, when it is not the default one.
    project: Option<PathBuf>,
    /// The path, starting with the crate name.
    path: Box<str>,
    /// The target asked for.
    target: Option<TargetTriple>,
    /// How much to show.
    detail: Detail,
}

/// What is looked up, once the version and the target are decided.
#[derive(Debug)]
struct Request {
    /// The crate, as crates.io spells it when the index was read.
    name: CrateName,
    version: Version,
    /// Where the project came from, when the project decided the version.
    project_from: Option<&'static str>,
    /// The path, starting with the crate name.
    path: Box<str>,
    resolved_from: ResolvedFrom,
    target: TargetTriple,
    detail: Detail,
    /// The sha256 of the `.crate` in the crates.io index, when the index was read.
    checksum: Option<Box<str>>,
    /// How docs.rs built it, once its `.crate` is read (requirement R-FT-1).
    build: Option<Box<DocsRsBuild>>,
    /// The project's build of it, when the project decided the version.
    in_build: InBuild,
    /// How many crates the path was followed through to get here (requirement R-PR-4).
    hops: usize,
}

/// The most crates that a path is followed into, through re-exports of their items, after the
/// one asked for (requirement R-PR-4).
const MAX_FOLLOWED: usize = 3;

/// The project's build of the crate, for `enabled_in_project` (requirement R-FT-2). Empty when
/// the project did not decide the version, and without local features.
#[derive(Debug, Default)]
struct InBuild {
    /// The workspace that decided the version.
    #[cfg(feature = "local")]
    workspace: Option<Workspace>,
    /// The features of its packages in its build for the target, once read.
    #[cfg(feature = "local")]
    sets: Option<Arc<FeatureSets>>,
}

impl InBuild {
    /// The features of `name` at `version` in the build, if they were read.
    #[cfg(feature = "local")]
    fn features(&self, name: &str, version: &Version) -> Option<ProjectFeatures<'_>> {
        let sets = self.sets.as_ref()?;
        Some(ProjectFeatures {
            normal: sets.normal(name, version),
            with_dev: sets.with_dev(name, version),
        })
    }

    /// Without local features, there is no project.
    #[cfg(not(feature = "local"))]
    #[expect(
        clippy::unused_self,
        reason = "the same signature as with local features"
    )]
    const fn features(&self, _name: &str, _version: &Version) -> Option<ProjectFeatures<'_>> {
        None
    }
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio), or the crate and its version as name@version \
                                (tokio@1.47.1).",
            },
            "version": {
                "type": "string",
                "description": "The version: exact (1.47.1, not ^1.47.1), a semver requirement \
                                (^1.47, ~1.47) or latest. Defaults to the latest.",
            },
            "target": {
                "type": "string",
                "description": "The target triple to document for. Defaults to the host's.",
            },
            "project": {
                "type": "string",
                "description": "The workspace (its directory or Cargo.toml) whose Cargo.lock \
                                decides the version when none is given. Defaults to the project \
                                Claude Code was opened in.",
            },
            "path": {
                "type": "string",
                "description": "The path of the item (tokio::sync::Mutex::lock). The crate name \
                                may be left out.",
            },
            "detail": {
                "type": "string",
                "enum": ["brief", "normal", "full"],
                "description": "brief: the signature only. normal (the default): also the first \
                                paragraph of the documentation and the headings of its sections. \
                                full: the whole documentation and the attributes.",
            },
        },
        "required": ["crate", "path"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Get a Rust item")
        .with_raw_output_schema(Arc::new(output_schema()))
        // It reads docs.rs and writes the cache, and will build docs locally (requirement
        // R-MCP-4).
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(true),
        )
}

/// The schema of the structured content: `ItemResponse` in `query::get_item` (requirement
/// R-OUT-1). Every property but the required ones is left out when it has no value.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "enum": ["docs.rs", "std"] },
                    "crate": { "type": "string" },
                    "version": { "type": "string" },
                    "resolved_from": {
                        "type": "string",
                        "enum": [
                            "explicit",
                            "explicit_req",
                            "latest",
                            "cargo_lock",
                            "cargo_lock_raw",
                            "reexport",
                        ],
                    },
                    "project_from": {
                        "type": "string",
                        "enum": ["argument", "CLAUDE_PROJECT_DIR", "cwd"],
                    },
                    "toolchain": { "type": "string" },
                    "std_from": {
                        "type": "object",
                        "properties": {
                            "source": { "type": "string", "enum": ["dist", "local"] },
                            "reason": { "type": "string" },
                        },
                        "required": ["source", "reason"],
                    },
                    "target": { "type": "string" },
                    "format_version": { "type": "integer" },
                    "features": strings,
                    "all_features": { "type": "boolean" },
                    "cfg": strings,
                },
                "required": ["source", "crate", "version", "target"],
            },
            "warnings": strings,
            "data": data_schema(),
            "hints": strings,
        },
        "required": ["provenance", "data"],
    }))
}

/// The schema of `data`: `ItemData` in `query::get_item`.
fn data_schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "The canonical path." },
            "kind": { "type": "string" },
            "parent": { "type": "string" },
            "impl": { "type": "string" },
            "signature": { "type": "string" },
            "where": strings,
            "fields": strings,
            "variants": strings,
            "required_features": {
                "type": "string",
                "description": "The features the item needs (any(rt, rt-multi-thread)).",
            },
            "other_cfg": strings,
            "cfg_source": {
                "type": "string",
                "enum": ["doc_cfg", "cfg_trace", "inherited", "unknown"],
            },
            "enabled_in_project": {
                "enum": [true, false, "unknown"],
                "description": "Whether the project's normal build has the item.",
            },
            "enabled_in_dev_only": { "type": "boolean" },
            "docs": {
                "type": "string",
                "description": "The first paragraph of the documentation, or with \
                                detail=full what comes before its first heading.",
            },
            "docs_truncated": { "type": "boolean" },
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "heading": { "type": "string" },
                        "lines": { "type": "integer" },
                        "text": { "type": "string" },
                    },
                    "required": ["heading", "lines"],
                },
            },
            "attrs": strings,
            "deprecated": {
                "type": "object",
                "properties": {
                    "since": { "type": "string" },
                    "note": { "type": "string" },
                },
            },
            "must_use": { "type": "boolean" },
            "non_exhaustive": { "type": "boolean" },
            "stability": {
                "type": "object",
                "description": "For std: since which Rust release it is stable, or the feature it \
                                needs; the same for its use in const.",
                "properties": {
                    "stable_since": { "type": "string" },
                    "unstable_feature": { "type": "string" },
                    "const_stable_since": { "type": "string" },
                    "const_unstable_feature": { "type": "string" },
                },
            },
            "also_reexported_at": strings,
            "reexport_of": {
                "type": "object",
                "properties": {
                    "crate": { "type": "string" },
                    "path": { "type": "string" },
                },
                "required": ["crate", "path"],
            },
            "source_location": {
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "lines": { "type": "string" },
                    "url": { "type": "string" },
                },
                "required": ["file", "lines"],
            },
            "truncated": {
                "type": "boolean",
                "description": "Something was cut to keep the response small.",
            },
        },
        "required": ["path", "kind"],
    })
}

/// The object inside `value`, which is always an object here.
fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            JsonObject::new()
        },
    }
}

/// Checks the arguments that name a release: `crate`, `version`, `project` and `target`
/// (requirement R-ARG-1). `resolve_version` takes these, too.
pub(super) fn parse_release(
    arguments: Option<&JsonObject>,
) -> Result<ReleaseArguments, InvalidArgument> {
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let string = |name: &'static str, example| match argument(name) {
        Some(Value::String(value)) => Ok(Some(value.trim())),
        Some(_) => Err(InvalidArgument::new(
            format!("`{name}` must be a string"),
            example,
        )),
        None => Ok(None),
    };
    let Some(crate_argument) = string("crate", CRATE_EXAMPLE)? else {
        return Err(InvalidArgument::new("`crate` is required", CRATE_EXAMPLE));
    };
    let (name, in_crate) = match crate_argument.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (crate_argument, None),
    };
    let name: CrateName = name.parse().map_err(|error| {
        InvalidArgument::new(
            format!("`crate` has an invalid name: {error}"),
            CRATE_EXAMPLE,
        )
    })?;
    let parse_version = |version: &str, example| {
        version.parse::<VersionSpec>().map_err(|error| {
            InvalidArgument::new(format!("the version is not valid: {error}"), example)
        })
    };
    let version = match (in_crate, string("version", VERSION_EXAMPLE)?) {
        (Some(in_crate), Some(argument)) => {
            let (in_crate, argument) = (
                parse_version(in_crate, CRATE_EXAMPLE)?,
                parse_version(argument, VERSION_EXAMPLE)?,
            );
            if in_crate != argument {
                return Err(InvalidArgument::new(
                    format!(
                        "`crate` has the version {in_crate} but `version` is {argument}; give it \
                         once"
                    ),
                    VERSION_EXAMPLE,
                ));
            }
            Some(in_crate)
        },
        (Some(version), None) => Some(parse_version(version, CRATE_EXAMPLE)?),
        (None, Some(version)) => Some(parse_version(version, VERSION_EXAMPLE)?),
        (None, None) => None,
    };
    let project = string("project", PROJECT_EXAMPLE)?.map(PathBuf::from);
    let target = string("target", TARGET_EXAMPLE)?
        .map(|target| {
            target.parse::<TargetTriple>().map_err(|error| {
                InvalidArgument::new(format!("`target` is not valid: {error}"), TARGET_EXAMPLE)
            })
        })
        .transpose()?;
    Ok(ReleaseArguments {
        name,
        version,
        project,
        target,
    })
}

/// The arguments that name a release, checked.
#[derive(Debug)]
pub(super) struct ReleaseArguments {
    pub(super) name: CrateName,
    /// The version asked for, in `crate` or `version`. Without one, the project's is used.
    pub(super) version: Option<VersionSpec>,
    /// The project whose Cargo.lock decides the version, when it is not the default one.
    pub(super) project: Option<PathBuf>,
    /// The target asked for.
    pub(super) target: Option<TargetTriple>,
}

/// Checks the arguments (requirement R-ARG-1).
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<Arguments, InvalidArgument> {
    let ReleaseArguments {
        name,
        version,
        project,
        target,
    } = parse_release(arguments)?;
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let string = |name: &'static str, example| match argument(name) {
        Some(Value::String(value)) => Ok(Some(value.trim())),
        Some(_) => Err(InvalidArgument::new(
            format!("`{name}` must be a string"),
            example,
        )),
        None => Ok(None),
    };
    let Some(path) = string("path", PATH_EXAMPLE)? else {
        return Err(InvalidArgument::new("`path` is required", PATH_EXAMPLE));
    };
    let path = path.trim_start_matches("::");
    if path.is_empty() {
        return Err(InvalidArgument::new(
            "`path` must not be empty",
            PATH_EXAMPLE,
        ));
    }
    let path = with_crate_name(&name, path);
    let detail = match string("detail", DETAIL_EXAMPLE)? {
        None | Some("normal") => Detail::Normal,
        Some("brief") => Detail::Brief,
        Some("full") => Detail::Full,
        Some(other) => {
            return Err(InvalidArgument::new(
                format!("`detail` is {other:?}, not brief, normal or full"),
                DETAIL_EXAMPLE,
            ));
        },
    };
    Ok(Arguments {
        name,
        version,
        project,
        path,
        target,
        detail,
    })
}

/// Decides the version (requirements R-VR-1 and R-VR-3) for `target`. An exact version whose JSON
/// for `target` is cached is taken as it is; otherwise the crates.io index is read, which also
/// gives the crate's published spelling. Returns the request with the warnings about how it was
/// decided, or the tool error (boxed, as a result is large).
async fn resolve(
    state: &ToolState,
    arguments: Arguments,
    target: TargetTriple,
) -> Result<(Request, Vec<Box<str>>), Box<CallToolResult>> {
    let mut warnings: Vec<Box<str>> = stand_in_warning(state, &arguments, &target)
        .into_iter()
        .collect();
    let Arguments {
        name,
        version,
        project,
        path,
        detail,
        ..
    } = arguments;
    // Without a version, the project decides it (requirement R-VR-2), or else it is the latest.
    #[cfg(feature = "local")]
    let mut from_project = None;
    let spec = match version {
        Some(spec) => spec,
        None => match project::version_in_project(state, &name, project.as_deref()).await {
            #[cfg(feature = "local")]
            InProject::Found(found) => {
                warnings.extend(found.warnings.iter().cloned());
                let version = found.package.version.clone();
                from_project = Some(found);
                VersionSpec::Exact(version)
            },
            InProject::Elsewhere(elsewhere) => {
                warnings.extend(elsewhere);
                VersionSpec::Latest
            },
            #[cfg(feature = "local")]
            InProject::Error(error) => return Err(error),
        },
    };
    // A package of the project that is not from crates.io is not looked for there: docs.rs does
    // not have it, even when crates.io has a crate of that name (requirement R-VR-2).
    #[cfg(feature = "local")]
    if let Some(found) = &from_project
        && found.package.source != Source::CratesIo
    {
        return Err(Box::new(local_build_required(
            &found.package,
            DocsRsCheck::NotCratesIo,
        )));
    }
    #[cfg(feature = "local")]
    let checked_later = from_project.is_some();
    #[cfg(not(feature = "local"))]
    let checked_later = false;
    if !checked_later && let Some(version) = cached_exact(state, &name, &spec, &target) {
        let request = Request {
            version,
            path: with_lib_name(&name.cache_key(), &path),
            name,
            project_from: None,
            resolved_from: ResolvedFrom::Explicit,
            target,
            detail,
            checksum: None,
            build: None,
            in_build: InBuild::default(),
            hops: 0,
        };
        return Ok((request, warnings));
    }
    let entry = state
        .index
        .entry(&name)
        .await
        .map_err(|error| Box::new(index_error(&name, &error)))?;
    // docs.rs has the project's package only if it is the one on crates.io (requirement R-VR-2).
    #[cfg(feature = "local")]
    if let Some(found) = &from_project {
        let check = found.package.docs_rs_check(&entry);
        if check != DocsRsCheck::Allowed {
            return Err(Box::new(local_build_required(&found.package, check)));
        }
    }
    let resolved = spec
        .resolve(&entry.versions)
        .map_err(|no_version| Box::new(version_not_found(&entry.name, &spec, &no_version)))?;
    warnings.extend(resolved.warnings.into_iter().map(Into::into));
    #[cfg(feature = "local")]
    let (resolved_from, project_from, in_build) = chosen_by(&spec, from_project);
    #[cfg(not(feature = "local"))]
    let (resolved_from, project_from, in_build) = chosen_by(&spec);
    // The name as published, for docs.rs and the cache, and at the head of the path as code writes
    // it (`Serde_Json::Value` is `serde_json::Value`).
    let published: CrateName = entry.name.parse().unwrap_or(name);
    let path = with_lib_name(published.as_str(), &path);
    let request = Request {
        name: published,
        version: resolved.version.version.clone(),
        checksum: Some(resolved.version.checksum.clone()),
        build: None,
        in_build,
        hops: 0,
        project_from,
        path,
        resolved_from,
        target,
        detail,
    };
    Ok((request, warnings))
}

/// The warning that `target` stands in for the host target, when no target was asked for and the
/// host target is not known.
fn stand_in_warning(
    state: &ToolState,
    arguments: &Arguments,
    target: &TargetTriple,
) -> Option<Box<str>> {
    let why = state.host.stand_in_because.as_ref()?;
    arguments.target.is_none().then(|| {
        format!("the target {target} is used, as the host target is not known: {why}").into()
    })
}

/// How the version was chosen, where the project came from if it chose it, and the project's
/// build of the crate.
#[cfg(feature = "local")]
fn chosen_by(
    spec: &VersionSpec,
    from_project: Option<Box<project::Found>>,
) -> (ResolvedFrom, Option<&'static str>, InBuild) {
    from_project.map_or_else(
        || chosen_by_argument(spec),
        |found| {
            let in_build = InBuild {
                workspace: Some(found.workspace),
                sets: None,
            };
            (found.resolved_from, Some(found.project_from), in_build)
        },
    )
}

/// How the version was chosen: without local features, by the argument.
#[cfg(not(feature = "local"))]
fn chosen_by(spec: &VersionSpec) -> (ResolvedFrom, Option<&'static str>, InBuild) {
    chosen_by_argument(spec)
}

/// How the version was chosen by the argument `spec`.
fn chosen_by_argument(spec: &VersionSpec) -> (ResolvedFrom, Option<&'static str>, InBuild) {
    let resolved_from = match spec {
        VersionSpec::Exact(_) => ResolvedFrom::Explicit,
        VersionSpec::Requirement(_) => ResolvedFrom::ExplicitReq,
        VersionSpec::Latest => ResolvedFrom::Latest,
    };
    (resolved_from, None, InBuild::default())
}

/// The exact version that `spec` names, if the JSON of `name` at it for `target` is cached: it is
/// then taken without reading the crates.io index, so that it is found offline.
fn cached_exact(
    state: &ToolState,
    name: &CrateName,
    spec: &VersionSpec,
    target: &TargetTriple,
) -> Option<Version> {
    let VersionSpec::Exact(version) = spec else {
        return None;
    };
    let dir = state.cache.docs_rs_dir(name, version, Some(target));
    matches!(state.cache.docs_rs_source(&dir), Ok(Some(_))).then(|| version.clone())
}

/// `version_not_found` (§8), with the versions to suggest.
pub(super) fn version_not_found(
    name: &str,
    spec: &VersionSpec,
    no_version: &NoVersion,
) -> CallToolResult {
    error_result(&json!({
        "code": "version_not_found",
        "message": format!("{name} has no version {spec} on crates.io"),
        "retryable": false,
        "suggestions": ["ask for one of the candidates"],
        "candidates": no_version
            .suggestions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    }))
}

/// `docs_unavailable` with `reason: local_build_required`: the project's package is not the one
/// on crates.io, and this server does not build documentation yet (decided in M1, until M2).
#[cfg(feature = "local")]
fn local_build_required(package: &ProjectPackage, check: DocsRsCheck) -> CallToolResult {
    let why = not_on_docs_rs(package, check);
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!(
            "{}@{} in the project is {why}, so docs.rs does not have it, and this server does not \
             build documentation yet",
            package.name, package.version
        ),
        "retryable": false,
        "suggestions": [
            "give the version, as crate@version, to read the crates.io release of that name on \
             docs.rs, which may differ from the project's",
        ],
        "reason": "local_build_required",
    }))
}

/// What the project's `package` is, that docs.rs does not have it (`check`).
#[cfg(feature = "local")]
pub(super) fn not_on_docs_rs(package: &ProjectPackage, check: DocsRsCheck) -> Cow<'_, str> {
    match (check, &package.source) {
        (DocsRsCheck::NotCratesIo, Source::Local) => {
            "a workspace member or a path dependency".into()
        },
        (DocsRsCheck::NotCratesIo, Source::Git(source)) => format!("from git ({source})").into(),
        (DocsRsCheck::NotCratesIo, Source::Registry(source)) => {
            format!("from another registry ({source})").into()
        },
        (DocsRsCheck::ChecksumMismatch, _) => {
            "a copy whose checksum in Cargo.lock differs from crates.io's (patched or vendored)"
                .into()
        },
        (DocsRsCheck::NoChecksum, _) => "without a checksum in Cargo.lock".into(),
        (DocsRsCheck::NotInIndex, _) => "a version that crates.io does not list".into(),
        (DocsRsCheck::NotCratesIo, Source::CratesIo) | (DocsRsCheck::Allowed, _) => {
            "not the release on crates.io".into()
        },
    }
}

/// The tool error for an index that could not be read (§8).
pub(super) fn index_error(name: &CrateName, error: &IndexError) -> CallToolResult {
    let message = format!(
        "the versions of {name} could not be read: {}",
        ErrorChain(error)
    );
    match error {
        IndexError::NotFound => error_result(&json!({
            "code": "crate_not_found",
            "message": format!("crates.io has no crate {name}"),
            "retryable": false,
            "suggestions": ["check the spelling of the crate name"],
        })),
        IndexError::Transient { .. } | IndexError::Network(_) => error_result(&json!({
            "code": "network_error",
            "message": message,
            "retryable": true,
            "suggestions": ["try again in a while"],
        })),
        IndexError::UnexpectedStatus { .. }
        | IndexError::TooLarge { .. }
        | IndexError::Malformed(_) => error_result(&json!({
            "code": "network_error",
            "message": message,
            "retryable": false,
            "suggestions": [],
        })),
        IndexError::Cache(_) => super::tool_error(
            "internal_error",
            "the cache of the crates.io index could not be used",
            false,
            &["check the server's stderr log for the cache directory and the error"],
        ),
    }
}

/// `path`, starting with the crate name: it is added when the first segment is something else.
/// `-` and `_` in the crate name are the same.
fn with_crate_name(name: &CrateName, path: &str) -> Box<str> {
    let first = path.split("::").next().unwrap_or_default();
    if same_crate_name(first, name.as_str()) {
        path.into()
    } else {
        format!("{}::{path}", name.as_str().replace('-', "_")).into()
    }
}

/// `path` with the crate name at its head written as code writes it: `name` with `_` for `-`
/// (`Serde-JSON::Value` asked of `serde_json` is `serde_json::Value`). A lib name that differs
/// more (`md5` of `md-5`) is put in its place by the lookup, from the JSON.
fn with_lib_name(name: &str, path: &str) -> Box<str> {
    let lib = name.replace('-', "_");
    let rest = match path.split_once("::") {
        Some((first, rest)) if same_crate_name(first, name) => rest,
        None if same_crate_name(path, name) => "",
        Some(_) | None => path,
    };
    if rest.is_empty() {
        lib.into()
    } else {
        format!("{lib}::{rest}").into()
    }
}

/// Whether `a` and `b` name the same crate, with `-` and `_` the same and the case ignored, as
/// crates.io does (requirement R-PR-1).
fn same_crate_name(a: &str, b: &str) -> bool {
    let normalized = |byte: &u8| {
        if *byte == b'-' {
            b'_'
        } else {
            byte.to_ascii_lowercase()
        }
    };
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(a, b)| normalized(&a) == normalized(&b))
}

/// Looks the item up in the archive of the crate, or else loads the JSON of the crate and archives
/// it first, on a blocking thread, since parsing a large crate takes a large part of a second.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    if let Some(krate) = std_crate(&arguments.name) {
        return execute_std(arguments, krate, state, Vec::new()).await;
    }
    let target = arguments
        .target
        .clone()
        .unwrap_or_else(|| state.host.triple.clone());
    let (arguments, warnings) = match resolve(&state, arguments, target).await {
        Ok(resolved) => resolved,
        Err(error) => return *error,
    };
    look_up_release(&state, arguments, warnings).await
}

/// Looks the item up in the release that `arguments` names, from its archive or its JSON on
/// docs.rs, and follows the path into the crate it goes on in, if any (requirement R-PR-4).
async fn look_up_release(
    state: &Arc<ToolState>,
    mut arguments: Request,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    arguments.build = docs_rs_build(state, &arguments, &mut warnings)
        .await
        .map(Box::new);
    // The name as published, when the version was decided without the crates.io index.
    if let Some(published) = arguments
        .build
        .as_ref()
        .and_then(|build| build.package_name.as_deref()?.parse::<CrateName>().ok())
    {
        arguments.name = published;
    }
    #[cfg(feature = "local")]
    {
        arguments.in_build.sets = feature_sets(state, &arguments, &mut warnings).await;
    }
    let target = Some(&arguments.target);
    let dir = state
        .cache
        .docs_rs_dir(&arguments.name, &arguments.version, target);
    let arguments =
        match look_up_archived(state.cache.clone(), dir.clone(), Box::new(arguments)).await {
            Some(Ok((looked_up, arguments))) => {
                return conclude(state, looked_up, *arguments, warnings).await;
            },
            Some(Err(arguments)) => *arguments,
            None => return shutting_down(),
        };
    let loaded = match data::docs_rs_json(
        &state.cache,
        &state.docs_rs,
        &arguments.name,
        &arguments.version,
        Some(&arguments.target),
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(error) => return load_error(&arguments, &error),
    };
    warnings.extend(loaded.warnings);
    let lookup = Lookup {
        cache: state.cache.clone(),
        dir,
    };
    let Some((looked_up, arguments, json)) = lookup.on_blocking(loaded.json, arguments).await
    else {
        return shutting_down();
    };
    match recover(state, &lookup, looked_up, arguments, &json, &mut warnings).await {
        Ok((looked_up, arguments)) => conclude(state, looked_up, arguments, warnings).await,
        Err(result) => *result,
    }
}

/// The result of a lookup, or of the lookup in the crate that the path goes on in.
async fn conclude(
    state: &Arc<ToolState>,
    looked_up: Result<Value, Failure>,
    arguments: Request,
    warnings: Vec<Box<str>>,
) -> CallToolResult {
    match looked_up {
        Err(Failure::Item(GetItemError::Elsewhere(destination))) => {
            follow(state, *destination, arguments, warnings).await
        },
        looked_up => finish(looked_up, &arguments, &warnings),
    }
}

/// Looks the path up in the crate that it goes on in, through a re-export of that crate's item
/// (requirement R-PR-4): a crate of std, or a release on docs.rs. If that fails, the re-export
/// itself is returned from `origin`, with why.
async fn follow(
    state: &Arc<ToolState>,
    destination: Destination,
    mut origin: Request,
    warnings: Vec<Box<str>>,
) -> CallToolResult {
    let mut followed = warnings.clone();
    let std = destination
        .lib_name
        .parse::<CrateName>()
        .ok()
        .and_then(|name| Some((std_crate(&name)?, name)));
    let result = match std {
        Some((krate, name)) if destination.is_std => {
            followed.push(shown_from(&destination, &origin, krate.as_str()));
            let arguments = Arguments {
                name,
                version: None,
                project: None,
                path: destination.path.clone(),
                target: Some(origin.target.clone()),
                detail: origin.detail,
            };
            Box::pin(execute_std(arguments, krate, Arc::clone(state), followed)).await
        },
        Some(_) | None => {
            match destination_request(state, &destination, &origin, &mut followed).await {
                Ok(request) => {
                    followed.push(shown_from(
                        &destination,
                        &origin,
                        &format!("{} {}", request.name, request.version),
                    ));
                    Box::pin(look_up_release(state, request, followed)).await
                },
                Err(result) => *result,
            }
        },
    };
    if result.is_error != Some(true) {
        return result;
    }
    let mut warnings = warnings;
    warnings.push(
        format!(
            "{} goes on as {}, which could not be looked up: {}",
            origin.path,
            destination.path,
            error_summary(&result)
        )
        .into(),
    );
    origin.hops = MAX_FOLLOWED;
    let fallback = Box::pin(look_up_release(state, origin, warnings)).await;
    // Without the re-export to show, the error of the crate the path goes on in tells more.
    if fallback.is_error == Some(true) {
        result
    } else {
        fallback
    }
}

/// The warning that the path asked for in `origin` is shown from `krate`.
fn shown_from(destination: &Destination, origin: &Request, krate: &str) -> Box<str> {
    let (asked, there) = (&origin.path, &destination.path);
    if asked == &destination.reexport {
        format!("{asked} is a re-export of {there} ({krate}), shown from there").into()
    } else {
        format!(
            "{} is a re-export of {} ({krate}), so {asked} is shown as {there} from there",
            destination.reexport, destination.reexported
        )
        .into()
    }
}

/// The code and the message of the tool error `result`.
fn error_summary(result: &CallToolResult) -> String {
    let body: Option<Value> = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str(&text.text).ok());
    body.map_or_else(
        || "an error".to_owned(),
        |body| {
            format!(
                "{}: {}",
                body.get("code").and_then(Value::as_str).unwrap_or("error"),
                body.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )
        },
    )
}

/// The request for the crate that the path goes on in (requirement R-PR-4). Its version is the
/// one in the project's build when the project decided the version of `origin`, or else the one
/// that `origin` was built with on docs.rs, or else the latest, with a warning.
async fn destination_request(
    state: &ToolState,
    destination: &Destination,
    origin: &Request,
    warnings: &mut Vec<Box<str>>,
) -> Result<Request, Box<CallToolResult>> {
    #[cfg(feature = "local")]
    if let Some(request) = in_project_build(state, destination, origin).await {
        return Ok(request);
    }
    let (name, version, resolved_from) = if let Some((name, version)) = &destination.release {
        (name.clone(), version.clone(), ResolvedFrom::Reexport)
    } else {
        let name: CrateName = destination.lib_name.parse().map_err(|error| {
            Box::new(super::tool_error(
                "item_not_found",
                &format!(
                    "the crate that the path goes on in has a name that is not valid: {error}"
                ),
                false,
                &[],
            ))
        })?;
        let entry = state
            .index
            .entry(&name)
            .await
            .map_err(|error| Box::new(index_error(&name, &error)))?;
        let resolved = VersionSpec::Latest
            .resolve(&entry.versions)
            .map_err(|no_version| {
                Box::new(version_not_found(
                    &entry.name,
                    &VersionSpec::Latest,
                    &no_version,
                ))
            })?;
        let version = resolved.version.version.clone();
        warnings.push(
            format!(
                "the version of {} that {} {} was built with is not known, so the latest, \
                 {version}, is shown",
                entry.name, origin.name, origin.version
            )
            .into(),
        );
        (
            entry.name.parse().unwrap_or(name),
            version,
            ResolvedFrom::Latest,
        )
    };
    Ok(Request {
        name,
        version,
        project_from: None,
        path: destination.path.clone(),
        resolved_from,
        target: origin.target.clone(),
        detail: origin.detail,
        checksum: None,
        build: None,
        in_build: InBuild::default(),
        hops: origin.hops.saturating_add(1),
    })
}

/// The request for the crate that the path goes on in, at its version in the project's build,
/// when the project decided the version of `origin` and has the crate from crates.io. Of several
/// versions, the one that `origin` was built with on docs.rs.
#[cfg(feature = "local")]
async fn in_project_build(
    state: &ToolState,
    destination: &Destination,
    origin: &Request,
) -> Option<Request> {
    let workspace = origin.in_build.workspace.as_ref()?;
    let resolution = state.resolver.resolve(workspace).await.ok()?;
    let graph = PackageGraph::new(&resolution);
    let built_with = destination.release.as_ref().map(|(_, version)| version);
    let name = destination
        .release
        .as_ref()
        .map_or(&*destination.lib_name, |(name, _)| name.as_str());
    let package = match graph.look_up(name) {
        PackageLookup::Found {
            package,
            other_versions,
        } => other_versions
            .into_iter()
            .find(|other| Some(&other.version) == built_with)
            .unwrap_or(package),
        PackageLookup::AmbiguousVersion(candidates) => candidates
            .into_iter()
            .map(|candidate| candidate.package)
            .find(|package| Some(&package.version) == built_with)?,
        PackageLookup::AmbiguousPackage(_) | PackageLookup::NotFound => return None,
    };
    if package.source != Source::CratesIo {
        return None;
    }
    Some(Request {
        name: package.name.parse().ok()?,
        version: package.version.clone(),
        project_from: origin.project_from,
        path: destination.path.clone(),
        resolved_from: origin.resolved_from,
        target: origin.target.clone(),
        detail: origin.detail,
        checksum: package.checksum.clone(),
        build: None,
        in_build: InBuild {
            workspace: Some(workspace.clone()),
            sets: None,
        },
        hops: origin.hops.saturating_add(1),
    })
}

/// The features of the packages in the build of the workspace that decided the version of
/// `request`, for its target (requirement R-FT-2). `None`, with a warning, when they cannot be
/// read, as when `cargo metadata` failed.
#[cfg(feature = "local")]
async fn feature_sets(
    state: &ToolState,
    request: &Request,
    warnings: &mut Vec<Box<str>>,
) -> Option<Arc<FeatureSets>> {
    let workspace = request.in_build.workspace.as_ref()?;
    match state.resolver.features(workspace, &request.target).await {
        Ok(sets) => Some(sets),
        Err(error) => {
            warnings.push(
                format!(
                    "whether items are in the project's build is not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
    }
}

/// How docs.rs built the release of `request`, from the `Cargo.toml` in its `.crate`
/// (requirement R-FT-1): cached, or fetched and checked against the checksum in the crates.io
/// index, which is read for it if the version was decided without it. `None`, with a warning,
/// when it cannot be read; the lookup goes on without it.
async fn docs_rs_build(
    state: &ToolState,
    request: &Request,
    warnings: &mut Vec<Box<str>>,
) -> Option<DocsRsBuild> {
    let (name, version) = (&request.name, &request.version);
    let mut checksum = request.checksum.clone();
    let mut contents = state
        .static_crates
        .crate_file(&state.cache, name, version, checksum.as_deref())
        .await;
    if matches!(contents, Ok(None)) {
        checksum = match state.index.entry(name).await {
            Ok(entry) => entry
                .versions
                .iter()
                .find(|found| found.version == *version)
                .map(|found| found.checksum.clone()),
            Err(error) => {
                tracing::debug!(error = %ErrorChain(&error), "the index was not read for the .crate");
                None
            },
        };
        contents = state
            .static_crates
            .crate_file(&state.cache, name, version, checksum.as_deref())
            .await;
    }
    let read = contents.and_then(|contents| {
        contents
            .map(|contents| crate_file::docs_rs_build(&contents, name, version))
            .transpose()
    });
    match read {
        Ok(Some(build)) => Some(build),
        Ok(None) => {
            warnings.push(
                format!(
                    "the features that docs.rs built {name} {version} with are not known: its \
                     checksum is not in the crates.io index"
                )
                .into(),
            );
            None
        },
        Err(error) => {
            warnings.push(
                format!(
                    "the features that docs.rs built {name} {version} with are not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
    }
}

/// When `looked_up` failed because the JSON did not parse, tries again as far as that can help:
/// another format (requirement R-DS-2), or the JSON fetched once more (requirement R-CA-4).
/// Returns the result to finish with, or the tool error to return as it is (boxed, as a result is
/// large).
async fn recover(
    state: &ToolState,
    lookup: &Lookup,
    looked_up: Result<Value, Failure>,
    arguments: Request,
    json: &[u8],
    warnings: &mut Vec<Box<str>>,
) -> Result<(Result<Value, Failure>, Request), Box<CallToolResult>> {
    Ok(match &looked_up {
        // A format this server does not read: docs.rs may have the release in another one
        // (requirement R-DS-2).
        Err(Failure::Parse(ParseError::UnsupportedFormat { found, .. })) => {
            match data::docs_rs_json_in_another_format(
                &state.cache,
                &state.docs_rs,
                &arguments.name,
                &arguments.version,
                Some(&arguments.target),
                *found,
            )
            .await
            {
                Ok(Some(json)) => {
                    let Some((again, returned, _)) = lookup.on_blocking(json, arguments).await
                    else {
                        return Err(Box::new(shutting_down()));
                    };
                    (again, returned)
                },
                Ok(None) => (looked_up, arguments),
                Err(error) => return Err(Box::new(load_error(&arguments, &error))),
            }
        },
        // Its header was read when it was fetched, yet it does not parse: docs.rs may have served
        // it broken, so it is fetched again, once (requirement R-CA-4).
        Err(Failure::Parse(
            ParseError::Json { .. } | ParseError::FormatMismatch { .. } | ParseError::TooDeep,
        )) => match data::refetch_unparsable(
            &state.cache,
            &state.docs_rs,
            &arguments.name,
            &arguments.version,
            Some(&arguments.target),
            json,
        )
        .await
        {
            Ok(again) => {
                warnings.extend(again.warnings);
                let Some((again, returned, fetched)) =
                    lookup.on_blocking(again.json, arguments).await
                else {
                    return Err(Box::new(shutting_down()));
                };
                if let Err(Failure::Parse(_)) = &again
                    && let Err(error) = data::remember_unparsable(
                        &state.cache,
                        &returned.name,
                        &returned.version,
                        Some(&returned.target),
                        &fetched,
                    )
                    .await
                {
                    tracing::warn!(error = %ErrorChain(&error), "failed to remember a JSON that cannot be parsed");
                }
                (again, returned)
            },
            Err(error) => return Err(Box::new(load_error(&arguments, &error))),
        },
        Ok(_)
        | Err(Failure::Parse(_) | Failure::Archive(_) | Failure::Item(_) | Failure::Serialize(_)) => {
            (looked_up, arguments)
        },
    })
}

/// The crate of std that `name` is, if it is one (requirement R-STD-1).
pub(super) fn std_crate(name: &CrateName) -> Option<StdCrate> {
    StdCrate::from_name(&name.as_str().to_ascii_lowercase().replace('-', "_"))
}

/// Looks the item up in std, core, alloc, `proc_macro` or test: in the JSON of the nightly
/// installed here or of the distribution server (requirements R-STD-1 and R-STD-6), following
/// re-exports between them (`std::vec::Vec` is `alloc::vec::Vec`).
async fn execute_std(
    arguments: Arguments,
    krate: StdCrate,
    state: Arc<ToolState>,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let target = arguments
        .target
        .clone()
        .unwrap_or_else(|| state.host.triple.clone());
    if arguments.version.is_some() {
        warnings.push(
            "std has no versions on crates.io, so the version is ignored; std is that of a nightly \
             toolchain"
                .into(),
        );
    }
    let at_least = project_toolchain(&state).await;
    let chosen = match std_json::choose(
        &state.cache,
        &state.dist,
        state.cache.dir(),
        &target,
        &state.host.triple,
        at_least.as_ref(),
    )
    .await
    {
        Ok(chosen) => chosen,
        Err(error) => return dist_error(krate, &error),
    };
    warnings.extend(chosen.warnings.iter().cloned());
    let request = Request {
        name: arguments.name,
        version: chosen
            .json
            .release()
            .unwrap_or_else(|| Version::new(0, 0, 0)),
        project_from: None,
        path: with_lib_name(krate.as_str(), &arguments.path),
        resolved_from: ResolvedFrom::Latest,
        target,
        detail: arguments.detail,
        checksum: None,
        build: None,
        in_build: InBuild::default(),
        hops: 0,
    };
    let cache = state.cache.clone();
    let task = tokio::task::spawn_blocking(move || {
        let looked_up = look_up_std(&cache, &chosen, &request, at_least.as_ref());
        (looked_up, request)
    });
    match task.await {
        Ok(((looked_up, through), request)) => {
            if let [asked, .., found] = through.as_slice() {
                warnings.push(
                    format!("{asked} is a re-export of {found}, shown from its crate").into(),
                );
            }
            finish(looked_up, &request, &warnings)
        },
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the get_item lookup in std did not finish");
            shutting_down()
        },
    }
}

/// The release of the project's toolchain, which a nightly installed here must not be older than
/// to be used for std (requirement R-STD-1). `None` without a single default workspace, or when
/// its toolchain cannot be run without installing it.
#[cfg(feature = "local")]
async fn project_toolchain(state: &ToolState) -> Option<Version> {
    let project = state.project.as_ref()?;
    let [workspace] = &*project.workspaces else {
        return None;
    };
    data::local_std::rustc_release(&workspace.root).await
}

/// Without local features, no toolchain is run.
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs rustc"
)]
async fn project_toolchain(_state: &ToolState) -> Option<Version> {
    None
}

/// Follows `request.path` through the crates of std in `chosen`, archiving each the first time,
/// and looks the item up where it is. Returns the result and the paths it went through.
fn look_up_std(
    cache: &Cache,
    chosen: &Chosen,
    request: &Request,
    project_release: Option<&Version>,
) -> (Result<Value, Failure>, Vec<Box<str>>) {
    let mut loaded: HashMap<StdCrate, Arc<Mapped<Indexed>>> = HashMap::new();
    let mut failed = None;
    let load = |name: &str| {
        let krate = StdCrate::from_name(name)?;
        if let Some(mapped) = loaded.get(&krate) {
            return Some(Arc::clone(mapped));
        }
        let archived = chosen
            .json
            .archive_key(cache, krate)
            .map_err(load::LoadError::Read)
            .and_then(|(dir, key)| {
                load::archive_of(cache, &dir, krate.as_str(), &key, || {
                    chosen.json.read(cache, krate)
                })
            });
        match archived {
            Ok(mapped) => {
                let mapped = Arc::new(mapped);
                loaded.insert(krate, Arc::clone(&mapped));
                Some(mapped)
            },
            Err(error) => {
                tracing::warn!(krate = krate.as_str(), error = %ErrorChain(&error), "the JSON of std could not be loaded");
                failed = Some(error);
                None
            },
        }
    };
    let Some(followed) = across::follow(&request.path, load) else {
        let failure = match failed {
            Some(load::LoadError::Parse(error)) => Failure::Parse(error),
            Some(load::LoadError::Archive(error) | load::LoadError::Read(error)) => {
                Failure::Archive(error)
            },
            None => Failure::Archive(std::io::Error::other("std could not be loaded")),
        };
        return (Err(failure), Vec::new());
    };
    let view = CrateView::new(followed.krate.get());
    // The crate where the item is (alloc for `std::vec::Vec`), at the version its JSON says.
    let name = view
        .item(view.root())
        .and_then(|root| root.name.as_deref())
        .and_then(|name| name.parse::<CrateName>().ok())
        .unwrap_or_else(|| request.name.clone());
    let version = view
        .crate_version()
        .and_then(|version| Version::parse(version).ok())
        .unwrap_or_else(|| request.version.clone());
    let toolchain = chosen.json.toolchain();
    let looked_up = get_item(&view, ItemRequest {
        name: &name,
        version: &version,
        path: &followed.path,
        origin: Origin::Std {
            toolchain: &toolchain,
            std_from: chosen.json.source(),
            why: &chosen.why,
            project_release,
        },
        target: Some(request.target.as_str()),
        detail: request.detail,
        project_features: None,
        follow: false,
    })
    .map_err(Failure::Item)
    .and_then(|response| serde_json::to_value(&response).map_err(Failure::Serialize));
    (looked_up, followed.through)
}

/// The tool error for std that the distribution server could not give (requirement R-STD-6).
fn dist_error(krate: StdCrate, error: &DistError) -> CallToolResult {
    let message = format!(
        "the rustdoc JSON of {} could not be had: {}",
        krate.as_str(),
        ErrorChain(error)
    );
    let (code, reason, retryable): (&str, Option<&str>, bool) = match error {
        DistError::Network(_) | DistError::Status { .. } => ("network_error", None, true),
        DistError::Certificate(_) | DistError::Manifest(_) => ("network_error", None, false),
        DistError::NotAvailable { .. } => ("docs_unavailable", Some("no_json"), false),
        DistError::TooLarge { .. } => ("docs_unavailable", Some("too_large"), false),
        DistError::ChecksumMismatch | DistError::Archive(_) => {
            ("docs_unavailable", Some("corrupt"), true)
        },
        DistError::Cache(_) => ("internal_error", None, false),
    };
    let mut body = json!({
        "code": code,
        "message": message,
        "retryable": retryable,
        "suggestions": [
            "install rust-docs-json for a nightly toolchain: rustup component add rust-docs-json \
             --toolchain nightly",
        ],
    });
    if let (Some(reason), Some(body)) = (reason, body.as_object_mut()) {
        body.insert("reason".to_owned(), json!(reason));
    }
    error_result(&body)
}

/// Looks the item up in the archive of the crate in `dir`, on a blocking thread. `Some(Ok(_))`
/// with the result when the archive is there, `Some(Err(_))` with the arguments when it is not (or
/// cannot be read, which is logged), and `None` if the runtime is shutting down.
async fn look_up_archived(
    cache: Cache,
    dir: PathBuf,
    arguments: Box<Request>,
) -> Option<Result<(Result<Value, Failure>, Box<Request>), Box<Request>>> {
    let task = tokio::task::spawn_blocking(move || match load::open_archived(&cache, &dir) {
        Ok(Some(mapped)) => Ok((look_up_in(&mapped, &arguments), arguments)),
        Ok(None) => Err(arguments),
        Err(error) => {
            tracing::warn!(%error, "the archive of the crate could not be read");
            Err(arguments)
        },
    });
    match task.await {
        Ok(done) => Some(done),
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the get_item lookup did not finish");
            None
        },
    }
}

/// Where the JSON of the crate is archived.
struct Lookup {
    cache: Cache,
    dir: PathBuf,
}

impl Lookup {
    /// Parses `json`, archives it, and looks the item up, on a blocking thread, since parsing a
    /// large crate takes a large part of a second. The arguments and the JSON come back with the
    /// result. `None` if the runtime is shutting down; a panic goes on to `run_tool`.
    async fn on_blocking(
        &self,
        json: Box<[u8]>,
        arguments: Request,
    ) -> Option<(Result<Value, Failure>, Request, Box<[u8]>)> {
        let (cache, dir) = (self.cache.clone(), self.dir.clone());
        match tokio::task::spawn_blocking(move || {
            let looked_up = load::archive_json(&cache, &dir, &json)
                .map_err(|error| match error {
                    load::LoadError::Parse(error) => Failure::Parse(error),
                    load::LoadError::Archive(error) | load::LoadError::Read(error) => {
                        Failure::Archive(error)
                    },
                })
                .and_then(|mapped| look_up_in(&mapped, &arguments));
            (looked_up, arguments, json)
        })
        .await
        {
            Ok(done) => Some(done),
            Err(error) => {
                let error = crate::task::resume_if_panic(error);
                tracing::error!(%error, "the get_item lookup did not finish");
                None
            },
        }
    }
}

/// The tool error when the lookup was stopped because the server is shutting down.
fn shutting_down() -> CallToolResult {
    super::tool_error(
        "internal_error",
        "the server stopped the lookup because it is shutting down",
        false,
        &[],
    )
}

/// The tool result of a lookup, with the `warnings` about how the JSON was loaded first.
fn finish(
    looked_up: Result<Value, Failure>,
    arguments: &Request,
    warnings: &[Box<str>],
) -> CallToolResult {
    match looked_up {
        Ok(mut response) => {
            add_warnings(&mut response, warnings);
            CallToolResult::structured(response)
        },
        Err(Failure::Parse(error)) => parse_error(arguments, &error),
        Err(Failure::Item(error)) => item_error(arguments, &error),
        Err(Failure::Archive(error)) => {
            tracing::error!(%error, "the crate could not be archived");
            super::tool_error(
                "internal_error",
                "the server could not keep the crate in memory",
                false,
                &["check the server's stderr log"],
            )
        },
        Err(Failure::Serialize(error)) => {
            tracing::error!(%error, "the get_item response could not be serialized");
            super::tool_error(
                "internal_error",
                "the server could not build the response (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            )
        },
    }
}

/// Puts `warnings` first among the `warnings` of `response`, which come right after `provenance`
/// (§7.3).
fn add_warnings(response: &mut Value, warnings: &[Box<str>]) {
    if warnings.is_empty() {
        return;
    }
    let Some(fields) = response.as_object_mut() else {
        return;
    };
    let mut all: Vec<Value> = Vec::with_capacity(warnings.len());
    // The same warning comes twice when a path is looked up again after it could not be followed.
    for warning in warnings {
        let warning = json!(warning);
        if !all.contains(&warning) {
            all.push(warning);
        }
    }
    if let Some(Value::Array(more)) = fields.get_mut("warnings") {
        all.append(more);
        fields.insert("warnings".to_owned(), Value::Array(all));
        return;
    }
    // The fields are in order: rebuild them with `warnings` after `provenance`.
    let old = std::mem::take(fields);
    let mut all = Some(Value::Array(all));
    for (key, value) in old {
        let after = key == "provenance";
        fields.insert(key, value);
        if after && let Some(all) = all.take() {
            fields.insert("warnings".to_owned(), all);
        }
    }
    if let Some(all) = all {
        fields.insert("warnings".to_owned(), all);
    }
}

/// Why the item could not be looked up.
#[derive(Debug)]
enum Failure {
    Parse(ParseError),
    Archive(std::io::Error),
    Item(GetItemError),
    Serialize(serde_json::Error),
}

/// Looks the item up in the archive `mapped`, and serializes the response while the items it
/// borrows from are still there.
fn look_up_in(mapped: &Mapped<Indexed>, arguments: &Request) -> Result<Value, Failure> {
    let view = CrateView::new(mapped.get());
    get_item(&view, ItemRequest {
        name: &arguments.name,
        version: &arguments.version,
        path: &arguments.path,
        origin: Origin::DocsRs {
            resolved_from: arguments.resolved_from,
            project_from: arguments.project_from,
            build: arguments.build.as_deref(),
        },
        target: Some(arguments.target.as_str()),
        detail: arguments.detail,
        project_features: arguments
            .in_build
            .features(arguments.name.as_str(), &arguments.version),
        follow: arguments.hops < MAX_FOLLOWED,
    })
    .map_err(Failure::Item)
    .and_then(|response| serde_json::to_value(&response).map_err(Failure::Serialize))
}

/// `name@version` of the request.
fn release(arguments: &Request) -> String {
    format!("{}@{}", arguments.name, arguments.version)
}

/// The tool error for JSON that could not be loaded (§8).
fn load_error(arguments: &Request, error: &LoadError) -> CallToolResult {
    let release = release(arguments);
    tracing::warn!(%release, error = %ErrorChain(error), "rustdoc JSON could not be loaded");
    let fetch = match error {
        LoadError::Fetch(fetch) => fetch,
        LoadError::WrongVersion { found } => {
            return error_result(&json!({
                "code": "docs_unavailable",
                "message": format!(
                    "docs.rs returned the rustdoc JSON of version {found} for {release}, so it is not \
                     used (no other version is shown in place of the one asked for)"
                ),
                "retryable": false,
                "suggestions": ["ask for the version with its build metadata, such as 2.1.0+zstd.1.5.6"],
                "reason": "version_mismatch",
            }));
        },
        LoadError::NoTargetJson { target } => {
            return error_result(&json!({
                "code": "docs_unavailable",
                "message": format!(
                    "docs.rs has rustdoc JSON of {release}, but not for {target}: since 2026-05-01 it \
                     builds most new releases for one target only"
                ),
                "retryable": false,
                "suggestions": [
                    "the documentation for the target docs.rs built is there without the target \
                     argument; items for this target only may be missing from it",
                ],
                "reason": "no_target_json",
            }));
        },
        LoadError::RecentFailure { retry_after } => {
            return error_result(&json!({
                "code": "network_error",
                "message": format!("rustdoc JSON of {release} could not be loaded: {}", ErrorChain(error)),
                "retryable": true,
                "suggestions": ["try again after retry_after_secs"],
                "retry_after_secs": retry_after.as_secs().max(1),
            }));
        },
        LoadError::Unparsable { retry_after } => {
            return remembered_error(&release, error, "unparsable", false, *retry_after, &[
                "docs.rs served JSON that this server cannot parse; it is fetched again after \
                 retry_after_secs, or read by a newer version of the server",
            ]);
        },
        LoadError::Corrupt { retry_after } => {
            return remembered_error(&release, error, "corrupt", true, *retry_after, &[
                "check the disk that holds the cache, then try again after retry_after_secs",
            ]);
        },
        LoadError::Cache(_) => {
            return super::tool_error(
                "internal_error",
                "the cache of rustdoc JSON could not be read",
                false,
                &["check the server's stderr log for the cache directory and the error"],
            );
        },
    };
    let (code, reason, retryable, suggestions): (&str, Option<&str>, bool, &[&str]) = match fetch {
        FetchError::NotFound | FetchError::BadRequest => {
            ("docs_unavailable", Some("no_json"), false, &[
                "check that the crate and the version exist on crates.io",
                "docs.rs has rustdoc JSON only for releases it built on or after 2025-05-23, and none \
             for a release whose docs.rs build failed",
            ])
        },
        FetchError::TooLarge { .. } => ("docs_unavailable", Some("too_large"), false, &[]),
        // More zstd layers than the limit count as broken (requirement R-DS-4).
        FetchError::TooManyLayers | FetchError::Corrupt(_) => {
            ("docs_unavailable", Some("unparsable"), false, &[])
        },
        FetchError::Transient { .. } | FetchError::Network(_) => {
            ("network_error", None, true, &["try again in a while"])
        },
        FetchError::UnexpectedStatus { .. } => ("network_error", None, false, &[]),
        FetchError::Certificate(_) => ("network_error", None, false, &[
            "the TLS certificate of docs.rs was rejected: if a proxy inspects TLS, add its CA \
             certificate to the system's trusted certificates",
        ]),
    };
    let mut body = json!({
        "code": code,
        "message": format!("rustdoc JSON of {release} could not be loaded: {}", ErrorChain(fetch)),
        "retryable": retryable,
        "suggestions": suggestions,
    });
    if let (Some(reason), Some(body)) = (reason, body.as_object_mut()) {
        body.insert("reason".to_owned(), json!(reason));
    }
    error_result(&body)
}

/// The `docs_unavailable` error for a failure that is remembered for `retry_after` (§8).
fn remembered_error(
    release: &str,
    error: &LoadError,
    reason: &str,
    retryable: bool,
    retry_after: Duration,
    suggestions: &[&str],
) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!("rustdoc JSON of {release} could not be loaded: {}", ErrorChain(error)),
        "retryable": retryable,
        "suggestions": suggestions,
        "reason": reason,
        "retry_after_secs": retry_after.as_secs().max(1),
    }))
}

/// The tool error for JSON that could not be parsed (§8).
fn parse_error(arguments: &Request, error: &ParseError) -> CallToolResult {
    let release = release(arguments);
    tracing::warn!(%release, error = %ErrorChain(error), "rustdoc JSON could not be parsed");
    let message = format!(
        "rustdoc JSON of {release} could not be read: {}",
        ErrorChain(error)
    );
    match error {
        ParseError::UnsupportedFormat { found, supported } => error_result(&json!({
            "code": "unsupported_format",
            "message": message,
            "retryable": false,
            "suggestions": [format!(
                "this server reads formats {} to {}; a release that docs.rs built at another time \
                 may have a supported format",
                supported.start(),
                supported.end()
            )],
            "found": found,
            "supported": { "from": supported.start(), "to": supported.end() },
        })),
        ParseError::NoFormatVersion(_)
        | ParseError::Json { .. }
        | ParseError::FormatMismatch { .. }
        | ParseError::TooDeep => error_result(&json!({
            "code": "docs_unavailable",
            "message": message,
            "retryable": false,
            "suggestions": [],
            "reason": "unparsable",
        })),
    }
}

/// The tool error for a path that does not name one item (§8).
fn item_error(arguments: &Request, error: &GetItemError) -> CallToolResult {
    let release = release(arguments);
    match error {
        GetItemError::NotFound => error_result(&json!({
            "code": "item_not_found",
            "message": format!(
                "{release} has no item at {}: it was looked up as a public path, a re-export \
                 from another crate, a definition path, and a member of the item at its parent \
                 path",
                arguments.path
            ),
            "retryable": false,
            "suggestions": [
                "check the spelling and the case of each segment",
                "a method, associated item or variant is found through its type, trait or enum \
                 (tokio::sync::Mutex::lock)",
            ],
        })),
        // Followed before the error is made; reached only if following is turned off.
        GetItemError::Elsewhere(destination) => error_result(&json!({
            "code": "item_not_found",
            "message": format!(
                "{release} has no item at {}; it goes on as {} in another crate",
                arguments.path, destination.path
            ),
            "retryable": false,
            "suggestions": [format!("call get_item with the path {}", destination.path)],
        })),
        GetItemError::Ambiguous { candidates } => error_result(&json!({
            "code": "ambiguous_path",
            "message": format!("{release} has more than one item at {}", arguments.path),
            "retryable": false,
            "suggestions": ["call get_item again with the path of one of the candidates"],
            "candidates": candidates.get(..MAX_CANDIDATES).unwrap_or(candidates),
            "candidates_total": candidates.len(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(value: Value) -> Result<Arguments, InvalidArgument> {
        let Value::Object(arguments) = value else {
            panic!("the arguments are an object");
        };
        parse_arguments(Some(&arguments))
    }

    #[test]
    fn the_crate_name_is_added_to_a_path_without_it() {
        let parsed =
            arguments(json!({ "crate": "axum-core@0.5.6", "path": "body::Body" })).expect("valid");
        assert_eq!(&*parsed.path, "axum_core::body::Body");
        let parsed = arguments(json!({ "crate": "axum-core@0.5.6", "path": "::axum-core::body" }))
            .expect("valid");
        assert_eq!(&*parsed.path, "axum-core::body");
        assert_eq!(
            parsed.version,
            Some(VersionSpec::Exact(Version::new(0, 5, 6)))
        );
    }

    #[test]
    fn the_crates_of_std_are_told_by_name() {
        let krate = |name: &str| std_crate(&name.parse().expect("a crate name"));
        assert_eq!(krate("std"), Some(StdCrate::Std));
        assert_eq!(krate("proc-macro"), Some(StdCrate::ProcMacro));
        assert_eq!(krate("Core"), Some(StdCrate::Core));
        assert_eq!(krate("tokio"), None);
    }

    #[test]
    fn the_head_of_a_path_is_written_as_code_writes_it() {
        assert_eq!(
            &*with_lib_name("serde_json", "Serde-Json::Value"),
            "serde_json::Value"
        );
        assert_eq!(&*with_lib_name("serde_json", "Value"), "serde_json::Value");
        assert_eq!(&*with_lib_name("serde-json", "serde_json"), "serde_json");
        assert_eq!(
            &*with_lib_name("tokio", "sync::Mutex"),
            "tokio::sync::Mutex"
        );
    }

    #[test]
    fn the_version_is_in_crate_or_version_or_left_out() {
        let version = |value: Value| arguments(value).expect("valid").version;
        assert_eq!(version(json!({ "crate": "tokio", "path": "spawn" })), None);
        assert_eq!(
            version(json!({ "crate": "tokio@latest", "path": "spawn" })),
            Some(VersionSpec::Latest)
        );
        assert_eq!(
            version(json!({ "crate": "tokio@^1.47", "path": "spawn" })),
            Some("^1.47".parse().expect("a requirement"))
        );
        assert_eq!(
            version(json!({ "crate": "tokio", "version": "1.47.1", "path": "spawn" })),
            Some(VersionSpec::Exact(Version::new(1, 47, 1)))
        );
        assert_eq!(
            version(json!({ "crate": "tokio@1.47.1", "version": "1.47.1", "path": "spawn" })),
            Some(VersionSpec::Exact(Version::new(1, 47, 1)))
        );
        let project = arguments(json!({ "crate": "tokio", "path": "spawn", "project": "app" }))
            .expect("valid")
            .project;
        assert_eq!(project, Some(PathBuf::from("app")));
        let target = arguments(json!({
            "crate": "tokio", "path": "spawn", "target": "aarch64-apple-darwin",
        }))
        .expect("valid")
        .target;
        assert_eq!(
            target.as_ref().map(TargetTriple::as_str),
            Some("aarch64-apple-darwin")
        );
    }

    #[test]
    fn bad_arguments_are_invalid_arguments() {
        for (value, about) in [
            (json!({ "crate": "1tokio", "path": "spawn" }), "`crate`"),
            (
                json!({ "crate": "tokio@not a version", "path": "spawn" }),
                "version",
            ),
            (
                json!({ "crate": "tokio@1.47.1", "version": "1.47.0", "path": "spawn" }),
                "`version`",
            ),
            (
                json!({ "crate": "tokio", "path": "spawn", "target": "../x" }),
                "`target`",
            ),
            (json!({ "crate": "tokio", "path": " " }), "`path`"),
            (json!({ "crate": "tokio" }), "`path`"),
            (json!({ "path": "spawn" }), "`crate`"),
            (
                json!({ "crate": "tokio", "path": "spawn", "detail": "all" }),
                "`detail`",
            ),
        ] {
            let error = arguments(value.clone()).expect_err("invalid");
            assert!(error.message.contains(about), "{value}: {error:?}");
        }
    }

    #[test]
    fn detail_is_normal_unless_asked() {
        let detail = |value: Value| arguments(value).map(|arguments| arguments.detail);
        assert_eq!(
            detail(json!({ "crate": "tokio", "path": "spawn" })).ok(),
            Some(Detail::Normal)
        );
        assert_eq!(
            detail(json!({ "crate": "tokio", "path": "spawn", "detail": "brief" })).ok(),
            Some(Detail::Brief)
        );
        assert_eq!(
            detail(json!({ "crate": "tokio", "path": "spawn", "detail": " full " })).ok(),
            Some(Detail::Full)
        );
    }

    #[test]
    fn ambiguous_paths_list_at_most_twenty_candidates() {
        let request = Request {
            name: "tokio".parse().expect("a crate name"),
            version: Version::new(1, 0, 0),
            project_from: None,
            path: "tokio::x".into(),
            resolved_from: ResolvedFrom::Explicit,
            target: "x86_64-unknown-linux-gnu".parse().expect("a triple"),
            detail: Detail::Normal,
            checksum: None,
            build: None,
            in_build: InBuild::default(),
            hops: 0,
        };
        let candidates = (0..25).map(|n| format!("item {n}").into()).collect();
        let result = item_error(&request, &GetItemError::Ambiguous { candidates });
        let text = result.content[0].as_text().map(|text| text.text.clone());
        let body: Value = serde_json::from_str(&text.unwrap_or_default()).expect("JSON");
        assert_eq!(body["code"], "ambiguous_path");
        assert_eq!(body["candidates"].as_array().map(Vec::len), Some(20));
        assert_eq!(body["candidates_total"], 25);
    }

    /// Every property of a full response is declared in the output schema.
    #[test]
    fn the_output_schema_covers_the_response() {
        use crate::query::get_item::{
            Deprecated,
            Enabled,
            ItemData,
            ItemResponse,
            Provenance,
            ReexportOf,
            ResolvedFrom,
            Section,
            Source,
            SourceLocation,
            StabilityNote,
            StdFrom,
        };

        let some_str = || Some("x");

        let some = || Some(Box::<str>::from("x"));
        let many = || Box::from([Box::<str>::from("x")]);
        let features: Box<[Box<str>]> = many();
        let response = ItemResponse {
            provenance: Provenance {
                source: Source::DocsRs,
                name: "x",
                version: "1.0.0".into(),
                resolved_from: Some(ResolvedFrom::Explicit),
                project_from: Some("cwd"),
                toolchain: some_str(),
                std_from: Some(StdFrom {
                    source: "dist",
                    reason: "x",
                }),
                target: "x",
                format_version: 61,
                features: Some(&features),
                all_features: true,
                cfg: &features,
            },
            warnings: Box::from(["x".into()]),
            data: ItemData {
                path: "x".into(),
                kind: "struct",
                parent: some(),
                impl_header: some(),
                signature: some(),
                where_predicates: many(),
                fields: many(),
                variants: many(),
                required_features: some(),
                other_cfg: many(),
                cfg_source: Some("doc_cfg"),
                enabled_in_project: Some(Enabled::Unknown),
                enabled_in_dev_only: true,
                docs: Some("x".into()),
                docs_truncated: true,
                deprecated: Some(Deprecated {
                    since: Some("x"),
                    note: Some("x"),
                }),
                must_use: true,
                non_exhaustive: true,
                stability: Some(StabilityNote {
                    stable_since: some_str(),
                    unstable_feature: some_str(),
                    const_stable_since: some_str(),
                    const_unstable_feature: some_str(),
                }),
                also_reexported_at: many(),
                reexport_of: Some(ReexportOf {
                    name: "x".into(),
                    path: "x".into(),
                }),
                source_location: Some(SourceLocation {
                    file: "x",
                    lines: "1".into(),
                    url: some(),
                }),
                sections: Box::from([Section {
                    heading: "x".into(),
                    lines: 1,
                    text: some(),
                }]),
                attrs: many(),
                truncated: true,
            },
            hints: many(),
        };
        let value = serde_json::to_value(&response).expect("serializes");
        assert_declared(&value, &Value::Object(output_schema()), "");
    }

    /// Checks that every key of the objects in `value` is a property in `schema`.
    fn assert_declared(value: &Value, schema: &Value, at: &str) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    let property = &schema["properties"][key];
                    assert!(!property.is_null(), "{at}/{key} is not in the schema");
                    assert_declared(value, property, &format!("{at}/{key}"));
                }
            },
            Value::Array(items) => {
                for item in items {
                    assert_declared(item, &schema["items"], at);
                }
            },
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {},
        }
    }

    #[test]
    fn the_description_fits_the_limit() {
        let len = DESCRIPTION.chars().count();
        assert!(len <= 2048, "{len} characters");
    }
}
