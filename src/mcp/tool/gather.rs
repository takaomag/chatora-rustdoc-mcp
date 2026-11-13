//! Loading or building many crates at once, up to a time limit, for the tools that look across
//! them: the search of a project's crates (requirements R-SR-2 and R-SR-5) and the implementors of
//! a trait across the dependency graph (requirement R-TI-3, decided in M4a).
//!
//! Those still being fetched, indexed or built when the call stops waiting are told as going; their
//! loading goes on, for the next call (requirement R-ARCH-6).

use std::sync::Arc;

#[cfg(feature = "local")]
use rmcp::model::CallToolResult;
use semver::Version;
use serde_json::Value;
#[cfg(feature = "local")]
use serde_json::{Map, json};
#[cfg(feature = "local")]
use tokio::time::Instant;

#[cfg(feature = "local")]
use super::{
    ErrorChain,
    ToolState,
    get_item,
    local::{self, Member as Planned},
    recorded_workspace,
};
use crate::{
    data::{archive::Mapped, crate_name::CrateName, flight::Doing},
    query::across::Indexed,
};
#[cfg(feature = "local")]
use crate::{
    data::{
        features::FeatureSets,
        flight::Joined,
        packages::ProjectPackage,
        project::Workspace,
        target::TargetTriple,
    },
    query::{
        load::{self, ArchiveError},
        local::LocalBuilt,
        view::CrateView,
    },
};

/// A crate of the scope, and how it fared.
pub(super) struct Member {
    pub(super) name: CrateName,
    pub(super) version: Version,
    pub(super) outcome: Outcome,
}

impl Member {
    /// The root of its paths: the name of its crate built here, which a lib's name may make other
    /// than its package's, or else its name.
    pub(super) fn root(&self) -> &str {
        #[cfg(feature = "local")]
        if let Outcome::Built(built) | Outcome::Cached(built) | Outcome::Recorded(built, _) =
            &self.outcome
        {
            return &built.record.crate_name;
        }
        self.name.as_str()
    }
}

/// How loading a crate of the scope fared.
pub(super) enum Outcome {
    /// Its archive, to search.
    Loaded(Arc<Mapped<Indexed>>),
    /// It could not be loaded: the code, the reason and whether trying again may help (§8).
    Failed(Value),
    /// It was still being fetched, indexed or built, as this says, when the search stopped
    /// waiting.
    #[cfg_attr(
        not(feature = "local"),
        expect(
            dead_code,
            reason = "only the dependencies of a project are fetched at once"
        )
    )]
    Going(Doing),
    /// Built before, a dependency's, read as it is without looking at whether its inputs
    /// changed, which is not built again by this call (decided in M4's plan, J2, and in M4a,
    /// decision 4).
    #[cfg(feature = "local")]
    Cached(Arc<LocalBuilt>),
    /// Built here, a workspace member's or a dependency's (decided in M2's D7).
    #[cfg(feature = "local")]
    Built(Arc<LocalBuilt>),
    /// Built by `build_external_docs`, a crate of a workspace outside the default project, with
    /// whether what it is made from changed since (decided in M2e's E4b-3).
    #[cfg(feature = "local")]
    Recorded(Arc<LocalBuilt>, bool),
}

impl Outcome {
    /// Whether it was built here (decided in M2's D7).
    #[cfg_attr(
        not(feature = "local"),
        expect(
            clippy::unused_self,
            reason = "without local features, nothing is built here"
        )
    )]
    pub(super) const fn is_built(&self) -> bool {
        #[cfg(feature = "local")]
        if let Self::Built(_) | Self::Cached(_) | Self::Recorded(..) = self {
            return true;
        }
        false
    }
}

