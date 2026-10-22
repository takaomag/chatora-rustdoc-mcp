//! The lookups in the records of `build_external_docs` (requirement R-WS-10, decided in M2e's
//! E1b): of releases that the project does not use, which the query tools never build.
//!
//! They read the record and start nothing (requirement R-MCP-3).

use std::sync::Arc;

use rmcp::model::CallToolResult;
use semver::Version;
use serde_json::{Map, Value, json};

use super::{
    ToolState,
    error_result,
    get_item::{being, building_response, shutting_down},
    local,
};
use crate::{
    data::{
        build::Failed,
        cache::Cache,
        crate_name::CrateName,
        external::{self, Key, Recorded},
        flight::Doing,
        target::TargetTriple,
        version::VersionSpec,
    },
    query::{
        load::LoadError,
        local::{LocalBuilt, LocalError, RECORDED_BY},
    },
};

/// The key of the record of the release `name` at `version` for `target`.
fn key(name: &CrateName, version: &Version, target: &TargetTriple) -> Key {
    Key::Release {
        name: name.cache_key().into(),
        version: version.to_string().into(),
        target: target.as_str().into(),
        hidden: false,
    }
}

/// The exact version that `spec` names, if `build_external_docs` has a record of `name` at it for
/// `target`, or is making one: it is then taken without reading the crates.io index, so that it
/// is found offline, as a cached JSON of docs.rs is.
pub(super) fn recorded_exact(
    cache: &Cache,
    name: &CrateName,
    spec: &VersionSpec,
    target: &TargetTriple,
) -> Option<Version> {
    let VersionSpec::Exact(version) = spec else {
        return None;
    };
    external::exists(cache, &key(name, version, target)).then(|| version.clone())
}

/// What the record of a release is for a lookup.
pub(super) enum Found {
    /// The build that it records, to look up in.
    Built(Arc<LocalBuilt>),
    /// The response to return: the record is being made, failed or is broken.
    Response(Box<CallToolResult>),
    /// There is none.
    Missing,
}

/// The record of the release `name` at `version` for `target`, which the project does not use: a
/// build of `build_external_docs`, archived, or else the response to return.
///
/// The response tells that the record is being made, with `provenance` (as `source: local`),
/// failed or is broken (decided in M2e's E1b).
pub(super) async fn recorded(
    state: &ToolState,
    (name, version, target): (&CrateName, &Version, &TargetTriple),
    provenance: impl FnOnce() -> Map<String, Value>,
    warnings: &[Box<str>],
) -> Found {
    let key = key(name, version, target);
    // Most lookups have none, which a look at the directory tells without a thread.
    if !key.dir(state.cache.dir()).is_dir() {
        return Found::Missing;
    }
    let cache = state.cache.clone();
    let read = {
        let (cache, key) = (cache.clone(), key.clone());
        crate::task::spawn_blocking(move || external::read(&cache, &key))
    };
    let Ok(recorded) = read.await else {
        return Found::Response(Box::new(shutting_down()));
    };
    let what = format!("{name}@{version}");
    let response = |result| Found::Response(Box::new(result));
    let building = |provenance: Map<String, Value>| {
        let mut provenance = provenance;
        provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
        response(building_response(
            provenance,
            Doing::Building,
            &being(&what, Doing::Building, "build_external_docs"),
            warnings,
        ))
    };
    match recorded {
        Recorded::Built(built) => match crate::query::local::recorded(&state.cache, *built).await {
            Ok(built) => Found::Built(Arc::new(built)),
            // The JSON is not the one recorded: being made again, as the external tool may have
            // published another meanwhile, or else broken.
            Err(LocalError::Load(LoadError::Read(error))) => {
                let again = crate::task::spawn_blocking(move || external::being_made(&cache, &key));
                if again.await.unwrap_or(false) {
                    building(provenance())
                } else {
                    response(broken(&what, &error.to_string()))
                }
            },
            Err(error) => response(with_suggestion(
                local::local_error(&what, &error),
                CALL_AGAIN,
            )),
        },
        Recorded::Building => building(provenance()),
        Recorded::Failed(failed) => response(failed_again(&failed)),
        Recorded::Broken(why) => response(broken(&what, &why)),
        Recorded::Missing => Found::Missing,
    }
}

