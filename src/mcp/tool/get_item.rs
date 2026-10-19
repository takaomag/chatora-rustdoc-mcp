//! The `get_item` tool (`doc/m0-plan.md`, A8): one item of a crate from its docs.rs rustdoc JSON.
//!
//! M0a takes only an exact version, reads only docs.rs, and parses the JSON on every call.

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use semver::Version;
use serde_json::{Value, json};

use super::{InvalidArgument, ToolState, error_result};
use crate::{
    data::{
        self,
        LoadError,
        crate_name::CrateName,
        docs_rs::FetchError,
        rustdoc_json::{self, ParseError},
    },
    query::{
        get_item::{GetItemError, ItemRequest, get_item},
        path_index::PathIndex,
    },
};

/// Name of the tool in `tools/list` and `tools/call`.
pub(super) const NAME: &str = "get_item";

/// Description for `tools/list` (§6.2), at most 2,048 characters.
const DESCRIPTION: &str = include_str!("get_item.md");

/// How to pass `crate`, for `invalid_argument`.
const CRATE_EXAMPLE: &[&str] =
    &[r#"pass the crate with an exact version, for example {"crate": "tokio@1.47.1"}"#];

/// How to pass `path`, for `invalid_argument`.
const PATH_EXAMPLE: &[&str] =
    &[r#"pass the path of the item, for example {"path": "tokio::sync::Mutex::lock"}"#];

/// Checked arguments.
#[derive(Debug)]
pub(super) struct Arguments {
    name: CrateName,
    version: Version,
    /// The path, starting with the crate name.
    path: Box<str>,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate and its exact version, as name@version (tokio@1.47.1).",
            },
            "path": {
                "type": "string",
                "description": "The path of the item (tokio::sync::Mutex::lock). The crate name \
                                may be left out.",
            },
        },
        "required": ["crate", "path"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Get a Rust item")
        .with_raw_output_schema(Arc::new(output_schema()))
        // It reads docs.rs and writes the cache, and will build docs locally (requirement
        // R-MCP-4).
        .with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(true),
        )
}

/// The schema of the structured content: `ItemResponse` in `query::get_item` (requirement
/// R-OUT-1). Every property but the required ones is left out when it has no value.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "enum": ["docs.rs"] },
                    "crate": { "type": "string" },
                    "version": { "type": "string" },
                    "resolved_from": { "type": "string", "enum": ["explicit"] },
                    "target": { "type": "string" },
                    "format_version": { "type": "integer" },
                },
                "required": ["source", "crate", "version", "target"],
            },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The canonical path." },
                    "kind": { "type": "string" },
                    "parent": { "type": "string" },
                    "impl": { "type": "string" },
                    "signature": { "type": "string" },
                    "where": strings,
                    "fields": strings,
                    "variants": strings,
                    "docs": {
                        "type": "string",
                        "description": "The first paragraph of the documentation.",
                    },
                    "docs_truncated": { "type": "boolean" },
                    "deprecated": {
                        "type": "object",
                        "properties": {
                            "since": { "type": "string" },
                            "note": { "type": "string" },
                        },
                    },
                    "must_use": { "type": "boolean" },
                    "non_exhaustive": { "type": "boolean" },
                    "also_reexported_at": strings,
                    "reexport_of": {
                        "type": "object",
                        "properties": {
                            "crate": { "type": "string" },
                            "path": { "type": "string" },
                        },
                        "required": ["crate", "path"],
                    },
                    "source_location": {
                        "type": "object",
                        "properties": {
                            "file": { "type": "string" },
                            "lines": { "type": "string" },
                            "url": { "type": "string" },
                        },
                        "required": ["file", "lines"],
                    },
                },
                "required": ["path", "kind"],
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
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let crate_argument = match argument("crate") {
        Some(Value::String(value)) => value,
        Some(_) => {
            return Err(InvalidArgument::new(
                "`crate` must be a string",
                CRATE_EXAMPLE,
            ));
        },
        None => return Err(InvalidArgument::new("`crate` is required", CRATE_EXAMPLE)),
    };
    let Some((name, version)) = crate_argument.trim().split_once('@') else {
        return Err(InvalidArgument::new(
            "`crate` must have an exact version, as name@version; finding the version from \
             Cargo.lock is not supported yet",
            CRATE_EXAMPLE,
        ));
    };
    let name: CrateName = name.parse().map_err(|error| {
        InvalidArgument::new(
            format!("`crate` has an invalid name: {error}"),
            CRATE_EXAMPLE,
        )
    })?;
    let version = Version::parse(version).map_err(|error| {
        InvalidArgument::new(
            format!(
                "`crate` must have an exact version such as 1.47.1; version requirements are not \
                 supported yet ({error})"
            ),
            CRATE_EXAMPLE,
        )
    })?;
    let path = match argument("path") {
        Some(Value::String(path)) => path.trim().trim_start_matches("::"),
        Some(_) => {
            return Err(InvalidArgument::new(
                "`path` must be a string",
                PATH_EXAMPLE,
            ));
        },
        None => return Err(InvalidArgument::new("`path` is required", PATH_EXAMPLE)),
    };
    if path.is_empty() {
        return Err(InvalidArgument::new(
            "`path` must not be empty",
            PATH_EXAMPLE,
        ));
    }
    let path = with_crate_name(&name, path);
    Ok(Arguments {
        name,
        version,
        path,
    })
}

