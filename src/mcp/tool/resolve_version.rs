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
    data::{
        crate_name::CrateName,
        docs_rs::FetchError,
        target::TargetTriple,
        version::VersionSpec,
    },
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
                                decides the version when none is given, and whose package of an \
                                exact version given is shown in place of crates.io's. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Resolve a crate version")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1). Every property but the required
/// ones is left out when it has no value.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let mut schema = object(json!({
        "type": "object",
        "properties": {
            "provenance": {
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "enum": ["crates.io", "project", "local"],
                        "description": "local: a crate of git that build_external_docs recorded.",
                    },
                    "crate": { "type": "string" },
                    "version": { "type": "string" },
                    "resolved_from": {
                        "type": "string",
                        "enum": ["explicit", "explicit_req", "latest", "cargo_lock", "cargo_lock_raw", "git", "external"],
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
                    "package_source": { "type": "string", "enum": ["git"] },
                    "recorded_by": { "type": "string", "enum": ["build_external_docs"] },
                    "stale": { "type": "boolean" },
                    "stale_reason": {
                        "type": "string",
                        "enum": ["ref_ttl_expired", "inputs_changed", "project_missing"],
                        "description": "ref_ttl_expired: a branch or a tag of git was resolved \
                                        more than a day ago, at resolved_at. inputs_changed: the \
                                        workspace outside the default project that \
                                        build_external_docs recorded changed since. \
                                        project_missing: its directory is gone.",
                    },
                },
                // Before the version is known, a response while it is resolved has no `version`
                // or `source` (requirement R-OUT-9).
                "required": ["crate", "target"],
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
            "hints": strings,
        },
        "required": ["provenance", "data"],
    }));
    if let Some(Value::Object(provenance)) = schema
        .get_mut("properties")
        .and_then(|properties| properties.get_mut("provenance"))
        && let Some(Value::Object(properties)) = provenance.get_mut("properties")
    {
        properties.extend(get_item::git_provenance_properties());
    }
    schema
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
    // Acted on once for the crate in the project, as `get_item` does (decided in M2's D5).
    #[cfg(feature = "local")]
    {
        let mut arguments = arguments;
        let scope = format!("crate {:?} {}", arguments.project, arguments.name);
        arguments.refresh = arguments.refresh && super::local::refresh_acts(&scope);
        let result = resolve(arguments, state).await;
        super::local::refresh_told(&scope, &result);
        result
    }
    #[cfg(not(feature = "local"))]
    resolve(arguments, state).await
}

/// [`execute`], forgetting what failed first when it asks for `refresh`.
async fn resolve(arguments: ReleaseArguments, state: Arc<ToolState>) -> CallToolResult {
    // In the records of `build_external_docs`, which `refresh` is not of (decided in M2e's E3b).
    #[cfg(feature = "local")]
    if let Some(git) = &arguments.git {
        return of_git(&arguments, git, &state).await;
    }
    let until = state.until();
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
    // What failed is forgotten before it is read again (§6.4, decided in M2's D5).
    #[cfg(feature = "local")]
    if arguments.refresh
        && super::local::refresh_project(&state, arguments.project.as_deref()).await
    {
        state.index.forget(&arguments.name);
    }
    let (lookup, mut warnings) = project::lookup_target(
        &state,
        arguments.target.as_ref(),
        arguments.project.as_deref(),
    )
    .await;
    #[cfg(feature = "local")]
    let target = lookup.triple.clone();
    let ReleaseArguments {
        name,
        version,
        project,
        ..
    } = arguments;
    let decided = match version {
        // The project's package of the exact version given, if any (requirement R-VR-1, decided
        // in M2's D7).
        #[cfg(feature = "local")]
        Some(VersionSpec::Exact(exact)) => {
            let at = (&exact, &target);
            let found = match get_item::explicit_found(
                &state,
                project.as_deref(),
                &name,
                at,
                until,
                &mut warnings,
            )
            .await
            {
                Ok(found) => found.map(|(found, _)| Box::new(found)),
                Err(response) => return *response,
            };
            Decided {
                spec: VersionSpec::Exact(exact),
                found,
            }
        },
        Some(spec) => Decided {
            spec,
            #[cfg(feature = "local")]
            found: None,
        },
        None => match project::version_in_project(&state, &name, project.as_deref(), until).await {
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
            #[cfg(feature = "local")]
            InProject::Going(doing, project_from) => {
                return project::resolving_crate(&name, &target, (doing, project_from), &warnings);
            },
        },
    };
    answered(&state, name, decided, &lookup, warnings).await
}

