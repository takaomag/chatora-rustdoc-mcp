//! The `inspect_features` tool: the features of a crate, what each enables and what enables it
//! (requirement §5.9's features, §3 Q11, decided in M3d).
//!
//! The release is resolved as `resolve_version` resolves it, without its documentation nor a
//! build; its features are read from the crates.io index's line of that release (decision 41),
//! and those that docs.rs's build did not enable from its `.crate` (decisions 46 and 47).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use semver::Version;
use serde_json::{Map, Value, json};

use super::{
    ErrorChain,
    InvalidArgument,
    ToolState,
    get_item::{self, Paging, ReleaseArguments},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
    resolve_version,
};
use crate::{
    data::{
        cache::sha256_hex,
        crate_file,
        crate_name::CrateName,
        feature_tree::FeatureTree,
        sparse_index::{IndexEntry, IndexError},
    },
    query::{get_item::MAX_RESPONSE_CHARS, search::edit_distance},
};

/// The name of the tool.
pub(super) const NAME: &str = "inspect_features";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("inspect_features.md");

/// The features in a page unless `limit` says otherwise (decided in M3d, decision 39).
const DEFAULT_LIMIT: usize = 100;

/// The most features in a page, as for the other lists.
const MAX_LIMIT: usize = 200;

/// The most names in one list of a feature, as what enables a feature that thousands of others do
/// (`windows`'s `Win32`): the rest is counted (requirement R-OUT-3).
const MAX_LISTED: usize = 50;

/// How to pass `feature`, for `invalid_argument`.
const FEATURE_EXAMPLE: &[&str] =
    &[r#"pass a feature of the crate, for example {"feature": "net"}"#];

/// What is asked of the features, besides the release (decided in M3d, decisions 38 and 39).
#[derive(Debug, Clone)]
pub(super) struct FeatureAsk {
    /// The feature asked for alone, if one was.
    feature: Option<Box<str>>,
    /// The page of the features.
    paging: Paging,
}

/// The arguments checked: the release's, for `resolve_version`, and what is asked of its
/// features.
#[derive(Debug)]
pub(super) struct Inspect {
    release: ReleaseArguments,
    ask: FeatureAsk,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio), or the crate and its version as name@version.",
            },
            "feature": {
                "type": "string",
                "description": "One feature alone (net). All of them if left out.",
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
                    "The target triple, for whether docs.rs has its documentation.",
                    super::TARGET_DEFAULT
                ),
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most features in a page (default 100).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Inspect a Rust crate's features")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let total = json!({
        "type": "integer",
        "description": "How many there are, when the list is cut.",
    });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": resolve_version::provenance_schema(),
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "default": strings,
                    "default_total": total,
                    "docs_rs_json": {
                        "type": "string",
                        "enum": ["available", "no_json", "no_target_json", "not_on_docs_rs", "unknown"],
                        "description": "Whether docs.rs has the documentation, as resolve_version \
                                        tells.",
                    },
                    "features": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "enables": strings,
                                "enables_total": total,
                                "closure": strings,
                                "closure_total": total,
                                "dependencies": strings,
                                "dependencies_total": total,
                                "enabled_by": strings,
                                "enabled_by_total": total,
                                "default": { "type": "boolean" },
                                "implicit": { "type": "boolean" },
                                "in_docs": { "type": "boolean" },
                            },
                            "required": ["name", "enables"],
                        },
                    },
                },
                "required": ["default", "features"],
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

/// `value` as a JSON object, or an empty one.
fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            JsonObject::new()
        },
    }
}