/// The features of the packages in the build of `workspace` for `target`, waited for until
/// `until`, or `None` with a warning: without them, all direct dependencies are searched.
#[cfg(feature = "local")]
pub(super) async fn build_sets(
    state: &ToolState,
    workspace: &Workspace,
    target: &TargetTriple,
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Option<Arc<FeatureSets>> {
    match super::project::features_until(state, workspace, target, until).await {
        Joined::Done(Ok(sets)) => Some(sets),
        Joined::Done(Err(error)) => {
            warnings.push(
                format!(
                    "which dependencies the project builds for {target} is not known, so none is \
                     left out for it: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
        Joined::Ended => {
            warnings.push(
                format!(
                    "which dependencies the project builds for {target} is not known, so none is \
                     left out for it: {}",
                    get_item::NOT_READ
                )
                .into(),
            );
            None
        },
        Joined::Going(doing) => {
            warnings.push(
                format!(
                    "which dependencies the project builds for {target} is not known yet, so \
                     none is left out for it: {}",
                    get_item::still_reading(doing)
                )
                .into(),
            );
            None
        },
    }
}

/// When to ask again for the crates still being fetched or indexed.
#[cfg(feature = "local")]
const RETRY_AFTER_SECS: u64 = get_item::RETRY_AFTER_SECS;

/// What a crate that was not read is, for the `sources` of the response (requirement R-OUT-9):
/// `unavailable` when it cannot be documented here, `failed`, or `building` while it is fetched,
/// indexed or built (decided in M2's D7). `None` for one that was read.
#[cfg(feature = "local")]
pub(super) fn unread_source(member: &Member) -> Option<Value> {
    let mut source = Map::new();
    source.insert("crate".to_owned(), json!(member.name.as_str()));
    source.insert("version".to_owned(), json!(member.version.to_string()));
    match &member.outcome {
        Outcome::Failed(failure) => {
            let unavailable = failure
                .get("reason")
                .and_then(Value::as_str)
                .is_some_and(|reason| {
                    matches!(
                        reason,
                        "local_build_required"
                            | "too_many"
                            | "build_unsupported"
                            | "target_unsupported"
                            | "scope_unknown"
                            | "no_json"
                            | "no_target_json"
                            | "no_record"
                    )
                });
            let status = if unavailable { "unavailable" } else { "failed" };
            source.insert("status".to_owned(), json!(status));
            if let Some(failure) = failure.as_object() {
                source.extend(failure.clone());
            }
        },
        Outcome::Going(doing) => {
            source.insert("status".to_owned(), json!("building"));
            source.insert("building_reason".to_owned(), json!(doing.as_str()));
            source.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
        },
        Outcome::Loaded(_) | Outcome::Built(_) | Outcome::Cached(_) | Outcome::Recorded(..) => {
            return None;
        },
    }
    Some(Value::Object(source))
}

/// A crate whose loading or looking at ended without an answer: failed, to be asked for again.
#[cfg(feature = "local")]
fn ended(name: CrateName, version: Version) -> Member {
    Member {
        name,
        version,
        outcome: Outcome::Failed(json!({ "code": "internal_error", "retryable": true })),
    }
}

/// Why the crates still being loaded, doing `going`, are not in yet: `indexing` when any is
/// being indexed, as that is the last step (requirement R-OUT-9).
pub(super) fn building_reason<'a>(going: impl Iterator<Item = &'a Doing>) -> &'static str {
    let going: Vec<&Doing> = going.collect();
    // What docs.rs is read for first, then what is built here (decided in M2's D7).
    [
        Doing::Indexing,
        Doing::Fetching,
        Doing::Building,
        Doing::WaitingForSlot,
        Doing::WaitingForLock,
    ]
    .into_iter()
    .find(|doing| going.contains(&doing))
    .unwrap_or(Doing::Fetching)
    .as_str()
}

/// Whether the crates that are not built yet may be built here.
#[cfg(feature = "local")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Builds {
    /// They are built, as the lookups of the project do.
    Allow,
    /// A dependency is not: one built before is read, as stale, or else it is told as
    /// `local_build_required` (decided in M4's plan, J2, and in M4a, decision 4). A workspace member
    /// is built still (decided in M4a, decision 2).
    CachedOnly,
}

/// Most crates that one call loads, past which the others are told as too many (decided in M4a,
/// decision 10).
#[cfg(feature = "local")]
pub(super) const MAX_CRATES: usize = 300;

/// [`MAX_CRATES`], or, in a debug build, what the tests set in its place
/// ([`crate::testing::max_crates`]).
#[cfg(all(feature = "local", debug_assertions))]
pub(super) fn max_crates() -> usize {
    crate::testing::max_crates().unwrap_or(MAX_CRATES)
}

/// [`MAX_CRATES`].
#[cfg(all(feature = "local", not(debug_assertions)))]
pub(super) const fn max_crates() -> usize {
    MAX_CRATES
}

/// What a package of the scope of the search is to the project.
#[cfg(feature = "local")]
#[derive(Debug, Clone, Copy)]
pub(super) enum Kind {
    /// A workspace member, built here.
    Member,
    /// A dependency not from crates.io, built here, or read as built before
    /// ([`Builds::CachedOnly`]).
    Dependency,
    /// A dependency from crates.io: its release on docs.rs, or built here if it is a copy, or if
    /// docs.rs has no JSON of it (decided in M2's D7), or read as built before then
    /// ([`Builds::CachedOnly`]).
    CratesIo,
}

/// The packages of the scope of a search, with what they are to the project, and how they may be
/// loaded.
#[cfg(feature = "local")]
pub(super) type Scope = (Vec<(ProjectPackage, Kind)>, Builds);

/// Loads or builds the packages of `scope` of `workspace` at once, as `builds` lets it, waiting for
/// them until `until`, for `target`, whose JSON on docs.rs is of `docs_rs` (requirements R-SR-2 and
/// R-SR-5, decided in M2's D7). What has not finished then goes on, so that the next call has it.
/// A member that has no crate to search is left out. With [`Builds::CachedOnly`], those past
/// [`MAX_CRATES`], in the order of `scope`, are told as too many: as they are not built, whether
/// they have a crate is not known (decided in M4a, decision 10).
#[cfg(feature = "local")]
pub(super) async fn project_all(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    scope: Scope,
    at: (&TargetTriple, Option<&TargetTriple>),
    until: Instant,
    warnings: &mut Vec<Box<str>>,
) -> Vec<Member> {
    project_with(state, workspace, (scope, None), at, until, warnings, None).await
}

#[cfg(feature = "local")]
/// A crate that [`project_each`] read: how, and what the call made of it.
pub(super) struct Read<R> {
    /// Its name.
    pub(super) name: CrateName,
    /// Its version.
    pub(super) version: Version,
    /// Where its documentation came from.
    pub(super) how: How,
    /// What the call made of it.
    pub(super) result: R,
}

#[cfg(feature = "local")]
/// Where the documentation of a crate that [`project_each`] read came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum How {
    /// Its release on docs.rs.
    Release,
    /// Built here, made again while it is rebuilt if `stale` (decided in M2's D7).
    Built {
        /// It is the build before, while it is made again.
        stale: bool,
    },
    /// The build before, read as it is ([`Outcome::Cached`]).
    Cached,
    /// Recorded by `build_external_docs`, made from what changed since if `changed`.
    Recorded {
        /// What it is made from changed since.
        changed: bool,
    },
    /// The documentation of std of a nightly toolchain, read by its caller, not by
    /// [`project_each`].
    Std,
}

