//! The `list_methods` tool: the methods that can be called on a type (requirements R-TI-1,
//! R-LS-3 and R-SH-4, §3 Q7).

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments, MethodFilter, Paging},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
};
use crate::query::{get_item::Detail, methods::ORIGINS};

/// The name of the tool.
pub(super) const NAME: &str = "list_methods";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("list_methods.md");

/// Methods in a page unless `limit` says otherwise.
const DEFAULT_LIMIT: usize = 50;

/// The most methods in a page.
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
            "name": {
                "type": "string",
                "description": "Only methods whose name has this text, in any case.",
            },
            "from": {
                "anyOf": [
                    { "type": "string", "enum": ORIGINS },
                    { "type": "array", "items": { "type": "string", "enum": ORIGINS } },
                ],
                "description": "Only methods from these: inherent, trait, blanket.",
            },
            "expand_blanket": {
                "type": "boolean",
                "description": "List the methods of blanket impls one by one.",
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
                "description": "The most methods in a page (default 50).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate", "path"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("List a Rust type's methods")
        .with_raw_output_schema(Arc::new(output_schema()))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `MethodsResponse` in `query::methods`
/// (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let mut method = get_item::data_schema();
    if let Some(properties) = method.get_mut("properties").and_then(Value::as_object_mut) {
        for (name, schema) in [
            ("from", json!({ "type": "string", "enum": ORIGINS })),
            ("trait", json!({ "type": "string" })),
            ("needs_use", json!({ "type": "boolean" })),
            ("provided", json!({ "type": "boolean" })),
        ] {
            properties.insert(name.to_owned(), schema);
        }
    }
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
                    "methods": { "type": "array", "items": method },
                    "std_blanket": strings,
                    "blanket": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "trait": { "type": "string" },
                                "crate": { "type": "string" },
                                "methods": strings,
                            },
                            "required": ["trait", "methods"],
                        },
                    },
                },
                "required": ["path", "kind", "methods"],
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
    let name = match argument("name") {
        None => None,
        Some(Value::String(name)) if !name.trim().is_empty() => Some(name.trim().into()),
        Some(_) => {
            return Err(InvalidArgument::new(
                "`name` must be a string that is not empty",
                &[r#"pass part of a name, for example {"name": "lock"}"#],
            ));
        },
    };
    let from: Vec<Value> = match argument("from") {
        None => Vec::new(),
        Some(Value::Array(from)) => from.clone(),
        Some(from) => vec![from.clone()],
    };
    let from = from
        .iter()
        .map(|origin| {
            origin
                .as_str()
                .and_then(|origin| ORIGINS.iter().find(|&&known| known == origin.trim()).copied())
                .ok_or_else(|| {
                    InvalidArgument::new(
                        format!("`from` has {origin}, which is not inherent, trait or blanket"),
                        &[r#"pass some of inherent, trait and blanket, for example {"from": "trait"}"#],
                    )
                })
        })
        .collect::<Result<Box<[&str]>, _>>()?;
    let expand_blanket = match argument("expand_blanket") {
        None => false,
        Some(Value::Bool(expand)) => *expand,
        Some(_) => {
            return Err(InvalidArgument::new(
                "`expand_blanket` must be true or false",
                &[r#"pass a boolean, for example {"expand_blanket": true}"#],
            ));
        },
    };
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
    Ok(Arguments::for_methods(
        release,
        path,
        detail,
        MethodFilter {
            name,
            from,
            expand_blanket,
            paging: Paging {
                limit,
                offset,
                request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
                cursor_data,
            },
        },
    ))
}

/// Lists the methods as `get_item` looks an item up: the same version, target and cache, and
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
