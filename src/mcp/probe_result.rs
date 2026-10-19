//! The temporary `probe_result` tool, used in A3 (`doc/m0-plan.md`) to see which part of a tool
//! result Claude Code passes to the model (requirements R-OUT-1 and R-ERR-2). It goes away in A8
//! together with `echo` and `sleep`.
//!
//! Unlike real tools, it puts different JSON in `structuredContent` and in the text content, and
//! it also sends `structuredContent` on an error, so that the two can be told apart.

use std::sync::Arc;

use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool};
use serde_json::{Value, json};

/// Name of the tool in `tools/list` and `tools/call`.
pub(super) const NAME: &str = "probe_result";

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema: JsonObject = [
        ("type".to_owned(), json!("object")),
        (
            "properties".to_owned(),
            json!({
                "is_error": {
                    "type": "boolean",
                    "description": "Whether to return the result as an error. Defaults to false.",
                },
            }),
        ),
    ]
    .into_iter()
    .collect();
    let output_schema: JsonObject = [
        ("type".to_owned(), json!("object")),
        (
            "properties".to_owned(),
            json!({ "from": { "type": "string", "description": "Which part of the result this is." } }),
        ),
        ("required".to_owned(), json!(["from"])),
    ]
    .into_iter()
    .collect();
    Tool::new(
        NAME,
        "Return a result whose structured content and text content differ. A temporary tool for \
         testing which of them the client passes to the model.",
        input_schema,
    )
    .with_title("Probe the tool result")
    .with_raw_output_schema(Arc::new(output_schema))
}

/// Checks the arguments and returns whether to answer with an error (requirement R-ARG-1).
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<bool, CallToolResult> {
    match arguments.and_then(|arguments| arguments.get("is_error")) {
        None => Ok(false),
        Some(Value::Bool(is_error)) => Ok(*is_error),
        Some(_) => Err(super::invalid_argument("`is_error` must be a boolean", &[
            r#"pass true or false, for example {"is_error": true}, or omit it"#,
        ])),
    }
}

/// Returns `{"from":"structuredContent"}` as structured content and `{"from":"text"}` as text.
pub(super) fn execute(is_error: bool) -> CallToolResult {
    let mut result = CallToolResult::structured(json!({ "from": "structuredContent" }));
    result.content = vec![ContentBlock::text(json!({ "from": "text" }).to_string())];
    result.is_error = Some(is_error);
    result
}
