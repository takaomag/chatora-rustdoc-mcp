//! Where a package built here is, and what its files are judged under: the project's members,
//! path and git dependencies, and the crates of git that `build_external_docs` recorded, for
//! `get_readme` and `find_examples` (requirement R-SC-4, decided in M3e, decisions 78 to 80, 82 and
//! 86). No cargo is run but what resolved the version; copies of crates.io and other registries are
//! not read (decision 79).

use std::path::{Path, PathBuf};

use rmcp::model::CallToolResult;

use super::{
    ErrorChain,
    ToolState,
    get_item,
    get_source,
    project::{self, Found},
    recorded_git::GitFound,
    resolve_version::Release,
};
use crate::data::{
    examples::{self, Example},
    flight::Joined,
    metadata::Resolution,
    packages::Source,
};

/// Where a package is, and what of it `cargo metadata` tells.
#[derive(Debug)]
pub(super) struct Here {
    /// The directory of its `Cargo.toml`.
    pub(super) dir: PathBuf,
    /// What its files are judged under.
    pub(super) root: HereRoot,
    /// Its readme as `cargo metadata` resolved it, relative to `dir`; else its manifest is read.
    pub(super) readme: Option<PathBuf>,
    /// Its examples as `cargo metadata` tells its targets; else they are found in its directory
    /// (decided in M3e, decisions 83 and 86).
    pub(super) examples: Option<Vec<Example>>,
}

/// What the files of a package here are judged under (requirement R-SC-4).
#[derive(Debug)]
pub(super) enum HereRoot {
    /// A path package, under the root of its workspace: the project's, or else none known, its own
    /// directory unless what is read says otherwise (decided in M3e, decision 78).
    Package { workspace: Option<PathBuf> },
    /// A git package, in the checkout that cargo finished (decided in M3e, decision 80).
    Checkout(PathBuf),
}

/// Why a package's files are not read where it is.
#[derive(Debug)]
pub(super) enum NotHere {
    /// The answer to give instead: while cargo reads the project, or an error.
    Answer(Box<CallToolResult>),
    /// The project uses a copy from this source, whose files may differ from the release's
    /// (decided in M3e, decision 79, as in M3c, decision 31).
    Copy(String),
}

/// Where the crate of git `git` is, as `build_external_docs` recorded it, in its checkout.
///
/// # Errors
///
/// Returns the tool error when the record is too old to tell where its package is, or the
/// checkout is not there any more.
pub(super) fn of_git(state: &ToolState, git: &GitFound) -> Result<Here, Box<CallToolResult>> {
    let build_again = |message: String| {
        Box::new(super::tool_error("docs_unavailable", &message, false, &[
            "build it again with build_external_docs",
        ]))
    };
    let Some(dir) = git.built.record.package_root.clone() else {
        return Err(build_again(format!(
            "the record of {} is from before where its package is was kept",
            git.package
        )));
    };
    let checkout = state
        .resolver
        .cargo_home()
        .and_then(|home| get_source::checkout_of(&dir, home))
        .ok_or_else(|| {
            build_again(format!(
                "the checkout of {} that was built is not there any more",
                git.package
            ))
        })?;
    Ok(Here {
        dir,
        root: HereRoot::Checkout(checkout),
        readme: None,
        examples: None,
    })
}

/// Where the project's package `found` of `release` is, from the project's `cargo metadata`,
/// waiting for it until `until`. `noun` names what is read (`readme`), and `suggestion` what to
/// call when it is not known.
///
/// # Errors
///
/// Returns the answer while cargo reads the project, or the error when it could not be read or
/// does not have the package; or that the project uses a copy, which is not read.
pub(super) async fn of_project(
    state: &ToolState,
    (found, release): (&Found, &Release),
    until: tokio::time::Instant,
    (noun, suggestion): (&str, &str),
) -> Result<Here, NotHere> {
    let what = format!("{} {}", release.name, release.version);
    let not_known = |why: String| {
        NotHere::Answer(Box::new(super::tool_error(
            "docs_unavailable",
            &format!("the {noun} of {what} cannot be found: {why}"),
            false,
            &[suggestion],
        )))
    };
    let resolution = match project::resolve_until(state, &found.workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return Err(not_known(format!(
                "the project could not be read: {}",
                ErrorChain(&error)
            )));
        },
        Joined::Going(doing) => {
            let target = match release.target() {
                Some(target) => target,
                None => state.host().await.triple.clone(),
            };
            return Err(NotHere::Answer(Box::new(project::resolving_crate(
                &release.name,
                &target,
                (doing, found.project_from),
                &release.warnings,
            ))));
        },
        Joined::Ended => return Err(NotHere::Answer(Box::new(get_item::stopped()))),
    };
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return Err(not_known("cargo metadata of the project failed".to_owned()));
    };
    let Some(listed) = project::metadata_package(metadata, &found.package) else {
        return Err(not_known(
            "it is not in the project's cargo metadata".to_owned(),
        ));
    };
    let dir = listed
        .manifest_path
        .parent()
        .map_or_default(Path::to_path_buf);
    let root = match &found.package.source {
        Source::Local => HereRoot::Package {
            workspace: dir
                .starts_with(&metadata.workspace_root)
                .then(|| metadata.workspace_root.clone()),
        },
        Source::Git(_) => {
            let checkout = state
                .resolver
                .cargo_home()
                .and_then(|home| get_source::checkout_of(&dir, home));
            match checkout {
                Some(checkout) => HereRoot::Checkout(checkout),
                None => {
                    return Err(NotHere::Copy(
                        "a vendored copy of a git repository".to_owned(),
                    ));
                },
            }
        },
        Source::CratesIo => {
            return Err(NotHere::Copy(
                "a copy of crates.io's, vendored or from a source that replaces crates.io, which \
                 may differ from it"
                    .to_owned(),
            ));
        },
        Source::Registry(registry) => {
            return Err(NotHere::Copy(format!("the registry {registry}")));
        },
    };
    // A record that did not keep the targets' files: they are found in its directory (decided in
    // M3e, decision 86).
    let example_targets = || {
        listed
            .targets
            .iter()
            .filter(|target| target.kind.iter().any(|kind| &**kind == "example"))
    };
    let examples = (!example_targets().any(|target| target.src_path.as_os_str().is_empty()))
        .then(|| examples::of_targets(&listed.targets, &dir));
    Ok(Here {
        dir,
        root,
        readme: listed.readme.clone(),
        examples,
    })
}

/// Whether the project's package `found` is one of crates.io that cargo unpacked as it is, whose
/// `.crate` is read (decided in M3e, decision 79, as in M3c, decision 31).
pub(super) fn unpacked_from_crates_io(state: &ToolState, found: &Found) -> bool {
    matches!(found.package.source, Source::CratesIo)
        && found
            .package
            .manifest_path
            .as_deref()
            .and_then(Path::parent)
            .zip(state.resolver.cargo_home())
            .is_some_and(|(dir, home)| get_source::unpacked_from_crates_io(dir, home))
}
