//! The `search_items` tool: the items of a crate that match a query, a page at a time
//! (requirements R-SR-1 to R-SR-5).

use std::{path::PathBuf, sync::Arc};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments, Searching},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
    search_across,
};
use crate::{
    data::target::TargetTriple,
    query::{get_item::Detail, list_module::KINDS, search::Fields},
};

/// The name of the tool.
pub(super) const NAME: &str = "search_items";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("search_items.md");

/// The most results in a page.
const MAX_LIMIT: usize = 200;

/// The fields that `fields` takes.
const FIELDS: &[&str] = &["name", "alias", "docs", "path"];

/// What `search_items` searches.
#[derive(Debug)]
pub(super) enum SearchArguments {
    /// One crate on docs.rs, as `get_item` reads it.
    One(Arguments),
    /// std with alloc and core.
    Std {
        target: Option<TargetTriple>,
        detail: Detail,
        searching: Searching,
        /// Whether what failed is to be tried again (§6.4).
        refresh: bool,
    },
    /// The project's direct dependencies (requirement R-SR-2).
    Dependencies {
        project: Option<PathBuf>,
        target: Option<TargetTriple>,
        detail: Detail,
        searching: Searching,
        /// Whether what failed for the project is to be tried again (§6.4).
        refresh: bool,
    },
}

/// How to pass `query`, for `invalid_argument`.
const QUERY_EXAMPLE: &[&str] = &[r#"pass words or a path, for example {"query": "sync::Mutex"}"#];

/// How to pass `fields`, for `invalid_argument`.
const FIELDS_EXAMPLE: &[&str] =
    &[r#"pass some of name, alias, docs and path, for example {"fields": ["name", "alias"]}"#];

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate to search (tokio), or the crate and its version as \
                                name@version.",
            },
            "query": {
                "type": "string",
                "description": "Words or a path to look for (sync::Mutex, spawn blocking).",
            },
            "version": {
                "type": "string",
                "description": "The version: exact, a semver requirement or latest. Defaults to \
                                the project's, or the latest.",
            },
            "target": {
                "type": "string",
                "description": "The target triple to document for. Defaults to the host's.",
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "fields": {
                "anyOf": [
                    { "type": "string", "enum": FIELDS },
                    { "type": "array", "items": { "type": "string", "enum": FIELDS } },
                ],
                "description": "Where to look: name, alias, docs, path (default: all).",
            },
            "kind": {
                "anyOf": [
                    { "type": "string", "enum": KINDS },
                    { "type": "array", "items": { "type": "string", "enum": KINDS } },
                ],
                "description": "Only items of this kind or these kinds.",
            },
            "detail": {
                "type": "string",
                "enum": ["brief", "normal"],
                "description": "brief (the default) or normal.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "Results in each page. Without it, the first page ends where the \
                                scores drop.",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["query"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Search a Rust crate")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `SearchResponse` in `query::search`
/// (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    // One crate's provenance, or with several crates the target and project they share and a
    // source for each (requirement R-OUT-9).
    let mut source = get_item::provenance_schema();
    if let Some(properties) = source.get_mut("properties").and_then(Value::as_object_mut) {
        for (name, schema) in [
            (
                "status",
                json!({ "type": "string", "enum": ["ok", "building", "failed", "unavailable"] }),
            ),
            ("building_reason", super::building_reason_schema()),
            ("retry_after_secs", json!({ "type": "integer" })),
            ("code", json!({ "type": "string" })),
            ("reason", json!({ "type": "string" })),
            ("retryable", json!({ "type": "boolean" })),
        ] {
            properties.insert(name.to_owned(), schema);
        }
    }
    if let Some(fields) = source.as_object_mut() {
        // Without `target` before docs.rs's JSON of the default target is read (decided in M2's
        // D7).
        fields.insert("required".to_owned(), json!(["crate", "version"]));
    }
    let mut provenance = get_item::provenance_schema();
    if let Some(properties) = provenance
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.insert(
            "sources".to_owned(),
            json!({ "type": "array", "items": source }),
        );
    }
    if let Some(fields) = provenance.as_object_mut() {
        fields.remove("required");
    }
    object(json!({
        "type": "object",
        "properties": {
            "provenance": provenance,
            "partial": { "type": "boolean" },
            "retry_after_secs": { "type": "integer" },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "items": { "type": "array", "items": get_item::data_schema() },
                    "did_you_mean": strings,
                },
                "required": ["query", "items"],
            },
            "page": {
                "type": "object",
                "properties": {
                    "shown": { "type": "integer" },
                    "total": { "type": "integer" },
                    "has_more": { "type": "boolean" },
                    "next_cursor": { "type": "string" },
                },
                "required": ["shown", "total", "has_more"],
            },
            "hints": strings,
        },
        "required": ["provenance", "data", "page"],
    }))
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

