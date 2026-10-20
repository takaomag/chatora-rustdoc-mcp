//! The `list_module` tool: the items of a module, and with `recursive` those of its submodules,
//! a page at a time (requirements R-LS-1, R-LS-2 and R-OUT-3).

use std::{collections::BTreeMap, sync::Arc};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments, Listing},
};
use crate::{
    data::cache::sha256_hex,
    query::{get_item::Detail, list_module::KINDS},
};

/// The name of the tool.
pub(super) const NAME: &str = "list_module";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("list_module.md");

/// Items in a page unless `limit` says otherwise.
const DEFAULT_LIMIT: usize = 50;

/// The most items in a page.
const MAX_LIMIT: usize = 200;

/// Levels of submodules that `recursive` lists unless `depth` says otherwise.
const DEFAULT_DEPTH: usize = 3;

/// The most levels of submodules that `recursive` lists.
const MAX_DEPTH: usize = 8;

/// What every cursor starts with: the form of this server's cursors.
pub(super) const CURSOR_FORM: &str = "c1";

/// How to pass `cursor`, for `invalid_argument`.
pub(super) const CURSOR_EXAMPLE: &[&str] =
    &["pass the next_cursor of the previous page as it is, with the same other arguments"];

/// How to pass `kind`, for `invalid_argument`.
const KIND_EXAMPLE: &[&str] =
    &[r#"pass kinds as get_item names them, for example {"kind": ["struct", "enum"]}"#];

/// How to pass a number, for `invalid_argument`.
const NUMBER_EXAMPLE: &[&str] = &[r#"pass a whole number, for example {"limit": 50}"#];

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (axum), or the crate and its version as name@version.",
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
            "path": {
                "type": "string",
                "description": "The module (axum::routing). Defaults to the crate root.",
            },
            "recursive": {
                "type": "boolean",
                "description": "Also list the submodules' items.",
            },
            "depth": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_DEPTH,
                "description": "With recursive, how many levels of submodules (default 3).",
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
                "enum": ["brief", "normal", "full"],
                "description": "brief (the default): the signature line and the features. \
                                normal: also the where clause and the first paragraph of the docs.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most items in a page (default 50).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("List a Rust module")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `ModuleResponse` in `query::list_module`
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
                    "defined_at": get_item::defined_at_schema(),
                    "docs": { "type": "string" },
                    "docs_truncated": { "type": "boolean" },
                    "items": { "type": "array", "items": get_item::data_schema() },
                },
                "required": ["path", "items"],
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
        Some(Value::String(path)) if !path.trim().is_empty() => Some(path.trim()),
        Some(Value::String(_)) | None => None,
        Some(_) => {
            return Err(InvalidArgument::new("`path` must be a string", &[
                r#"pass the path of a module, for example {"path": "axum::routing"}"#,
            ]));
        },
    };
    let recursive = match argument("recursive") {
        Some(Value::Bool(recursive)) => *recursive,
        None => false,
        Some(_) => {
            return Err(InvalidArgument::new(
                "`recursive` must be true or false",
                &[r#"pass a boolean, for example {"recursive": true}"#],
            ));
        },
    };
    let number = |name: &'static str, default: usize, max: usize| {
        argument(name).map_or(Ok(default), |value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| (1..=max).contains(value))
                .ok_or_else(|| {
                    InvalidArgument::new(
                        format!("`{name}` must be a whole number from 1 to {max}"),
                        NUMBER_EXAMPLE,
                    )
                })
        })
    };
    let depth = number("depth", DEFAULT_DEPTH, MAX_DEPTH)?;
    let limit = number("limit", DEFAULT_LIMIT, MAX_LIMIT)?;
    let kinds = parse_kinds(argument("kind"))?;
    let detail = match argument("detail") {
        None => Detail::Brief,
        Some(Value::String(detail)) => match detail.trim() {
            "brief" => Detail::Brief,
            "normal" => Detail::Normal,
            "full" => Detail::Full,
            other => {
                return Err(InvalidArgument::new(
                    format!("`detail` is {other:?}, not brief, normal or full"),
                    &[r#"pass brief, normal or full, for example {"detail": "normal"}"#],
                ));
            },
        },
        Some(_) => {
            return Err(InvalidArgument::new("`detail` must be a string", &[
                r#"pass brief, normal or full, for example {"detail": "normal"}"#,
            ]));
        },
    };
    let request_digest = request_digest(NAME, arguments);
    let (offset, cursor_data) = match argument("cursor") {
        None => (0, None),
        Some(Value::String(cursor)) => parse_cursor(cursor, &request_digest)?,
        Some(_) => {
            return Err(InvalidArgument::new(
                "`cursor` must be a string",
                CURSOR_EXAMPLE,
            ));
        },
    };
    Ok(Arguments::for_listing(release, path, detail, Listing {
        recursive,
        depth,
        kinds,
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    }))
}

