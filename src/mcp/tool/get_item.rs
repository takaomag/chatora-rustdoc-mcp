//! The `get_item` tool (`doc/m0-plan.md`, A8): one item of a crate from its docs.rs rustdoc JSON.
//!
//! M0a takes only an exact version, reads only docs.rs, and parses the JSON on every call.

#[cfg(feature = "local")]
use std::borrow::Cow;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use semver::Version;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

#[cfg(feature = "local")]
use super::local::{self, LocalSource, Member};
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
#[cfg(feature = "local")]
use crate::query::local::LocalWanted;
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
        flight::{Doing, Joined},
        rustdoc_json::ParseError,
        sparse_index::{IndexEntry, IndexError},
        std_json::Chosen,
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
        implementors::{ImplementorOptions, list_implementors},
        impls::{ImplOptions, list_impls},
        list_module::{ListOptions, list_module},
        load::{self, ArchiveError},
        methods::{MethodOptions, list_methods},
        overview::crate_overview,
        search::{Fields, SearchOptions, search_items},
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
    /// Whether what failed is to be tried again (§6.4).
    #[cfg_attr(
        all(not(feature = "local"), not(test)),
        expect(dead_code, reason = "only local features declare and take it")
    )]
    refresh: bool,
    /// How much to show, and for `list_module`, of what.
    shape: Shape,
}

/// What a lookup returns of the item at the path: the item (`get_item`), or the items of the
/// module (`list_module`), and how much of each.
#[derive(Debug, Clone)]
pub(super) struct Shape {
    pub(super) detail: Detail,
    pub(super) what: What,
}

/// What a lookup returns.
#[derive(Debug, Clone)]
pub(super) enum What {
    /// The item (`get_item`).
    Item,
    /// The items of the module (`list_module`).
    List(Box<Listing>),
    /// The overview of the crate (`crate_overview`).
    Overview,
    /// The items that match a query (`search_items`).
    Search(Box<Searching>),
    /// The traits that the type implements (`list_impls`).
    Impls(Box<Paging>),
    /// The methods that can be called on the type (`list_methods`).
    Methods(Box<MethodFilter>),
    /// The types that implement the trait (`list_implementors`).
    Implementors(Box<Paging>),
}

/// Which methods `list_methods` lists (requirement R-LS-3), and the page.
#[derive(Debug, Clone)]
pub(super) struct MethodFilter {
    pub(super) name: Option<Box<str>>,
    pub(super) from: Box<[&'static str]>,
    pub(super) expand_blanket: bool,
    pub(super) paging: Paging,
}

/// A page of a list: how many items, from where, and what the cursor was made for
/// (requirement R-OUT-3).
#[derive(Debug, Clone)]
pub(super) struct Paging {
    pub(super) limit: usize,
    pub(super) offset: usize,
    /// What cursors start with: the form and the digest of the arguments.
    pub(super) request_digest: Box<str>,
    /// The digest of the data that the cursor was made for, when there is a cursor.
    pub(super) cursor_data: Option<Box<str>>,
}

/// How `search_items` searches (requirements R-SR-1 to R-SR-5).
#[derive(Debug, Clone)]
pub(super) struct Searching {
    pub(super) query: Box<str>,
    pub(super) fields: Fields,
    pub(super) kinds: Box<[&'static str]>,
    pub(super) limit: Option<usize>,
    pub(super) offset: usize,
    /// What cursors start with: the form and the digest of the arguments.
    pub(super) request_digest: Box<str>,
    /// The digest of the data that the cursor was made for, when there is a cursor.
    pub(super) cursor_data: Option<Box<str>>,
}

/// How `list_module` lists a module (requirements R-LS-1, R-LS-2 and R-OUT-3).
#[derive(Debug, Clone)]
pub(super) struct Listing {
    pub(super) recursive: bool,
    pub(super) depth: usize,
    pub(super) kinds: Box<[&'static str]>,
    pub(super) limit: usize,
    pub(super) offset: usize,
    /// The digest of the arguments but `cursor` and `limit`, which cursors carry.
    pub(super) request_digest: Box<str>,
    /// The digest of the data that the cursor was made for, when there is a cursor.
    pub(super) cursor_data: Option<Box<str>>,
}

impl Arguments {
    /// The path, for the tests of the tools that make arguments.
    #[cfg(test)]
    pub(super) fn path_for_test(&self) -> Box<str> {
        self.path.clone()
    }

    /// The arguments of `list_module`: those that name a release, the module's path (the
    /// crate root if `None`), and how to list it.
    pub(super) fn for_listing(
        release: ReleaseArguments,
        path: Option<&str>,
        detail: Detail,
        listing: Listing,
    ) -> Self {
        let path = path.map_or_else(
            || release.name.as_str().replace('-', "_").into(),
            |path| with_crate_name(&release.name, path.trim_start_matches("::")),
        );
        Self {
            name: release.name,
            version: release.version,
            project: release.project,
            path,
            target: release.target,
            refresh: release.refresh,
            shape: Shape {
                detail,
                what: What::List(Box::new(listing)),
            },
        }
    }

    /// The arguments of `search_items`: those that name a release, and how to search.
    pub(super) fn for_search(
        release: ReleaseArguments,
        detail: Detail,
        searching: Searching,
    ) -> Self {
        Self {
            path: release.name.as_str().replace('-', "_").into(),
            name: release.name,
            version: release.version,
            project: release.project,
            target: release.target,
            refresh: release.refresh,
            shape: Shape {
                detail,
                what: What::Search(Box::new(searching)),
            },
        }
    }

    /// The arguments of `list_implementors`: those that name a release, the trait's path, and
    /// the page.
    pub(super) fn for_implementors(release: ReleaseArguments, path: &str, paging: Paging) -> Self {
        let mut arguments = Self::for_impls(release, path, paging);
        if let What::Impls(paging) = arguments.shape.what {
            arguments.shape.what = What::Implementors(paging);
        }
        arguments
    }

    /// The arguments of `list_impls`: those that name a release, the type's path, and the page.
    pub(super) fn for_impls(release: ReleaseArguments, path: &str, paging: Paging) -> Self {
        Self {
            path: with_crate_name(&release.name, path.trim_start_matches("::")),
            name: release.name,
            version: release.version,
            project: release.project,
            target: release.target,
            refresh: release.refresh,
            shape: Shape {
                detail: Detail::Normal,
                what: What::Impls(Box::new(paging)),
            },
        }
    }

    /// The arguments of `list_methods`: those that name a release, the type's path, how much
    /// to show, and which methods.
    pub(super) fn for_methods(
        release: ReleaseArguments,
        path: &str,
        detail: Detail,
        filter: MethodFilter,
    ) -> Self {
        Self {
            path: with_crate_name(&release.name, path.trim_start_matches("::")),
            name: release.name,
            version: release.version,
            project: release.project,
            target: release.target,
            refresh: release.refresh,
            shape: Shape {
                detail,
                what: What::Methods(Box::new(filter)),
            },
        }
    }

    /// The arguments of `crate_overview`: those that name a release, and how much to show.
    pub(super) fn for_overview(release: ReleaseArguments, detail: Detail) -> Self {
        Self {
            path: release.name.as_str().replace('-', "_").into(),
            name: release.name,
            version: release.version,
            project: release.project,
            target: release.target,
            refresh: release.refresh,
            shape: Shape {
                detail,
                what: What::Overview,
            },
        }
    }
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
    shape: Shape,
    /// The sha256 of the `.crate` in the crates.io index, when the index was read.
    checksum: Option<Box<str>>,
    /// How docs.rs built it, once its `.crate` is read (requirement R-FT-1).
    build: Option<Box<DocsRsBuild>>,
    /// The project's build of it, when the project decided the version.
    in_build: InBuild,
    /// How many crates the path was followed through to get here (requirement R-PR-4).
    hops: usize,
    /// When the crates.io index that decided the version was read, when it was older than its
    /// TTL (requirements R-CA-3 and R-CA-7), in RFC 3339.
    index_fetched_at: Option<Box<str>>,
    /// Until when the call waits for work that goes on without it (requirement R-ARCH-6).
    until: Instant,
    /// The workspace member's crate that is built here instead, if it is one (decided in M2's
    /// D2).
    #[cfg(feature = "local")]
    local: Option<LocalSource>,
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
                "description": const_format::concatcp!("The workspace (its directory or Cargo.toml) whose Cargo.lock \
                                decides the version when none is given. Defaults to the project \
                                Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
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
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `ItemResponse` in `query::get_item` (requirement
/// R-OUT-1). Every property but the required ones is left out when it has no value.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": provenance_schema(),
            "warnings": strings,
            "data": data_schema(),
            "hints": strings,
        },
        "required": ["provenance", "data"],
    }))
}

/// The schema of `provenance`: `Provenance` in `query::get_item`.
pub(super) fn provenance_schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "source": { "type": "string", "enum": ["docs.rs", "std", "local"] },
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
                "enum": ["argument", "claude_project_dir", "cwd"],
            },
            "index_fetched_at": {
                "type": "string",
                "description": "When the crates.io index that decided the version was read (RFC \
                                3339), when it was older than it is trusted for, as offline.",
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
            "built_at": {
                "type": "string",
                "description": "When the documentation of a workspace member was built here (RFC \
                                3339): edits made since are not in it.",
            },
            "stale": {
                "type": "boolean",
                "description": "The documentation is older than the code: stale_reason tells why.",
            },
            "stale_reason": {
                "type": "string",
                "enum": ["rebuilding"],
                "description": "rebuilding: a workspace member is being built again, as what it \
                                is made from changed; this is the build before, of built_at.",
            },
            "features_requested": {
                "type": "array",
                "items": { "type": "string" },
                "description": "The features that a workspace member was built with, as in the \
                                build of the whole workspace, with no default features besides.",
            },
            "features_effective": {
                "type": "array",
                "items": { "type": "string" },
                "description": "The features that cargo built a workspace member with.",
            },
        },
        // Before the version is known, a response while it is fetched has no `version`, and may
        // have no `source` (requirement R-OUT-9).
        "required": ["crate", "target"],
    })
}