/// Checks the arguments (requirement R-ARG-1).
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<SearchArguments, InvalidArgument> {
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let searching = parse_searching(arguments)?;
    let detail = match argument("detail") {
        None => Detail::Brief,
        Some(Value::String(detail)) if detail.trim() == "brief" => Detail::Brief,
        Some(Value::String(detail)) if detail.trim() == "normal" => Detail::Normal,
        Some(_) => {
            return Err(InvalidArgument::new("`detail` must be brief or normal", &[
                r#"pass brief or normal, for example {"detail": "normal"}"#,
            ]));
        },
    };
    if argument("crate").is_none() {
        let target_example: &'static [&'static str] =
            &[r#"pass a target triple, for example {"target": "x86_64-unknown-linux-gnu"}"#];
        let target = match argument("target") {
            None => None,
            Some(Value::String(target)) => {
                Some(target.trim().parse::<TargetTriple>().map_err(|error| {
                    InvalidArgument::new(format!("`target` is not valid: {error}"), target_example)
                })?)
            },
            Some(_) => {
                return Err(InvalidArgument::new(
                    "`target` must be a string",
                    target_example,
                ));
            },
        };
        let project = match argument("project") {
            None => None,
            Some(Value::String(project)) => Some(PathBuf::from(project.trim())),
            Some(_) => {
                return Err(InvalidArgument::new("`project` must be a string", &[
                    r#"pass the directory or the Cargo.toml of a workspace, for example {"project": "/home/me/app"}"#,
                ]));
            },
        };
        return Ok(SearchArguments::Dependencies {
            project,
            target,
            detail,
            searching,
            refresh: get_item::parse_refresh(arguments)?,
        });
    }
    let release = get_item::parse_release(arguments)?;
    if release.name.as_str() == "std" && release.version.is_none() {
        return Ok(SearchArguments::Std {
            target: release.target,
            detail,
            searching,
            refresh: release.refresh,
        });
    }
    Ok(SearchArguments::One(Arguments::for_search(
        release, detail, searching,
    )))
}

/// The arguments that say what and how to search, and the page: `query`, `fields`, `kind`,
/// `limit` and `cursor`.
fn parse_searching(arguments: Option<&JsonObject>) -> Result<Searching, InvalidArgument> {
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let query = match argument("query") {
        Some(Value::String(query)) if !query.trim().is_empty() => query.trim(),
        Some(_) | None => {
            return Err(InvalidArgument::new(
                "`query` is required and must be a string that is not empty",
                QUERY_EXAMPLE,
            ));
        },
    };
    let limit = match argument("limit") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| (1..=MAX_LIMIT).contains(value))
                .ok_or_else(|| {
                    InvalidArgument::new(
                        format!("`limit` must be a whole number from 1 to {MAX_LIMIT}"),
                        &[r#"pass a whole number, for example {"limit": 20}"#],
                    )
                })?,
        ),
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
    Ok(Searching {
        query: query.into(),
        fields: parse_fields(argument("fields"))?,
        kinds: list_module::parse_kinds(argument("kind"))?,
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    })
}

/// The fields that `fields` asks for: all of them if it is absent.
fn parse_fields(fields: Option<&Value>) -> Result<Fields, InvalidArgument> {
    let names: Vec<&Value> = match fields {
        None => return Ok(Fields::ALL),
        Some(Value::Array(names)) => names.iter().collect(),
        Some(name) => vec![name],
    };
    let mut chosen = Fields {
        name: false,
        alias: false,
        docs: false,
        path: false,
    };
    for name in names {
        match name.as_str().map(str::trim) {
            Some("name") => chosen.name = true,
            Some("alias") => chosen.alias = true,
            Some("docs") => chosen.docs = true,
            Some("path") => chosen.path = true,
            _ => {
                return Err(InvalidArgument::new(
                    format!("`fields` has {name}, which is not name, alias, docs or path"),
                    FIELDS_EXAMPLE,
                ));
            },
        }
    }
    if chosen
        == (Fields {
            name: false,
            alias: false,
            docs: false,
            path: false,
        })
    {
        return Err(InvalidArgument::new("`fields` is empty", FIELDS_EXAMPLE));
    }
    Ok(chosen)
}

