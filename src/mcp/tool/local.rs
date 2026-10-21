//! Lookups in the workspace's members, whose documentation is built here (decided in M2's D2):
//! which member a crate name is, the shared work that builds it, and how its progress and its
//! failures are told (requirements R-WS-6, R-WS-7 and R-OUT-9).

use std::{path::Path, sync::Arc};

use rmcp::model::CallToolResult;
use semver::Version;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{ToolState, error_result, get_item, project};
use crate::{
    data::{
        build::{self, Bootstrap, BuildError, Plan, TargetDir},
        cargo::{Failure, RunError},
        crate_name::CrateName,
        flight::{Doing, Joined},
        metadata::{CargoError, Resolution},
        project::Workspace,
        rustdoc_json::ParseError,
        target::TargetTriple,
        toolchain::{ChooseError, Missing},
    },
    query::{
        load::LoadError,
        local::{LocalBuilt, LocalError, LocalWanted, local_archive},
    },
};

/// A workspace member's crate that a lookup is in: what it is built from, until it is built.
#[derive(Debug)]
pub(super) enum LocalSource {
    /// To build.
    Wanted(Box<LocalWanted>),
    /// Built, for the lookups in it after the first, as when a re-export is not followed.
    Built(Arc<LocalBuilt>),
}

/// What a lookup asks for, to tell whether it is in a workspace member.
#[derive(Debug, Clone, Copy)]
pub(super) enum Asked<'a> {
    /// The project's package `name` at `version`, which the project resolved the crate name to.
    Package {
        /// Its name.
        name: &'a str,
        /// Its version.
        version: &'a Version,
    },
    /// The crate at `version`, given with the name (requirement R-VR-1).
    Explicit {
        /// The version.
        version: &'a Version,
    },
}

/// What a crate name is in a workspace.
#[derive(Debug)]
pub(super) enum Member {
    /// A member's crate, to build, of the package `package`. Its version is the plan's.
    Is {
        /// What builds it.
        wanted: Box<LocalWanted>,
        /// The member's package name, which its features are found by.
        package: Box<str>,
    },
    /// A member's crate at another version than the one asked for.
    OtherVersion(Version),
    /// Not a member's crate.
    Not,
    /// Whether it is a member's cannot be told, as `cargo metadata` failed: the error to return
    /// when the project's package is not from crates.io (requirement R-VR-2).
    Unknown(Box<CallToolResult>),
    /// cargo is still resolving the packages, past the deadline of the call, as this says.
    Resolving(Doing),
}

/// What the crate `name`, as `asked`, is in `workspace`, to build for `target`, waiting for
/// `cargo metadata` until `until`.
pub(super) async fn member(
    state: &ToolState,
    workspace: &Workspace,
    (name, asked): (&CrateName, Asked<'_>),
    target: &TargetTriple,
    until: Instant,
) -> Result<Member, Box<CallToolResult>> {
    let resolution = match state.resolver.resolve_until(workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return Err(Box::new(project::resolve_error(workspace, &error)));
        },
        Joined::Ended => return Err(Box::new(get_item::stopped())),
        Joined::Going(doing) => return Ok(Member::Resolving(doing)),
    };
    let metadata = match &*resolution {
        Resolution::Metadata { metadata, .. } => metadata,
        Resolution::CargoLockRaw { because, .. } => {
            return Ok(Member::Unknown(Box::new(metadata_failed(name, because))));
        },
    };
    let crate_name = name.as_str().replace('-', "_");
    let found = match asked {
        Asked::Package { name, version } => build::member_package(metadata, name, version)
            .and_then(|package| Some((package, build::crate_of(package, &crate_name)?))),
        Asked::Explicit { version } => match build::member_crate(metadata, &crate_name) {
            Some((package, _)) if package.version != *version => {
                return Ok(Member::OtherVersion(package.version.clone()));
            },
            found => found,
        },
    };
    let Some((package, documented)) = found else {
        return Ok(Member::Not);
    };
    let root = &workspace.root;
    // The member's features in the build of the whole workspace, as rust-analyzer checks it and
    // `enabled_in_project` tells (requirement R-FT-3, decided in M2's D3), or else cargo's default.
    let mut warnings: Vec<Box<str>> = Vec::new();
    let features = match state.resolver.features(workspace, target).await {
        Ok(sets) => sets
            .normal(&package.name, &package.version)
            .map(|features| build::declared_features(package, features)),
        Err(error) => {
            tracing::debug!(%error, "the features of the workspace could not be read");
            None
        },
    };
    if features.is_none() {
        warnings.push(
            format!(
                "the features of {} in the build of the workspace could not be read, so it is \
                 built with cargo's default for it alone",
                package.name
            )
            .into(),
        );
    }
    let plan = Plan {
        root: root.clone(),
        offline: state.resolver.offline_in(root).await,
        toolchain: None,
        bootstrap: Bootstrap::of(metadata),
        target_dir: TargetDir::of(state.cache.dir(), root),
        package: package.id.clone(),
        manifest: package.manifest_path.clone(),
        crate_name: build::documented_name(package, &documented).into(),
        documented,
        version: package.version.clone(),
        target: None,
        cfg_docsrs: build::cfg_docsrs(),
        features,
    };
    Ok(Member::Is {
        wanted: Box::new(LocalWanted {
            plan,
            target: target.clone(),
            project_sets_target: project::sets_build_target(state, root).await,
            cargo_home: state.resolver.cargo_home().map(Path::to_path_buf),
            warnings,
        }),
        package: package.name.clone(),
    })
}

