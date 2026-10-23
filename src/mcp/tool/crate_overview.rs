//! The `crate_overview` tool: the overview of a crate (§6.3, §3 Q3).

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments},
};
use crate::query::get_item::Detail;

/// The name of the tool.
pub(super) const NAME: &str = "crate_overview";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("crate_overview.md");

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio), or the crate and its version as name@version.",
            },
            "version": {
                "type": "string",
                "description": const_format::concatcp!(
                    "The version: exact, a semver requirement or latest.",
                    super::VERSION_DEFAULT
                ),
            },
            "target": {
                "type": "string",
                "description": const_format::concatcp!(
                    "The target triple to document for.",
                    super::TARGET_DEFAULT
                ),
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "detail": {
                "type": "string",
                "enum": ["brief", "normal", "full"],
                "description": "How much of the root's documentation: brief, normal (the \
                                default) or full.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Overview of a Rust crate")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content: `OverviewResponse` in `query::overview`
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
                    "docs": { "type": "string" },
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
                    "counts": {
                        "type": "object",
                        "additionalProperties": { "type": "integer" },
                        "description": "How many public items of each kind.",
                    },
                    "modules": { "type": "array", "items": get_item::data_schema() },
                    "items": { "type": "array", "items": get_item::data_schema() },
                    "items_total": { "type": "integer" },
                    "features": {
                        "type": "object",
                        "properties": {
                            "all": strings,
                            "default": strings,
                            "not_in_docs": strings,
                            "enabled_in_project": strings,
                        },
                        "required": ["all", "default"],
                    },
                    "truncated": { "type": "boolean" },
                },
                "required": ["path", "counts", "items_total"],
            },
            "hints": strings,
        },
        "required": ["provenance", "data"],
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
    let detail = match arguments.and_then(|arguments| arguments.get("detail")) {
        None => Detail::Normal,
        Some(Value::String(detail)) => match detail.trim() {
            "brief" => Detail::Brief,
            "normal" => Detail::Normal,
            "full" => Detail::Full,
            other => {
                return Err(InvalidArgument::new(
                    format!("`detail` is {other:?}, not brief, normal or full"),
                    &[r#"pass brief, normal or full, for example {"detail": "full"}"#],
                ));
            },
        },
        Some(_) => {
            return Err(InvalidArgument::new("`detail` must be a string", &[
                r#"pass brief, normal or full, for example {"detail": "full"}"#,
            ]));
        },
    };
    Ok(Arguments::for_overview(release, detail))
}

/// Gives the overview as `get_item` looks an item up: the same version, target and cache.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    get_item::execute(arguments, state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_fits_the_limit() {
        let len = super::super::for_this_build(DESCRIPTION).chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    #[test]
    fn the_root_of_the_crate_is_looked_at() {
        let Value::Object(arguments) = json!({ "crate": "tokio-util@0.7" }) else {
            panic!("an object");
        };
        let parsed = parse_arguments(Some(&arguments)).expect("valid");
        assert_eq!(&*parsed.path_for_test(), "tokio_util");
        let Value::Object(bad) = json!({ "crate": "tokio", "detail": 1 }) else {
            panic!("an object");
        };
        let error = parse_arguments(Some(&bad)).expect_err("invalid");
        assert!(error.message.contains("`detail`"), "{error:?}");
    }
}