#[cfg(feature = "local")]
/// A crate of the scope of [`project_each`].
pub(super) enum Gathered<R> {
    /// Read, and looked at.
    Read(Read<R>),
    /// Not read: failed, still going, or too many ([`Outcome::Failed`] or [`Outcome::Going`]).
    Unread(Member),
}

#[cfg(feature = "local")]
/// What [`project_each`] does with each crate that it read.
pub(super) type PerCrate<R> = Arc<dyn Fn(&Member, &CrateView<'_>) -> R + Send + Sync>;

#[cfg(feature = "local")]
/// [`project_all`], with `per_crate` done with each crate as soon as it is read, on a blocking
/// thread, after which its archive is let go of: for a lookup across many crates, which would
/// otherwise hold them all mapped (decided in M4a, decision 5). `asked`, the package of what the
/// call asks of, which it looked up itself, is built as for a lookup of its own, even when the
/// others are not.
pub(super) async fn project_each<R: Send + 'static>(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    (scope, asked): (Scope, Option<ProjectPackage>),
    at: (&TargetTriple, Option<&TargetTriple>),
    until: Instant,
    warnings: &mut Vec<Box<str>>,
    per_crate: PerCrate<R>,
) -> Vec<Gathered<R>> {
    let finish: Finish<Gathered<R>> = Arc::new(move |member: Member| {
        let how = match &member.outcome {
            Outcome::Loaded(_) => How::Release,
            Outcome::Built(built) => How::Built { stale: built.stale },
            Outcome::Cached(_) => How::Cached,
            Outcome::Recorded(_, changed) => How::Recorded { changed: *changed },
            Outcome::Failed(_) | Outcome::Going(_) => return Gathered::Unread(member),
        };
        let result = match &member.outcome {
            Outcome::Loaded(mapped) => per_crate(&member, &CrateView::new(mapped.get())),
            Outcome::Built(built) | Outcome::Cached(built) | Outcome::Recorded(built, _) => {
                per_crate(&member, &CrateView::new(built.archived.mapped.get()))
            },
            Outcome::Failed(_) | Outcome::Going(_) => return Gathered::Unread(member),
        };
        Gathered::Read(Read {
            name: member.name,
            version: member.version,
            how,
            result,
        })
    });
    project_with(
        state,
        workspace,
        (scope, asked),
        at,
        until,
        warnings,
        Some(finish),
    )
    .await
}

