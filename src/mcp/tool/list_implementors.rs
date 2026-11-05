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

/// The values of `scope`: the dependency graph only where the project is read (decided in M4b).
#[cfg(feature = "local")]
const SCOPES: &[&str] = &["crate", "dependency_graph"];
/// The values of `scope`: the dependency graph only where the project is read (decided in M4b).
#[cfg(not(feature = "local"))]
const SCOPES: &[&str] = &["crate"];

/// What `scope` is.
#[cfg(feature = "local")]
const SCOPE_DESCRIPTION: &str = "Where to look for implementors: crate (the trait's crate; the \
                                 default) or dependency_graph (every crate of the project's build \
                                 that depends on it).";
/// What `scope` is.
#[cfg(not(feature = "local"))]
const SCOPE_DESCRIPTION: &str = "Where to look for implementors: crate, the trait's crate.";

/// What the tool is asked: the trait and its first page in its crate, and the dependency graph to
/// look across, if asked.
#[derive(Debug)]
pub(super) struct Request {
    /// The trait, and the page of its implementors in its crate.
    arguments: Arguments,
    /// The dependency graph to look across (decided in M4b).
    #[cfg(feature = "local")]
    graph: Option<super::implementors_across::Graph>,
}

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
            "scope": {
                "type": "string",
                "enum": SCOPES,
                "description": SCOPE_DESCRIPTION,
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
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("List a Rust trait's implementors")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
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
        // Across the dependency graph, the package that has it (decided in M4b).
        properties.insert("crate".to_owned(), json!({ "type": "string" }));
        properties.insert("version".to_owned(), json!({ "type": "string" }));
    }
    // Across the dependency graph: how many crates were read, and those that were not, or were
    // read as built before (decided in M4b, decision 13).
    if let Some(Value::Object(properties)) = schema.pointer_mut("/properties/provenance/properties")
    {
        properties.insert("crates_read".to_owned(), json!({ "type": "integer" }));
        properties.insert("sources".to_owned(), super::search_items::sources_schema());
    }
    if let Some(Value::Object(properties)) = schema.pointer_mut("/properties") {
        properties.insert("partial".to_owned(), json!({ "type": "boolean" }));
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
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<Request, InvalidArgument> {
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
    let graph = match argument("scope") {
        None => false,
        Some(Value::String(scope)) if scope.trim() == "crate" => false,
        Some(Value::String(scope))
            if scope.trim() == "dependency_graph" && cfg!(feature = "local") =>
        {
            true
        },
        Some(other) => {
            return Err(InvalidArgument::new(
                format!("`scope` is {other}, not one of {}", SCOPES.join(", ")),
                &[r#"leave scope out, or pass {"scope": "crate"}"#],
            ));
        },
    };
    let paging = Paging {
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    };
    // The trait's crate for what the trait is, its first implementor only; the page is of the
    // graph's (decided in M4b).
    #[cfg(feature = "local")]
    if graph {
        let first = Paging {
            limit: 1,
            offset: 0,
            request_digest: paging.request_digest.clone(),
            cursor_data: None,
        };
        let again = get_item::parse_release(arguments)?;
        return Ok(Request {
            arguments: Arguments::for_implementors(release, path, first),
            graph: Some(super::implementors_across::Graph {
                project: again.project,
                target: again.target,
                paging,
            }),
        });
    }
    // Without local features, `scope: dependency_graph` is refused above.
    #[cfg(not(feature = "local"))]
    debug_assert!(!graph, "no dependency graph without local features");
    Ok(Request {
        arguments: Arguments::for_implementors(release, path, paging),
        #[cfg(feature = "local")]
        graph: None,
    })
}

/// Lists the implementors as `get_item` looks an item up: the same version, target and cache,
/// and following a trait re-exported from another crate.
pub(super) async fn execute(request: Request, state: Arc<ToolState>) -> CallToolResult {
    #[cfg(feature = "local")]
    if let Some(graph) = request.graph {
        return super::implementors_across::execute(request.arguments, graph, state).await;
    }
    without_definition(get_item::execute(request.arguments, state).await)
}

/// `result` without the trait's definition path, which only the lookup across the dependency
/// graph uses (decided in M4b).
fn without_definition(result: CallToolResult) -> CallToolResult {
    match result.structured_content {
        Some(Value::Object(mut response)) if result.is_error != Some(true) => {
            if let Some(Value::Object(data)) = response.get_mut("data") {
                data.shift_remove("_definition");
            }
            CallToolResult::structured(Value::Object(response))
        },
        _ => result,
    }
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
        assert_eq!(&*parsed.arguments.path_for_test(), "tower::Service");
        let Value::Object(arguments) =
            json!({ "crate": "tower", "path": "Service", "scope": "dependency_graph" })
        else {
            panic!("an object");
        };
        // Where the project is read only (decided in M4b).
        let parsed = parse_arguments(Some(&arguments));
        if cfg!(feature = "local") {
            assert!(parsed.is_ok(), "{parsed:?}");
        } else {
            let error = parsed.expect_err("not in this build");
            assert!(error.message.contains("dependency_graph"), "{error:?}");
        }
        let Value::Object(arguments) =
            json!({ "crate": "tower", "path": "Service", "scope": "world" })
        else {
            panic!("an object");
        };
        parse_arguments(Some(&arguments)).expect_err("an unknown scope");
    }
}