/// The kinds that `kind` asks for: none for all.
pub(super) fn parse_kinds(kind: Option<&Value>) -> Result<Box<[&'static str]>, InvalidArgument> {
    let names: Vec<&Value> = match kind {
        None => return Ok(Box::default()),
        Some(Value::Array(kinds)) => kinds.iter().collect(),
        Some(kind) => vec![kind],
    };
    names
        .into_iter()
        .map(|name| {
            name.as_str()
                .and_then(|name| KINDS.iter().find(|&&kind| kind == name.trim()).copied())
                .ok_or_else(|| {
                    InvalidArgument::new(
                        format!(
                            "`kind` has {name}, which is not one of {}",
                            KINDS.join(", ")
                        ),
                        KIND_EXAMPLE,
                    )
                })
        })
        .collect()
}

/// The digest of the tool and its arguments but `cursor` and `limit`, which a cursor is bound
/// to, so that it is not used for another list (requirement R-OUT-3): not even for another
/// tool's list with the same arguments. The order of the arguments does not matter.
pub(super) fn request_digest(tool: &str, arguments: Option<&JsonObject>) -> String {
    let kept: BTreeMap<&str, &Value> = arguments
        .into_iter()
        .flatten()
        .filter(|(name, _)| !matches!(name.as_str(), "cursor" | "limit"))
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    let canonical = format!(
        "{tool}\n{}",
        serde_json::to_string(&kept).unwrap_or_default()
    );
    let digest = sha256_hex(canonical.as_bytes());
    digest.get(..16).unwrap_or(&digest).to_owned()
}

/// The offset and the data digest of `cursor`, which must be one this tool made for these
/// arguments: `c1.<request>.<data>.<offset>`.
pub(super) fn parse_cursor(
    cursor: &str,
    request_digest: &str,
) -> Result<(usize, Option<Box<str>>), InvalidArgument> {
    let mut parts = cursor.trim().split('.');
    let (Some(form), Some(request), Some(data), Some(offset), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(InvalidArgument::new(
            "`cursor` is not a cursor of this tool",
            CURSOR_EXAMPLE,
        ));
    };
    let offset = offset
        .parse::<usize>()
        .ok()
        .filter(|_| form == CURSOR_FORM && data.chars().all(|c| c.is_ascii_hexdigit()));
    let Some(offset) = offset else {
        return Err(InvalidArgument::new(
            "`cursor` is not a cursor of this tool",
            CURSOR_EXAMPLE,
        ));
    };
    if request != request_digest {
        return Err(InvalidArgument::new(
            "`cursor` was made for other arguments; list again without it, or pass the same \
             arguments as for the previous page",
            CURSOR_EXAMPLE,
        ));
    }
    Ok((offset, Some(data.into())))
}

/// Lists the module as `get_item` looks an item up: the same version, target, cache and
/// following into other crates.
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
    fn a_cursor_is_for_the_same_arguments_in_any_order() {
        let digest = |value: Value| {
            let Value::Object(object) = value else {
                panic!("an object");
            };
            request_digest(NAME, Some(&object))
        };
        let first = digest(json!({ "crate": "axum", "path": "routing", "limit": 10 }));
        assert_eq!(
            first,
            digest(json!({ "path": "routing", "crate": "axum", "cursor": "x" }))
        );
        assert_ne!(first, digest(json!({ "crate": "axum", "path": "extract" })));
        let cursor = format!("{CURSOR_FORM}.{first}.abcd.50");
        assert_eq!(
            parse_cursor(&cursor, &first).expect("valid"),
            (50, Some("abcd".into()))
        );
        let other = digest(json!({ "crate": "axum" }));
        drop(parse_cursor(&cursor, &other).expect_err("rejected"));
        // Nor is it for another tool with the same arguments.
        let Value::Object(same) = json!({ "crate": "axum", "path": "routing" }) else {
            panic!("an object");
        };
        let another_tool = request_digest("list_impls", Some(&same));
        drop(parse_cursor(&cursor, &another_tool).expect_err("rejected"));
        drop(parse_cursor("c1.x.y", &first).expect_err("rejected"));
        drop(parse_cursor(&format!("c2.{first}.abcd.50"), &first).expect_err("rejected"));
    }

    #[test]
    fn bad_arguments_are_invalid_arguments() {
        for (value, about) in [
            (json!({ "crate": "axum", "kind": "class" }), "`kind`"),
            (json!({ "crate": "axum", "limit": 0 }), "`limit`"),
            (json!({ "crate": "axum", "depth": 9 }), "`depth`"),
            (
                json!({ "crate": "axum", "recursive": "yes" }),
                "`recursive`",
            ),
            (json!({ "crate": "axum", "cursor": "page 2" }), "`cursor`"),
            (json!({ "crate": "axum", "detail": "all" }), "`detail`"),
        ] {
            let error = arguments(value.clone()).expect_err("invalid");
            assert!(error.message.contains(about), "{value}: {error:?}");
        }
    }

    #[test]
    fn the_crate_root_is_listed_by_default() {
        let parsed =
            arguments(json!({ "crate": "tokio-util", "kind": ["struct", "enum"] })).expect("valid");
        assert_eq!(&*parsed.path_for_test(), "tokio_util");
    }
}