/// Searches one crate as `get_item` looks an item up, or several together.
pub(super) async fn execute(arguments: SearchArguments, state: Arc<ToolState>) -> CallToolResult {
    match arguments {
        SearchArguments::One(arguments) => get_item::execute(arguments, state).await,
        SearchArguments::Std {
            target,
            detail,
            searching,
            refresh,
        } => {
            // Acted on once, until a call for it ends (decided in M2's D5).
            #[cfg(feature = "local")]
            let scope = "std";
            #[cfg(feature = "local")]
            if refresh
                && super::local::refresh_acts(scope)
                && super::local::refresh_project(&state, None).await
            {
                crate::data::forget_failures(state.cache.dir().join("std"), true).await;
            }
            #[cfg(not(feature = "local"))]
            let _ = refresh;
            // The default project's target (requirement R-TG-1); its warnings go with the
            // search's own.
            let (target, warnings) = match target {
                Some(target) => (target, Vec::new()),
                None => super::project::default_target(&state, None).await,
            };
            let result = search_across::std(state, target, detail, searching, warnings).await;
            #[cfg(feature = "local")]
            super::local::refresh_told(scope, &result);
            result
        },
        SearchArguments::Dependencies {
            project,
            target,
            detail,
            searching,
            refresh,
        } => {
            // Of the workspace only, not of each crate searched, and once until a call for it ends
            // (decided in M2's D5).
            #[cfg(feature = "local")]
            let scope = format!("project {project:?}");
            #[cfg(feature = "local")]
            if refresh && super::local::refresh_acts(&scope) {
                super::local::refresh_project(&state, project.as_deref()).await;
            }
            #[cfg(not(feature = "local"))]
            let _ = refresh;
            let (target, warnings) =
                super::project::lookup_target(&state, target.as_ref(), project.as_deref()).await;
            let result = dependencies(state, project, target, detail, searching, warnings).await;
            #[cfg(feature = "local")]
            super::local::refresh_told(&scope, &result);
            result
        },
    }
}

/// Searches the project's direct dependencies.
#[cfg(feature = "local")]
async fn dependencies(
    state: Arc<ToolState>,
    project: Option<PathBuf>,
    target: super::project::LookupTarget,
    detail: Detail,
    searching: Searching,
    warnings: Vec<Box<str>>,
) -> CallToolResult {
    search_across::dependencies(state, project, target, detail, searching, warnings).await
}

/// Without local features, there is no project to search the dependencies of.
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it reads the project"
)]
async fn dependencies(
    _state: Arc<ToolState>,
    _project: Option<PathBuf>,
    _target: super::project::LookupTarget,
    _detail: Detail,
    _searching: Searching,
    _warnings: Vec<Box<str>>,
) -> CallToolResult {
    super::tool_error(
        "invalid_argument",
        "this server is built without local features, so it has no project to search the \
         dependencies of; `crate` is required",
        false,
        &[r#"pass the crate to search, for example {"crate": "tokio"}"#],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(value: Value) -> Result<SearchArguments, InvalidArgument> {
        let Value::Object(arguments) = value else {
            panic!("the arguments are an object");
        };
        parse_arguments(Some(&arguments))
    }

    #[test]
    fn the_description_fits_the_limit() {
        let len = DESCRIPTION.chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    #[test]
    fn bad_arguments_are_invalid_arguments() {
        for (value, about) in [
            (json!({ "crate": "tokio" }), "`query`"),
            (json!({ "crate": "tokio", "query": " " }), "`query`"),
            (
                json!({ "crate": "tokio", "query": "x", "fields": "body" }),
                "`fields`",
            ),
            (
                json!({ "crate": "tokio", "query": "x", "fields": [] }),
                "`fields`",
            ),
            (
                json!({ "crate": "tokio", "query": "x", "detail": "full" }),
                "`detail`",
            ),
            (
                json!({ "crate": "tokio", "query": "x", "limit": 500 }),
                "`limit`",
            ),
        ] {
            let error = arguments(value.clone()).expect_err("invalid");
            assert!(error.message.contains(about), "{value}: {error:?}");
        }
    }

    #[test]
    fn fields_default_to_all() {
        assert_eq!(parse_fields(None).expect("valid"), Fields::ALL);
        let name_only = parse_fields(Some(&json!(["name"]))).expect("valid");
        assert!(name_only.name && !name_only.docs);
    }
}