/// The error when the project's package `name`, not from crates.io, cannot be built, as `cargo
/// metadata` failed (requirements R-VR-2 and R-WS-7).
fn metadata_failed(name: &CrateName, because: &CargoError) -> CallToolResult {
    let failure = match because {
        CargoError::Failed { failure, .. } => *failure,
        CargoError::Run(_) | CargoError::Unreadable(_) => Failure::Other,
    };
    let mut body = json!({
        "code": project::failure_name(failure),
        "message": format!(
            "{name} is not from crates.io, so its documentation is built here, but cargo metadata \
             failed: {because}"
        ),
        // A failure that its messages do not tell apart may pass (requirement R-WS-7).
        "retryable": matches!(failure, Failure::Killed | Failure::Other),
        "suggestions": ["run `cargo metadata` in the project's terminal to see why it fails"],
    });
    if let (Failure::GitFetch(reason), Some(fields)) = (failure, body.as_object_mut()) {
        fields.insert("reason".to_owned(), json!(reason.as_str()));
    }
    error_result(&body)
}

/// The build of `wanted`, by work that the calls for it share, waiting for it until `until`; or
/// the response to return: `status: building` while it goes on, with `provenance` and `warnings`,
/// or the error.
///
/// `until` is `(build, call)`: a call waits for the build until the first, and for indexing what
/// it built until the second, the deadline of the call, as for any work but a build (requirement
/// R-WS-6, decided in M2's D3).
pub(super) async fn built(
    state: &ToolState,
    wanted: LocalWanted,
    (until_built, until): (Instant, Instant),
    provenance: Map<String, Value>,
    warnings: &[Box<str>],
) -> Result<Arc<LocalBuilt>, Box<CallToolResult>> {
    let what = format!("{} {}", wanted.plan.crate_name, wanted.plan.version);
    let mut joined = local_archive(&state.locals, &state.cache, wanted.clone(), until_built).await;
    if matches!(joined, Joined::Going(Doing::Indexing)) && until_built < until {
        joined = local_archive(&state.locals, &state.cache, wanted, until).await;
    }
    match joined {
        Joined::Done(Ok(built)) => Ok(built),
        Joined::Done(Err(error)) => Err(Box::new(local_error(&what, &error))),
        Joined::Ended => Err(Box::new(get_item::stopped())),
        Joined::Going(doing) => Err(Box::new(get_item::building_response(
            provenance,
            doing,
            &get_item::being(&what, doing, "the registry"),
            warnings,
        ))),
    }
}

