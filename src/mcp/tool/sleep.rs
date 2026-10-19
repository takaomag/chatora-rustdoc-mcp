//! The `sleep` tool, used to check progress notifications and cancellation (`doc/m0-plan.md`, A2
//! and A3, and `tests/stdio.rs`). It is built only in debug builds.

use std::{sync::Arc, time::Duration};

use rmcp::{
    RoleServer,
    model::{CallToolResult, JsonObject, ProgressNotificationParam, Tool},
    service::RequestContext,
};
use serde_json::{Value, json};

/// Name of the tool in `tools/list` and `tools/call`.
pub(super) const NAME: &str = "sleep";

/// Longest sleep the tool accepts.
const MAX_SECONDS: u32 = 60;

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema: JsonObject = [
        ("type".to_owned(), json!("object")),
        (
            "properties".to_owned(),
            json!({
                "seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SECONDS,
                    "description": "How many seconds to sleep.",
                },
            }),
        ),
        ("required".to_owned(), json!(["seconds"])),
    ]
    .into_iter()
    .collect();
    let output_schema: JsonObject = [
        ("type".to_owned(), json!("object")),
        (
            "properties".to_owned(),
            json!({ "slept_secs": { "type": "integer", "description": "How many seconds it slept." } }),
        ),
        ("required".to_owned(), json!(["slept_secs"])),
    ]
    .into_iter()
    .collect();
    Tool::new(
        NAME,
        "Sleep for the given number of seconds, reporting progress every second. A tool for \
         testing progress notifications and cancellation, only in debug builds.",
        input_schema,
    )
    .with_title("Sleep")
    .with_raw_output_schema(Arc::new(output_schema))
}

/// Checks the arguments and returns the number of seconds to sleep (requirement R-ARG-1).
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<u32, super::InvalidArgument> {
    let seconds = match arguments.and_then(|arguments| arguments.get("seconds")) {
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|seconds| u32::try_from(seconds).ok())
            .filter(|seconds| (1..=MAX_SECONDS).contains(seconds)),
        Some(_) => None,
        None => {
            return Err(super::InvalidArgument::new("`seconds` is required", &[
                r#"pass the seconds to sleep, for example {"seconds": 5}"#,
            ]));
        },
    };
    seconds.ok_or_else(|| {
        super::InvalidArgument::new("`seconds` must be an integer from 1 to 60", &[
            r#"pass a whole number of seconds, for example {"seconds": 5}"#,
        ])
    })
}

/// Sleeps one second at a time, sending a progress notification after each second when the
/// request has a progress token, and stops as soon as the request is cancelled (requirement R-WS-6).
pub(super) async fn execute(seconds: u32, context: RequestContext<RoleServer>) -> CallToolResult {
    let progress_token = context.meta.get_progress_token();
    for elapsed in 1..=seconds {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(1)) => {},
            () = context.ct.cancelled() => {
                // rmcp drops the response of a cancelled request, so this result is never sent.
                tracing::info!(request_id = ?context.id, during_second = elapsed, "sleep was cancelled");
                return super::tool_error("cancelled", "the request was cancelled", false, &[]);
            },
        }
        // Progress is sent only when the client asked for it with a token.
        if let Some(token) = &progress_token {
            let progress = ProgressNotificationParam::new(token.clone(), f64::from(elapsed))
                .with_total(f64::from(seconds))
                .with_message(format!("slept {elapsed} of {seconds} seconds"));
            if let Err(error) = context.peer.notify_progress(progress).await {
                tracing::warn!(%error, "failed to send a progress notification");
            }
        }
    }
    CallToolResult::structured(json!({ "slept_secs": seconds }))
}
