//! The version of a crate in the project, when the caller gives none (requirements R-MCP-3 and
//! R-VR-2): the project is the default one, or the one the `project` argument names.

use std::path::Path;

#[cfg(feature = "local")]
use rmcp::model::CallToolResult;
#[cfg(feature = "local")]
use serde_json::json;

#[cfg(feature = "local")]
use super::{ErrorChain, ToolState, error_result};
use crate::data::crate_name::CrateName;
#[cfg(feature = "local")]
use crate::data::{
    cargo::Failure,
    metadata::{CargoError, Resolution, ResolveError},
    packages::{Candidate, Lookup, PackageGraph, ProjectPackage},
    project::{self, DefaultProject, ProjectFrom, Workspace},
};
#[cfg(feature = "local")]
use crate::query::get_item::ResolvedFrom;

/// What the project says about a crate.
#[derive(Debug)]
pub(super) enum InProject {
    /// The project resolves the crate to this package.
    #[cfg(feature = "local")]
    Found(Box<Found>),
    /// The project does not decide the version: there is no project, or the crate is not one of
    /// its packages. The latest version is used, with these warnings.
    Elsewhere(Vec<Box<str>>),
    /// The version cannot be decided: the tool error to return.
    #[cfg(feature = "local")]
    Error(Box<CallToolResult>),
}

/// A package of the project that a crate name means.
#[cfg(feature = "local")]
#[derive(Debug)]
pub(super) struct Found {
    /// The package, with its source and checksum.
    pub(super) package: ProjectPackage,
    /// `cargo_lock`, or `cargo_lock_raw` when `cargo metadata` failed.
    pub(super) resolved_from: ResolvedFrom,
    /// Where the project came from (§7.3).
    pub(super) project_from: &'static str,
    /// What the response warns about.
    pub(super) warnings: Vec<Box<str>>,
    /// The workspace, whose build decides `enabled_in_project` (requirement R-FT-2).
    pub(super) workspace: Workspace,
}

/// The package that `name` means in the project: the one `project` names (a `Cargo.toml` or a
/// directory, relative to the default project's directory), or else the default project.
#[cfg(feature = "local")]
pub(super) async fn version_in_project(
    state: &ToolState,
    name: &CrateName,
    project: Option<&Path>,
) -> InProject {
    let (workspace, project_from) = match workspace(state.project.as_ref(), project) {
        Ok(Some(found)) => found,
        Ok(None) => {
            return InProject::Elsewhere(vec![
                "there is no project here, so the latest release is used".into(),
            ]);
        },
        Err(error) => return InProject::Error(error),
    };
    let resolution = match state.resolver.resolve(&workspace).await {
        Ok(resolution) => resolution,
        Err(error) => return InProject::Error(Box::new(resolve_error(&workspace, &error))),
    };
    let mut warnings = Vec::new();
    let resolved_from = match &*resolution {
        Resolution::Metadata { .. } => ResolvedFrom::CargoLock,
        Resolution::CargoLockRaw { because, .. } => {
            warnings.extend(raw_warnings(because));
            ResolvedFrom::CargoLockRaw
        },
    };
    let graph = PackageGraph::new(&resolution);
    let package = match graph.look_up(name.as_str()) {
        Lookup::Found {
            package,
            other_versions,
        } => {
            if !other_versions.is_empty() {
                let others: Vec<String> = other_versions
                    .iter()
                    .map(|other| other.version.to_string())
                    .collect();
                warnings.push(
                    format!(
                        "the project also has {name} {}, which no member depends on directly; \
                         {} is shown",
                        others.join(", "),
                        package.version
                    )
                    .into(),
                );
            }
            package.clone()
        },
        Lookup::AmbiguousVersion(candidates) => {
            return InProject::Error(Box::new(ambiguous("ambiguous_version", name, &candidates)));
        },
        Lookup::AmbiguousPackage(candidates) => {
            return InProject::Error(Box::new(ambiguous("ambiguous_crate", name, &candidates)));
        },
        Lookup::NotFound => {
            return InProject::Elsewhere(vec![
                format!("{name} is not a dependency of the project, so the latest release is used")
                    .into(),
            ]);
        },
    };
    InProject::Found(Box::new(Found {
        package,
        resolved_from,
        project_from: project_from.as_str(),
        warnings,
        workspace,
    }))
}