/// The tool error of a member's build that failed (requirement R-WS-7, decided in M2's D2).
fn local_error(what: &str, error: &LocalError) -> CallToolResult {
    let failed = |message: String, retryable: bool, suggestions: &[&str]| {
        json!({
            "code": "build_failed",
            "message": message,
            "retryable": retryable,
            "suggestions": suggestions,
        })
    };
    let missing = |message: String, install: String| {
        json!({
            "code": "toolchain_missing",
            "message": message,
            "retryable": false,
            "suggestions": [install],
        })
    };
    let body = match error {
        LocalError::Programs(problem) => failed(
            format!("the documentation of {what} cannot be built: {problem}"),
            false,
            &[
                "fix build.cargo_path, build.rustc_path or build.rustdoc_path in the server's settings",
            ],
        ),
        LocalError::Work(error) => failed(
            format!("the documentation of {what} cannot be built: {error}"),
            false,
            &["check that the server's cache directory can be written and locked"],
        ),
        LocalError::Mismatch(mismatch) => failed(
            format!("the documentation of {what} cannot be built: {mismatch}"),
            false,
            &["make rustc and rustdoc those of one toolchain"],
        ),
        LocalError::Choose(
            ChooseError::NotInstalled(missing_one)
            | ChooseError::NightlyMissing {
                missing: missing_one,
                ..
            },
        )
        | LocalError::Missing(missing_one) => missing_toolchain(what, missing_one, &missing),
        LocalError::Choose(ChooseError::TooOld { found }) => unsupported(what, *found),
        LocalError::Choose(ChooseError::Failed(why)) => failed(
            format!("the toolchain that builds {what} could not be told: {why}"),
            false,
            &[],
        ),
        LocalError::Build(error) => return build_error(what, error),
        LocalError::Load(LoadError::Parse(ParseError::UnsupportedFormat { found, .. })) => {
            unsupported(what, *found)
        },
        LocalError::Load(LoadError::Read(error)) => failed(
            format!("the documentation of {what} could not be read: {error}"),
            true,
            &[],
        ),
        LocalError::Load(error) => failed(
            format!("the documentation of {what} could not be read: {error}"),
            false,
            &[],
        ),
        LocalError::Stopped => return get_item::stopped(),
    };
    error_result(&body)
}

/// `toolchain_missing` for what `missing` lacks, with what installs it.
fn missing_toolchain(
    what: &str,
    missing: &Missing,
    body: &impl Fn(String, String) -> Value,
) -> Value {
    match missing {
        Missing::NotInstalled { toolchain } => body(
            format!("the toolchain {toolchain} that builds {what} is not installed"),
            format!("rustup toolchain install {toolchain}"),
        ),
        Missing::Target { target } => body(
            format!("the toolchain that builds {what} has no standard library for {target}"),
            format!("rustup target add {target}"),
        ),
        Missing::NotRun(why) => json!({
            "code": "build_failed",
            "message": format!("the toolchain that builds {what} could not be run: {why}"),
            "retryable": false,
            "suggestions": [],
        }),
    }
}

/// `unsupported_format` for a toolchain that writes `found` (requirement R-WS-8).
fn unsupported(what: &str, found: u32) -> Value {
    let supported = crate::data::rustdoc_json::SUPPORTED;
    json!({
        "code": "unsupported_format",
        "message": format!(
            "the toolchain that builds {what} writes the rustdoc JSON format {found}, which this \
             server does not read ({} to {})",
            supported.start(),
            supported.end()
        ),
        "retryable": false,
        "suggestions": [
            "update the server, or the project's toolchain (rust-toolchain.toml)",
            "or install a nightly toolchain with rustup, which the server builds with when the \
             project's is too old",
        ],
        "found": found,
        "supported": [supported.start(), supported.end()],
    })
}

/// The tool error of a build that failed (requirement R-WS-7).
fn build_error(what: &str, error: &BuildError) -> CallToolResult {
    let body = match error {
        BuildError::Failed {
            failure,
            errors,
            stderr,
            ..
        } => {
            let mut body = json!({
                "code": project::failure_name(*failure),
                "message": format!("cargo failed to build the documentation of {what}"),
                // Killed, as by the OOM killer, it may pass (requirement R-WS-7).
                "retryable": *failure == Failure::Killed,
                "suggestions": ["run `cargo doc` in the project's terminal to see the error there"],
            });
            if let Some(fields) = body.as_object_mut() {
                if let Failure::GitFetch(reason) = failure {
                    fields.insert("reason".to_owned(), json!(reason.as_str()));
                }
                if errors.is_empty() {
                    fields.insert("stderr".to_owned(), json!(stderr));
                } else {
                    fields.insert("diagnostics".to_owned(), json!(errors));
                }
            }
            body
        },
        BuildError::Run(RunError::TimedOut(_)) => json!({
            "code": "build_failed",
            "message": format!("the build of the documentation of {what} took too long: {error}"),
            "retryable": false,
            "suggestions": [],
            "reason": "timeout",
        }),
        BuildError::Io(error) => json!({
            "code": "build_failed",
            "message": format!("the documentation of {what} could not be published: {error}"),
            "retryable": true,
            "suggestions": [],
        }),
        BuildError::Run(RunError::NotStarted(_))
        | BuildError::NoJson(_)
        | BuildError::Fresh(_)
        | BuildError::Unreadable(_)
        | BuildError::NotTheCrate { .. } => json!({
            "code": "build_failed",
            "message": format!("the documentation of {what} could not be built: {error}"),
            "retryable": false,
            "suggestions": [],
        }),
    };
    error_result(&body)
}
