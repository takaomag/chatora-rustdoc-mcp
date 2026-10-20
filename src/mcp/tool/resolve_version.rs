//! The `resolve_version` tool: which version of a crate is meant, and what docs.rs has of it
//! (§5.3 and §6.3).

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use semver::Version;
use serde_json::{Value, json};

#[cfg(feature = "local")]
use super::project::Found;
use super::{
    ErrorChain,
    InvalidArgument,
    ToolState,
    get_item::{self, ReleaseArguments},
    project::{self, InProject},
};
#[cfg(feature = "local")]
use crate::data::{
    packages::{DocsRsCheck, Source},
    sparse_index::IndexEntry,
};
use crate::{
    data::{crate_name::CrateName, target::TargetTriple, version::VersionSpec},
    query::get_item::ResolvedFrom,
};

/// The name of the tool.
pub(super) const NAME: &str = "resolve_version";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("resolve_version.md");

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio), or the crate and its version as name@version \
                                (tokio@1.47.1).",
            },
            "version": {
                "type": "string",
                "description": "The version: exact (1.47.1), a semver requirement (^1.47, ~1.47) \
                                or latest. Defaults to the project's, or the latest.",
            },
            "target": {
                "type": "string",
                "description": "The target triple whose rustdoc JSON is looked for. Defaults to \
                                the host's.",
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace (its directory or Cargo.toml) whose Cargo.lock \
                                decides the version when none is given. Defaults to the project \
                                Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Resolve a crate version")
        .with_raw_output_schema(Arc::new(output_schema()))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1). Every property but the required
/// ones is left out when it has no value.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "enum": ["crates.io", "project"] },
                    "crate": { "type": "string" },
                    "version": { "type": "string" },
                    "resolved_from": {
                        "type": "string",
                        "enum": ["explicit", "explicit_req", "latest", "cargo_lock", "cargo_lock_raw"],
                    },
                    "project_from": {
                        "type": "string",
                        "enum": ["argument", "claude_project_dir", "cwd"],
                    },
                    "index_fetched_at": {
                        "type": "string",
                        "description": "When the crates.io index was read (RFC 3339), when it \
                                        was older than it is trusted for, as offline.",
                    },
                    "target": { "type": "string" },
                },
                "required": ["source", "crate", "version", "target"],
            },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "version": { "type": "string" },
                    "source": {
                        "type": "string",
                        "enum": ["crates.io", "path", "git", "registry"],
                    },
                    "yanked": { "type": "boolean" },
                    "rust_version": { "type": "string" },
                    "latest": {
                        "type": "string",
                        "description": "The latest release on crates.io.",
                    },
                    "docs_rs_json": {
                        "type": "string",
                        "enum": ["available", "no_json", "no_target_json", "not_on_docs_rs", "unknown"],
                    },
                },
                "required": ["version", "source", "docs_rs_json"],
            },
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

/// Checks the arguments (requirement R-ARG-1): those of `get_item` that name a release.
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<ReleaseArguments, InvalidArgument> {
    get_item::parse_release(arguments)
}

/// What the version was decided by.
struct Decided {
    spec: VersionSpec,
    /// The project's package, when the project decided the version.
    #[cfg(feature = "local")]
    found: Option<Box<Found>>,
}

/// Decides the version as `get_item` does (requirements R-VR-1 to R-VR-3), and tells what
/// crates.io and docs.rs have of it.
pub(super) async fn execute(arguments: ReleaseArguments, state: Arc<ToolState>) -> CallToolResult {
    if get_item::std_crate(&arguments.name).is_some() {
        return super::tool_error(
            "invalid_argument",
            &format!(
                "{} comes with the Rust toolchain and has no versions on crates.io",
                arguments.name
            ),
            false,
            &["call get_item for its items, which come from a nightly toolchain's documentation"],
        );
    }
    let host = state.host().await;
    let target = arguments
        .target
        .clone()
        .unwrap_or_else(|| host.triple.clone());
    let mut warnings: Vec<Box<str>> = Vec::new();
    if arguments.target.is_none()
        && let Some(why) = &host.stand_in_because
    {
        warnings.push(
            format!("the target {target} is used, as the host target is not known: {why}").into(),
        );
    }
    let ReleaseArguments {
        name,
        version,
        project,
        ..
    } = arguments;
    let decided = match version {
        Some(spec) => Decided {
            spec,
            #[cfg(feature = "local")]
            found: None,
        },
        None => match project::version_in_project(&state, &name, project.as_deref()).await {
            #[cfg(feature = "local")]
            InProject::Found(found) => {
                warnings.extend(found.warnings.iter().cloned());
                Decided {
                    spec: VersionSpec::Exact(found.package.version.clone()),
                    found: Some(found),
                }
            },
            InProject::Elsewhere(elsewhere) => {
                warnings.extend(elsewhere);
                Decided {
                    spec: VersionSpec::Latest,
                    #[cfg(feature = "local")]
                    found: None,
                }
            },
            #[cfg(feature = "local")]
            InProject::Error(error) => return *error,
        },
    };
    // A package of the project that is not from crates.io is not looked for there: a crate of
    // the same name on crates.io is another (requirement R-VR-2).
    #[cfg(feature = "local")]
    if let Some(found) = &decided.found
        && found.package.source != Source::CratesIo
    {
        return not_from_crates_io(found, &target, &warnings);
    }
    from_crates_io(&state, name, decided, &target, warnings).await
}

