//! The temporary `echo` tool, used to check the MCP plumbing in M0a (`doc/m0-plan.md`, A2).
//! `get_item` replaces it in A8.

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

/// Name of the tool in `tools/list` and `tools/call`.
pub(super) const NAME: &str = "echo";

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema: JsonObject = [
        ("type".to_owned(), json!("object")),
        (
            "properties".to_owned(),
            json!({ "text": { "type": "string", "description": "Text to echo back." } }),
        ),
        ("required".to_owned(), json!(["text"])),
    ]
    .into_iter()
    .collect();
    let output_schema: JsonObject = [
        ("type".to_owned(), json!("object")),
        (
            "properties".to_owned(),
            json!({ "text": { "type": "string", "description": "The text that was given." } }),
        ),
        ("required".to_owned(), json!(["text"])),
    ]
    .into_iter()
    .collect();
    Tool::new(
        NAME,
        "Echo the given text back. A temporary tool for testing the server.",
        input_schema,
    )
    .with_title("Echo text")
    .with_raw_output_schema(Arc::new(output_schema))
}

/// Runs the tool. The arguments are checked here, not by the SDK (requirement R-ARG-1).
pub(super) fn execute(arguments: Option<&JsonObject>) -> CallToolResult {
    match arguments.and_then(|arguments| arguments.get("text")) {
        Some(Value::String(text)) => CallToolResult::structured(json!({ "text": text })),
        Some(_) => super::invalid_argument("`text` must be a string", &[
            r#"pass `text` as a JSON string, for example {"text": "hello"}"#,
        ]),
        None => super::invalid_argument("`text` is required", &[
            r#"pass the text to echo, for example {"text": "hello"}"#,
        ]),
    }
}
