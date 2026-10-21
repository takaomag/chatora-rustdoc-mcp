//! The `list_crates` tool: the project's crates at their resolved versions (§6.3, §3 Q1). It needs
//! a project, so a server without local features does not offer it (requirement R-ARCH-4).

use std::{path::PathBuf, sync::Arc};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Map, Value, json};

use super::{
    InvalidArgument,
    ToolState,
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
};
use crate::data::{
    cache::sha256_hex,
    features::FeatureSets,
    flight::Joined,
    metadata::Resolution,
    packages::{PackageGraph, ProjectPackage, Source},
    project::Workspace,
    target::TargetTriple,
};

/// The name of the tool.
pub(super) const NAME: &str = "list_crates";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("list_crates.md");

/// Crates in a page unless `limit` says otherwise.
const DEFAULT_LIMIT: usize = 100;

/// The most crates in a page.
const MAX_LIMIT: usize = 500;

/// The most chains of `used_by` for each crate.
const MAX_CHAINS: usize = 5;

/// Checked arguments.
#[derive(Debug)]
pub(super) struct Arguments {
    project: Option<PathBuf>,
    target: Option<TargetTriple>,
    all: bool,
    name: Option<Box<str>>,
    limit: usize,
    offset: usize,
    request_digest: Box<str>,
    cursor_data: Option<Box<str>>,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace (its directory or Cargo.toml). Defaults to the \
                                project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "target": {
                "type": "string",
                "description": "The target triple whose build is looked at. Defaults to the \
                                host's.",
            },
            "scope": {
                "type": "string",
                "enum": ["direct", "all"],
                "description": "direct (the default): the members and their direct \
                                dependencies. all: every package.",
            },
            "crate": {
                "type": "string",
                "description": "Only this crate, with the chains of dependencies to it.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most crates in a page (default 100).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("List the project's crates")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "enum": ["project"] },
                    "workspace": { "type": "string" },
                    "project_from": {
                        "type": "string",
                        "enum": ["argument", "claude_project_dir", "cwd"],
                    },
                    "resolved_from": {
                        "type": "string",
                        "enum": ["cargo_lock", "cargo_lock_raw"],
                    },
                    "target": { "type": "string" },
                },
                "required": ["source", "target"],
            },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "crates": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "crate": { "type": "string" },
                                "version": { "type": "string" },
                                "source": {
                                    "type": "string",
                                    "enum": ["crates.io", "path", "git", "registry"],
                                },
                                "member": { "type": "boolean" },
                                "direct": { "type": "boolean" },
                                "build": {
                                    "type": "string",
                                    "enum": ["normal", "dev", "none"],
                                },
                                "features": strings,
                                "docs_rs_json": { "type": "string", "enum": ["cached"] },
                                "used_by": {
                                    "type": "array",
                                    "items": strings,
                                },
                            },
                            "required": ["crate", "version", "source"],
                        },
                    },
                },
                "required": ["crates"],
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
        },
        "required": ["provenance", "data", "page"],
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
    let string = |name: &'static str| match argument(name) {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.trim())),
        Some(_) => Err(InvalidArgument::new(
            format!("`{name}` must be a string that is not empty"),
            &[r#"pass a string, for example {"crate": "tokio"}"#],
        )),
    };
    let target = string("target")?
        .map(|target| {
            target.parse::<TargetTriple>().map_err(|error| {
                InvalidArgument::new(format!("`target` is not valid: {error}"), &[
                    r#"pass a target triple, for example {"target": "x86_64-unknown-linux-gnu"}"#,
                ])
            })
        })
        .transpose()?;
    let all = match string("scope")? {
        None | Some("direct") => false,
        Some("all") => true,
        Some(other) => {
            return Err(InvalidArgument::new(
                format!("`scope` is {other:?}, not direct or all"),
                &[r#"pass direct or all, for example {"scope": "all"}"#],
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
    Ok(Arguments {
        project: string("project")?.map(PathBuf::from),
        target,
        all,
        name: string("crate")?.map(Into::into),
        limit,
        offset,
        request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
        cursor_data,
    })
}

/// The features of the packages in the build of `workspace` for `target`, waited for until
/// `until`, or `None` with a warning when the build is not known though `cargo metadata` ran
/// (`resolved_from` is `cargo_lock`), or not yet.
async fn build_sets(
    state: &ToolState,
    (workspace, target, until): (&Workspace, &TargetTriple, tokio::time::Instant),
    resolved_from: &str,
    warnings: &mut Vec<Value>,
) -> Option<Arc<FeatureSets>> {
    // Until the call's deadline: they only annotate the list (requirement R-ARCH-6).
    match state
        .resolver
        .features_until(workspace, target, until)
        .await
    {
        Joined::Done(Ok(sets)) => Some(sets),
        Joined::Done(Err(error)) => {
            if resolved_from == "cargo_lock" {
                warnings.push(json!(format!(
                    "which crates the build for {target} uses is not known: {}",
                    super::ErrorChain(&error)
                )));
            }
            None
        },
        Joined::Ended => {
            warnings.push(json!(super::get_item::NOT_READ));
            None
        },
        Joined::Going(doing) => {
            warnings.push(json!(super::get_item::still_reading(doing)));
            None
        },
    }
}

/// The provenance of the list while cargo resolves the packages of `workspace` (requirement
/// R-OUT-9).
fn resolving_provenance(
    workspace: &Workspace,
    project_from: &str,
    target: &TargetTriple,
) -> Map<String, Value> {
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("project"));
    provenance.insert(
        "workspace".to_owned(),
        json!(workspace.root.to_string_lossy()),
    );
    provenance.insert("project_from".to_owned(), json!(project_from));
    provenance.insert("target".to_owned(), json!(target.as_str()));
    provenance
}

/// Lists the project's crates.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    let until = state.until();
    let (workspace, project_from) =
        match super::project::workspace(state.project.as_ref(), arguments.project.as_deref()) {
            Ok(Some(found)) => found,
            Ok(None) => {
                return super::tool_error(
                    "project_not_found",
                    "there is no project here whose crates to list",
                    false,
                    &[r#"pass the workspace, for example {"project": "/home/me/app"}"#],
                );
            },
            Err(error) => return *error,
        };
    let (target, target_warnings) = match &arguments.target {
        Some(target) => (target.clone(), Vec::new()),
        None => super::project::default_target(&state, arguments.project.as_deref()).await,
    };
    let resolution = match state.resolver.resolve_until(&workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => return super::project::resolve_error(&workspace, &error),
        Joined::Ended => return super::get_item::stopped(),
        Joined::Going(doing) => {
            let provenance = resolving_provenance(&workspace, project_from.as_str(), &target);
            return super::project::resolving(provenance, doing, &target_warnings);
        },
    };
    let mut warnings: Vec<Value> = target_warnings
        .iter()
        .map(|warning| json!(warning))
        .collect();
    let resolved_from = match &*resolution {
        Resolution::Metadata { .. } => "cargo_lock",
        Resolution::CargoLockRaw { .. } => {
            warnings.push(json!(
                "cargo metadata failed, so Cargo.lock is read directly: the members cannot be \
                 told from path dependencies, and which crates the build uses is not known"
            ));
            "cargo_lock_raw"
        },
    };
    let built_for = (&workspace, &target, until);
    let sets = build_sets(&state, built_for, resolved_from, &mut warnings).await;
    let graph = PackageGraph::new(&resolution);
    let context = Listing {
        graph: &graph,
        sets: sets.as_deref(),
        state: &state,
        target: &target,
    };
    let crates = selected(&graph, &arguments);
    let listed: Vec<String> = graph
        .packages()
        .iter()
        .map(|package| format!("{}@{}", package.name, package.version))
        .collect();
    let digest = sha256_hex(listed.join(",").as_bytes());
    let data_digest = digest.get(..16).unwrap_or(&digest);
    if arguments
        .cursor_data
        .as_deref()
        .is_some_and(|cursor| cursor != data_digest)
    {
        return super::tool_error(
            "invalid_argument",
            "the cursor does not fit: the project's crates changed since it was made",
            false,
            &["call list_crates again without cursor"],
        );
    }
    let entries: Vec<Value> = crates
        .iter()
        .skip(arguments.offset)
        .take(arguments.limit)
        .map(|package| context.entry(package, arguments.name.is_some()))
        .collect();
    let page = page(&arguments, entries.len(), crates.len(), data_digest);
    let mut response = Map::new();
    response.insert(
        "provenance".to_owned(),
        json!({
            "source": "project",
            "workspace": workspace.root.display().to_string(),
            "project_from": project_from.as_str(),
            "resolved_from": resolved_from,
            "target": target.as_str(),
        }),
    );
    if !warnings.is_empty() {
        response.insert("warnings".to_owned(), Value::Array(warnings));
    }
    response.insert("data".to_owned(), json!({ "crates": entries }));
    response.insert("page".to_owned(), page);
    CallToolResult::structured(Value::Object(response))
}

/// The crates to list: those of the name asked for, or the members and their direct
/// dependencies (all with `scope: all`); the members first, then the direct dependencies, then
/// the others, by name and version.
fn selected<'g>(graph: &'g PackageGraph, arguments: &Arguments) -> Vec<&'g ProjectPackage> {
    let mut crates: Vec<&ProjectPackage> = graph
        .packages()
        .iter()
        .filter(|package| {
            arguments.name.as_deref().map_or_else(
                || arguments.all || graph.is_member(package) || graph.is_direct(package),
                |name| same_name(&package.name, name) || same_name(&package.lib_name, name),
            )
        })
        .collect();
    // The members, then the direct dependencies, then the others, by name and version.
    crates.sort_by(|a, b| {
        let rank = |package: &ProjectPackage| {
            if graph.is_member(package) {
                0
            } else if graph.is_direct(package) {
                1
            } else {
                2
            }
        };
        (rank(a), &a.name, &a.version).cmp(&(rank(b), &b.name, &b.version))
    });
    crates
}

/// The page of `shown` crates of `total` (requirement R-OUT-3).
fn page(arguments: &Arguments, shown: usize, total: usize, data_digest: &str) -> Value {
    let next = arguments.offset.saturating_add(shown);
    let mut page = Map::new();
    page.insert("shown".to_owned(), json!(shown));
    page.insert("total".to_owned(), json!(total));
    page.insert("has_more".to_owned(), json!(next < total));
    if next < total {
        page.insert(
            "next_cursor".to_owned(),
            json!(format!("{}.{data_digest}.{next}", arguments.request_digest)),
        );
    }
    Value::Object(page)
}

/// What the entries of the list are made from.
struct Listing<'a> {
    graph: &'a PackageGraph,
    sets: Option<&'a FeatureSets>,
    state: &'a ToolState,
    target: &'a TargetTriple,
}