#[cfg(feature = "local")]
/// What [`project_with`] does with each member, on a blocking thread.
type Finish<T> = Arc<dyn Fn(Member) -> T + Send + Sync>;

#[cfg(feature = "local")]
/// [`project_all`], each member passed to `finish` in the task that loaded it, if given.
async fn project_with<T: From<Member> + Send + 'static>(
    state: &Arc<ToolState>,
    workspace: &Workspace,
    ((mut scope, builds), asked): (Scope, Option<ProjectPackage>),
    (target, docs_rs): (&TargetTriple, Option<&TargetTriple>),
    until: Instant,
    warnings: &mut Vec<Box<str>>,
    finish: Option<Finish<T>>,
) -> Vec<T> {
    // What the call asks of first, which is read whatever the most.
    if let Some(at) = scope
        .iter()
        .position(|(package, _)| asked.as_ref() == Some(package))
        && let Some(before) = scope.get_mut(..=at)
    {
        before.rotate_right(1);
    }
    // Past the most, told as too many, not loaded (decided in M4a, decision 10).
    let most = max_crates();
    let too_many = if builds == Builds::CachedOnly && scope.len() > most {
        scope.split_off(most)
    } else {
        Vec::new()
    };
    // By the package's name, which the features of the project's build go by, as `get_item`
    // tells it; one that is not a crate's name is left out.
    let handles: Vec<_> = scope
        .into_iter()
        .filter_map(|(package, kind)| {
            let name = package.name.parse::<CrateName>().ok()?;
            let (state, workspace) = (Arc::clone(state), workspace.clone());
            let (target, docs_rs) = (target.clone(), docs_rs.cloned());
            let finish = finish.clone();
            let package_name = package.name.clone();
            // Told as failed if its task ends without an answer, not left out (requirement
            // R-SR-5).
            let failed = (name.clone(), package.version.clone());
            let builds = if asked.as_ref() == Some(&package) {
                Builds::Allow
            } else {
                builds
            };
            // Aborted if the call is cancelled while it waits (requirement R-WS-6).
            let handle = super::AbortOnDrop::new(super::spawn(async move {
                let at = (&target, docs_rs.as_ref());
                let (outcome, checked) =
                    project_outcome(&state, &workspace, (&package, kind, builds), at, until).await;
                let member = outcome.map(|outcome| Member {
                    name,
                    version: package.version.clone(),
                    outcome,
                });
                let finished = match (member, finish) {
                    (Some(member), Some(finish)) => {
                        let (name, version) = (member.name.clone(), member.version.clone());
                        Some(
                            crate::task::spawn_blocking(move || finish(member))
                                .await
                                .unwrap_or_else(|error| {
                                    tracing::error!(%error, "looking at a crate did not finish");
                                    T::from(ended(name, version))
                                }),
                        )
                    },
                    (member, _) => member.map(T::from),
                };
                (finished, checked, package.version)
            }));
            Some((package_name, failed, handle))
        })
        .collect();
    let mut finished = Vec::new();
    let mut unchecked = Vec::new();
    for (name, (crate_name, version), handle) in handles {
        match handle.await {
            Ok((one, checked, version)) => {
                if !checked {
                    unchecked.push(format!("{name} {version}"));
                }
                finished.extend(one);
            },
            Err(error) => {
                tracing::error!(%error, "loading a crate to look at did not finish");
                finished.push(T::from(ended(crate_name, version)));
            },
        }
    }
    for (package, _) in too_many {
        if let Ok(name) = package.name.parse::<CrateName>() {
            finished.push(T::from(Member {
                name,
                version: package.version,
                outcome: Outcome::Failed(json!({
                    "code": "docs_unavailable",
                    "reason": "too_many",
                    "retryable": false,
                })),
            }));
        }
    }
    // Once for all, as the index could not be read for them (decided in M2's D7).
    if !unchecked.is_empty() {
        warnings.push(
            format!(
                "whether the project's {} are the releases on crates.io is not known, as the \
                 index could not be read: their JSON on docs.rs is searched",
                unchecked.join(", ")
            )
            .into(),
        );
    }
    finished
}

