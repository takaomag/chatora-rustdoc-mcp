//! The log record of each tool call, and its `request_id` (requirements R-OBS-1 and R-ERR-2).

use std::{
    hash::{BuildHasher as _, Hasher as _, RandomState},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use rmcp::model::{CallToolResult, ContentBlock, ErrorData, JsonObject};
use serde_json::{Map, Value, json};

use crate::{
    data::fetches::Fetches,
    logging::{self, REQUEST_ID_FIELD},
};

/// Characters per token, to estimate the tokens of a response from its length (requirement
/// R-OBS-1). The initial value, which R-OUT-3's 32,000 characters for 8,000 tokens also assumes;
/// the eval calibrates it (M5).
const CHARS_PER_TOKEN: usize = 4;

/// Arguments whose strings are logged whole: the crate, the paths and the cursor (requirement
/// R-OBS-2). Other strings are cut to [`MAX_SUMMARY_CHARS`].
const KEPT_WHOLE: &[&str] = &["crate", "path", "project", "cursor"];

/// The most characters of a string argument that is not kept whole.
const MAX_SUMMARY_CHARS: usize = 200;

/// The most characters of any string argument, so that a huge argument does not fill the log.
const MAX_WHOLE_CHARS: usize = 2000;

/// The most arguments, and the most items of an array argument, that are logged.
const MAX_ITEMS: usize = 32;

/// A tool call being handled.
pub(super) struct Call {
    request_id: String,
    tool: String,
    /// The arguments as they are logged.
    arguments: Value,
    started: Instant,
    fetches: Fetches,
}

impl Call {
    /// Starts the record of a call to `tool` with `arguments`, with a new `request_id`.
    pub(super) fn start(tool: &str, arguments: Option<&JsonObject>) -> Self {
        Self {
            request_id: next_request_id(),
            tool: tool.into(),
            arguments: arguments.map_or(Value::Null, summarize),
            started: Instant::now(),
            fetches: Fetches::default(),
        }
    }

    /// The span that the call runs in: what is logged in it carries its `request_id`.
    pub(super) fn span(&self) -> tracing::Span {
        tracing::info_span!(
            logging::CALL_SPAN,
            request_id = %self.request_id,
            tool = %self.tool,
        )
    }

    /// Where the requests that the call sends are noted.
    pub(super) fn fetches(&self) -> Fetches {
        self.fetches.clone()
    }

    /// Puts the `request_id` in the error of `result`, if it is one, and logs the call.
    pub(super) fn finish(self, result: &mut CallToolResult) {
        let error = if result.is_error == Some(true) {
            Some(add_request_id(result, &self.request_id))
        } else {
            None
        };
        let response = result.structured_content.as_ref().map_or_else(
            || {
                result
                    .content
                    .iter()
                    .filter_map(|block| Some(block.as_text()?.text.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
            Value::to_string,
        );
        let sources = result
            .structured_content
            .as_ref()
            .map(|structured| sources(&structured["provenance"]))
            .unwrap_or_default();
        // `building`, when the answer is still to come (requirement R-OUT-9).
        let status = result
            .structured_content
            .as_ref()
            .and_then(|structured| structured.get("status")?.as_str());
        self.log(error.flatten(), status, &response, &sources);
    }

    /// Logs a call that was rejected with a JSON-RPC error, such as one to an unknown tool.
    pub(super) fn finish_rejected(self, error: &ErrorData) {
        let code = format!("jsonrpc {}", error.code.0);
        self.log(Some(code), None, "", &[]);
    }

    /// Writes the record of the call to the log file.
    fn log(self, error: Option<String>, status: Option<&str>, response: &str, sources: &[String]) {
        let chars = response.chars().count();
        let fetches = self.fetches.to_vec();
        let mut record = json!({
            "ts": logging::now(),
            "kind": "call",
            REQUEST_ID_FIELD: self.request_id,
            "tool": self.tool,
            "arguments": self.arguments,
            "ms": u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "bytes": response.len(),
            "tokens": chars.div_ceil(CHARS_PER_TOKEN),
        });
        if let Some(record) = record.as_object_mut() {
            if let Some(error) = error {
                record.insert("error".to_owned(), error.into());
            }
            if let Some(status) = status {
                record.insert("status".to_owned(), status.into());
            }
            // Whether the cache had what the call answered with: not told without an answer.
            if !sources.is_empty() && status.is_none() {
                record.insert("sources".to_owned(), json!(sources));
                // Whether the cache had everything the call needed: nothing was downloaded. A
                // request that only confirmed what the cache has (304) does not count.
                let downloaded = fetches.iter().any(|fetch| {
                    fetch
                        .status
                        .is_some_and(|status| (200..300).contains(&status))
                });
                let cache = if downloaded { "miss" } else { "hit" };
                record.insert("cache".to_owned(), cache.into());
            }
            if !fetches.is_empty() {
                record.insert("fetches".to_owned(), json!(fetches));
            }
        }
        logging::write_record(&record);
    }
}

/// A new `request_id`: a random number for this process and the number of the call in it, as in
/// `5f2c9a1e-17`. It is unique among the servers of several sessions, so that a reported error
/// leads to its log lines (requirement R-ERR-2).
fn next_request_id() -> String {
    static PROCESS: OnceLock<String> = OnceLock::new();
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let process = PROCESS.get_or_init(|| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        hasher.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
        );
        format!("{:08x}", hasher.finish() >> 32)
    });
    let call = CALLS.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    format!("{process}-{call}")
}

/// Puts `request_id` in the JSON of the error's text (requirement R-ERR-2), and returns its
/// `code`.
fn add_request_id(result: &mut CallToolResult, request_id: &str) -> Option<String> {
    let block = result.content.first_mut()?;
    let mut body: Map<String, Value> = serde_json::from_str(&block.as_text()?.text).ok()?;
    let code = body.get("code").and_then(Value::as_str).map(str::to_owned);
    body.insert(REQUEST_ID_FIELD.to_owned(), request_id.into());
    *block = ContentBlock::text(Value::Object(body).to_string());
    code
}

/// The sources in a response's `provenance`: its `source`, or those of its `sources`.
fn sources(provenance: &Value) -> Vec<String> {
    if let Some(source) = provenance["source"].as_str() {
        return vec![source.to_owned()];
    }
    let mut sources: Vec<String> = provenance["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| Some(source["source"].as_str()?.to_owned()))
        .collect();
    sources.sort();
    sources.dedup();
    sources
}

/// The arguments as they are logged: the crate, paths and cursor whole, and other strings cut
/// short (requirement R-OBS-2).
fn summarize(arguments: &JsonObject) -> Value {
    let summarized: Map<String, Value> = arguments
        .iter()
        .take(MAX_ITEMS)
        .map(|(key, value)| {
            let max = if KEPT_WHOLE.contains(&key.as_str()) {
                MAX_WHOLE_CHARS
            } else {
                MAX_SUMMARY_CHARS
            };
            (key.clone(), summarize_value(value, max))
        })
        .collect();
    Value::Object(summarized)
}

fn summarize_value(value: &Value, max: usize) -> Value {
    match value {
        Value::String(text) => cut(text, max).into(),
        Value::Array(items) => {
            let mut summarized: Vec<Value> = items
                .iter()
                .take(MAX_ITEMS)
                .map(|item| match item {
                    Value::Array(_) => "[…]".into(),
                    Value::Object(_) => "{…}".into(),
                    other
                    @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => {
                        summarize_value(other, max)
                    },
                })
                .collect();
            if items.len() > MAX_ITEMS {
                summarized.push(format!("… {} in all", items.len()).into());
            }
            summarized.into()
        },
        Value::Object(_) => "{…}".into(),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

/// `text`, or its first `max` characters and how long it is.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => {
            let kept = text.get(..end).unwrap_or_default();
            format!("{kept}… ({} characters)", text.chars().count())
        },
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_differ_and_share_the_process_part() {
        let first = next_request_id();
        let second = next_request_id();
        assert_ne!(first, second);
        let process = |id: &str| id.split_once('-').map(|(process, _)| process.to_owned());
        assert_eq!(process(&first), process(&second));
        assert_eq!(process(&first).map(|process| process.len()), Some(8));
    }

    #[test]
    fn the_request_id_goes_in_the_error_text() {
        let mut result = super::super::tool_error("not_found", "nothing", false, &[]);
        let code = add_request_id(&mut result, "abc-1");
        assert_eq!(code.as_deref(), Some("not_found"));
        let text = &result.content[0].as_text().expect("text").text;
        let body: Value = serde_json::from_str(text).expect("JSON");
        assert_eq!(body["request_id"], "abc-1");
        assert_eq!(body["code"], "not_found");
    }

    #[test]
    fn paths_are_kept_and_other_strings_cut() {
        let Value::Object(arguments) = json!({
            "path": "a".repeat(300),
            "query": "q".repeat(300),
            "kinds": ["fn", "struct"],
            "limit": 5,
        }) else {
            panic!("an object");
        };
        let logged = summarize(&arguments);
        assert_eq!(logged["path"], "a".repeat(300));
        assert_eq!(
            logged["query"],
            format!("{}… (300 characters)", "q".repeat(MAX_SUMMARY_CHARS))
        );
        assert_eq!(logged["kinds"], json!(["fn", "struct"]));
        assert_eq!(logged["limit"], 5);
    }

    #[test]
    fn sources_come_from_the_provenance() {
        assert_eq!(sources(&json!({ "source": "docs.rs" })), ["docs.rs"]);
        assert_eq!(
            sources(
                &json!({ "sources": [{ "source": "std" }, { "source": "docs.rs" }, { "source": "std" }] })
            ),
            ["docs.rs", "std"]
        );
        assert_eq!(sources(&Value::Null), Vec::<String>::new());
    }
}