/// What a response from the record `built` warns about: the toolchain that built it, which may have
/// been updated since, as the query tools do not tell (requirement R-MCP-3).
pub(super) fn toolchain_warning(built: &LocalBuilt) -> Box<str> {
    let record = &built.record;
    let toolchain = record
        .toolchain
        .as_deref()
        .and_then(|toolchain| toolchain.lines().next())
        .unwrap_or("a toolchain not known");
    format!(
        "built by build_external_docs at {} with {toolchain}; an update of the toolchain since is \
         not looked for: call build_external_docs again to build it with the current one",
        record.built_at
    )
    .into()
}

/// What calls `build_external_docs` again with the same arguments, which makes the record again.
const CALL_AGAIN: &str = "call build_external_docs again with the same crate, version and target \
                          to build it again";

/// `build_failed` as the record of the external tool's build that failed tells, until it is called
/// again (requirement R-WS-10): the query tools do not build it.
fn failed_again(failed: &Failed) -> CallToolResult {
    let mut body = failed.body.clone();
    if let Some(fields) = body.as_object_mut() {
        fields.insert("retryable".to_owned(), json!(false));
        fields.insert("failed_at".to_owned(), json!(failed.time));
        let suggestions = fields
            .entry("suggestions")
            .or_insert_with(|| Value::Array(Vec::new()));
        // Unless the body tells it already.
        if let Value::Array(suggestions) = suggestions
            && !suggestions.iter().any(|suggestion| {
                suggestion
                    .as_str()
                    .is_some_and(|text| text.contains("build_external_docs"))
            })
        {
            suggestions.push(json!(CALL_AGAIN));
        }
    }
    error_result(&body)
}

/// `docs_unavailable` with `record_broken` for the record of `what`, as `why` tells: the query
/// tools neither fetch nor build it again (requirement R-WS-10).
fn broken(what: &str, why: &str) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!("the record of {what} that build_external_docs made is broken: {why}"),
        "retryable": false,
        "suggestions": [CALL_AGAIN],
        "reason": "record_broken",
    }))
}

/// How to build here a release that docs.rs has no JSON of, when the project does not use it
/// (requirement R-WS-10).
pub(super) const BUILD_IT: &str = "build it here with build_external_docs, which runs its build \
                                   scripts and proc macros, and then look it up again";

/// `result`, a tool error, with `suggestion` after its suggestions.
pub(super) fn with_suggestion(result: CallToolResult, suggestion: &str) -> CallToolResult {
    let body = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str::<Map<String, Value>>(&text.text).ok());
    let Some(mut body) = body else {
        return result;
    };
    if let Value::Array(suggestions) = body
        .entry("suggestions")
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        suggestions.push(json!(suggestion));
    }
    error_result(&Value::Object(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_without_json_on_docs_rs_is_told_to_be_built_by_the_external_tool() {
        let result = error_result(&json!({
            "code": "docs_unavailable",
            "message": "docs.rs has no rustdoc JSON of x@1.0.0",
            "retryable": false,
            "suggestions": ["ask for another version"],
            "reason": "no_json",
        }));
        let result = with_suggestion(result, BUILD_IT);
        let text = &result.content[0].as_text().expect("text").text;
        let body: Value = serde_json::from_str(text).expect("JSON");
        assert_eq!(body["reason"], "no_json");
        assert_eq!(body["suggestions"][0], "ask for another version");
        assert!(
            body["suggestions"][1]
                .as_str()
                .is_some_and(|suggestion| suggestion.contains("build_external_docs")),
            "{body}"
        );
    }
}