/// The properties of `Conditions` in `query::get_item`, which a list's entries that are not items
/// of their own have (an `impl`), taken from those of [`data_schema`].
pub(super) fn conditions_properties() -> JsonObject {
    let data = data_schema();
    [
        "required_features",
        "other_cfg",
        "cfg_source",
        "enabled_in_project",
        "enabled_in_dev_only",
    ]
    .into_iter()
    .filter_map(|name| {
        let schema = data.get("properties")?.get(name)?;
        Some((name.to_owned(), schema.clone()))
    })
    .collect()
}

/// The schema of `defined_at`, which [`show_at_entry`] adds to `data`.
pub(super) fn defined_at_schema() -> Value {
    json!({
        "type": "string",
        "description": "When the path asked for is a re-export of another crate's item, the \
                        item's path in that crate, the crate of provenance.",
    })
}

/// The schema of the visibility of an item, which a workspace member's items have (decided in M2's
/// D6).
fn visibility_schema() -> Value {
    json!({
        "type": "string",
        "description": "In a workspace member: pub, pub(crate), pub(in crate::path) or private.",
    })
}

/// The schema of `data`: `ItemData` in `query::get_item`.
#[expect(
    clippy::too_many_lines,
    reason = "the schema of each field of the data, in the order of the data"
)]
pub(super) fn data_schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "The canonical path, or in a list the path it is listed at.",
            },
            "defined_at": defined_at_schema(),
            "canonical": {
                "type": "string",
                "description": "In a list, the canonical path of an item listed at another.",
            },
            "kind": { "type": "string" },
            "visibility": visibility_schema(),
            "hidden": { "type": "boolean" },
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
                                needs; the same for its use in const; and the feature that the \
                                default of a trait's item needs.",
                "properties": {
                    "stable_since": { "type": "string" },
                    "unstable_feature": { "type": "string" },
                    "const_stable_since": { "type": "string" },
                    "const_unstable_feature": { "type": "string" },
                    "default_unstable": { "type": "string" },
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
        refresh: parse_refresh(arguments)?,
    })
}

/// Checks `refresh` (§6.4): `false` when it is not given.
pub(super) fn parse_refresh(arguments: Option<&JsonObject>) -> Result<bool, InvalidArgument> {
    match arguments.and_then(|arguments| arguments.get("refresh")) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(refresh)) => Ok(*refresh),
        Some(_) => Err(InvalidArgument::new("`refresh` must be true or false", &[
            r#"pass true to try again what failed, for example {"refresh": true}"#,
        ])),
    }
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
    /// Whether what failed is to be tried again (§6.4, decided in M2's D5).
    pub(super) refresh: bool,
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
        refresh,
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
        refresh,
        shape: Shape {
            detail,
            what: What::Item,
        },
    })
}

/// The request for a workspace member's crate, built here, when the lookup is in one (decided in
/// M2's D2): the project decided the version, as `found`, and it is not from crates.io, which
/// docs.rs does not have even when crates.io has a crate of that name (requirement R-VR-2); or
/// `spec` is the exact version of the member of that name in the project that `project` names
/// (requirement R-VR-1).
#[cfg(feature = "local")]
async fn member_in_project(
    state: &ToolState,
    (found, spec, project): (
        Option<&project::Found>,
        &VersionSpec,
        Option<&std::path::Path>,
    ),
    name: &CrateName,
    (path, target, shape): (&str, &TargetTriple, &Shape),
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Result<Option<Request>, Box<CallToolResult>> {
    match (found, spec) {
        (Some(found), _) if found.package.source != Source::CratesIo => {
            in_member(state, found, name, path, target, shape, until)
                .await
                .map(Some)
        },
        (None, VersionSpec::Exact(version)) => {
            explicit_member(
                state,
                project,
                name,
                (version, path, target, shape),
                until,
                warnings,
            )
            .await
        },
        _ => Ok(None),
    }
}

/// The request for the project's package `found`, not from crates.io: the workspace member's crate
/// `name`, built here, or else `local_build_required` (decided in M2's D2).
#[cfg(feature = "local")]
async fn in_member(
    state: &ToolState,
    found: &project::Found,
    name: &CrateName,
    path: &str,
    target: &TargetTriple,
    shape: &Shape,
    until: Instant,
) -> Result<Request, Box<CallToolResult>> {
    let asked = local::Asked::Package {
        name: &found.package.name,
        version: &found.package.version,
    };
    match local::member(state, &found.workspace, (name, asked), target, until).await? {
        Member::Is { wanted, package } => Ok(member_request(
            &package,
            wanted,
            (name, path),
            (target, shape),
            (found.resolved_from, found.project_from, &found.workspace),
            until,
        )),
        Member::Unknown(error) => Err(error),
        Member::Resolving(doing) => Err(Box::new(project::resolving_crate(
            name,
            target,
            (doing, found.project_from),
            &[],
        ))),
        Member::Planning(doing) => Err(Box::new(project::planning_crate(
            name,
            target,
            (doing, found.project_from),
            &[],
        ))),
        Member::OtherVersion(_) | Member::Not => Err(Box::new(local_build_required(
            &found.package,
            DocsRsCheck::NotCratesIo,
        ))),
    }
}

/// The request for the crate `name` at the exact version that `explicit` gives, with the path,
/// target and shape, when it is the version of the member of that name in the project that
/// `project` names, or else the default project's (requirement R-VR-1, decided in M2's D2). A
/// member at another version is warned about.
#[cfg(feature = "local")]
async fn explicit_member(
    state: &ToolState,
    project: Option<&std::path::Path>,
    name: &CrateName,
    (version, path, target, shape): (&Version, &str, &TargetTriple, &Shape),
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Result<Option<Request>, Box<CallToolResult>> {
    // A project that cannot be read leaves the lookup to crates.io, as asked.
    let Ok(Some((workspace, project_from))) = project::workspace(state.project.as_ref(), project)
    else {
        return Ok(None);
    };
    let asked = local::Asked::Explicit { version };
    match local::member(state, &workspace, (name, asked), target, until).await {
        Ok(Member::Is { wanted, package }) => {
            warnings.push(
                format!(
                    "{name} {version} is the workspace's member, built here, which may differ \
                     from the release of that name on crates.io"
                )
                .into(),
            );
            Ok(Some(member_request(
                &package,
                wanted,
                (name, path),
                (target, shape),
                (ResolvedFrom::Explicit, project_from.as_str(), &workspace),
                until,
            )))
        },
        Ok(Member::OtherVersion(member)) => {
            warnings.push(
                format!(
                    "the workspace's member {name} is at {member}, so {version} is the release of \
                     that name on crates.io, not the member"
                )
                .into(),
            );
            Ok(None)
        },
        // The member, whose build is being planned: not the release of that name.
        Ok(Member::Planning(doing)) => Err(Box::new(project::planning_crate(
            name,
            target,
            (doing, project_from.as_str()),
            warnings,
        ))),
        // Not told while cargo resolves, nor when it fails: the release is looked up as asked.
        Ok(Member::Not | Member::Unknown(_) | Member::Resolving(_)) | Err(_) => Ok(None),
    }
}

/// The request for the crate of the member `package` that `wanted` builds, with `path` from the
/// name it was asked by to the crate's, how its version was decided, where the project came from,
/// and its workspace. The request's name is the package's, which its features are found by.
#[cfg(feature = "local")]
fn member_request(
    package: &str,
    wanted: Box<LocalWanted>,
    (name, path): (&CrateName, &str),
    (target, shape): (&TargetTriple, &Shape),
    (resolved_from, project_from, workspace): (ResolvedFrom, &'static str, &Workspace),
    until: Instant,
) -> Request {
    // The crate's name, which `[lib] name` may make other than the package's.
    let path = with_lib_name(name.as_str(), path);
    let path = match path.split_once("::") {
        Some((_, rest)) => format!("{}::{rest}", wanted.plan.crate_name).into(),
        None => wanted.plan.crate_name.clone(),
    };
    Request {
        name: package.parse().unwrap_or_else(|_| name.clone()),
        version: wanted.plan.version.clone(),
        project_from: Some(project_from),
        path,
        resolved_from,
        target: target.clone(),
        shape: shape.clone(),
        checksum: None,
        build: None,
        in_build: InBuild {
            workspace: Some(workspace.clone()),
            sets: None,
        },
        hops: 0,
        index_fetched_at: None,
        until,
        local: Some(LocalSource::Wanted(wanted)),
    }
}

/// Decides the version (requirements R-VR-1 and R-VR-3) for `target`. An exact version whose JSON
/// for `target` is cached is taken as it is; otherwise the crates.io index is read, which also
/// gives the crate's published spelling. Returns the request with the warnings about how it was
/// decided, or the tool error (boxed, as a result is large).
async fn resolve(
    state: &ToolState,
    arguments: Arguments,
    target: TargetTriple,
    mut warnings: Vec<Box<str>>,
    until: Instant,
) -> Result<(Request, Vec<Box<str>>), Box<CallToolResult>> {
    let Arguments {
        name,
        version,
        project,
        path,
        shape,
        ..
    } = arguments;
    // Without a version, the project decides it (requirement R-VR-2), or else it is the latest.
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_variables, reason = "only a project decides it")
    )]
    let (spec, from_project) = if let Some(spec) = version {
        (spec, FromProject::default())
    } else {
        let in_project = project::version_in_project(state, &name, project.as_deref(), until).await;
        decided_in_project(in_project, &name, &target, &mut warnings)?
    };
    #[cfg(feature = "local")]
    if let Some(request) = member_in_project(
        state,
        (from_project.as_deref(), &spec, project.as_deref()),
        &name,
        (&path, &target, &shape),
        until,
        &mut warnings,
    )
    .await?
    {
        return Ok((request, warnings));
    }
    // When the project did not decide the version, its build still tells `enabled_in_project`
    // if it has the release from crates.io (requirement R-VR-1), which the index tells below.
    #[cfg(feature = "local")]
    let same_release = match from_project {
        None => same_release(state, &name, project.as_deref(), &spec, until).await,
        Some(_) => None,
    };
    #[cfg(feature = "local")]
    let checked_later = from_project.is_some() || same_release.is_some();
    #[cfg(not(feature = "local"))]
    let checked_later = false;
    if !checked_later && let Some(version) = cached_exact(state, &name, &spec, &target) {
        return Ok((
            cached_request(name, version, &path, target, shape, until),
            warnings,
        ));
    }
    let entry = state
        .index
        .entry(&name)
        .await
        .map_err(|error| Box::new(index_error(&state.cache, &name, &error)))?;
    warnings.extend(entry.stale.clone());
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
        .map_err(|no_version| Box::new(version_not_found(state, &entry, &spec, &no_version)))?;
    warnings.extend(resolved.warnings.into_iter().map(Into::into));
    #[cfg(feature = "local")]
    let same_release = same_release.and_then(|same| {
        the_release_in_build(same, &resolved.version.version, &entry, &mut warnings)
    });
    #[cfg(feature = "local")]
    let (resolved_from, project_from, in_build) = chosen_by(&spec, from_project, same_release);
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
        index_fetched_at: entry
            .stale
            .as_ref()
            .map(|_| data::rfc3339(entry.fetched_at).into()),
        project_from,
        path,
        resolved_from,
        target,
        shape,
        until,
        #[cfg(feature = "local")]
        local: None,
    };
    Ok((request, warnings))
}