/// `path`, starting with the crate name: it is added when the first segment is something else.
/// `-` and `_` in the crate name are the same.
fn with_crate_name(name: &CrateName, path: &str) -> Box<str> {
    let first = path.split("::").next().unwrap_or_default();
    if same_crate_name(first, name.as_str()) {
        path.into()
    } else {
        format!("{}::{path}", name.as_str().replace('-', "_")).into()
    }
}

/// Whether `a` and `b` name the same crate, with `-` and `_` the same.
fn same_crate_name(a: &str, b: &str) -> bool {
    let normalized = |byte: &u8| if *byte == b'-' { b'_' } else { *byte };
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(a, b)| normalized(&a) == normalized(&b))
}

/// Loads the JSON of the crate, then looks the item up on a blocking thread, since parsing a large
/// crate takes a large part of a second.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    let json = match data::docs_rs_json(
        &state.cache,
        &state.docs_rs,
        &arguments.name,
        &arguments.version,
    )
    .await
    {
        Ok(json) => json,
        Err(error) => return load_error(&arguments, &error),
    };
    let Some((looked_up, arguments)) = look_up_on_blocking(json, arguments).await else {
        return shutting_down();
    };
    // A format this server does not read: docs.rs may have the release in another one
    // (requirement R-DS-2).
    if let Err(Failure::Parse(ParseError::UnsupportedFormat { found, .. })) = &looked_up {
        match data::docs_rs_json_in_another_format(
            &state.cache,
            &state.docs_rs,
            &arguments.name,
            &arguments.version,
            *found,
        )
        .await
        {
            Ok(Some(json)) => {
                let Some((again, returned)) = look_up_on_blocking(json, arguments).await else {
                    return shutting_down();
                };
                return finish(again, &returned);
            },
            Ok(None) => {},
            Err(error) => return load_error(&arguments, &error),
        }
    }
    finish(looked_up, &arguments)
}

/// Looks the item up in `json` on a blocking thread, since parsing a large crate takes a large
/// part of a second. The arguments come back with the result. `None` if the runtime is shutting
/// down; a panic goes on to `run_tool`.
async fn look_up_on_blocking(
    json: Box<[u8]>,
    arguments: Arguments,
) -> Option<(Result<Value, Failure>, Arguments)> {
    match tokio::task::spawn_blocking(move || {
        let looked_up = look_up(&json, &arguments);
        (looked_up, arguments)
    })
    .await
    {
        Ok(done) => Some(done),
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the get_item lookup did not finish");
            None
        },
    }
}

/// The tool error when the lookup was stopped because the server is shutting down.
fn shutting_down() -> CallToolResult {
    super::tool_error(
        "internal_error",
        "the server stopped the lookup because it is shutting down",
        false,
        &[],
    )
}

/// The tool result of a lookup.
fn finish(looked_up: Result<Value, Failure>, arguments: &Arguments) -> CallToolResult {
    match looked_up {
        Ok(response) => CallToolResult::structured(response),
        Err(Failure::Parse(error)) => parse_error(arguments, &error),
        Err(Failure::Item(error)) => item_error(arguments, &error),
        Err(Failure::Serialize(error)) => {
            tracing::error!(%error, "the get_item response could not be serialized");
            super::tool_error(
                "internal_error",
                "the server could not build the response (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            )
        },
    }
}

/// Why the item could not be looked up in the JSON.
#[derive(Debug)]
enum Failure {
    Parse(ParseError),
    Item(GetItemError),
    Serialize(serde_json::Error),
}

/// Parses `json`, builds the path index, looks the item up, and serializes the response while
/// the crate that it borrows from is still there.
fn look_up(json: &[u8], arguments: &Arguments) -> Result<Value, Failure> {
    let parsed = rustdoc_json::parse(json).map_err(Failure::Parse)?;
    let index = PathIndex::build(&parsed.krate);
    let response = get_item(&parsed, &index, ItemRequest {
        name: &arguments.name,
        version: &arguments.version,
        path: &arguments.path,
    })
    .map_err(Failure::Item)
    .and_then(|response| serde_json::to_value(&response).map_err(Failure::Serialize));
    // Freeing a parsed crate takes milliseconds (about 9 ms for rmcp's 14 MB of JSON), so the
    // response does not wait for it.
    drop_in_background((parsed, index));
    response
}

/// Drops `value` on a blocking thread of its own, without waiting for it.
fn drop_in_background<T: Send + 'static>(value: T) {
    drop(tokio::task::spawn_blocking(move || drop(value)));
}