/// What the project's package that `decided` found, or else crates.io, tells of the version that
/// it decided for `lookup`.
async fn answered(
    state: &ToolState,
    name: CrateName,
    decided: Decided,
    lookup: &project::LookupTarget,
    #[cfg_attr(
        not(feature = "local"),
        expect(unused_mut, reason = "only a project's package warns more")
    )]
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    #[cfg(feature = "local")]
    if let Some(found) = decided.found.clone() {
        let target = &lookup.triple;
        // A package of the project that is not from crates.io is not looked for there: a crate
        // of the same name on crates.io is another (requirement R-VR-2).
        let result = if found.package.source == Source::CratesIo {
            from_crates_io(state, name, decided, lookup, warnings).await
        } else {
            explicitly_the_projects(&found, &mut warnings);
            not_from_crates_io(&found, target, &warnings)
        };
        return of_changed_record(state, &found.workspace, target, result).await;
    }
    from_crates_io(state, name, decided, lookup, warnings).await
}

/// `result`, of the project `workspace` for `target`, marked stale when it is of the record of a
/// workspace outside the default project that changed since (decided in M2e's E4b-3, 1).
#[cfg(feature = "local")]
async fn of_changed_record(
    state: &ToolState,
    workspace: &crate::data::project::Workspace,
    target: &TargetTriple,
    result: CallToolResult,
) -> CallToolResult {
    let Some(mut response) = result
        .structured_content
        .clone()
        .filter(|_| result.is_error != Some(true))
    else {
        return result;
    };
    let Some((recorded_at, again, since)) =
        super::recorded_workspace::changed_since(state, workspace, Some(target)).await
    else {
        return result;
    };
    super::recorded_workspace::mark_changed(&mut response, &recorded_at, &again, since);
    CallToolResult::structured(response)
}

/// Warns that the project's package `found` of the exact version given is answered for, which
/// may differ from the release of that name on crates.io (requirement R-VR-1, decided in M2's D7).
#[cfg(feature = "local")]
fn explicitly_the_projects(found: &Found, warnings: &mut Vec<Box<str>>) {
    if found.resolved_from == ResolvedFrom::Explicit {
        warnings.push(
            format!(
                "{} {} is the project's, which may differ from the release of that name on \
                 crates.io",
                found.package.name, found.package.version
            )
            .into(),
        );
    }
}

/// The name that the index has the project's package `found` of crates.io by, its package's; or
/// the answer, when the index does not list its version, as a copy that docs.rs does not have, or
/// when the index as last read does not and cannot be read again (requirement R-VR-2, decided in
/// M2's D7).
#[cfg(feature = "local")]
async fn copy_in_project(
    state: &ToolState,
    found: &Found,
    name: CrateName,
    target: &TargetTriple,
    warnings: &mut Vec<Box<str>>,
) -> Result<CrateName, Box<CallToolResult>> {
    if found.package.source != Source::CratesIo {
        return Ok(name);
    }
    let package_name = found.package.name.parse().unwrap_or(name);
    match get_item::project_entry(state, &found.package, &package_name).await {
        Ok(get_item::InIndex::Copy(DocsRsCheck::NotInIndex)) => {
            explicitly_the_projects(found, warnings);
            warnings.push(
                format!(
                    "the project's {}@{} is {}, so docs.rs does not have it",
                    found.package.name,
                    found.package.version,
                    get_item::not_on_docs_rs(&found.package, DocsRsCheck::NotInIndex)
                )
                .into(),
            );
            Err(Box::new(not_from_crates_io(found, target, warnings)))
        },
        Ok(get_item::InIndex::Unlisted(entry)) => {
            let what = format!("{} {}", entry.name, found.package.version);
            Err(Box::new(get_item::stale_index_error(state, &what, &entry)))
        },
        // The index tells the rest, as before.
        Ok(get_item::InIndex::Release(_) | get_item::InIndex::Copy(_)) | Err(_) => Ok(package_name),
    }
}