/// The project's package that decided the version, when it did.
#[cfg(feature = "local")]
type FromProject = Option<Box<project::Found>>;
/// Without local features, no project decides the version.
#[cfg(not(feature = "local"))]
type FromProject = ();

/// The version that `in_project` tells for `name`, the project's or else the latest, and the
/// package that decided it; or the response to return, while the project is being resolved for
/// `target` or when it cannot be.
#[cfg_attr(
    not(feature = "local"),
    expect(
        clippy::unnecessary_wraps,
        reason = "only a project gives a response to return"
    )
)]
fn decided_in_project(
    in_project: InProject,
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_variables, reason = "for a project")
    )]
    name: &CrateName,
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_variables, reason = "for a project")
    )]
    target: &TargetTriple,
    warnings: &mut Vec<Box<str>>,
) -> Result<(VersionSpec, FromProject), Box<CallToolResult>> {
    match in_project {
        #[cfg(feature = "local")]
        InProject::Found(found) => {
            warnings.extend(found.warnings.iter().cloned());
            let version = found.package.version.clone();
            Ok((VersionSpec::Exact(version), Some(found)))
        },
        InProject::Elsewhere(elsewhere) => {
            warnings.extend(elsewhere);
            Ok((VersionSpec::Latest, FromProject::default()))
        },
        #[cfg(feature = "local")]
        InProject::Error(error) => Err(error),
        #[cfg(feature = "local")]
        InProject::Going(doing, project_from) => Err(Box::new(project::resolving_crate(
            name,
            target,
            (doing, project_from),
            warnings,
        ))),
    }
}

/// The request for `name` at the exact `version` whose JSON is cached, without the crates.io
/// index.
fn cached_request(
    name: CrateName,
    version: Version,
    path: &str,
    target: TargetTriple,
    shape: Shape,
    until: Instant,
) -> Request {
    Request {
        version,
        path: with_lib_name(&name.cache_key(), path),
        name,
        project_from: None,
        resolved_from: ResolvedFrom::Explicit,
        target,
        shape,
        checksum: None,
        build: None,
        in_build: InBuild::default(),
        hops: 0,
        index_fetched_at: None,
        until,
        #[cfg(feature = "local")]
        local: None,
    }
}

/// The project's packages from crates.io of `name` (the one `project` names, or the default
/// project), when one may be the release that `spec` asks for: the crates.io index then tells
/// whether it is ([`the_release_in_build`]).
#[cfg(feature = "local")]
async fn same_release(
    state: &ToolState,
    name: &CrateName,
    project: Option<&std::path::Path>,
    spec: &VersionSpec,
    until: Instant,
) -> Option<project::FromCratesIo> {
    let same = project::crates_io_in_project(state, name, project, until).await?;
    let may_be = match spec {
        VersionSpec::Exact(version) => same
            .packages
            .iter()
            .any(|package| package.version == *version),
        VersionSpec::Requirement(_) | VersionSpec::Latest => true,
    };
    may_be.then_some(same)
}

/// The project's workspace and where it came from, when its package of `same` at `version` is the
/// release on crates.io (its checksum is the index's): its build then tells `enabled_in_project`
/// (requirements R-VR-1 and R-FT-2). A copy that differs is warned about.
#[cfg(feature = "local")]
fn the_release_in_build(
    same: project::FromCratesIo,
    version: &Version,
    entry: &IndexEntry,
    warnings: &mut Vec<Box<str>>,
) -> Option<(Workspace, &'static str)> {
    let package = same
        .packages
        .iter()
        .find(|package| package.version == *version)?;
    let check = package.docs_rs_check(entry);
    if check == DocsRsCheck::Allowed {
        return Some((same.workspace, same.project_from));
    }
    warnings.push(
        format!(
            "the project's {}@{} is {}, so whether items are in its build is not shown",
            package.name,
            package.version,
            not_on_docs_rs(package, check)
        )
        .into(),
    );
    None
}

/// How the version was chosen, where the project came from if it chose it or has the same
/// release (`same_release`), and the project's build of the crate.
#[cfg(feature = "local")]
fn chosen_by(
    spec: &VersionSpec,
    from_project: Option<Box<project::Found>>,
    same_release: Option<(Workspace, &'static str)>,
) -> (ResolvedFrom, Option<&'static str>, InBuild) {
    if let Some(found) = from_project {
        let in_build = InBuild {
            workspace: Some(found.workspace),
            sets: None,
        };
        return (found.resolved_from, Some(found.project_from), in_build);
    }
    let (resolved_from, ..) = chosen_by_argument(spec);
    match same_release {
        Some((workspace, project_from)) => {
            let in_build = InBuild {
                workspace: Some(workspace),
                sets: None,
            };
            (resolved_from, Some(project_from), in_build)
        },
        None => (resolved_from, None, InBuild::default()),
    }
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
    state: &ToolState,
    entry: &IndexEntry,
    spec: &VersionSpec,
    no_version: &NoVersion,
) -> CallToolResult {
    let name = &entry.name;
    // An index file read long ago may lack the version, which may be newer (requirements R-CA-3
    // and R-CA-7).
    if let Some(stale) = &entry.stale {
        let message = format!("{name} {spec} is not in the crates.io index as last read: {stale}");
        return if state.index.is_offline() {
            let suggestions = name.parse::<CrateName>().map_or_else(
                |_| Vec::new(),
                |name| offline_suggestions(&state.cache, &name),
            );
            error_result(&json!({
                "code": "offline_miss",
                "message": message,
                "retryable": false,
                "suggestions": suggestions,
            }))
        } else {
            error_result(&json!({
                "code": "network_error",
                "message": message,
                "retryable": true,
                "suggestions": ["try again in a while"],
            }))
        };
    }
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
/// on crates.io, and this server builds only the workspace's members yet (decided in M1, and in
/// M2's D2 until D7).
#[cfg(feature = "local")]
fn local_build_required(package: &ProjectPackage, check: DocsRsCheck) -> CallToolResult {
    let why = not_on_docs_rs(package, check);
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!(
            "{}@{} in the project is {why}, so docs.rs does not have it, and this server builds \
             the documentation of the workspace's members only yet",
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

/// The most versions in the cache that `offline_miss` names.
const MAX_CACHED_VERSIONS: usize = 5;

/// What `offline_miss` suggests: the versions of `name` in the cache, newest first, and else to
/// turn offline mode off (§8 and requirement R-CA-7).
pub(super) fn offline_suggestions(cache: &Cache, name: &CrateName) -> Vec<String> {
    let versions = cache.docs_rs_versions(name);
    let mut suggestions = Vec::new();
    if !versions.is_empty() {
        let listed: Vec<String> = versions
            .iter()
            .take(MAX_CACHED_VERSIONS)
            .map(|version| format!("{name}@{version}"))
            .collect();
        suggestions.push(format!(
            "the cache has the documentation of {}{}: ask for one of these",
            listed.join(", "),
            if versions.len() > MAX_CACHED_VERSIONS {
                " and older versions"
            } else {
                ""
            }
        ));
    }
    suggestions.push(
        "the server is offline (network.offline): turn network.offline off to fetch what the \
         cache lacks"
            .to_owned(),
    );
    suggestions
}

/// The tool error for an index that could not be read (§8).
pub(super) fn index_error(cache: &Cache, name: &CrateName, error: &IndexError) -> CallToolResult {
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
        IndexError::RecentFailure { retry_after } => error_result(&json!({
            "code": "network_error",
            "message": message,
            "retryable": true,
            "suggestions": ["try again in a while"],
            "retry_after_secs": retry_after.as_secs().max(1),
        })),
        IndexError::UnexpectedStatus { .. }
        | IndexError::TooLarge { .. }
        | IndexError::Malformed(_) => error_result(&json!({
            "code": "network_error",
            "message": message,
            "retryable": false,
            "suggestions": [],
        })),
        IndexError::Offline => error_result(&json!({
            "code": "offline_miss",
            "message": message,
            "retryable": false,
            "suggestions": offline_suggestions(cache, name),
        })),
        IndexError::Cache(_) => super::tool_error(
            "internal_error",
            "the cache of the crates.io index could not be used",
            false,
            &["check the server's stderr log for the cache directory and the error"],
        ),
    }
}

/// `path`, starting with the crate name: it is added when the first segment is something else,
/// and takes the place of `crate`. `-` and `_` in the crate name are the same.
fn with_crate_name(name: &CrateName, path: &str) -> Box<str> {
    let first = path.split("::").next().unwrap_or_default();
    // `crate::a::Item`, as an item of a workspace member without a public path is shown
    // (requirement R-PR-2, decided in M2's D6).
    if first == "crate" {
        let rest = path.strip_prefix("crate").unwrap_or_default();
        return format!("{}{rest}", name.as_str().replace('-', "_")).into();
    }
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
    // `refresh` is acted on once for the crate in the project, until a call for it ends (decided
    // in M2's D5).
    #[cfg(feature = "local")]
    {
        let scope = format!("crate {:?} {}", arguments.project, arguments.name);
        let refresh = arguments.refresh && local::refresh_acts(&scope);
        let result = look_up(arguments, state, refresh).await;
        local::refresh_told(&scope, &result);
        result
    }
    #[cfg(not(feature = "local"))]
    look_up(arguments, state).await
}

/// [`execute`], forgetting what failed first when `refresh`.
async fn look_up(
    arguments: Arguments,
    state: Arc<ToolState>,
    #[cfg(feature = "local")] refresh: bool,
) -> CallToolResult {
    let until = state.until();
    // What failed is forgotten before it is read again (§6.4, decided in M2's D5).
    #[cfg(feature = "local")]
    let refresh = refresh && local::refresh_project(&state, arguments.project.as_deref()).await;
    if let Some(krate) = std_crate(&arguments.name) {
        #[cfg(feature = "local")]
        if refresh {
            data::forget_failures(state.cache.dir().join("std"), true).await;
        }
        return execute_std(arguments, krate, state, Vec::new(), until).await;
    }
    #[cfg(feature = "local")]
    if refresh {
        state.index.forget(&arguments.name);
    }
    let (target, warnings) = match &arguments.target {
        Some(target) => (target.clone(), Vec::new()),
        None => project::default_target(&state, arguments.project.as_deref()).await,
    };
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_mut, reason = "only local features refresh the release")
    )]
    let (mut arguments, warnings) = match resolve(&state, arguments, target, warnings, until).await
    {
        Ok(resolved) => resolved,
        Err(error) => return *error,
    };
    #[cfg(feature = "local")]
    if refresh {
        refresh_release(&state, &mut arguments).await;
    }
    look_up_release(&state, arguments, warnings).await
}