/// The answer for a version on crates.io.
async fn from_crates_io(
    state: &ToolState,
    name: CrateName,
    decided: Decided,
    target: &TargetTriple,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let entry = match state.index.entry(&name).await {
        Ok(entry) => entry,
        Err(error) => return get_item::index_error(&state.cache, &name, &error),
    };
    warnings.extend(entry.stale.clone());
    let spec = &decided.spec;
    let resolved = match spec.resolve(&entry.versions) {
        Ok(resolved) => resolved,
        Err(no_version) => {
            return get_item::version_not_found(state, &entry, spec, &no_version);
        },
    };
    warnings.extend(resolved.warnings.into_iter().map(Into::into));
    let found = resolved.version;
    let published: CrateName = entry.name.parse().unwrap_or(name);
    let latest = VersionSpec::Latest
        .resolve(&entry.versions)
        .ok()
        .map(|latest| latest.version.version.to_string());
    let by_argument = match spec {
        VersionSpec::Exact(_) => ResolvedFrom::Explicit,
        VersionSpec::Requirement(_) => ResolvedFrom::ExplicitReq,
        VersionSpec::Latest => ResolvedFrom::Latest,
    };
    #[cfg(feature = "local")]
    let (resolved_from, project_from, on_docs_rs) = decided
        .found
        .as_deref()
        .map_or((by_argument, None, true), |found| {
            in_project(found, &entry, &mut warnings)
        });
    #[cfg(not(feature = "local"))]
    let (resolved_from, project_from, on_docs_rs) = (by_argument, None::<&str>, true);
    let docs_rs_json = if on_docs_rs {
        json_status(state, &published, &found.version, target, &mut warnings).await
    } else {
        "not_on_docs_rs"
    };
    let mut data = JsonObject::new();
    data.insert("version".into(), json!(found.version.to_string()));
    data.insert("source".into(), json!("crates.io"));
    if found.yanked {
        data.insert("yanked".into(), json!(true));
    }
    if let Some(rust_version) = &found.rust_version {
        data.insert("rust_version".into(), json!(rust_version));
    }
    if let Some(latest) = latest {
        data.insert("latest".into(), json!(latest));
    }
    data.insert("docs_rs_json".into(), json!(docs_rs_json));
    respond(
        json!({
            "source": "crates.io",
            "crate": published.as_str(),
            "version": found.version.to_string(),
            "resolved_from": resolved_from,
            "project_from": project_from,
            "index_fetched_at": entry.stale_since(),
            "target": target.as_str(),
        }),
        Value::Object(data),
        &warnings,
    )
}

/// How the project decided the version, where the project came from, and whether docs.rs has
/// the project's package: only if it is the one on crates.io (requirement R-VR-2).
#[cfg(feature = "local")]
fn in_project(
    found: &Found,
    entry: &IndexEntry,
    warnings: &mut Vec<Box<str>>,
) -> (ResolvedFrom, Option<&'static str>, bool) {
    let check = found.package.docs_rs_check(entry);
    if check != DocsRsCheck::Allowed {
        warnings.push(
            format!(
                "the project's {}@{} is {}, so docs.rs does not have it",
                found.package.name,
                found.package.version,
                get_item::not_on_docs_rs(&found.package, check)
            )
            .into(),
        );
    }
    (
        found.resolved_from,
        Some(found.project_from),
        check == DocsRsCheck::Allowed,
    )
}