#[cfg(feature = "local")]
impl<R> From<Member> for Gathered<R> {
    fn from(member: Member) -> Self {
        Self::Unread(member)
    }
}

/// How a call finds `package`, of `kind`, of `workspace`, as `builds` lets it, for `target`, whose
/// JSON on docs.rs is of `docs_rs`, waiting for it until `until`: `None` for a member that has no
/// crate to search; and whether a package of crates.io was checked against the index (decided in
/// M2's D7).
#[cfg(feature = "local")]
pub(super) async fn project_outcome(
    state: &ToolState,
    workspace: &Workspace,
    (package, kind, builds): (&ProjectPackage, Kind, Builds),
    (target, docs_rs): (&TargetTriple, Option<&TargetTriple>),
    until: Instant,
) -> (Option<Outcome>, bool) {
    // Without a lib, or not built and not to build here.
    let not_built = || {
        Outcome::Failed(json!({
            "code": "docs_unavailable",
            "reason": "local_build_required",
            "retryable": false,
        }))
    };
    let dependency = async || {
        let planned = local::dependency(state, workspace, package, target, until).await;
        // Not built here, but read if built before (decided in M4's plan, J2).
        if builds == Builds::CachedOnly
            && let Ok(Planned::Is { wanted, .. }) = &planned
        {
            let host = state.host().await.triple.clone();
            // Its archive, made again from its JSON, is waited for until `until` only: the
            // making goes on, for the next call (requirement R-ARCH-6).
            let before = tokio::time::timeout_at(
                until,
                crate::query::local::published_before(&state.cache, wanted, &host),
            )
            .await;
            return Some(before.map_or(Outcome::Going(Doing::Indexing), |before| {
                before.map_or_else(not_built, |built| Outcome::Cached(Arc::new(built)))
            }));
        }
        let failure = not_built();
        Some(
            planned_outcome(state, planned, until)
                .await
                .unwrap_or(failure),
        )
    };
    match kind {
        Kind::Member => {
            let Ok(name) = package.lib_name.parse::<CrateName>() else {
                return (None, true);
            };
            let asked = local::Asked::Package {
                name: &package.name,
                version: &package.version,
            };
            let planned = local::member(state, workspace, (&name, asked), target, until).await;
            (planned_outcome(state, planned, until).await, true)
        },
        Kind::Dependency => (dependency().await, true),
        Kind::CratesIo => {
            let Ok(name) = package.name.parse::<CrateName>() else {
                return (None, true);
            };
            // Within the call's deadline, past which it is not checked (requirement R-ARCH-6).
            let checked =
                tokio::time::timeout_at(until, get_item::project_entry(state, package, &name))
                    .await
                    .ok()
                    .and_then(Result::ok);
            let unchecked = checked.is_none();
            match checked {
                Some(get_item::InIndex::Copy(_)) => (dependency().await, true),
                Some(get_item::InIndex::Unlisted(entry)) => {
                    let what = format!("{} {}", entry.name, package.version);
                    let error = get_item::stale_index_error(state, &what, &entry);
                    (Some(Outcome::Failed(failure_of(&error))), true)
                },
                Some(get_item::InIndex::Release(_)) | None => {
                    match load_release(state, (&name, &package.version, docs_rs), until, builds)
                        .await
                    {
                        // docs.rs has no JSON of it: built here.
                        Outcome::Failed(failure)
                            if failure.get("reason").and_then(Value::as_str).is_some_and(
                                |reason| matches!(reason, "no_json" | "no_target_json"),
                            ) =>
                        {
                            (dependency().await, !unchecked)
                        },
                        outcome @ (Outcome::Loaded(_)
                        | Outcome::Failed(_)
                        | Outcome::Going(_)
                        | Outcome::Built(_)
                        | Outcome::Cached(_)
                        | Outcome::Recorded(..)) => (Some(outcome), !unchecked),
                    }
                },
            }
        },
    }
}