/// Without local features, there is no project context (requirement R-ARCH-3).
#[cfg(not(feature = "local"))]
#[expect(
    clippy::unused_async,
    reason = "the same signature as with local features, where it runs cargo"
)]
pub(super) async fn version_in_project(
    _state: &super::ToolState,
    _name: &CrateName,
    project: Option<&Path>,
) -> InProject {
    let mut warnings: Vec<Box<str>> = vec![
        "this server is built without local features, so it does not read the project; the \
         latest release is used"
            .into(),
    ];
    if project.is_some() {
        warnings.push("`project` is ignored".into());
    }
    InProject::Elsewhere(warnings)
}

/// The workspace to read and where it came from: the one `project` names, or the default
/// project's only one. `None` when there is no project. A project outside the default one is not
/// read with cargo (requirement R-MCP-3), so it is an error until `build_external_docs` (M2).
#[cfg(feature = "local")]
fn workspace(
    default: Option<&DefaultProject>,
    project: Option<&Path>,
) -> Result<Option<(Workspace, ProjectFrom)>, Box<CallToolResult>> {
    if let Some(project) = project {
        let path = match default {
            Some(default) if project.is_relative() => default.dir.join(project),
            Some(_) | None => project.to_path_buf(),
        };
        let workspace = project::workspace_of(&path).map_err(|error| {
            Box::new(project_not_found(
                &format!("{}: {}", path.display(), ErrorChain(&error)),
                &[],
            ))
        })?;
        if default.is_some_and(|default| default.contains(&workspace)) {
            return Ok(Some((workspace, ProjectFrom::Argument)));
        }
        return Err(Box::new(error_result(&json!({
            "code": "docs_unavailable",
            "message": format!(
                "{} is outside the default project, whose dependencies are not resolved with \
                 cargo; give the version, or document it with build_external_docs (in a later \
                 version)",
                workspace.root.display()
            ),
            "retryable": false,
            "suggestions": ["give the version, as crate@version"],
            "reason": "no_record",
        }))));
    }
    let Some(default) = default else {
        return Ok(None);
    };
    match &*default.workspaces {
        [] => Ok(None),
        [workspace] => Ok(Some((workspace.clone(), default.from))),
        workspaces => {
            let candidates: Vec<String> = workspaces
                .iter()
                .map(|workspace| workspace.root.display().to_string())
                .collect();
            Err(Box::new(project_not_found(
                &format!(
                    "{} has several workspaces directly inside it",
                    default.dir.display()
                ),
                &candidates,
            )))
        },
    }
}

/// `project_not_found` (§8), with the workspaces to choose from.
#[cfg(feature = "local")]
fn project_not_found(why: &str, candidates: &[String]) -> CallToolResult {
    error_result(&json!({
        "code": "project_not_found",
        "message": format!("the project to read the version from is not known: {why}"),
        "retryable": false,
        "suggestions": [
            "pass `project` with one of the candidates, or give the version, as crate@version",
        ],
        "candidates": candidates,
    }))
}

/// The tool error for a workspace whose packages cannot be resolved.
#[cfg(feature = "local")]
fn resolve_error(workspace: &Workspace, error: &ResolveError) -> CallToolResult {
    let root = workspace.root.display();
    match error {
        ResolveError::NoCargoLock => error_result(&json!({
            "code": "lockfile_outdated",
            "message": format!("{root} has no Cargo.lock, which this server does not write"),
            "retryable": false,
            "suggestions": [
                "run `cargo fetch` in the workspace, which writes Cargo.lock without building",
                "or give the version, as crate@version",
            ],
        })),
        ResolveError::Unreadable(_) | ResolveError::Malformed(_) => error_result(&json!({
            "code": "lockfile_outdated",
            "message": format!("the Cargo.lock of {root} cannot be read: {}", ErrorChain(error)),
            "retryable": false,
            "suggestions": ["check Cargo.lock with `cargo fetch`", "or give the version, as crate@version"],
        })),
    }
}