/// Forgets what failed of the release that `request` names, as the argument `refresh` asks (§6.4,
/// decided in M2's D5): a member's build forgets it as it begins, and for docs.rs, its JSON and its
/// `.crate` are fetched again.
#[cfg(feature = "local")]
async fn refresh_release(state: &ToolState, request: &mut Request) {
    match &mut request.local {
        Some(LocalSource::Wanted(wanted)) => wanted.refresh = true,
        Some(LocalSource::Built(_)) => {},
        None => {
            for target in [Some(&request.target), None] {
                let dir = state
                    .cache
                    .docs_rs_dir(&request.name, &request.version, target);
                data::forget_failures(dir, false).await;
            }
            data::forget_failure_of(&state.cache.crate_file_path(&request.name, &request.version));
        },
    }
}

/// Looks the item up in the release that `arguments` names, from its archive or its JSON on
/// docs.rs, and follows the path into the crate it goes on in, if any (requirement R-PR-4).
async fn look_up_release(
    state: &Arc<ToolState>,
    mut arguments: Request,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    // Built here: docs.rs and static.crates.io have another crate of its name, if any.
    #[cfg(feature = "local")]
    if let Some(source) = arguments.local.take() {
        return look_up_member(state, arguments, source, warnings).await;
    }
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
    let dir = state
        .cache
        .docs_rs_dir(&arguments.name, &arguments.version, Some(&arguments.target));
    let looked_up = load_and_look_up(state, dir, arguments, &mut warnings).await;
    match looked_up {
        Ok((looked_up, arguments)) => conclude(state, looked_up, arguments, warnings).await,
        Err(result) => *result,
    }
}

/// Looks the item up in the build of the workspace member's crate that `source` names, made by
/// work that the calls for it share, and follows the path into the crate it goes on in, if any
/// (decided in M2's D2).
#[cfg(feature = "local")]
async fn look_up_member(
    state: &Arc<ToolState>,
    mut arguments: Request,
    source: LocalSource,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let built = match source {
        LocalSource::Built(built) => built,
        LocalSource::Wanted(wanted) => {
            let provenance = local_provenance(&arguments);
            // `build.wait_secs` from now, within the call's deadline (decided in M2's D3).
            let until_built = Instant::now()
                .checked_add(state.build_wait)
                .map_or(arguments.until, |until| until.min(arguments.until));
            let until = (until_built, arguments.until);
            match local::built(state, *wanted, until, provenance, &warnings).await {
                Ok(built) => {
                    warnings.extend(built.archived.warnings.iter().cloned());
                    built
                },
                Err(result) => return *result,
            }
        },
    };
    // The build before, while the member is built again (decided in M2's D4).
    let built_before = built.stale.then(|| built.record.built_at.clone());
    let rebuilt = built.stale.then(|| built.record.crate_name.clone());
    if let (Some(built_at), Some(krate)) = (&built_before, &rebuilt) {
        warnings.push(
            format!(
                "this is the build of {krate} made at {built_at}, given while it is built again, \
                 as what it is made from changed: the lines of `span` may have moved"
            )
            .into(),
        );
    }
    arguments.in_build.sets = feature_sets(state, &arguments, &mut warnings).await;
    // Kept for the lookup again when a re-export is not followed.
    arguments.local = Some(LocalSource::Built(Arc::clone(&built)));
    let task = crate::task::spawn_blocking(move || {
        let record = &built.record;
        let origin = Origin::Local {
            resolved_from: arguments.resolved_from,
            project_from: arguments.project_from,
            toolchain: record.toolchain.as_deref(),
            built_at: &record.built_at,
            features_requested: record.features_requested.as_deref(),
            features_effective: record.features_effective.as_deref(),
        };
        let mut looked_up = look_up_in(&built.archived.mapped, &arguments, origin);
        if let (Some(built_at), Ok(response)) = (&built_before, &mut looked_up) {
            mark_rebuilding(response, built_at);
        }
        (looked_up, arguments)
    });
    match task.await {
        // Not in the build before: maybe in the one being made, as an item just added.
        Ok((Err(Failure::Item(error)), arguments))
            if !matches!(error, GetItemError::Elsewhere(_)) && rebuilt.is_some() =>
        {
            let being = format!(
                "{} is built again, and the build before has no such item, which the new build \
                 may have",
                rebuilt.unwrap_or_default()
            );
            building_response(
                local_provenance(&arguments),
                Doing::Building,
                &being,
                &warnings,
            )
        },
        Ok((looked_up, arguments)) => conclude(state, looked_up, arguments, warnings).await,
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the get_item lookup did not finish");
            shutting_down()
        },
    }
}

/// Marks `response` as of the build of a member made at `built_at`, given while the member is built
/// again (requirements R-WS-9 and R-OUT-9, decided in M2's D4): `stale` and `stale_reason` in its
/// provenance, `retry_after_secs` after it, and a hint to see the code changed since with the LSP.
#[cfg(feature = "local")]
fn mark_rebuilding(response: &mut Value, built_at: &str) {
    let Some(fields) = response.as_object_mut() else {
        return;
    };
    if let Some(Value::Object(provenance)) = fields.get_mut("provenance") {
        provenance.insert("stale".to_owned(), json!(true));
        provenance.insert("stale_reason".to_owned(), json!("rebuilding"));
    }
    let hint = json!(format!(
        "for the code changed since {built_at}, use the LSP tool (rust-analyzer); call again in \
         about {RETRY_AFTER_SECS} seconds for the new build"
    ));
    // The fields are in order: `retry_after_secs` after `provenance`, the hint last (§7.3).
    let old = std::mem::take(fields);
    let mut hints = None;
    for (key, value) in old {
        let after = key == "provenance";
        if key == "hints" {
            hints = Some(value);
            continue;
        }
        fields.insert(key, value);
        if after {
            fields.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
        }
    }
    let mut hints = match hints {
        Some(Value::Array(hints)) => hints,
        _ => Vec::new(),
    };
    hints.push(hint);
    fields.insert("hints".to_owned(), Value::Array(hints));
}

/// The provenance of the workspace member's crate that `arguments` names, as far as it is known
/// before it is built.
#[cfg(feature = "local")]
fn local_provenance(arguments: &Request) -> Map<String, Value> {
    let mut provenance = docs_rs_provenance(arguments);
    provenance.insert("source".to_owned(), json!("local"));
    provenance
}

/// Looks the item up in the archive of the crate in `dir`, or else in the archive that work made
/// from the JSON of the crate on docs.rs.
///
/// The calls for the same crate share the work, which holds the lock of `dir` (requirements R-NW-2,
/// R-WS-6 and R-CA-4). A call waits for it until `until` in the request, and then returns
/// `status: building` while the work goes on (requirements R-ARCH-6 and R-OUT-9).
async fn load_and_look_up(
    state: &Arc<ToolState>,
    dir: PathBuf,
    arguments: Request,
    warnings: &mut Vec<Box<str>>,
) -> Result<(Result<Value, Failure>, Request), Box<CallToolResult>> {
    let arguments =
        match look_up_archived(state.cache.clone(), dir.clone(), Box::new(arguments)).await {
            Some(Ok((looked_up, arguments))) => return Ok((looked_up, *arguments)),
            Some(Err(arguments)) => *arguments,
            None => return Err(Box::new(shutting_down())),
        };
    let archived = match load::docs_rs_archive(
        &state.archives,
        &state.cache,
        &state.docs_rs,
        &arguments.name,
        &arguments.version,
        Some(&arguments.target),
        arguments.until,
    )
    .await
    {
        Joined::Done(Ok(archived)) => archived,
        Joined::Done(Err(error)) => {
            return Err(Box::new(archive_error(&state.cache, &arguments, &error)));
        },
        Joined::Ended => return Err(Box::new(stopped())),
        Joined::Going(doing) => return Err(Box::new(building(&arguments, doing, warnings))),
    };
    warnings.extend(archived.warnings.iter().cloned());
    let mapped = Arc::clone(&archived.mapped);
    let task = crate::task::spawn_blocking(move || {
        let looked_up = look_up_in(&mapped, &arguments, docs_rs_origin(&arguments));
        (looked_up, arguments)
    });
    match task.await {
        Ok(done) => Ok(done),
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the get_item lookup did not finish");
            Err(Box::new(shutting_down()))
        },
    }
}

