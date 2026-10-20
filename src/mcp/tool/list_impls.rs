//! The `list_impls` tool: the traits that a type implements (requirements R-TI-2, R-SH-4 and
//! R-TI-5, §3 Q8).

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments, Paging},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
};

/// The name of the tool.
pub(super) const NAME: &str = "list_impls";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("list_impls.md");

/// Impls in a page unless `limit` says otherwise.
const DEFAULT_LIMIT: usize = 50;

/// The most impls in a page.
const MAX_LIMIT: usize = 200;

/// How to pass `path`, for `invalid_argument`.
const PATH_EXAMPLE: &[&str] =
    &[r#"pass the path of a type, for example {"path": "tokio::sync::Mutex"}"#];

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio), or the crate and its version as name@version.",
            },
            "path": {
                "type": "string",
                "description": "The type (tokio::sync::Mutex). The crate name may be left out.",
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
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most impls in a page (default 50).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate", "path"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("List a Rust type's impls")
        .with_raw_output_schema(Arc::new(output_schema()))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `ImplsResponse` in `query::impls`
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
                    "path": { "type": "string" },
                    "kind": { "type": "string" },
                    "non_exhaustive": { "type": "boolean" },
                    "has_private_fields": { "type": "boolean" },
                    "auto_traits": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Each auto trait: yes, no, or if and its conditions.",
                    },
                    "std_blanket": strings,
                    "impls": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": {
                                    "type": "string",
                                    "enum": ["inherent", "trait", "derive", "blanket", "negative"],
                                },
                                "trait": { "type": "string" },
                                "crate": { "type": "string" },
                                "header": { "type": "string" },
                            },
                            "required": ["kind", "header"],
                        },
                    },
                },
                "required": ["path", "kind", "impls"],
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
    let release = get_item::parse_release(arguments)?;
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let path = match argument("path") {
        Some(Value::String(path)) if !path.trim().is_empty() => path.trim(),
        Some(_) | None => {
            return Err(InvalidArgument::new(
                "`path` is required and must be the path of a type",
                PATH_EXAMPLE,
            ));
        },
    };
    let limit = match argument("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=MAX_LIMIT).contains(value))
            .ok_or_else(|| {
                InvalidArgument::new(
                    format!("`limit` must be a whole number from 1 to {MAX_LIMIT}"),
                    &[r#"pass a whole number, for example {"limit": 50}"#],
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
    Ok(Arguments::for_impls(release, path, Paging {
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    }))
}

/// Lists the impls as `get_item` looks an item up: the same version, target and cache, and
/// following a type re-exported from another crate.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    get_item::execute(arguments, state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_fits_the_limit() {
        let len = DESCRIPTION.chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    #[test]
    fn the_path_of_a_type_is_required() {
        let Value::Object(arguments) = json!({ "crate": "tokio" }) else {
            panic!("an object");
        };
        let error = parse_arguments(Some(&arguments)).expect_err("invalid");
        assert!(error.message.contains("`path`"), "{error:?}");
        let Value::Object(arguments) = json!({ "crate": "tokio", "path": "sync::Mutex" }) else {
            panic!("an object");
        };
        let parsed = parse_arguments(Some(&arguments)).expect("valid");
        assert_eq!(&*parsed.path_for_test(), "tokio::sync::Mutex");
    }
}