/// `name@version` of the request.
fn release(arguments: &Arguments) -> String {
    format!("{}@{}", arguments.name, arguments.version)
}

/// The tool error for JSON that could not be loaded (§8).
fn load_error(arguments: &Arguments, error: &LoadError) -> CallToolResult {
    let release = release(arguments);
    tracing::warn!(%release, error = %ErrorChain(error), "rustdoc JSON could not be loaded");
    let fetch = match error {
        LoadError::Fetch(fetch) => fetch,
        LoadError::WrongVersion { found } => {
            return error_result(&json!({
                "code": "docs_unavailable",
                "message": format!(
                    "docs.rs returned the rustdoc JSON of version {found} for {release}, so it is not \
                     used (no other version is shown in place of the one asked for)"
                ),
                "retryable": false,
                "suggestions": ["ask for the version with its build metadata, such as 2.1.0+zstd.1.5.6"],
                "reason": "version_mismatch",
            }));
        },
        LoadError::RecentFailure { retry_after } => {
            return error_result(&json!({
                "code": "network_error",
                "message": format!("rustdoc JSON of {release} could not be loaded: {}", ErrorChain(error)),
                "retryable": true,
                "suggestions": ["try again after retry_after_secs"],
                "retry_after_secs": retry_after.as_secs().max(1),
            }));
        },
        LoadError::Cache(_) => {
            return super::tool_error(
                "internal_error",
                "the cache of rustdoc JSON could not be read",
                false,
                &["check the server's stderr log for the cache directory and the error"],
            );
        },
    };
    let (code, reason, retryable, suggestions): (&str, Option<&str>, bool, &[&str]) = match fetch {
        FetchError::NotFound | FetchError::BadRequest => {
            ("docs_unavailable", Some("no_json"), false, &[
                "check that the crate and the version exist on crates.io",
                "docs.rs has rustdoc JSON only for releases it built on or after 2025-05-23, and none \
             for a release whose docs.rs build failed",
            ])
        },
        FetchError::TooLarge { .. } => ("docs_unavailable", Some("too_large"), false, &[]),
        // More zstd layers than the limit count as broken (requirement R-DS-4).
        FetchError::TooManyLayers | FetchError::Corrupt(_) => {
            ("docs_unavailable", Some("unparsable"), false, &[])
        },
        FetchError::Transient { .. } | FetchError::Network(_) => {
            ("network_error", None, true, &["try again in a while"])
        },
        FetchError::UnexpectedStatus { .. } => ("network_error", None, false, &[]),
        FetchError::Certificate(_) => ("network_error", None, false, &[
            "the TLS certificate of docs.rs was rejected: if a proxy inspects TLS, add its CA \
             certificate to the system's trusted certificates",
        ]),
    };
    let mut body = json!({
        "code": code,
        "message": format!("rustdoc JSON of {release} could not be loaded: {}", ErrorChain(fetch)),
        "retryable": retryable,
        "suggestions": suggestions,
    });
    if let (Some(reason), Some(body)) = (reason, body.as_object_mut()) {
        body.insert("reason".to_owned(), json!(reason));
    }
    error_result(&body)
}

/// The tool error for JSON that could not be parsed (§8).
fn parse_error(arguments: &Arguments, error: &ParseError) -> CallToolResult {
    let release = release(arguments);
    tracing::warn!(%release, error = %ErrorChain(error), "rustdoc JSON could not be parsed");
    let message = format!(
        "rustdoc JSON of {release} could not be read: {}",
        ErrorChain(error)
    );
    match error {
        ParseError::UnsupportedFormat { found, supported } => error_result(&json!({
            "code": "unsupported_format",
            "message": message,
            "retryable": false,
            "suggestions": [format!(
                "this server reads formats {} to {}; a release that docs.rs built at another time \
                 may have a supported format",
                supported.start(),
                supported.end()
            )],
            "found": found,
            "supported": { "from": supported.start(), "to": supported.end() },
        })),
        ParseError::NoFormatVersion(_)
        | ParseError::Json { .. }
        | ParseError::FormatMismatch { .. }
        | ParseError::TooDeep => error_result(&json!({
            "code": "docs_unavailable",
            "message": message,
            "retryable": false,
            "suggestions": [],
            "reason": "unparsable",
        })),
    }
}