/// Why std's JSON is not there to look in.
pub(super) enum StdUnready {
    /// It could not be had.
    Failed(Arc<DistError>),
    /// The work ended without a result, as when it panicked.
    Stopped,
    /// It is still being fetched or indexed, as this says.
    Going(Doing),
}

/// The JSON of std for `target`, chosen, fetched and archived by work that the calls for it share,
/// waiting for it until `until` (requirements R-STD-6, R-ARCH-6 and R-WS-6).
pub(super) async fn shared_std(
    state: &ToolState,
    target: &TargetTriple,
    host: &TargetTriple,
    at_least: Option<&Version>,
    until: Instant,
) -> Result<Arc<Chosen>, StdUnready> {
    let wanted = load::StdWanted {
        target: target.clone(),
        host: host.clone(),
        at_least: at_least.cloned(),
        neutral_dir: state.cache.dir().to_owned(),
    };
    match load::std_json(&state.stds, &state.cache, &state.dist, wanted, until).await {
        Joined::Done(Ok(chosen)) => Ok(chosen),
        Joined::Done(Err(error)) => Err(StdUnready::Failed(error)),
        Joined::Ended => Err(StdUnready::Stopped),
        Joined::Going(doing) => Err(StdUnready::Going(doing)),
    }
}

/// When to call again for a crate that is being fetched or indexed (requirement R-OUT-9).
pub(super) const RETRY_AFTER_SECS: u64 = 10;

/// The response while the crate of `arguments` is being fetched or indexed, as `doing` says, by
/// work that goes on after the call stopped waiting for it (requirements R-ARCH-6 and R-OUT-9):
/// `status: building`, why, when to call again, and the provenance of what was asked for.
fn building(arguments: &Request, doing: Doing, warnings: &[Box<str>]) -> CallToolResult {
    let what = format!("{} {}", arguments.name, arguments.version);
    let being = being(&what, doing, "docs.rs");
    building_response(docs_rs_provenance(arguments), doing, &being, warnings)
}

/// What `what` is going through, for people: fetched from `from`, indexed, or waiting for cargo.
pub(super) fn being(what: &str, doing: Doing, from: &str) -> String {
    match doing {
        Doing::Fetching => format!("{what} is being fetched from {from}"),
        Doing::Indexing => format!("{what} is being indexed"),
        Doing::WaitingForLock => format!("{what} waits for a lock that another cargo holds"),
        Doing::Building => format!("the documentation of {what} is being built"),
        Doing::WaitingForSlot => {
            format!("the documentation of {what} waits for one of the builds that run to end")
        },
    }
}

/// The response while the JSON of std for `target` is being chosen, fetched or indexed, before its
/// nightly is known, so without its version (requirement R-OUT-9). `name` is std's crate asked for.
pub(super) fn std_building(
    name: &str,
    target: &TargetTriple,
    doing: Doing,
    warnings: &[Box<str>],
) -> CallToolResult {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("std"));
    provenance.insert("crate".to_owned(), json!(name));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    let being = being(
        &format!("the documentation of std for {target}"),
        doing,
        "the Rust distribution server",
    );
    building_response(provenance, doing, &being, warnings)
}

/// The provenance of the release that `arguments` names on docs.rs, as far as it is known before
/// its JSON is.
fn docs_rs_provenance(arguments: &Request) -> Map<String, Value> {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("docs.rs"));
    provenance.insert("crate".to_owned(), json!(arguments.name.as_str()));
    provenance.insert("version".to_owned(), json!(arguments.version.to_string()));
    provenance.insert("resolved_from".to_owned(), json!(arguments.resolved_from));
    if let Some(project_from) = arguments.project_from {
        provenance.insert("project_from".to_owned(), json!(project_from));
    }
    if let Some(fetched_at) = &arguments.index_fetched_at {
        provenance.insert("index_fetched_at".to_owned(), json!(fetched_at));
    }
    provenance.insert("target".to_owned(), json!(arguments.target.as_str()));
    provenance
}

/// The response with `provenance` while what it is of is being fetched or indexed, as `doing`
/// says, and `being` tells people (requirement R-OUT-9).
pub(super) fn building_response(
    provenance: Map<String, Value>,
    doing: Doing,
    being: &str,
    warnings: &[Box<str>],
) -> CallToolResult {
    let mut response = Map::new();
    response.insert("provenance".to_owned(), Value::Object(provenance));
    response.insert("status".to_owned(), json!("building"));
    response.insert("building_reason".to_owned(), json!(doing.as_str()));
    response.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
    if !warnings.is_empty() {
        response.insert("warnings".to_owned(), json!(warnings));
    }
    response.insert(
        "hints".to_owned(),
        json!([format!(
            "{being}, which goes on without this call; call again in about {RETRY_AFTER_SECS} \
             seconds"
        )]),
    );
    CallToolResult::structured(Value::Object(response))
}

/// The tool error when the archive of the crate of `arguments` could not be made (§8).
fn archive_error(cache: &Cache, arguments: &Request, error: &ArchiveError) -> CallToolResult {
    match error {
        ArchiveError::Load(error) => load_error(cache, arguments, error),
        ArchiveError::Make(load::LoadError::Parse(error)) => parse_error(arguments, error),
        ArchiveError::Make(load::LoadError::Archive(error) | load::LoadError::Read(error)) => {
            kept_in_memory_error(error)
        },
        ArchiveError::Stopped => shutting_down(),
    }
}

/// The tool error when a crate could not be archived, not even in memory.
fn kept_in_memory_error(error: &std::io::Error) -> CallToolResult {
    tracing::error!(%error, "the crate could not be archived");
    super::tool_error(
        "internal_error",
        "the server could not keep the crate in memory",
        false,
        &["check the server's stderr log"],
    )
}

/// The tool error when the work that loads the crate ended without a result, as when it
/// panicked: the panic hook has written why to stderr.
pub(super) fn stopped() -> CallToolResult {
    super::tool_error(
        "internal_error",
        "the work that loads the crate stopped without a result (a bug)",
        true,
        &[
            "call again; if it fails again, report the bug to the server's maintainers, with the \
           server's stderr log",
        ],
    )
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
/// (requirement R-PR-4): a crate of std, a release on docs.rs, or another workspace member. If
/// that fails, the re-export itself is returned from `origin`, with why.
///
/// Through glob re-exports (`pub use other::*;`) of a workspace member, each is tried in turn
/// into another member, as one may not have the item; when none has it, the path is not found in
/// `origin` (decided in M2's D6).
async fn follow(
    state: &Arc<ToolState>,
    destination: Destination,
    mut origin: Request,
    warnings: Vec<Box<str>>,
) -> CallToolResult {
    let mut candidates = vec![destination.clone()];
    candidates.extend(destination.others.iter().cloned());
    let mut failed = None;
    for destination in candidates {
        let result = follow_into(state, &destination, &origin, &warnings).await;
        if result.is_error != Some(true) {
            return result_at_entry(result, &destination.entry);
        }
        if destination.via_glob && error_code(&result).as_deref() == Some("item_not_found") {
            continue;
        }
        failed = Some((destination, result));
        break;
    }
    origin.hops = MAX_FOLLOWED;
    let Some((destination, result)) = failed else {
        // No glob had it: as if there were none.
        return Box::pin(look_up_release(state, origin, warnings)).await;
    };
    // The glob's member failed to be built, as told, which looking it up itself tells again.
    if destination.via_glob {
        return with_suggestion(
            result,
            &format!(
                "{} goes on in {} through a glob re-export; look the item up there with get_item, \
                 which takes refresh",
                destination.entry, destination.reexported
            ),
        );
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
    let fallback = Box::pin(look_up_release(state, origin, warnings)).await;
    // Without the re-export to show, the error of the crate the path goes on in tells more.
    if fallback.is_error == Some(true) {
        result
    } else {
        fallback
    }
}

/// The lookup of the path in the crate that `destination` goes on in, from `origin`.
async fn follow_into(
    state: &Arc<ToolState>,
    destination: &Destination,
    origin: &Request,
    warnings: &[Box<str>],
) -> CallToolResult {
    let mut followed = warnings.to_vec();
    let std = destination
        .lib_name
        .parse::<CrateName>()
        .ok()
        .and_then(|name| Some((std_crate(&name)?, name)));
    match std {
        Some((krate, name)) if destination.is_std => {
            followed.push(shown_from(destination, origin, krate.as_str()));
            let arguments = Arguments {
                name,
                version: None,
                project: None,
                path: destination.path.clone(),
                target: Some(origin.target.clone()),
                refresh: false,
                shape: origin.shape.clone(),
            };
            Box::pin(execute_std(
                arguments,
                krate,
                Arc::clone(state),
                followed,
                origin.until,
            ))
            .await
        },
        Some(_) | None => {
            match destination_request(state, destination, origin, &mut followed).await {
                Ok(request) => {
                    followed.push(shown_from(
                        destination,
                        origin,
                        &format!("{} {}", request.name, request.version),
                    ));
                    Box::pin(look_up_release(state, request, followed)).await
                },
                Err(result) => *result,
            }
        },
    }
}

/// The code of the tool error `result`.
fn error_code(result: &CallToolResult) -> Option<String> {
    let text = &result.content.first()?.as_text()?.text;
    let body: Value = serde_json::from_str(text).ok()?;
    body.get("code")?.as_str().map(str::to_owned)
}

/// The tool error `result` with `suggestion` added to its suggestions.
fn with_suggestion(result: CallToolResult, suggestion: &str) -> CallToolResult {
    let Some(mut body) = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str::<Value>(&text.text).ok())
    else {
        return result;
    };
    if let Some(Value::Array(suggestions)) = body.get_mut("suggestions") {
        suggestions.push(json!(suggestion));
    }
    error_result(&body)
}

/// `result`, shown at `entry` by [`show_at_entry`].
fn result_at_entry(mut result: CallToolResult, entry: &str) -> CallToolResult {
    result
        .structured_content
        .take()
        .map_or(result, |mut response| {
            show_at_entry(&mut response, entry);
            CallToolResult::structured(response)
        })
}

/// Shows `response`, of an item found through a re-export of another crate's item, at the path
/// asked for, `entry` (`std::vec::Vec`), rather than at its path in that crate (`alloc::vec::Vec`),
/// which goes in `defined_at` (requirement R-PR-2). The paths of its members and of the items in
/// it go under `entry` too. A response already shown at an entry, after a path was followed
/// through more than one crate, keeps its `defined_at`.
fn show_at_entry(response: &mut Value, entry: &str) {
    let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(defined) = data.get("path").and_then(Value::as_str).map(str::to_owned) else {
        return;
    };
    if defined == entry {
        return;
    }
    for value in data.values_mut() {
        rebase(value, &defined, entry);
    }
    if let Some(Value::String(path)) = data.get_mut("path") {
        entry.clone_into(path);
    }
    // The type of a member: that of `alloc::vec::Vec::push` is `std::vec::Vec`.
    if let Some(((defined_parent, member), (entry_parent, asked_member))) =
        defined.rsplit_once("::").zip(entry.rsplit_once("::"))
        && member == asked_member
        && let Some(Value::String(parent)) = data.get_mut("parent")
        && parent == defined_parent
    {
        entry_parent.clone_into(parent);
    }
    if data.contains_key("defined_at") {
        return;
    }
    // Right after `path`.
    for (key, value) in std::mem::take(data) {
        let after = key == "path";
        data.insert(key, value);
        if after {
            data.insert("defined_at".to_owned(), Value::String(defined.clone()));
        }
    }
}

/// Moves the paths in `value` that are `from` or under it (`alloc::vec::Vec::push`) to `to`
/// (`std::vec::Vec::push`).
fn rebase(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                match value {
                    Value::String(path) if key == "path" || key == "parent" => {
                        if let Some(rest) = path.strip_prefix(from)
                            && (rest.is_empty() || rest.starts_with("::"))
                        {
                            *path = format!("{to}{rest}");
                        }
                    },
                    Value::Null
                    | Value::Bool(_)
                    | Value::Number(_)
                    | Value::String(_)
                    | Value::Array(_)
                    | Value::Object(_) => rebase(value, from, to),
                }
            }
        },
        Value::Array(values) => {
            for value in values {
                rebase(value, from, to);
            }
        },
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {},
    }
}