/// The outcome of the build that `planned` plans, waited for until `until`, or why there is
/// none: `None` for a crate that the package does not have (decided in M2's D7).
#[cfg(feature = "local")]
pub(super) async fn planned_outcome(
    state: &ToolState,
    planned: Result<Planned, Box<CallToolResult>>,
    until: Instant,
) -> Option<Outcome> {
    match planned {
        Ok(Planned::Is { wanted, .. }) => {
            // `build.wait_secs` from now, within the call's deadline (decided in M2's D3).
            let until_built = Instant::now()
                .checked_add(state.build_wait)
                .map_or(until, |until_built| until_built.min(until));
            let built = local::built(state, *wanted, (until_built, until), Map::new(), &[]).await;
            Some(match built {
                Ok(built) => Outcome::Built(built),
                Err(result) => outcome_of(&result),
            })
        },
        Err(error)
        | Ok(
            Planned::Unknown(error)
            | Planned::NotForTarget(error)
            | Planned::OtherTarget { error, .. }
            | Planned::Unnameable { error, .. },
        ) => Some(Outcome::Failed(failure_of(&error))),
        Ok(Planned::Recorded(recorded)) => Some(recorded_outcome(state, *recorded).await),
        Ok(Planned::Resolving(doing) | Planned::Planning(doing)) => Some(Outcome::Going(doing)),
        Ok(Planned::Not | Planned::OtherVersion(_)) => None,
    }
}

/// The outcome of what `build_external_docs` recorded of a crate of a workspace outside the default
/// project, as `recorded` tells: its build, with whether what it is made from changed since; while
/// it is made, as building; its failure; or else unavailable, with no record (decided in M2e's
/// E4b-3b).
#[cfg(feature = "local")]
pub(super) async fn recorded_outcome(state: &ToolState, recorded: local::RecordedCrate) -> Outcome {
    let (name, version) = recorded
        .package
        .name_and_version()
        .unwrap_or((&recorded.package.0, ""));
    let what = format!("{name} {version} for {}", recorded.target);
    // Not where it was, which is gone (decided in M2e's E4b-3, 2).
    let call_again = if recorded.since == recorded_workspace::Since::Missing {
        recorded_workspace::MOVED.to_owned()
    } else {
        format!(
            "call build_external_docs with {} to build it again",
            recorded.again
        )
    };
    let provenance = Map::new;
    match super::recorded::recorded_as(state, (&recorded.key, &what, &call_again), provenance, &[])
        .await
    {
        // Of the crate itself: what the workspace became is told of the whole search (decided in
        // M2e's E4b-3b, 3).
        super::recorded::Found::Built(built) => {
            let changed = recorded.since != recorded_workspace::Since::Missing
                && get_item::inputs_changed(&built).await.unwrap_or(false);
            Outcome::Recorded(built, changed)
        },
        super::recorded::Found::Response(result) => outcome_of(&result),
        super::recorded::Found::Missing => Outcome::Failed(failure_of(&recorded.error)),
    }
}