impl Listing<'_> {
    /// The entry of `package`, with the chains to it if `chains`.
    fn entry(&self, package: &ProjectPackage, chains: bool) -> Value {
        let mut entry = Map::new();
        entry.insert("crate".to_owned(), json!(package.name));
        entry.insert("version".to_owned(), json!(package.version.to_string()));
        let source = match &package.source {
            Source::CratesIo => "crates.io",
            Source::Local => "path",
            Source::Git(_) => "git",
            Source::Registry(_) => "registry",
        };
        entry.insert("source".to_owned(), json!(source));
        if self.graph.is_member(package) {
            entry.insert("member".to_owned(), json!(true));
        }
        if self.graph.is_direct(package) {
            entry.insert("direct".to_owned(), json!(true));
        }
        if let Some(sets) = self.sets {
            let normal = sets.normal(&package.name, &package.version);
            let build = if normal.is_some() {
                "normal"
            } else if sets.with_dev(&package.name, &package.version).is_some() {
                "dev"
            } else {
                "none"
            };
            entry.insert("build".to_owned(), json!(build));
            if let Some(features) =
                normal.or_else(|| sets.with_dev(&package.name, &package.version))
            {
                entry.insert("features".to_owned(), json!(features));
            }
        }
        if package.source == Source::CratesIo
            && let Ok(name) = package.name.parse()
        {
            let dir = self
                .state
                .cache
                .docs_rs_dir(&name, &package.version, Some(self.target));
            if matches!(self.state.cache.docs_rs_source(&dir), Ok(Some(_))) {
                entry.insert("docs_rs_json".to_owned(), json!("cached"));
            }
        }
        if chains {
            let used_by: Vec<Vec<String>> = self
                .graph
                .paths_to(package, MAX_CHAINS)
                .into_iter()
                .map(|chain| {
                    chain
                        .iter()
                        .map(|package| format!("{} {}", package.name, package.version))
                        .collect()
                })
                .collect();
            entry.insert("used_by".to_owned(), json!(used_by));
        }
        Value::Object(entry)
    }
}

/// Whether `a` and `b` name the same crate, with `-` and `_` the same and the case ignored.
fn same_name(a: &str, b: &str) -> bool {
    a.replace('-', "_")
        .eq_ignore_ascii_case(&b.replace('-', "_"))
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
    fn bad_arguments_are_invalid_arguments() {
        for (value, about) in [
            (json!({ "scope": "some" }), "`scope`"),
            (json!({ "limit": 0 }), "`limit`"),
            (json!({ "crate": 1 }), "`crate`"),
            (json!({ "target": "../x" }), "`target`"),
        ] {
            let Value::Object(arguments) = value.clone() else {
                panic!("an object");
            };
            let error = parse_arguments(Some(&arguments)).expect_err("invalid");
            assert!(error.message.contains(about), "{value}: {error:?}");
        }
        assert!(same_name("tokio-util", "Tokio_Util"));
    }
}