/// The warnings when the versions come from Cargo.lock alone, because `cargo metadata` failed
/// (requirement R-VR-2).
#[cfg(feature = "local")]
fn raw_warnings(because: &CargoError) -> Vec<Box<str>> {
    let failure = match because {
        CargoError::Failed { failure, .. } => *failure,
        CargoError::Run(_) | CargoError::Unreadable(_) => Failure::Other,
    };
    let mut warnings: Vec<Box<str>> = vec![
        format!(
            "cargo metadata failed ({}), so the version is read from Cargo.lock directly",
            failure_name(failure)
        )
        .into(),
    ];
    if failure == Failure::LockfileOutdated {
        warnings.push(
            "Cargo.lock does not match Cargo.toml, so the version may be out of date; run \
             `cargo fetch` to update Cargo.lock"
                .into(),
        );
    }
    warnings
}

/// The name of a kind of failure, as the error codes write it.
#[cfg(feature = "local")]
const fn failure_name(failure: Failure) -> &'static str {
    match failure {
        Failure::LockfileOutdated => "lockfile_outdated",
        Failure::OfflineMiss => "offline_miss",
        Failure::GitFetch(_) => "fetch_failed",
        Failure::Killed | Failure::Other => "build_failed",
    }
}

/// `ambiguous_version` or `ambiguous_crate` (§8), with each candidate and what uses it.
#[cfg(feature = "local")]
fn ambiguous(code: &str, name: &CrateName, candidates: &[Candidate<'_>]) -> CallToolResult {
    let (message, suggestion) = if code == "ambiguous_crate" {
        (
            format!("{name} is a dependency's name in code and another package's name"),
            "ask with the package name of one of the candidates",
        )
    } else {
        (
            format!(
                "the project has several versions of {name}, and no single one is a direct dependency"
            ),
            "give the version of one of the candidates, as crate@version",
        )
    };
    error_result(&json!({
        "code": code,
        "message": message,
        "retryable": false,
        "suggestions": [suggestion],
        "candidates": candidates
            .iter()
            .map(|candidate| json!({
                "crate": format!("{}@{}", candidate.package.name, candidate.package.version),
                "used_by": candidate
                    .used_by
                    .iter()
                    .map(|user| format!("{}@{}", user.name, user.version))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    }))
}

#[cfg(feature = "local")]
#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A directory with a package `name` in it.
    fn package(dir: &Path, name: &str) {
        fs::create_dir_all(dir).expect("made");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .expect("written");
    }

    fn code(error: &CallToolResult) -> serde_json::Value {
        let text = error
            .content
            .first()
            .and_then(|content| content.as_text())
            .map(|text| text.text.clone())
            .expect("a text block");
        serde_json::from_str(&text).expect("JSON")
    }

    #[test]
    fn the_project_is_the_default_one_or_the_one_named_inside_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        package(&dir.path().join("a"), "a");
        package(&dir.path().join("b"), "b");
        let default =
            DefaultProject::find(None, Some(dir.path().to_path_buf())).expect("a directory");

        // Two workspaces directly inside: one must be named.
        let error = workspace(Some(&default), None).expect_err("ambiguous");
        assert_eq!(code(&error)["code"], "project_not_found");
        assert_eq!(code(&error)["candidates"].as_array().map(Vec::len), Some(2));

        let (named, from) = workspace(Some(&default), Some(Path::new("b")))
            .expect("inside")
            .expect("found");
        assert!(named.root.ends_with("b"));
        assert_eq!(from, ProjectFrom::Argument);

        // A workspace outside the default project is not read with cargo.
        let outside = tempfile::tempdir().expect("a temporary directory");
        package(outside.path(), "c");
        let error = workspace(Some(&default), Some(outside.path())).expect_err("outside");
        assert_eq!(code(&error)["reason"], "no_record");

        assert!(workspace(None, None).expect("no error").is_none());
    }
}