/// The outcome that `result`, the response of a build here, tells: still going, as its
/// `building_reason` says, or failed.
#[cfg(feature = "local")]
pub(super) fn outcome_of(result: &CallToolResult) -> Outcome {
    let reason = result
        .structured_content
        .as_ref()
        .filter(|response| response["status"] == "building")
        .and_then(|response| response["building_reason"].as_str());
    match reason {
        Some("fetching") => Outcome::Going(Doing::Fetching),
        Some("indexing") => Outcome::Going(Doing::Indexing),
        Some("waiting_for_lock") => Outcome::Going(Doing::WaitingForLock),
        Some("waiting_for_slot") => Outcome::Going(Doing::WaitingForSlot),
        Some(_) => Outcome::Going(Doing::Building),
        None => Outcome::Failed(failure_of(result)),
    }
}

/// The archive of `name` at `version` for `target`: from the cache, or made from its JSON on
/// docs.rs by work that the calls for it share, waiting for it until `until`.
#[cfg(feature = "local")]
pub(super) async fn load_release(
    state: &ToolState,
    (name, version, target): (&CrateName, &Version, Option<&TargetTriple>),
    until: Instant,
    builds: Builds,
) -> Outcome {
    let dir = state.cache.docs_rs_dir(name, version, target);
    let cache = state.cache.clone();
    // Not kept open for a lookup across many crates (decided in M4a, decision 5).
    let opened = crate::task::spawn_blocking(move || match builds {
        Builds::Allow => load::open_archived(&cache, &dir),
        Builds::CachedOnly => load::open_archived_unkept(&cache, &dir),
    })
    .await
    .ok()
    .and_then(Result::ok)
    .flatten();
    if let Some(mapped) = opened {
        return Outcome::Loaded(mapped);
    }
    // Not kept open for a lookup across many crates either (decided in M4a, decision 5).
    let kept = match builds {
        Builds::Allow => load::Keep::Yes,
        Builds::CachedOnly => load::Keep::No,
    };
    let archived = load::docs_rs_archive(
        &state.archives,
        &state.cache,
        &state.docs_rs,
        (name, version, target),
        until,
        kept,
    )
    .await;
    match archived {
        Joined::Done(Ok(archived)) => Outcome::Loaded(Arc::clone(&archived.mapped)),
        Joined::Done(Err(error)) => match &*error {
            ArchiveError::Load(error) => {
                let result = get_item::load_error_of(
                    &state.cache,
                    name,
                    &format!("{name}@{version}"),
                    error,
                );
                Outcome::Failed(failure_of(&result))
            },
            ArchiveError::Make(error) => {
                tracing::warn!(%name, %version, error = %ErrorChain(error), "a crate to search could not be archived");
                Outcome::Failed(json!({
                    "code": "docs_unavailable",
                    "reason": "unparsable",
                    "retryable": false,
                }))
            },
            ArchiveError::Stopped => {
                Outcome::Failed(json!({ "code": "internal_error", "retryable": true }))
            },
        },
        Joined::Ended => Outcome::Failed(json!({ "code": "internal_error", "retryable": true })),
        Joined::Going(doing) => Outcome::Going(doing),
    }
}

/// The code, the reason and whether trying again may help, of the tool error `result`.
#[cfg(feature = "local")]
pub(super) fn failure_of(result: &CallToolResult) -> Value {
    let body: Option<Value> = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str(&text.text).ok());
    let mut failure = Map::new();
    for key in ["code", "reason", "retryable", "retry_after_secs"] {
        if let Some(value) = body.as_ref().and_then(|body| body.get(key)) {
            failure.insert(key.to_owned(), value.clone());
        }
    }
    Value::Object(failure)
}