/// The answer for a package of the project that is not from crates.io: a workspace member, or a
/// path, git or registry dependency.
#[cfg(feature = "local")]
fn not_from_crates_io(
    found: &Found,
    target: &TargetTriple,
    warnings: &[Box<str>],
) -> CallToolResult {
    let package = &found.package;
    let source = match &package.source {
        Source::Local => "path",
        Source::Git(_) => "git",
        Source::Registry(_) => "registry",
        Source::CratesIo => "crates.io",
    };
    respond(
        json!({
            "source": "project",
            "crate": &*package.name,
            "version": package.version.to_string(),
            "resolved_from": found.resolved_from,
            "project_from": found.project_from,
            "target": target.as_str(),
        }),
        json!({
            "version": package.version.to_string(),
            "source": source,
            "docs_rs_json": "not_on_docs_rs",
        }),
        warnings,
    )
}

/// Whether docs.rs has the rustdoc JSON of `name` at `version` for `target`: from the cache, or
/// asked with `HEAD` (requirement R-TG-1).
async fn json_status(
    state: &ToolState,
    name: &CrateName,
    version: &Version,
    target: &TargetTriple,
    warnings: &mut Vec<Box<str>>,
) -> &'static str {
    let dir = state.cache.docs_rs_dir(name, version, Some(target));
    if matches!(state.cache.docs_rs_source(&dir), Ok(Some(_))) {
        return "available";
    }
    let asked = match state
        .docs_rs
        .has_rustdoc_json_for(name, version, target)
        .await
    {
        Ok(true) => Ok("available"),
        Ok(false) => state
            .docs_rs
            .has_rustdoc_json(name, version)
            .await
            .map(|any| if any { "no_target_json" } else { "no_json" }),
        Err(error) => Err(error),
    };
    asked.unwrap_or_else(|error| {
        warnings.push(
            format!(
                "whether docs.rs has rustdoc JSON of {name} {version} is not known: {}",
                ErrorChain(&error)
            )
            .into(),
        );
        "unknown"
    })
}

/// The response, with the provenance's absent values left out.
fn respond(mut provenance: Value, data: Value, warnings: &[Box<str>]) -> CallToolResult {
    if let Some(fields) = provenance.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    let mut response = JsonObject::new();
    response.insert("provenance".into(), provenance);
    if !warnings.is_empty() {
        response.insert("warnings".into(), json!(warnings));
    }
    response.insert("data".into(), data);
    CallToolResult::structured(Value::Object(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_fits_the_limit() {
        let len = DESCRIPTION.chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    #[test]
    fn the_arguments_are_those_of_get_item_without_a_path() {
        let Value::Object(arguments) =
            json!({ "crate": "tokio@^1.47", "target": "x86_64-pc-windows-msvc" })
        else {
            panic!("an object");
        };
        let parsed = parse_arguments(Some(&arguments)).expect("valid");
        assert_eq!(parsed.name.as_str(), "tokio");
        assert!(matches!(parsed.version, Some(VersionSpec::Requirement(_))));
        let Value::Object(no_crate) = json!({ "version": "1" }) else {
            panic!("an object");
        };
        let error = parse_arguments(Some(&no_crate)).expect_err("no crate");
        assert!(error.message.contains("`crate`"), "{error:?}");
    }

    /// Every property of a full response is declared in the output schema, and the required
    /// ones are there.
    #[test]
    fn the_output_schema_covers_the_response() {
        let result = respond(
            json!({
                "source": "crates.io",
                "crate": "x",
                "version": "1.0.0",
                "resolved_from": ResolvedFrom::CargoLock,
                "project_from": "cwd",
                "target": "x",
            }),
            json!({
                "version": "1.0.0",
                "source": "crates.io",
                "yanked": true,
                "rust_version": "1.70",
                "latest": "1.1.0",
                "docs_rs_json": "available",
            }),
            &["x".into()],
        );
        let response = result.structured_content.expect("structured");
        let schema = Value::Object(output_schema());
        for (key, value) in response.as_object().expect("an object") {
            let property = &schema["properties"][key];
            assert!(!property.is_null(), "{key}");
            if let Some(fields) = value.as_object() {
                for field in fields.keys() {
                    assert!(!property["properties"][field].is_null(), "{key}/{field}");
                }
            }
        }
    }
}