/// The tool error for a path that does not name one item (§8).
fn item_error(arguments: &Arguments, error: &GetItemError) -> CallToolResult {
    let release = release(arguments);
    match error {
        GetItemError::NotFound => error_result(&json!({
            "code": "item_not_found",
            "message": format!(
                "{release} has no item at {}: it was looked up as a public path, a re-export \
                 from another crate, a definition path, and a member of the item at its parent \
                 path",
                arguments.path
            ),
            "retryable": false,
            "suggestions": [
                "check the spelling and the case of each segment",
                "a method, associated item or variant is found through its type, trait or enum \
                 (tokio::sync::Mutex::lock)",
            ],
        })),
        GetItemError::Ambiguous { candidates } => error_result(&json!({
            "code": "ambiguous_path",
            "message": format!("{release} has more than one item at {}", arguments.path),
            "retryable": false,
            "suggestions": ["call get_item again with the path of one of the candidates"],
            "candidates": candidates,
        })),
    }
}

/// An error and its sources, joined with `: `.
struct ErrorChain<'a>(&'a dyn std::error::Error);

impl std::fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        for source in std::iter::successors(self.0.source(), |&source| source.source()) {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
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
    fn the_crate_name_is_added_to_a_path_without_it() {
        let parsed =
            arguments(json!({ "crate": "axum-core@0.5.6", "path": "body::Body" })).expect("valid");
        assert_eq!(&*parsed.path, "axum_core::body::Body");
        let parsed = arguments(json!({ "crate": "axum-core@0.5.6", "path": "::axum-core::body" }))
            .expect("valid");
        assert_eq!(&*parsed.path, "axum-core::body");
        assert_eq!(parsed.version, Version::new(0, 5, 6));
    }

    #[test]
    fn a_crate_without_an_exact_version_is_an_invalid_argument() {
        for crate_argument in ["tokio", "tokio@1", "tokio@^1.47", "1tokio@1.47.1"] {
            let error = arguments(json!({ "crate": crate_argument, "path": "tokio::spawn" }))
                .expect_err(crate_argument);
            assert!(
                error.message.contains("`crate`"),
                "{crate_argument}: {error:?}"
            );
        }
        arguments(json!({ "crate": "tokio@1.47.1", "path": " " })).expect_err("an empty path");
        arguments(json!({ "crate": "tokio@1.47.1" })).expect_err("no path");
    }

    /// Every property of a full response is declared in the output schema.
    #[test]
    fn the_output_schema_covers_the_response() {
        use crate::query::get_item::{
            Deprecated,
            ItemData,
            ItemResponse,
            Provenance,
            ReexportOf,
            ResolvedFrom,
            Source,
            SourceLocation,
        };

        let some = || Some(Box::<str>::from("x"));
        let many = || Box::from([Box::<str>::from("x")]);
        let response = ItemResponse {
            provenance: Provenance {
                source: Source::DocsRs,
                name: "x",
                version: "1.0.0".into(),
                resolved_from: ResolvedFrom::Explicit,
                target: "x",
                format_version: 61,
            },
            warnings: Box::from(["x".into()]),
            data: ItemData {
                path: "x".into(),
                kind: "struct",
                parent: some(),
                impl_header: some(),
                signature: some(),
                where_predicates: many(),
                fields: many(),
                variants: many(),
                docs: Some("x"),
                docs_truncated: true,
                deprecated: Some(Deprecated {
                    since: Some("x"),
                    note: Some("x"),
                }),
                must_use: true,
                non_exhaustive: true,
                also_reexported_at: many(),
                reexport_of: Some(ReexportOf {
                    name: "x".into(),
                    path: "x".into(),
                }),
                source_location: Some(SourceLocation {
                    file: "x",
                    lines: "1".into(),
                    url: some(),
                }),
            },
            hints: many(),
        };
        let value = serde_json::to_value(&response).expect("serializes");
        assert_declared(&value, &Value::Object(output_schema()), "");
    }

    /// Checks that every key of the objects in `value` is a property in `schema`.
    fn assert_declared(value: &Value, schema: &Value, at: &str) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    let property = &schema["properties"][key];
                    assert!(!property.is_null(), "{at}/{key} is not in the schema");
                    assert_declared(value, property, &format!("{at}/{key}"));
                }
            },
            Value::Array(items) => {
                for item in items {
                    assert_declared(item, &schema["items"], at);
                }
            },
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {},
        }
    }

    #[test]
    fn the_description_fits_the_limit() {
        let len = DESCRIPTION.chars().count();
        assert!(len <= 2048, "{len} characters");
    }
}