/// The answer for a version on crates.io.
async fn from_crates_io(
    state: &ToolState,
    name: CrateName,
    decided: Decided,
    lookup: &project::LookupTarget,
    mut warnings: Vec<Box<str>>,
) -> CallToolResult {
    let target = &lookup.triple;
    // By the package's name, read again past the cache when the index does not list its version
    // (decided in M2's D7).
    #[cfg(feature = "local")]
    let name = match decided.found.as_deref() {
        Some(found) => match copy_in_project(state, found, name, target, &mut warnings).await {
            Ok(name) => name,
            Err(response) => return *response,
        },
        None => name,
    };
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
        let docs_rs = lookup.docs_rs();
        json_status(state, &published, &found.version, docs_rs, &mut warnings).await
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

/// The version of the crate of a git repository that `arguments` and `git` name, at the commit that
/// `build_external_docs` resolved its ref to, for the target asked for or the host's (decided in
/// M2e's E3b); or the response while it is made, its failure, or that it is not recorded.
#[cfg(feature = "local")]
async fn of_git(
    arguments: &ReleaseArguments,
    git: &get_item::GitSpec,
    state: &ToolState,
) -> CallToolResult {
    let target = match &arguments.target {
        Some(target) => target.clone(),
        None => state.host().await.triple.clone(),
    };
    let mut warnings = Vec::new();
    if arguments.refresh {
        warnings.push(super::recorded_git::NO_REFRESH.into());
    }
    let found = match super::recorded_git::recorded_git(
        state,
        (&arguments.name, git, &target),
        &mut warnings,
    )
    .await
    {
        Ok(found) => found,
        Err(result) => return *result,
    };
    let version = found.version.to_string();
    let mut response = response(
        json!({
            "source": "local",
            "crate": found.package.as_str(),
            "version": version,
            "resolved_from": ResolvedFrom::Git,
            "target": target.as_str(),
            "recorded_by": crate::query::local::RECORDED_BY,
        }),
        json!({
            "version": version,
            "source": "git",
            "docs_rs_json": "not_on_docs_rs",
        }),
        &warnings,
    );
    found.origin.mark(&mut response);
    CallToolResult::structured(response)
}

/// Whether docs.rs has the rustdoc JSON of `name` at `version` for `target`: from the cache, or
/// asked with `HEAD` (requirement R-TG-1).
async fn json_status(
    state: &ToolState,
    name: &CrateName,
    version: &Version,
    target: Option<&TargetTriple>,
    warnings: &mut Vec<Box<str>>,
) -> &'static str {
    let dir = state.cache.docs_rs_dir(name, version, target);
    if matches!(state.cache.docs_rs_source(&dir), Ok(Some(_))) {
        return "available";
    }
    // Without a target, of docs.rs's default target (decided in M2's D7).
    let Some(target) = target else {
        return match state.docs_rs.has_rustdoc_json(name, version).await {
            Ok(true) => "available",
            Ok(false) => "no_json",
            Err(error) => unknown_json(name, version, &error, warnings),
        };
    };
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
    asked.unwrap_or_else(|error| unknown_json(name, version, &error, warnings))
}

/// `unknown`, with a warning of why whether docs.rs has rustdoc JSON of `name` at `version` is
/// not known.
fn unknown_json(
    name: &CrateName,
    version: &Version,
    error: &FetchError,
    warnings: &mut Vec<Box<str>>,
) -> &'static str {
    warnings.push(
        format!(
            "whether docs.rs has rustdoc JSON of {name} {version} is not known: {}",
            ErrorChain(error)
        )
        .into(),
    );
    "unknown"
}

/// The response, with the provenance's absent values left out.
fn respond(provenance: Value, data: Value, warnings: &[Box<str>]) -> CallToolResult {
    CallToolResult::structured(response(provenance, data, warnings))
}

/// The structured content of [`respond`].
fn response(mut provenance: Value, data: Value, warnings: &[Box<str>]) -> Value {
    if let Some(fields) = provenance.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    let mut response = JsonObject::new();
    response.insert("provenance".into(), provenance);
    if !warnings.is_empty() {
        response.insert("warnings".into(), json!(warnings));
    }
    response.insert("data".into(), data);
    Value::Object(response)
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