/// The warning that the path asked for in `origin` is shown from `krate`.
fn shown_from(destination: &Destination, origin: &Request, krate: &str) -> Box<str> {
    let (asked, there) = (&origin.path, &destination.path);
    if destination.via_glob {
        format!(
            "{asked} is not in {} itself, which re-exports all of {} ({krate}), so it is shown as \
             {there} from there",
            destination.reexport, destination.reexported
        )
        .into()
    } else if asked == &destination.reexport {
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
    if let Some(found) = in_project_destination(state, destination, origin).await {
        return found;
    }
    let (name, version, resolved_from, index_fetched_at) =
        if let Some((name, version)) = &destination.release {
            (name.clone(), version.clone(), ResolvedFrom::Reexport, None)
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
                .map_err(|error| Box::new(index_error(&state.cache, &name, &error)))?;
            warnings.extend(entry.stale.clone());
            let resolved = VersionSpec::Latest
                .resolve(&entry.versions)
                .map_err(|no_version| {
                    Box::new(version_not_found(
                        state,
                        &entry,
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
                entry
                    .stale
                    .as_ref()
                    .map(|_| data::rfc3339(entry.fetched_at).into()),
            )
        };
    Ok(Request {
        name,
        version,
        project_from: None,
        path: destination.path.clone(),
        resolved_from,
        target: origin.target.clone(),
        shape: origin.shape.clone(),
        checksum: None,
        build: None,
        in_build: InBuild::default(),
        hops: origin.hops.saturating_add(1),
        index_fetched_at,
        until: origin.until,
        #[cfg(feature = "local")]
        local: None,
    })
}

/// What the project's build tells of the crate that `destination` goes on in from `origin`
/// (decided in M2's D2 and D6): the request for a release of crates.io or for another workspace
/// member, or why it is not followed; `None` when the project does not tell, from a crate that is
/// not a member.
#[cfg(feature = "local")]
async fn in_project_destination(
    state: &ToolState,
    destination: &Destination,
    origin: &Request,
) -> Option<Result<Request, Box<CallToolResult>>> {
    let in_build = in_project_build(state, destination, origin).await;
    // A glob of a member goes on into another member only (decided in M2's D6).
    if destination.via_glob && !matches!(in_build, InProjectBuild::Member(..)) {
        return Some(Err(Box::new(super::tool_error(
            "item_not_found",
            &format!("{} is not a workspace member", destination.reexported),
            false,
            &[],
        ))));
    }
    // From a member, a crate that its project does not tell is not taken at crates.io's latest:
    // the re-export is shown (decided in M2's D6).
    if origin.local.is_some() && matches!(in_build, InProjectBuild::Elsewhere) {
        return Some(Err(Box::new(super::tool_error(
            "docs_unavailable",
            &format!(
                "the version of {} that {} uses is not known from the project",
                destination.lib_name, origin.name
            ),
            true,
            &[],
        ))));
    }
    match in_build {
        InProjectBuild::Release(request) => Some(Ok(*request)),
        // Another workspace member, built here (decided in M2's D6).
        InProjectBuild::Member(package, workspace) => {
            Some(member_destination(state, &package, &workspace, destination, origin).await)
        },
        // Not the crate of that name on crates.io, and not a member (until M2's D7).
        InProjectBuild::NotFromCratesIo(package) => Some(Err(Box::new(super::tool_error(
            "docs_unavailable",
            &format!(
                "{package} in the project is not from crates.io, and a path is not followed \
                     into it yet"
            ),
            false,
            &[&format!("look the item up in {package} itself")],
        )))),
        InProjectBuild::Elsewhere => None,
    }
}

/// The crate that the path goes on in, in the project's build.
#[cfg(feature = "local")]
enum InProjectBuild {
    /// The request for it at its version there, a release of crates.io.
    Release(Box<Request>),
    /// It is a workspace member, of the workspace.
    Member(Box<ProjectPackage>, Box<Workspace>),
    /// It is the project's package, as `name@version`, which is not from crates.io.
    NotFromCratesIo(String),
    /// The project does not tell.
    Elsewhere,
}

/// The crate that the path goes on in, at its version in the project's build, when the project
/// decided the version of `origin`. Of several versions, the one that `origin` was built with on
/// docs.rs.
#[cfg(feature = "local")]
async fn in_project_build(
    state: &ToolState,
    destination: &Destination,
    origin: &Request,
) -> InProjectBuild {
    in_project_build_of(state, destination, origin)
        .await
        .unwrap_or(InProjectBuild::Elsewhere)
}

/// [`in_project_build`], `None` when the project does not tell.
#[cfg(feature = "local")]
async fn in_project_build_of(
    state: &ToolState,
    destination: &Destination,
    origin: &Request,
) -> Option<InProjectBuild> {
    let workspace = origin.in_build.workspace.as_ref()?;
    // Until the call's deadline: past it, the project does not tell (requirement R-ARCH-6).
    let Joined::Done(Ok(resolution)) = state.resolver.resolve_until(workspace, origin.until).await
    else {
        return None;
    };
    let graph = PackageGraph::new(&resolution);
    // From a workspace member, the package that it depends on by that name: its JSON does not
    // tell the version (decided in M2's D6).
    let from_member = origin
        .local
        .as_ref()
        .and_then(|_| graph.member_named(origin.name.as_str(), &origin.version))
        .and_then(|member| graph.dependency_of(member, &destination.lib_name));
    let built_with = destination.release.as_ref().map(|(_, version)| version);
    let name = destination
        .release
        .as_ref()
        .map_or(&*destination.lib_name, |(name, _)| name.as_str());
    let package = match from_member.map_or_else(
        || graph.look_up(name),
        |package| PackageLookup::Found {
            package,
            other_versions: Vec::new(),
        },
    ) {
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
    // From a member only: a docs.rs crate's re-export of a crate of that name is not the member.
    if origin.local.is_some() && graph.is_member(package) {
        return Some(InProjectBuild::Member(
            Box::new(package.clone()),
            Box::new(workspace.clone()),
        ));
    }
    if package.source != Source::CratesIo {
        return Some(InProjectBuild::NotFromCratesIo(format!(
            "{}@{}",
            package.name, package.version
        )));
    }
    Some(InProjectBuild::Release(Box::new(Request {
        name: package.name.parse().ok()?,
        version: package.version.clone(),
        project_from: origin.project_from,
        path: destination.path.clone(),
        resolved_from: origin.resolved_from,
        target: origin.target.clone(),
        shape: origin.shape.clone(),
        checksum: package.checksum.clone(),
        build: None,
        in_build: InBuild {
            workspace: Some(workspace.clone()),
            sets: None,
        },
        hops: origin.hops.saturating_add(1),
        // The version is the project's, not the index's.
        index_fetched_at: None,
        until: origin.until,
        local: None,
    })))
}

/// The request for the item that the path goes on to in the workspace member `package`, built here
/// by the work that the lookups of it share, from the member `origin` (decided in M2's D6); or the
/// response while cargo resolves or plans it, or the error.
#[cfg(feature = "local")]
async fn member_destination(
    state: &ToolState,
    package: &ProjectPackage,
    workspace: &Workspace,
    destination: &Destination,
    origin: &Request,
) -> Result<Request, Box<CallToolResult>> {
    // Its lib, by the name that the path goes on with, as `[lib] name` may make it other than the
    // package's, which a bin of the package has.
    let name: CrateName = destination.lib_name.parse().map_err(|error| {
        Box::new(super::tool_error(
            "item_not_found",
            &format!("the member that the path goes on in has a name that is not valid: {error}"),
            false,
            &[],
        ))
    })?;
    let asked = local::Asked::Package {
        name: &package.name,
        version: &package.version,
    };
    let project_from = origin.project_from.unwrap_or("argument");
    let path = destination.path.clone();
    match local::member(
        state,
        workspace,
        (&name, asked),
        &origin.target,
        origin.until,
    )
    .await?
    {
        Member::Is { wanted, package } => {
            let mut request = member_request(
                &package,
                wanted,
                (&name, &path),
                (&origin.target, &origin.shape),
                (origin.resolved_from, project_from, workspace),
                origin.until,
            );
            request.hops = origin.hops.saturating_add(1);
            Ok(request)
        },
        Member::Unknown(error) => Err(error),
        Member::Resolving(doing) => Err(Box::new(project::resolving_crate(
            &name,
            &origin.target,
            (doing, project_from),
            &[],
        ))),
        Member::Planning(doing) => Err(Box::new(project::planning_crate(
            &name,
            &origin.target,
            (doing, project_from),
            &[],
        ))),
        Member::OtherVersion(_) | Member::Not => Err(Box::new(super::tool_error(
            "item_not_found",
            &format!(
                "{name} {} is not a member that is built here",
                package.version
            ),
            false,
            &[],
        ))),
    }
}

/// What the response warns about the features of the project's build, when the work that reads
/// them ended without them, as when it panicked.
#[cfg(feature = "local")]
pub(super) const NOT_READ: &str = "the features of the project's build could not be read";

/// What the response warns about the features of the project's build, which cargo is still
/// reading past the deadline of the call, as `doing` says: the next call has them.
#[cfg(feature = "local")]
pub(super) fn still_reading(doing: Doing) -> String {
    format!(
        "the features of the project's build are not known yet: cargo is {} them, which goes on \
         without this call; call again for them",
        match doing {
            Doing::WaitingForLock => "waiting for a lock to read",
            Doing::Fetching | Doing::Indexing | Doing::Building | Doing::WaitingForSlot =>
                "reading",
        }
    )
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
    // Until the call's deadline: they only annotate the items (requirement R-ARCH-6).
    match state
        .resolver
        .features_until(workspace, &request.target, request.until)
        .await
    {
        Joined::Done(Ok(sets)) => Some(sets),
        Joined::Done(Err(error)) => {
            warnings.push(
                format!(
                    "whether items are in the project's build is not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
        Joined::Ended => {
            warnings.push(NOT_READ.into());
            None
        },
        Joined::Going(doing) => {
            warnings.push(still_reading(doing).into());
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
    // Unpacked until its manifest, which may be far into it: off the async threads.
    let read = match contents {
        Ok(Some(contents)) => {
            let (name, version) = (name.clone(), version.clone());
            crate::task::spawn_blocking(move || {
                crate_file::docs_rs_build(&contents, &name, &version).map(Some)
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
    until: Instant,
) -> CallToolResult {
    let host = state.host().await;
    let target = if let Some(target) = &arguments.target {
        target.clone()
    } else {
        let (target, chosen) = project::default_target(&state, arguments.project.as_deref()).await;
        warnings.extend(chosen);
        target
    };
    if arguments.version.is_some() {
        warnings.push(
            "std has no versions on crates.io, so the version is ignored; std is that of a nightly \
             toolchain"
                .into(),
        );
    }
    let at_least = project_toolchain(&state).await;
    let chosen = match shared_std(&state, &target, &host.triple, at_least.as_ref(), until).await {
        Ok(chosen) => chosen,
        Err(StdUnready::Failed(error)) => return dist_error(krate, &error),
        Err(StdUnready::Stopped) => return stopped(),
        Err(StdUnready::Going(doing)) => {
            return std_building(arguments.name.as_str(), &target, doing, &warnings);
        },
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
        shape: arguments.shape,
        checksum: None,
        build: None,
        in_build: InBuild::default(),
        hops: 0,
        index_fetched_at: None,
        until,
        #[cfg(feature = "local")]
        local: None,
    };
    // The archives that the lookup opens, made here as a task: the lookup, on a blocking thread,
    // then only opens them, without waiting there for a lock or a place among the crates being
    // parsed. One that cannot be made is told by the lookup.
    let mut opened = vec![krate];
    for other in [StdCrate::Std, StdCrate::Alloc, StdCrate::Core] {
        if !opened.contains(&other) {
            opened.push(other);
        }
    }
    // Until the call's deadline: another process may hold an archive's lock, and making one
    // takes long. What is being made goes on, and the next call opens it.
    let preparing = async {
        for krate in opened {
            if load::archive_std_crate(&state.cache, &chosen, krate)
                .await
                .is_none()
            {
                return false;
            }
        }
        true
    };
    match tokio::time::timeout_at(until, preparing).await {
        Ok(true) => {},
        Ok(false) => return stopped(),
        Err(_) => {
            return std_building(
                request.name.as_str(),
                &request.target,
                Doing::Indexing,
                &warnings,
            );
        },
    }
    let cache = state.cache.clone();
    let task = crate::task::spawn_blocking(move || {
        let looked_up = look_up_std(&cache, &chosen, &request, at_least.as_ref());
        (looked_up, request)
    });
    match task.await {
        Ok(((mut looked_up, through), request)) => {
            if let [asked, .., found] = through.as_slice() {
                warnings.push(
                    format!("{asked} is a re-export of {found}, shown from its crate").into(),
                );
                if let Ok(response) = &mut looked_up {
                    show_at_entry(response, asked);
                }
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
pub(super) async fn project_toolchain(state: &ToolState) -> Option<Version> {
    let project = state.project.as_ref()?;
    let [workspace] = &*project.workspaces else {
        return None;
    };
    let rustc = data::toolchain::Programs::this()
        .used_in(
            data::toolchain::Kind::Rustc,
            &workspace.root,
            state.resolver.cargo_home(),
        )
        .await;
    data::local_std::rustc_release(&workspace.root, &rustc).await
}

/// Without local features, no toolchain is run.
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs rustc"
)]
pub(super) async fn project_toolchain(_state: &ToolState) -> Option<Version> {
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
    let item_request = ItemRequest {
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
        detail: request.shape.detail,
        project_features: None,
        follow: false,
    };
    let looked_up = respond_in(&view, item_request, &request.shape.what);
    (looked_up, followed.through)
}

/// The tool error for std that the distribution server could not give (requirement R-STD-6).
pub(super) fn dist_error(krate: StdCrate, error: &DistError) -> CallToolResult {
    let message = format!(
        "the rustdoc JSON of {} could not be had: {}",
        krate.as_str(),
        ErrorChain(error)
    );
    let (code, reason, retryable): (&str, Option<&str>, bool) = match error {
        DistError::Network(_) | DistError::Status { .. } | DistError::RecentFailure { .. } => {
            ("network_error", None, true)
        },
        DistError::Certificate(_) | DistError::Manifest(_) => ("network_error", None, false),
        DistError::NotAvailable { .. } => ("docs_unavailable", Some("no_json"), false),
        DistError::TooLarge { .. } => ("docs_unavailable", Some("too_large"), false),
        DistError::ChecksumMismatch | DistError::Archive(_) => {
            ("docs_unavailable", Some("corrupt"), true)
        },
        DistError::Cache(_) => ("internal_error", None, false),
        // With the dist turned off, the nightly here is the only source (requirement R-STD-4).
        DistError::Disabled { .. } => ("toolchain_missing", None, false),
        // The install command below goes with it too (requirement R-STD-4).
        DistError::Offline => ("offline_miss", None, false),
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
    if let (DistError::RecentFailure { retry_after }, Some(body)) = (error, body.as_object_mut()) {
        body.insert(
            "retry_after_secs".to_owned(),
            json!(retry_after.as_secs().max(1)),
        );
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
    let task = crate::task::spawn_blocking(move || match load::open_archived(&cache, &dir) {
        Ok(Some(mapped)) => Ok((
            look_up_in(&mapped, &arguments, docs_rs_origin(&arguments)),
            arguments,
        )),
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

/// The tool error when the lookup was stopped because the server is shutting down.
pub(super) fn shutting_down() -> CallToolResult {
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
        Err(Failure::Archive(error)) => kept_in_memory_error(&error),
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
/// and the fields of its state, as `retry_after_secs` (§7.3).
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
    // The fields are in order: rebuild them with `warnings` after `provenance` and the state.
    let old: Vec<(String, Value)> = std::mem::take(fields).into_iter().collect();
    let state = [
        "provenance",
        "status",
        "building_reason",
        "retry_after_secs",
        "partial",
    ];
    let last = old
        .iter()
        .rposition(|(key, _)| state.contains(&key.as_str()));
    let mut all = Some(Value::Array(all));
    for (at, (key, value)) in old.into_iter().enumerate() {
        fields.insert(key, value);
        if Some(at) == last
            && let Some(all) = all.take()
        {
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

/// Where the release that `arguments` names on docs.rs comes from.
fn docs_rs_origin(arguments: &Request) -> Origin<'_> {
    Origin::DocsRs {
        resolved_from: arguments.resolved_from,
        project_from: arguments.project_from,
        build: arguments.build.as_deref(),
    }
}

/// Looks the item up in the archive `mapped`, of the crate from `origin`, and serializes the
/// response while the items it borrows from are still there.
fn look_up_in(
    mapped: &Mapped<Indexed>,
    arguments: &Request,
    origin: Origin<'_>,
) -> Result<Value, Failure> {
    let view = CrateView::new(mapped.get());
    let item_request = ItemRequest {
        name: &arguments.name,
        version: &arguments.version,
        path: &arguments.path,
        origin,
        target: Some(arguments.target.as_str()),
        detail: arguments.shape.detail,
        project_features: arguments
            .in_build
            .features(arguments.name.as_str(), &arguments.version),
        follow: arguments.hops < MAX_FOLLOWED,
    };
    let mut response = respond_in(&view, item_request, &arguments.shape.what)?;
    if let (Some(fetched_at), Some(provenance)) = (
        &arguments.index_fetched_at,
        response
            .get_mut("provenance")
            .and_then(Value::as_object_mut),
    ) {
        provenance.insert("index_fetched_at".to_owned(), json!(fetched_at));
    }
    Ok(response)
}

/// Looks the item up in `view`, lists the module, or gives the overview of the crate, as `what`
/// says, and serializes the response
/// while the items it borrows from are still there.
fn respond_in(
    view: &CrateView<'_>,
    request: ItemRequest<'_>,
    what: &What,
) -> Result<Value, Failure> {
    let listing = match what {
        What::Item => {
            let response = get_item(view, request).map_err(Failure::Item)?;
            return serde_json::to_value(&response).map_err(Failure::Serialize);
        },
        What::Overview => {
            let response = crate_overview(view, request).map_err(Failure::Item)?;
            return serde_json::to_value(&response).map_err(Failure::Serialize);
        },
        What::Search(searching) => {
            let kinds: Vec<&str> = searching.kinds.to_vec();
            let response = search_items(view, request, SearchOptions {
                query: &searching.query,
                fields: searching.fields,
                kinds: &kinds,
                limit: searching.limit,
                offset: searching.offset,
                cursor_request: &searching.request_digest,
                cursor_data: searching.cursor_data.as_deref(),
                entry: None,
            })
            .map_err(Failure::Item)?;
            return serde_json::to_value(&response).map_err(Failure::Serialize);
        },
        What::Impls(paging) => {
            let response = list_impls(view, request, ImplOptions {
                limit: paging.limit,
                offset: paging.offset,
                cursor_request: &paging.request_digest,
                cursor_data: paging.cursor_data.as_deref(),
            })
            .map_err(Failure::Item)?;
            return serde_json::to_value(&response).map_err(Failure::Serialize);
        },
        What::Methods(filter) => {
            let from: Vec<&str> = filter.from.to_vec();
            let response = list_methods(view, request, MethodOptions {
                name: filter.name.as_deref(),
                from: &from,
                expand_blanket: filter.expand_blanket,
                limit: filter.paging.limit,
                offset: filter.paging.offset,
                cursor_request: &filter.paging.request_digest,
                cursor_data: filter.paging.cursor_data.as_deref(),
            })
            .map_err(Failure::Item)?;
            return serde_json::to_value(&response).map_err(Failure::Serialize);
        },
        What::Implementors(paging) => {
            let response = list_implementors(view, request, ImplementorOptions {
                limit: paging.limit,
                offset: paging.offset,
                cursor_request: &paging.request_digest,
                cursor_data: paging.cursor_data.as_deref(),
            })
            .map_err(Failure::Item)?;
            return serde_json::to_value(&response).map_err(Failure::Serialize);
        },
        What::List(listing) => listing,
    };
    let kinds: Vec<&str> = listing.kinds.to_vec();
    let response = list_module(view, request, ListOptions {
        recursive: listing.recursive,
        depth: listing.depth,
        kinds: &kinds,
        limit: listing.limit,
        offset: listing.offset,
        cursor_request: &listing.request_digest,
        cursor_data: listing.cursor_data.as_deref(),
    })
    .map_err(Failure::Item)?;
    serde_json::to_value(&response).map_err(Failure::Serialize)
}

/// `name@version` of the request.
fn release(arguments: &Request) -> String {
    format!("{}@{}", arguments.name, arguments.version)
}

/// The tool error for JSON that could not be loaded (§8).
fn load_error(cache: &Cache, arguments: &Request, error: &LoadError) -> CallToolResult {
    load_error_of(cache, &arguments.name, &release(arguments), error)
}

/// The tool error for the JSON of `release` (`name@version`) that could not be loaded (§8).
pub(super) fn load_error_of(
    cache: &Cache,
    name: &CrateName,
    release: &str,
    error: &LoadError,
) -> CallToolResult {
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
            // It may become readable: docs.rs builds a release again, or a newer server reads it.
            return remembered_error(release, error, "unparsable", true, *retry_after, &[
                "docs.rs served JSON that this server cannot parse; it is fetched again after \
                 retry_after_secs, or read by a newer version of the server",
            ]);
        },
        LoadError::Corrupt { retry_after } => {
            return remembered_error(release, error, "corrupt", true, *retry_after, &[
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
        FetchError::Offline => {
            return error_result(&json!({
                "code": "offline_miss",
                "message": format!("rustdoc JSON of {release} could not be loaded: {}", ErrorChain(fetch)),
                "retryable": false,
                "suggestions": offline_suggestions(cache, name),
            }));
        },
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
        GetItemError::NotAModule { kind } => error_result(&json!({
            "code": "invalid_argument",
            "message": format!("{release} has a {kind}, not a module, at {}", arguments.path),
            "retryable": false,
            "suggestions": [format!("call get_item with the path {}", arguments.path)],
        })),
        GetItemError::NotATrait { kind } => error_result(&json!({
            "code": "invalid_argument",
            "message": format!("{release} has a {kind}, not a trait, at {}", arguments.path),
            "retryable": false,
            "suggestions": [format!("call get_item with the path {}", arguments.path)],
        })),
        GetItemError::NotAType { kind } => error_result(&json!({
            "code": "invalid_argument",
            "message": format!("{release} has a {kind}, not a type, at {}", arguments.path),
            "retryable": false,
            "suggestions": [format!("call get_item with the path {}", arguments.path)],
        })),
        GetItemError::BadCursor(why) => error_result(&json!({
            "code": "invalid_argument",
            "message": format!("the cursor does not fit: {why}"),
            "retryable": false,
            "suggestions": ["call the tool again without cursor"],
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
    fn refresh_is_true_or_false() {
        let parsed = |refresh: Value| {
            arguments(json!({ "crate": "serde", "path": "Serialize", "refresh": refresh }))
                .map(|parsed| parsed.refresh)
        };
        assert_eq!(parsed(json!(true)).ok(), Some(true));
        assert_eq!(parsed(json!(false)).ok(), Some(false));
        assert_eq!(parsed(Value::Null).ok(), Some(false));
        parsed(json!("yes")).expect_err("not true or false");
        let without = arguments(json!({ "crate": "serde", "path": "Serialize" })).expect("valid");
        assert!(!without.refresh);
        // A page after one asked with `refresh` may be asked without it.
        let digest = |value: Value| {
            let Value::Object(arguments) = value else {
                panic!("an object");
            };
            super::super::list_module::request_digest("list_module", Some(&arguments))
        };
        assert_eq!(
            digest(json!({ "crate": "serde", "refresh": true })),
            digest(json!({ "crate": "serde" }))
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
        let detail = |value: Value| arguments(value).map(|arguments| arguments.shape.detail);
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
            shape: Shape {
                detail: Detail::Normal,
                what: What::Item,
            },
            checksum: None,
            build: None,
            in_build: InBuild::default(),
            hops: 0,
            index_fetched_at: None,
            until: Instant::now(),
            #[cfg(feature = "local")]
            local: None,
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
    fn an_item_found_through_a_reexport_is_shown_at_the_path_asked_for() {
        // A member, and the type of a list.
        let mut member = json!({
            "provenance": { "crate": "alloc" },
            "data": {
                "path": "alloc::vec::Vec::push",
                "kind": "method",
                "parent": "alloc::vec::Vec",
            },
        });
        show_at_entry(&mut member, "std::vec::Vec::push");
        assert_eq!(
            member["data"].to_string(),
            json!({
                "path": "std::vec::Vec::push",
                "defined_at": "alloc::vec::Vec::push",
                "kind": "method",
                "parent": "std::vec::Vec",
            })
            .to_string()
        );
        let mut list = json!({ "data": {
            "path": "alloc::vec::Vec",
            "methods": [
                { "path": "alloc::vec::Vec::push", "parent": "alloc::vec::Vec" },
                // Not under the type: a path of another item, or text.
                { "path": "alloc::vec::Vector", "signature": "fn f(alloc::vec::Vec)" },
            ],
        }});
        show_at_entry(&mut list, "std::vec::Vec");
        assert_eq!(
            list["data"],
            json!({
                "path": "std::vec::Vec",
                "defined_at": "alloc::vec::Vec",
                "methods": [
                    { "path": "std::vec::Vec::push", "parent": "std::vec::Vec" },
                    { "path": "alloc::vec::Vector", "signature": "fn f(alloc::vec::Vec)" },
                ],
            })
        );
        // Followed through another crate: the path where it is defined stays.
        show_at_entry(&mut list, "my_crate::Vec");
        assert_eq!(list["data"]["path"], "my_crate::Vec");
        assert_eq!(list["data"]["defined_at"], "alloc::vec::Vec");
        assert_eq!(list["data"]["methods"][0]["path"], "my_crate::Vec::push");
        // A member's type is the asked one only when the member is.
        let mut renamed = json!({ "data": { "path": "other::Thing", "parent": "other" } });
        show_at_entry(&mut renamed, "my_crate::Alias");
        assert_eq!(renamed["data"]["parent"], "other");
    }

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
                built_at: some_str(),
                features_requested: Some(&features),
                features_effective: Some(&features),
            },
            warnings: Box::from(["x".into()]),
            data: ItemData {
                path: "x".into(),
                canonical: some(),
                kind: "struct",
                visibility: some(),
                hidden: true,
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
                    default_unstable: some_str(),
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