/// Checks the arguments (requirement R-ARG-1).
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<Inspect, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    if get_item::std_crate(&release.name).is_some() {
        return Err(InvalidArgument::new(
            format!("{} has no features to enable", release.name),
            &["std's items that need a feature of the compiler tell it in get_item"],
        ));
    }
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let feature = match argument("feature") {
        None => None,
        Some(Value::String(feature)) if !feature.trim().is_empty() => {
            Some(Box::from(feature.trim()))
        },
        Some(_) => {
            return Err(InvalidArgument::new(
                "`feature` must be a string that is not empty",
                FEATURE_EXAMPLE,
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
                    &[r#"pass a whole number, for example {"limit": 100}"#],
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
    Ok(Inspect {
        release,
        ask: FeatureAsk {
            feature,
            paging: Paging {
                limit,
                offset,
                request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
                cursor_data,
            },
        },
    })
}

/// Resolves the release as `resolve_version` does, and tells its features from the crates.io
/// index, with those that docs.rs's build did not enable from its `.crate` (decided in M3d,
/// decisions 41, 46 and 47): neither its documentation nor a build is needed.
pub(super) async fn execute(inspect: Inspect, state: Arc<ToolState>) -> CallToolResult {
    let Inspect { release, ask } = inspect;
    #[cfg(feature = "local")]
    let refresh = release.refresh;
    let resolved = resolve_version::execute(release, Arc::clone(&state)).await;
    // An error, or the response while the release is resolved, as it is.
    let Some(content) = resolved
        .structured_content
        .as_ref()
        .filter(|content| resolved.is_error != Some(true) && content.get("data").is_some())
    else {
        return resolved;
    };
    let provenance = content
        .get("provenance")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut warnings: Vec<Box<str>> = content
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(Box::from)
        .collect();
    let hints: Vec<Value> = content
        .get("hints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let field = |key: &str| {
        content
            .pointer(&format!("/data/{key}"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let told = |key: &str| {
        provenance
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let (Ok(name), Ok(version)) = (
        provenance
            .get("crate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .parse::<CrateName>(),
        Version::parse(field("version")),
    ) else {
        return resolved;
    };
    // A crate that is not a crates.io release has no line in the index, as the project's copy of
    // one that the index does not have (decided in M3d, D2).
    if field("source") != "crates.io" || told("source") != "crates.io" {
        let of = if field("source") == "crates.io" {
            "a copy of the project that the crates.io index does not have"
        } else {
            field("source")
        };
        return not_a_release(&format!("{name} {version}"), of);
    }
    let (tree, checksum) = match release_features(&state, &name, &version).await {
        Ok(found) => found,
        Err(answer) => return *answer,
    };
    let docs_rs_json = field("docs_rs_json");
    // The project's copy that is not crates.io's, as patched or vendored, may have others.
    if docs_rs_json == "not_on_docs_rs" && told("resolved_from").starts_with("cargo_lock") {
        warnings.push(
            format!(
                "these are the features of {name} {version} on crates.io, which the project's \
                 copy of it, not the same as crates.io's, may not have"
            )
            .into(),
        );
    }
    // A `.crate` that failed to be fetched a moment ago is asked for again (§6.4).
    #[cfg(feature = "local")]
    if refresh {
        crate::data::forget_failure_of(&state.cache.crate_file_path(&name, &version));
    }
    let not_in_docs = if matches!(docs_rs_json, "available" | "unknown") {
        not_in_docs(&state, (&name, &version), &checksum, &mut warnings).await
    } else {
        None
    };
    respond(
        &provenance,
        (&tree, not_in_docs.as_deref()),
        &ask,
        (docs_rs_json, &warnings, &hints),
    )
    .map_or_else(|error| *error, CallToolResult::structured)
}

/// The tool error for `what`, a crate of `of`, not a crates.io release, whose features are not
/// read yet (decided in M3d, D2).
fn not_a_release(what: &str, of: &str) -> CallToolResult {
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "build_unsupported",
        "message": format!(
            "{what} is of {of}: inspect_features reads the features of crates.io releases (those \
             of other crates in a later version)"
        ),
        "retryable": false,
        "suggestions": ["read the [features] of its Cargo.toml"],
    }))
}

/// The features of `name` at `version`, from its line of the crates.io index, read again if the
/// index as kept does not list it yet, and the sha256 of its `.crate` (decided in M3d, decision
/// 41); else the error: when the index could not be read again, that it may lack a version
/// published since (requirement R-CA-3).
async fn release_features(
    state: &ToolState,
    name: &CrateName,
    version: &Version,
) -> Result<(FeatureTree, Box<str>), Box<CallToolResult>> {
    let listed = |entry: &IndexEntry| {
        let tree = entry.features(version)?;
        let checksum = entry
            .versions
            .iter()
            .find(|listed| listed.version == *version)?
            .checksum
            .clone();
        Some((tree, checksum))
    };
    let entry = match state.index.entry(name).await {
        Ok(entry) => entry,
        Err(error) => return Err(Box::new(get_item::index_error(&state.cache, name, &error))),
    };
    if let Some(found) = listed(&entry) {
        return Ok(found);
    }
    let what = format!("{name} {version}");
    // Listed, but its line cannot be read: reading the index again does not change it.
    if entry
        .versions
        .iter()
        .any(|listed| listed.version == *version)
    {
        return Err(Box::new(super::tool_error(
            "network_error",
            &format!("the line of {what} in the crates.io index cannot be read"),
            false,
            &[],
        )));
    }
    match state.index.entry_fresh(name).await {
        Ok(entry) if let Some(found) = listed(&entry) => Ok(found),
        Ok(entry) if entry.stale.is_some() => {
            Err(Box::new(get_item::stale_index_error(state, &what, &entry)))
        },
        Ok(_) | Err(IndexError::NotFound) => Err(Box::new(super::tool_error(
            "version_not_found",
            &format!("the crates.io index does not list the features of {what}"),
            false,
            &["check the version with resolve_version"],
        ))),
        Err(error) => Err(Box::new(get_item::index_error(&state.cache, name, &error))),
    }
}

/// The features that docs.rs's build of `name` at `version` did not enable, as its `.crate`
/// tells, whose sha256 is `checksum` (requirement R-FT-1, decided in M3d, decision 46); `None`,
/// with a warning, when the `.crate` cannot be had or read by the call's deadline (requirement
/// R-ARCH-6): it may be large.
async fn not_in_docs(
    state: &ToolState,
    (name, version): (&CrateName, &Version),
    checksum: &str,
    warnings: &mut Vec<Box<str>>,
) -> Option<Box<[Box<str>]>> {
    let fetching = state
        .static_crates
        .crate_file(&state.cache, name, version, Some(checksum));
    let Ok(contents) = tokio::time::timeout_at(state.until(), fetching).await else {
        warnings.push(
            format!(
                "which features docs.rs built {name} {version} without is not known yet: its \
                 .crate is being fetched; call again for it"
            )
            .into(),
        );
        return None;
    };
    // Unpacked until its manifest, which may be far into it: off the async threads.
    let read = match contents {
        Ok(Some(contents)) => {
            let (name, version) = (name.clone(), version.clone());
            crate::task::spawn_blocking(move || {
                crate_file::docs_rs_build(&contents, &name, &version).map(Some)
            })
            .await
            .unwrap_or_else(|error| {
                let error = crate::task::resume_if_panic(error);
                Err(crate_file::CrateError::Cache(std::io::Error::other(error)))
            })
        },
        other => other.map(|_| None),
    };
    match read {
        Ok(Some(build)) => Some(build.missing),
        Ok(None) => None,
        Err(error) => {
            warnings.push(
                format!(
                    "which features docs.rs built {name} {version} without is not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
    }
}

/// The response for the features `tree`, those of `not_in_docs` not in the documentation, as
/// `ask` asks, with `provenance`, whether docs.rs has the documentation, `docs_rs_json`, and
/// `warnings` and `hints`; within the budget of a response, with fewer features than `limit` if
/// need be (requirement R-OUT-3, decided in M3d, decisions 38, 39, 40 and 46).
fn respond(
    provenance: &Map<String, Value>,
    (tree, not_in_docs): (&FeatureTree, Option<&[Box<str>]>),
    ask: &FeatureAsk,
    (docs_rs_json, warnings, hints): (&str, &[Box<str>], &[Value]),
) -> Result<Value, Box<CallToolResult>> {
    let names = names_asked(tree, ask.feature.as_deref())?;
    let digest = digest_of(tree);
    let paging = &ask.paging;
    if paging
        .cursor_data
        .as_deref()
        .is_some_and(|data| data != digest)
    {
        return Err(Box::new(
            InvalidArgument::new(
                "the features changed since the cursor was made; list again without the cursor",
                CURSOR_EXAMPLE,
            )
            .into_result(),
        ));
    }
    let total = names.len();
    if paging.offset > total {
        return Err(Box::new(
            InvalidArgument::new(
                "the cursor goes past the end of the list; list again without the cursor",
                CURSOR_EXAMPLE,
            )
            .into_result(),
        ));
    }
    let default: BTreeSet<&str> = if tree.contains("default") {
        tree.closure(["default"])
    } else {
        BTreeSet::new()
    };
    let enabled_by = tree.enabled_by();
    let not_in_docs: BTreeSet<&str> = not_in_docs
        .into_iter()
        .flatten()
        .map(AsRef::as_ref)
        .collect();
    let mut features: Vec<Value> = names
        .iter()
        .skip(paging.offset)
        .take(paging.limit)
        .map(|&name| feature_of(tree, name, (&default, &enabled_by, &not_in_docs)))
        .collect();
    let mut data = Map::new();
    let default_list: Vec<&str> = default
        .iter()
        .copied()
        .filter(|&feature| feature != "default")
        .collect();
    insert_listed(&mut data, "default", &default_list);
    if !docs_rs_json.is_empty() {
        data.insert("docs_rs_json".to_owned(), json!(docs_rs_json));
    }
    let answer = |features: &[Value]| {
        let shown = features.len();
        let next = paging.offset.saturating_add(shown);
        let mut page = Map::new();
        page.insert("shown".to_owned(), json!(shown));
        page.insert("total".to_owned(), json!(total));
        page.insert("has_more".to_owned(), json!(next < total));
        if next < total {
            page.insert(
                "next_cursor".to_owned(),
                json!(format!("{}.{digest}.{next}", paging.request_digest)),
            );
        }
        let mut data = data.clone();
        data.insert("features".to_owned(), json!(features));
        let mut answer = Map::new();
        answer.insert("provenance".to_owned(), Value::Object(provenance.clone()));
        answer.insert("data".to_owned(), Value::Object(data));
        answer.insert("page".to_owned(), Value::Object(page));
        if !hints.is_empty() {
            answer.insert("hints".to_owned(), json!(hints));
        }
        let mut answer = Value::Object(answer);
        get_item::add_warnings(&mut answer, warnings);
        answer
    };
    // Fewer features while the response is over its budget, one at least; the rest on the next
    // page.
    loop {
        let response = answer(&features);
        let size = response.to_string().chars().count();
        if size <= MAX_RESPONSE_CHARS || features.len() <= 1 {
            return Ok(response);
        }
        // In proportion to what is over, a feature less at least.
        let fits = features
            .len()
            .saturating_mul(MAX_RESPONSE_CHARS)
            .checked_div(size)
            .unwrap_or_default();
        features.truncate(fits.clamp(1, features.len().saturating_sub(1)));
    }
}

/// The names of the features of `tree` told: `feature` alone if one is asked for, which the
/// crate must have, or else all. One that it does not have is told with those of a name like it
/// (requirement R-PR-7).
fn names_asked<'t>(
    tree: &'t FeatureTree,
    feature: Option<&'t str>,
) -> Result<Vec<&'t str>, Box<CallToolResult>> {
    let Some(feature) = feature else {
        return Ok(tree.names().collect());
    };
    if tree.contains(feature) {
        return Ok(vec![feature]);
    }
    let wanted = feature.to_lowercase();
    let mut near: Vec<(usize, &str)> = tree
        .names()
        .filter_map(|name| {
            let distance = edit_distance(&wanted, &name.to_lowercase());
            let longer = wanted.chars().count().max(name.chars().count());
            // At least 70% alike, and at most 3 edits, as `search_items` suggests names.
            (distance <= 3 && distance.saturating_mul(10) <= longer.saturating_mul(3))
                .then_some((distance, name))
        })
        .collect();
    near.sort_unstable();
    let suggestion = if near.is_empty() {
        "list the features without `feature`".to_owned()
    } else {
        format!(
            "features with a name like it: {}",
            near.iter()
                .take(5)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Err(Box::new(super::tool_error(
        "invalid_argument",
        &format!("the crate has no feature `{feature}`"),
        false,
        &[&suggestion],
    )))
}

/// The feature `name` of `tree` as told: its values, the crate's features that it enables, what
/// it enables of dependencies, and what enables it, as `enabled_by` has it, each list of at most
/// [`MAX_LISTED`] names with its total when cut; whether `default` enables it, an optional
/// dependency makes it, or `not_in_docs` has it.
fn feature_of(
    tree: &FeatureTree,
    name: &str,
    (default, enabled_by, not_in_docs): (
        &BTreeSet<&str>,
        &BTreeMap<&str, BTreeSet<&str>>,
        &BTreeSet<&str>,
    ),
) -> Value {
    let closure = tree.closure([name]);
    let mut feature = Map::new();
    feature.insert("name".to_owned(), json!(name));
    let enables: Vec<&str> = tree
        .values(name)
        .unwrap_or_default()
        .iter()
        .map(AsRef::as_ref)
        .collect();
    insert_listed(&mut feature, "enables", &enables);
    let lists: [(&str, Vec<&str>); 3] = [
        (
            "closure",
            closure
                .iter()
                .copied()
                .filter(|&other| other != name)
                .collect(),
        ),
        (
            "dependencies",
            tree.dependencies(&closure).into_iter().collect(),
        ),
        (
            "enabled_by",
            enabled_by
                .get(name)
                .map(|by| by.iter().copied().collect())
                .unwrap_or_default(),
        ),
    ];
    for (key, list) in lists {
        if !list.is_empty() {
            insert_listed(&mut feature, key, &list);
        }
    }
    for (key, value, told) in [
        ("default", true, default.contains(name)),
        ("implicit", true, tree.is_implicit(name)),
        ("in_docs", false, not_in_docs.contains(name)),
    ] {
        if told {
            feature.insert(key.to_owned(), json!(value));
        }
    }
    Value::Object(feature)
}

/// Inserts `list` at `key` of `object`, at most [`MAX_LISTED`] of it, with its total at
/// `<key>_total` when it is cut.
fn insert_listed(object: &mut Map<String, Value>, key: &str, list: &[&str]) {
    object.insert(
        key.to_owned(),
        json!(list.iter().take(MAX_LISTED).collect::<Vec<_>>()),
    );
    if list.len() > MAX_LISTED {
        object.insert(format!("{key}_total"), json!(list.len()));
    }
}

/// The digest of the features `tree` that a cursor is made for: the list goes on only over the
/// same features.
fn digest_of(tree: &FeatureTree) -> String {
    let mut text = String::new();
    for name in tree.names() {
        text.push_str(name);
        for value in tree.values(name).unwrap_or_default() {
            text.push(' ');
            text.push_str(value);
        }
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
        .get(..16)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_fits_the_limit() {
        let len = super::super::for_this_build(DESCRIPTION).chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    fn tree() -> FeatureTree {
        FeatureTree::new(
            [
                ("default", vec!["rt"]),
                ("full", vec!["net", "rt", "hyper?/http1"]),
                ("net", vec!["mio/net"]),
                ("rt", vec![]),
            ],
            ["mio", "hyper"],
        )
    }

    fn asked(arguments: Value) -> FeatureAsk {
        let Value::Object(arguments) = arguments else {
            panic!("an object");
        };
        parse_arguments(Some(&arguments)).expect("valid").ask
    }

    /// Each feature is told with its values, the crate's features that it enables, what it
    /// enables of dependencies and what enables it; whether `default` enables it, an optional
    /// dependency makes it, and whether docs.rs's build had it (decided in M3d, decisions 38,
    /// 40 and 46).
    #[test]
    fn each_feature_tells_what_it_enables_and_what_enables_it() {
        let ask = asked(json!({ "crate": "demo" }));
        let answer = respond(
            &Map::new(),
            (&tree(), Some(&[Box::from("net")])),
            &ask,
            ("available", &[], &[]),
        )
        .expect("answered");
        let data = &answer["data"];
        assert_eq!(data["default"], json!(["rt"]));
        assert_eq!(data["docs_rs_json"], "available");
        let features = data["features"].as_array().expect("features");
        let names: Vec<&str> = features
            .iter()
            .filter_map(|feature| feature["name"].as_str())
            .collect();
        assert_eq!(names, ["default", "full", "hyper", "mio", "net", "rt"]);
        assert_eq!(
            features[1],
            json!({
                "name": "full",
                "enables": ["net", "rt", "hyper?/http1"],
                "closure": ["mio", "net", "rt"],
                "dependencies": ["dep:mio", "hyper?/http1", "mio/net"],
            })
        );
        let net = &features[4];
        assert_eq!(net["enabled_by"], json!(["full"]));
        assert_eq!(net["in_docs"], false);
        assert_eq!(features[3]["implicit"], true);
        assert_eq!(features[5]["default"], true);
        assert_eq!(features[5]["enabled_by"], json!(["default", "full"]));
        assert_eq!(
            answer["page"],
            json!({ "shown": 6, "total": 6, "has_more": false })
        );
    }

    /// The features are paged with a cursor made for them, and one is asked for alone; a feature
    /// that the crate does not have is told with those named like it (decided in M3d, decision
    /// 39, requirement R-PR-7).
    #[test]
    fn the_features_are_paged_and_one_is_asked_for_alone() {
        let first = respond(
            &Map::new(),
            (&tree(), None),
            &asked(json!({ "crate": "demo", "limit": 4 })),
            ("", &[], &[]),
        )
        .expect("answered");
        let cursor = first["page"]["next_cursor"].as_str().expect("a cursor");
        let rest = respond(
            &Map::new(),
            (&tree(), None),
            &asked(json!({ "crate": "demo", "limit": 4, "cursor": cursor })),
            ("", &[], &[]),
        )
        .expect("answered");
        assert_eq!(
            rest["page"],
            json!({ "shown": 2, "total": 6, "has_more": false })
        );
        assert_eq!(rest["data"]["features"][0]["name"], "net");
        // Of other features, the cursor is refused.
        let other = FeatureTree::new([("x", vec![])], []);
        let refused = respond(
            &Map::new(),
            (&other, None),
            &asked(json!({ "crate": "demo", "limit": 4, "cursor": cursor })),
            ("", &[], &[]),
        )
        .expect_err("refused");
        assert_eq!(refused.is_error, Some(true));
        let one = respond(
            &Map::new(),
            (&tree(), None),
            &asked(json!({ "crate": "demo", "feature": "net" })),
            ("", &[], &[]),
        )
        .expect("answered");
        assert_eq!(one["data"]["features"].as_array().map(Vec::len), Some(1));
        for (asked_for, near) in [("Full", true), ("fulll", true), ("c", false)] {
            let missing = respond(
                &Map::new(),
                (&tree(), None),
                &asked(json!({ "crate": "demo", "feature": asked_for })),
                ("", &[], &[]),
            )
            .expect_err("not a feature");
            let text = format!("{:?}", missing.content);
            assert!(text.contains("no feature"), "{text}");
            assert_eq!(text.contains("like it: full"), near, "{asked_for}: {text}");
        }
    }

    /// A response is kept within its budget: a long list of a feature is cut, with its total,
    /// and the page has fewer features, the rest after its cursor (requirement R-OUT-3).
    #[test]
    fn a_large_response_is_kept_within_its_budget() {
        let names: Vec<String> = (0..3_000)
            .map(|n| format!("Win32_Feature_{n:04}"))
            .collect();
        // Each enables a few dependencies too, so that a page of them is over the budget.
        let dependencies: Vec<String> = (0..8)
            .map(|n| format!("dep:a_dependency_with_a_long_name_{n}"))
            .collect();
        let values: Vec<&str> = std::iter::once("Win32")
            .chain(dependencies.iter().map(String::as_str))
            .collect();
        let tree = FeatureTree::new(
            names
                .iter()
                .map(|name| (name.as_str(), values.clone()))
                .chain([("Win32", vec![])]),
            [],
        );
        let answer = respond(
            &Map::new(),
            (&tree, None),
            &asked(json!({ "crate": "windows", "limit": 200 })),
            ("", &[], &[]),
        )
        .expect("answered");
        assert!(answer.to_string().chars().count() <= MAX_RESPONSE_CHARS);
        let shown = answer["page"]["shown"].as_u64().expect("a number");
        assert!((1..200).contains(&shown), "{shown}");
        assert_eq!(answer["page"]["has_more"], true);
        let win32 = respond(
            &Map::new(),
            (&tree, None),
            &asked(json!({ "crate": "windows", "feature": "Win32" })),
            ("", &[], &[]),
        )
        .expect("answered");
        let feature = &win32["data"]["features"][0];
        assert_eq!(
            feature["enabled_by"].as_array().map(Vec::len),
            Some(MAX_LISTED)
        );
        assert_eq!(feature["enabled_by_total"], 3_000);
    }

    /// std has no features to enable; `feature` is a name, and `limit` within its bounds.
    #[test]
    fn std_and_odd_arguments_are_refused() {
        for (arguments, part) in [
            (json!({ "crate": "std" }), "no features"),
            (json!({ "crate": "tokio", "feature": "" }), "`feature`"),
            (json!({ "crate": "tokio", "limit": 0 }), "`limit`"),
            (json!({ "crate": "tokio", "limit": 201 }), "`limit`"),
        ] {
            let Value::Object(arguments) = arguments else {
                panic!("an object");
            };
            let error = parse_arguments(Some(&arguments)).expect_err("invalid");
            assert!(error.message.contains(part), "{error:?}");
        }
    }
}
