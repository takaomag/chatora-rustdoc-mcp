//! The `search_items` tool: the items of a crate that match a query, a page at a time
//! (requirements R-SR-1 to R-SR-5).

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use serde_json::{Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments, Searching},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
};
use crate::query::{get_item::Detail, list_module::KINDS, search::Fields};

/// The name of the tool.
pub(super) const NAME: &str = "search_items";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("search_items.md");

/// The most results in a page.
const MAX_LIMIT: usize = 200;

/// The fields that `fields` takes.
const FIELDS: &[&str] = &["name", "alias", "docs", "path"];

/// How to pass `crate`, while the search of several crates is not there yet (C4b).
const CRATE_EXAMPLE: &[&str] =
    &[r#"pass the crate to search, for example {"crate": "tokio", "query": "mutex"}"#];

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
                "description": "The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.",
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
        "required": ["crate", "query"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Search a Rust crate")
        .with_raw_output_schema(Arc::new(output_schema()))
        // It reads docs.rs and writes the cache, as get_item does (requirement R-MCP-4).
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(true),
        )
}

/// The schema of the structured content: `SearchResponse` in `query::search`
/// (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": get_item::provenance_schema(),
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
) -> Result<Arguments, InvalidArgument> {
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    if argument("crate").is_none() {
        return Err(InvalidArgument::new(
            "`crate` is required: this version searches one crate at a time",
            CRATE_EXAMPLE,
        ));
    }
    let release = get_item::parse_release(arguments)?;
    let query = match argument("query") {
        Some(Value::String(query)) if !query.trim().is_empty() => query.trim(),
        Some(_) | None => {
            return Err(InvalidArgument::new(
                "`query` is required and must be a string that is not empty",
                QUERY_EXAMPLE,
            ));
        },
    };
    let fields = parse_fields(argument("fields"))?;
    let kinds = list_module::parse_kinds(argument("kind"))?;
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
    let request_digest = list_module::request_digest(arguments);
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
    Ok(Arguments::for_search(release, detail, Searching {
        query: query.into(),
        fields,
        kinds,
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    }))
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

/// Searches as `get_item` looks an item up: the same version, target and cache.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    get_item::execute(arguments, state).await
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
    fn the_description_fits_the_limit() {
        let len = DESCRIPTION.chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    #[test]
    fn bad_arguments_are_invalid_arguments() {
        for (value, about) in [
            (json!({ "query": "mutex" }), "`crate`"),
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
