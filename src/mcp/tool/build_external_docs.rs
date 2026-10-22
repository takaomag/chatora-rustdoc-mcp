//! The `build_external_docs` tool: builds the documentation of code that the query tools do not
//! build themselves, and records it for them (requirement R-WS-10, §3 Q22). It needs local
//! features, so a server without them does not offer it (requirement R-ARCH-4).
//!
//! It builds a release of crates.io that the project does not use (`super::external_build`); the
//! other rows of the table of requirement R-WS-10 come in the later steps of M2e.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Value, json};

use super::{
    ErrorChain,
    InvalidArgument,
    ToolState,
    error_result,
    get_item,
    local::targets_built,
    project::project_path,
};
use crate::data::{
    crate_name::CrateName,
    project::{self, ProjectError, ProjectFrom, Workspace},
    target::TargetTriple,
    version::VersionSpec,
};

/// The name of the tool, which users write in their `ask` rules (requirement R-SEC-2).
pub(super) const NAME: &str = "build_external_docs";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("build_external_docs.md");

/// How to say what to build, for `invalid_argument`.
const WHAT_EXAMPLE: &[&str] = &[
    r#"pass the crate, for example {"crate": "rand@0.9.2"}, or the workspace, for example {"project": "/home/me/other"}"#,
];

/// Checked arguments.
#[derive(Debug)]
pub(super) struct Arguments {
    /// The crate, and the version asked for.
    krate: Option<(CrateName, Option<VersionSpec>)>,
    /// The workspace, as given.
    project: Option<PathBuf>,
    /// The target asked for.
    target: Option<TargetTriple>,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate, with a version if it matters (rand@0.9.2).",
            },
            "version": {
                "type": "string",
                "description": "The version of the crate, unless in `crate`.",
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!(
                    "A workspace (its directory or Cargo.toml), relative to the project Claude \
                     Code was opened in unless absolute.",
                    super::PROJECT_IN_A_WORKTREE
                ),
            },
            "target": {
                "type": "string",
                "description": "The target triple to document for. Defaults to the project's \
                                build target for the project's crates, and to the host's for \
                                other crates.",
            },
        },
    }));
    Tool::new(NAME, DESCRIPTION, input_schema)
        .with_title("Build docs of code outside the project")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::external_annotations())
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
                    "source": { "type": "string", "enum": ["project", "docs.rs", "local"] },
                    "package_source": { "type": "string", "enum": ["crates.io"] },
                    "workspace": { "type": "string" },
                    "project_from": {
                        "type": "string",
                        "enum": ["argument", "claude_project_dir", "cwd"],
                    },
                    "crate": { "type": "string" },
                    "version": { "type": "string" },
                    "target": { "type": "string" },
                    "toolchain": { "type": "string" },
                    "built_at": { "type": "string" },
                    "features_requested": strings,
                    "features_effective": strings,
                    "recorded_by": { "type": "string", "enum": ["build_external_docs"] },
                },
                "required": ["source"],
            },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "built": { "type": "boolean" },
                    "reason": {
                        "type": "string",
                        "enum": ["query_tools_build", "docs_rs"],
                        "description": "Why nothing was built: the other tools build it \
                                        themselves, or docs.rs has it.",
                    },
                    "crate": { "type": "string" },
                    "version": { "type": "string" },
                    "target": { "type": "string" },
                },
                "required": ["built"],
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

/// Checks the arguments (requirement R-ARG-1): `crate` or `project`, and not std, which is
/// documented from the Rust distribution.
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<Arguments, InvalidArgument> {
    let krate = get_item::parse_crate(arguments)?;
    let (project, target) = get_item::parse_project_and_target(arguments)?;
    if let Some((name, _)) = &krate {
        if get_item::std_crate(name).is_some() {
            return Err(InvalidArgument::new(
                format!(
                    "{name} is documented from the Rust distribution, not built here; look it up \
                     with get_item"
                ),
                &[r#"call get_item, for example {"crate": "std", "path": "std::vec::Vec"}"#],
            ));
        }
    } else if project.is_none() {
        return Err(InvalidArgument::new(
            "say what to document: `crate` or `project`",
            WHAT_EXAMPLE,
        ));
    }
    if krate.is_none() && arguments.is_some_and(|arguments| arguments.contains_key("version")) {
        return Err(InvalidArgument::new(
            "`version` is the version of `crate`, which is not given",
            WHAT_EXAMPLE,
        ));
    }
    Ok(Arguments {
        krate,
        project,
        target,
    })
}

/// Runs the tool.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    let Arguments {
        krate,
        project,
        target,
    } = arguments;
    match (krate, project) {
        (None, Some(project)) => workspace(&state, &project, target.as_ref()).await,
        (Some((name, version)), None) => {
            super::external_build::release(state, name, version, target).await
        },
        (Some(_), Some(_)) | (None, None) => not_yet(
            "a crate of a workspace",
            "look it up with get_item, which reads docs.rs",
        ),
    }
}

