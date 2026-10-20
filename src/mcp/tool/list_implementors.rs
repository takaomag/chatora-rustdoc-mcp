//! The `list_implementors` tool: the types that implement a trait, in its crate, and what the
//! trait asks of them (requirements R-TI-3 and R-TI-5, §3 Q9).

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
pub(super) const NAME: &str = "list_implementors";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("list_implementors.md");

/// Implementors in a page unless `limit` says otherwise.
const DEFAULT_LIMIT: usize = 50;

/// The most implementors in a page.
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
                "description": "The trait (tower::Service). The crate name may be left out.",
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
            "scope": {
                "type": "string",
                "enum": ["crate"],
                "description": "Where to look for implementors: crate (the trait's crate; the \
                                default and, for now, the only one).",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most implementors in a page (default 50).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate", "path"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("List a Rust trait's implementors")
        .with_raw_output_schema(Arc::new(output_schema()))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `ImplementorsResponse` in `query::implementors`
/// (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let mut schema = json!({
        "type": "object",
        "properties": {
            "provenance": get_item::provenance_schema(),
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "defined_at": get_item::defined_at_schema(),
                    "signature": { "type": "string" },
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "kind": {
                                    "type": "string",
                                    "enum": ["method", "assoc_type", "assoc_const"],
                                },
                                "provided": { "type": "boolean" },
                            },
                            "required": ["name", "kind", "provided"],
                        },
                    },
                    "dyn_compatible": { "type": "boolean" },
                    "auto": { "type": "boolean" },
                    "unsafe": { "type": "boolean" },
                    "sealed": {
                        "type": "string",
                        "description": "Why only its crate can implement it, when that was found.",
                    },
                    "has_negative_impls": { "type": "boolean" },
                    "implementors": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "type": { "type": "string" },
                                "kind": {
                                    "type": "string",
                                    "enum": ["trait", "derive", "blanket", "negative", "auto"],
                                },
                                "header": { "type": "string" },
                            },
                            "required": ["type", "kind", "header"],
                        },
                    },
                },
                "required": ["path", "signature", "items", "dyn_compatible", "implementors"],
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
    });
    // Each entry has the conditions that its `impl` is written with (requirement R-FT-2).
    if let Some(Value::Object(properties)) =
        schema.pointer_mut("/properties/data/properties/implementors/items/properties")
    {
        properties.extend(get_item::conditions_properties());
    }
    object(schema)
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
                "`path` is required and must be the path of a trait",
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
    match argument("scope") {
        None => {},
        Some(Value::String(scope)) if scope.trim() == "crate" => {},
        Some(Value::String(scope)) if scope.trim() == "dependency_graph" => {
            return Err(InvalidArgument::new(
                "`scope: dependency_graph` is not there yet: implementors are listed in the \
                 trait's crate only",
                &[
                    r#"leave scope out, or pass {"scope": "crate"}; list_impls on a type tells whether it implements the trait"#,
                ],
            ));
        },
        Some(other) => {
            return Err(InvalidArgument::new(
                format!("`scope` is {other}, not crate"),
                &[r#"leave scope out, or pass {"scope": "crate"}"#],
            ));
        },
    }
    Ok(Arguments::for_implementors(release, path, Paging {
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    }))
}

/// Lists the implementors as `get_item` looks an item up: the same version, target and cache,
/// and following a trait re-exported from another crate.
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
    fn the_path_of_a_trait_is_required() {
        let Value::Object(arguments) = json!({ "crate": "tokio" }) else {
            panic!("an object");
        };
        let error = parse_arguments(Some(&arguments)).expect_err("invalid");
        assert!(error.message.contains("`path`"), "{error:?}");
        let Value::Object(arguments) = json!({ "crate": "tower", "path": "Service" }) else {
            panic!("an object");
        };
        let parsed = parse_arguments(Some(&arguments)).expect("valid");
        assert_eq!(&*parsed.path_for_test(), "tower::Service");
        let Value::Object(arguments) =
            json!({ "crate": "tower", "path": "Service", "scope": "dependency_graph" })
        else {
            panic!("an object");
        };
        let error = parse_arguments(Some(&arguments)).expect_err("not yet");
        assert!(error.message.contains("dependency_graph"), "{error:?}");
    }
}