/// Documents the workspace `project` for `target`: nothing to build when it is the default
/// project's, for a target that it builds for, which the query tools build themselves (requirement
/// R-WS-10).
async fn workspace(
    state: &ToolState,
    project: &Path,
    target: Option<&TargetTriple>,
) -> CallToolResult {
    let default = state.project.as_ref();
    let path = project_path(default, project);
    let workspace = match project::workspace_of(&path) {
        Ok(workspace) => workspace,
        Err(error) => return project_not_found(&path, &error),
    };
    if !default.is_some_and(|default| default.contains(&workspace)) {
        return not_yet(
            "a workspace outside the default project",
            "for a crate from crates.io that it uses, call get_item with crate@version, which \
             reads docs.rs",
        );
    }
    // A project that builds its own std is not documented here (requirement R-TG-1).
    let targets = match targets_built(state, &workspace.root, "the project's crates").await {
        Ok(targets) => targets,
        Err(error) => return *error,
    };
    let built = target.map_or_else(
        || targets.first(),
        |target| targets.iter().find(|built| *built == target),
    );
    let Some(target) = built else {
        return not_yet(
            "the default project for another target",
            "for a dependency from crates.io, call get_item with the target, which reads docs.rs",
        );
    };
    query_tools_build(&workspace, target)
}

/// The response for the default project's workspace for a target that it builds for: the query
/// tools build its members themselves, so nothing is built (requirement R-WS-10).
fn query_tools_build(workspace: &Workspace, target: &TargetTriple) -> CallToolResult {
    let root = workspace.root.display().to_string();
    CallToolResult::structured(json!({
        "provenance": {
            "source": "project",
            "workspace": root,
            "project_from": ProjectFrom::Argument.as_str(),
            "target": target.as_str(),
        },
        "data": {
            "built": false,
            "reason": "query_tools_build",
        },
        "hints": [
            format!(
                "nothing was built: the other tools build this project's crates for {target} \
                 themselves; look them up with crate_overview or get_item, passing {}, and list \
                 them with list_crates",
                json!({ "project": root })
            ),
        ],
    }))
}

/// `project_not_found` (§8) for `path`, which has no workspace (decided in M2e, 5: a workspace
/// that has a record is told stale instead, from E4b).
fn project_not_found(path: &Path, error: &ProjectError) -> CallToolResult {
    error_result(&json!({
        "code": "project_not_found",
        "message": format!(
            "no workspace to document at {}: {}",
            path.display(),
            ErrorChain(error)
        ),
        "retryable": false,
        "suggestions": [
            "pass `project` with the directory or the Cargo.toml of a workspace",
        ],
    }))
}

/// `docs_unavailable` for `what`, which this tool documents in a later version (M2e), with what
/// may do instead meanwhile.
pub(super) fn not_yet(what: &str, instead: &str) -> CallToolResult {
    error_result(&json!({
        "code": "docs_unavailable",
        "message": format!("documenting {what} is not supported yet (in a later version)"),
        "retryable": false,
        "suggestions": [instead],
        "reason": "build_unsupported",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &Value) -> Result<Arguments, InvalidArgument> {
        parse_arguments(arguments.as_object())
    }

    #[test]
    fn it_needs_a_crate_or_a_workspace() {
        parse(&json!({})).expect_err("nothing to build");
        parse(&json!({ "target": "x86_64-unknown-linux-gnu" })).expect_err("nothing to build");
        let arguments = parse(&json!({ "project": "../other" })).expect("valid");
        assert_eq!(arguments.project, Some(PathBuf::from("../other")));
        let arguments = parse(&json!({ "crate": "rand", "version": "0.9.2" })).expect("valid");
        let (name, version) = arguments.krate.expect("a crate");
        assert_eq!(name.as_str(), "rand");
        assert_eq!(version, Some("0.9.2".parse().expect("a version")));
    }

    #[test]
    fn it_takes_neither_std_nor_a_version_without_a_crate() {
        for name in ["std", "core", "alloc", "proc_macro", "test", "proc-macro"] {
            let error = parse(&json!({ "crate": name })).expect_err("std is not built");
            assert!(error.message.contains("get_item"), "{}", error.message);
        }
        let error = parse(&json!({ "project": ".", "version": "1" })).expect_err("no crate");
        assert!(error.message.contains("`version`"), "{}", error.message);
        parse(&json!({ "crate": "rand@0.9", "version": "0.8" })).expect_err("two versions");
    }
}
