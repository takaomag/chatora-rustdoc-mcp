//! `list_implementors` with `scope: dependency_graph`: the implementations of a trait in every
//! crate of the project's build that depends on the trait's crate (requirements R-TI-3 and R-TI-4,
//! decided in M4's plan, P1, J2 and J7, and in M4a and M4b).
//!
//! The trait is looked up in its crate first, as for `scope: crate`, for what it is and the
//! definition path that the reverse indexes key its implementations by. Then the crates whose
//! code depends on the trait's package are read at once, each looked at and let go of as soon as
//! it is read, without building a dependency (decided in M4a, decisions 4 and 5).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rmcp::model::CallToolResult;
use semver::Version;
use serde_json::{Map, Value, json};

use super::{
    ToolState,
    gather::{self, Builds, Gathered, How, Kind, Member, Outcome, PerCrate, Read},
    get_item::{self, Arguments, Paging, StdUnready},
    local,
};
use crate::{
    data::{
        crate_name::CrateName,
        dist::StdCrate,
        flight::{Doing, Joined},
        packages::{PackageGraph, ProjectPackage, Source},
        target::TargetTriple,
    },
    query::{
        get_item::{
            Detail,
            ItemRequest,
            MAX_RESPONSE_CHARS,
            Origin,
            ResolvedFrom,
            docs_rs_release,
        },
        implementors::{Implementor, TraitCrate, implementors_in},
        load,
        view::CrateView,
    },
};

/// When to ask again for what is still being loaded (requirement R-OUT-9).
const RETRY_AFTER_SECS: u64 = get_item::RETRY_AFTER_SECS;

/// The traits that the compiler implements, which no `impl` names: listing their implementors
/// across the dependency graph would tell none (decided in M4b, decision 16).
const BUILT_IN_MARKERS: &[&str] = &[
    "core::marker::Sized",
    "core::marker::MetaSized",
    "core::marker::PointeeSized",
    "core::marker::Unsize",
    "core::marker::Tuple",
    "core::marker::Destruct",
    "core::marker::FnPtr",
    "core::marker::PointerLike",
    "core::marker::Freeze",
];

/// What `scope: dependency_graph` takes besides the trait: the project, the target and the page.
#[derive(Debug)]
pub(super) struct Graph {
    /// The project whose build is looked at.
    pub(super) project: Option<PathBuf>,
    /// The target asked for.
    pub(super) target: Option<TargetTriple>,
    /// The page of the implementors.
    pub(super) paging: Paging,
}

/// An implementor found, with the package that has it.
struct Found {
    /// The package's name.
    package: CrateName,
    /// Its version.
    version: Version,
    /// Whether the package is the trait's own.
    own: bool,
    /// The implementor.
    implementor: Implementor,
}

/// The trait, as its crate tells it.
struct TheTrait {
    /// The answer for the trait in its crate, which the answer across the graph is made from.
    response: Map<String, Value>,
    /// Its definition path, which the reverse indexes key its implementations by.
    definition: Box<str>,
    /// Whether it is of std, alloc or core, read from a nightly toolchain's documentation.
    std: bool,
    /// Its crate's package name, as the provenance tells it.
    name: CrateName,
    /// Its version.
    version: Version,
}

/// What [`collect`] found across the graph.
#[derive(Default)]
struct Collected {
    /// The implementors that are of the trait asked of.
    listed: Vec<Found>,
    /// The crates not read, and those read as built before.
    sources: Vec<Value>,
    /// How many crates were read.
    crates_read: usize,
    /// How many crates are still being fetched, indexed or built, or built again, read as built
    /// before: to come.
    to_come: usize,
    /// How many crates failed or cannot be read here.
    failed: usize,
    /// What the crates still being loaded are doing.
    going: Vec<Doing>,
    /// How many implementations of another version's trait were left out.
    other_version: usize,
    /// How many implementations of a trait whose version is not known were kept.
    unknown_version: usize,
    /// The crates read, with what they were made from, for the cursor.
    read_crates: String,
    /// Whether an implementation listed is negative.
    negative: bool,
    /// The crates past the most read, as `name@version`.
    too_many: Vec<String>,
}

/// What a crate read tells of the trait's implementations.
pub(super) struct InCrate {
    /// The implementations, each with the crate of the trait that it implements.
    implementors: Vec<(Implementor, TraitCrate)>,
    /// What the crate's data was made from.
    digest: Box<str>,
}

/// What defines the trait asked of.
#[derive(Debug, Clone, Copy)]
enum Owner<'a> {
    /// A package of the project.
    Package(&'a ProjectPackage),
    /// std, alloc or core, which every crate depends on, in one version.
    Std,
}

/// Whether an implementation is of the trait asked of, or of another version's (decided in M4's
/// plan, J7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Judged {
    /// Of the trait asked of.
    Same,
    /// Of another version's.
    Other,
    /// Not known (decided in M4b, decision 17).
    Unknown,
}

/// Lists the implementors of the trait that `first` names, its first page in its crate, across
/// the project's dependency graph as `graph` says.
pub(super) async fn execute(
    first: Arguments,
    graph: Graph,
    state: Arc<ToolState>,
) -> CallToolResult {
    // std as the lookup reads it: for the project asked of, or, reached through a re-export,
    // for the default one (`get_item`'s `follow_into`).
    let std_project = if first.names_std() {
        graph.project.clone()
    } else {
        None
    };
    // The trait, in its crate, as `scope: crate` finds it, through re-exports.
    let the_trait = match the_trait(get_item::execute(first, Arc::clone(&state)).await) {
        Ok(the_trait) => the_trait,
        Err(result) => return *result,
    };
    let mut warnings: Vec<Box<str>> = Vec::new();
    let (target, told) =
        super::project::lookup_target(&state, graph.target.as_ref(), graph.project.as_deref())
            .await;
    warnings.extend(told);
    let until = state.until();
    let (workspace, resolution) = match project_of(&state, &graph, until, &warnings).await {
        Ok(found) => found,
        Err(result) => return *result,
    };
    let packages = PackageGraph::new(&resolution);
    // As the lookup of the trait chose it; what it told of the others is in its answer.
    let owner = match owner_of(&the_trait, &packages) {
        Ok(owner) => owner,
        Err(result) => return *result,
    };
    let triple = target.triple.clone();
    let sets = gather::build_sets(&state, &workspace, &triple, until, &mut warnings).await;
    let (scope, asked) = match owner {
        Owner::Std => (normal_scope(&packages, sets.as_deref()), None),
        Owner::Package(trait_package) => (
            scope_of(&packages, trait_package, sets.as_deref()),
            Some(trait_package.clone()),
        ),
    };
    // std, alloc and core first, then the project's packages (decided in M4b, decision 15).
    let mut gathered = if the_trait.std {
        let at = (&triple, std_project.as_deref());
        let asked = (&*the_trait.definition, the_trait.name.as_str());
        match read_std(&state, at, asked, until, &warnings).await {
            Ok(gathered) => gathered,
            Err(result) => return *result,
        }
    } else {
        Vec::new()
    };
    let docs_rs = target.docs_rs().cloned();
    gathered.extend(
        gather::project_each(
            &state,
            &workspace,
            ((scope, Builds::CachedOnly), asked),
            (&triple, docs_rs.as_ref()),
            until,
            &mut warnings,
            per_crate(&state, the_trait.definition.clone()),
        )
        .await,
    );
    let collected = collect(gathered, &packages, owner);
    warn(&collected, &the_trait, &mut warnings);
    let hints = match owner {
        Owner::Package(trait_package) => other_versions(
            (&packages, sets.as_deref()),
            trait_package,
            &the_trait,
            &mut warnings,
        ),
        // One std, of one toolchain.
        Owner::Std => Vec::new(),
    };
    let digest = crate::data::cache::sha256_hex(collected.read_crates.as_bytes());
    let state = Partly {
        to_come: collected.to_come,
        failed: collected.failed,
        building_reason: gather::building_reason(collected.going.iter()),
        negative: collected.negative,
    };
    respond(
        the_trait.response,
        (collected.listed, &graph.paging, &digest),
        (collected.sources, collected.crates_read, state),
        (warnings, hints),
    )
}

/// What defines `the_trait` in `packages`: std, or the package that the lookup of the trait chose;
/// or the answer to return when the project has none.
fn owner_of<'a>(
    the_trait: &TheTrait,
    packages: &'a PackageGraph,
) -> Result<Owner<'a>, Box<CallToolResult>> {
    if the_trait.std {
        return Ok(Owner::Std);
    }
    // As the lookup of the trait chose it; what it told of the others is in its answer.
    get_item::explicit_package(
        packages,
        &the_trait.name,
        &the_trait.version,
        &mut Vec::new(),
    )
    .map(Owner::Package)
    .ok_or_else(|| {
        Box::new(super::tool_error(
            "invalid_argument",
            &format!(
                "{} {}, whose trait {} is, is not a package of the project, so its \
                     dependency graph has no implementors of it",
                the_trait.name, the_trait.version, the_trait.definition
            ),
            false,
            &[
                "leave scope out to list the implementors in the trait's crate",
                "call list_impls on a type, which tells whether it implements the trait",
            ],
        ))
    })
}

/// The trait that `found`, the answer for it in its crate, tells; or the answer to return: an
/// error, `status: building`, or why a trait that the compiler implements is not looked for
/// (decided in M4a, decision 1, and in M4b, decision 16).
fn the_trait(found: CallToolResult) -> Result<TheTrait, Box<CallToolResult>> {
    let Some(Value::Object(mut response)) = found.structured_content.clone() else {
        return Err(Box::new(found));
    };
    if found.is_error == Some(true) || response.contains_key("status") {
        return Err(Box::new(found));
    }
    let data = response.get_mut("data").and_then(Value::as_object_mut);
    let auto = data
        .as_ref()
        .and_then(|data| data.get("auto"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let definition = data
        .and_then(|data| data.shift_remove("_definition"))
        .and_then(|definition| definition.as_str().map(Box::<str>::from))
        .unwrap_or_default();
    if auto || BUILT_IN_MARKERS.contains(&definition.as_ref()) {
        return Err(Box::new(super::tool_error(
            "invalid_argument",
            &format!(
                "{definition} is implemented by the compiler, not by impls that the dependency \
                 graph has: listing them would tell none"
            ),
            false,
            &["call list_impls on a type, which tells whether it implements the trait"],
        )));
    }
    let provenance = response.get("provenance");
    let field = |name: &str| {
        provenance
            .and_then(|provenance| provenance.get(name))
            .and_then(Value::as_str)
    };
    let (Some(name), Some(version)) = (
        field("crate").and_then(|name| name.parse::<CrateName>().ok()),
        field("version").and_then(|version| Version::parse(version).ok()),
    ) else {
        return Err(Box::new(found));
    };
    let std = field("source") == Some("std");
    Ok(TheTrait {
        response,
        definition,
        std,
        name,
        version,
    })
}

/// The workspace of the project that `graph` names and its packages, resolved until `until`; or
/// the answer to return.
async fn project_of(
    state: &ToolState,
    graph: &Graph,
    until: tokio::time::Instant,
    warnings: &[Box<str>],
) -> Result<
    (
        crate::data::project::Workspace,
        Arc<crate::data::metadata::Resolution>,
    ),
    Box<CallToolResult>,
> {
    let (workspace, project_from) = match super::project::located(state, graph.project.as_deref())
        .await
    {
        Ok(Some(found)) => found,
        Ok(None) => {
            return Err(Box::new(super::tool_error(
                "invalid_argument",
                "there is no project here whose dependency graph to look at",
                false,
                &[
                    "pass the project, or leave scope out to list the implementors in the trait's crate",
                ],
            )));
        },
        Err(error) => return Err(error),
    };
    match super::project::resolve_until(state, &workspace, until).await {
        Joined::Done(Ok(resolution)) => Ok((workspace, resolution)),
        Joined::Done(Err(error)) => {
            Err(Box::new(super::project::resolve_error(&workspace, &error)))
        },
        Joined::Ended => Err(Box::new(get_item::stopped())),
        Joined::Going(doing) => {
            let mut provenance = Map::new();
            provenance.insert("project_from".to_owned(), json!(project_from.as_str()));
            Err(Box::new(super::project::resolving(
                provenance, doing, warnings,
            )))
        },
    }
}

/// The packages to read: those whose code depends on `trait_package`, in the project's normal
/// build as `sets` tells it, or all of them without it, but procedural macros, which do not
/// export what they implement (decided in M4a, decisions 3 and 11, and in M4b).
fn scope_of(
    packages: &PackageGraph,
    trait_package: &ProjectPackage,
    sets: Option<&crate::data::features::FeatureSets>,
) -> Vec<(ProjectPackage, Kind)> {
    packages
        .dependents_closure(trait_package)
        .into_iter()
        // The trait's own crate is read whatever the build, as the lookup of the trait read it.
        .filter(|&package| {
            package == trait_package
                || (!packages.is_proc_macro(package)
                    && sets
                        .is_none_or(|sets| sets.normal(&package.name, &package.version).is_some()))
        })
        .map(|package| (package.clone(), kind_of(packages, package)))
        .collect()
}

/// The packages to read for a trait of std, alloc or core, which any may implement: those of the
/// project's normal build as `sets` tells it, or all of them without it, nearest first, but
/// procedural macros (decided in M4a, decision 1, and in M4b, decision 15).
fn normal_scope(
    packages: &PackageGraph,
    sets: Option<&crate::data::features::FeatureSets>,
) -> Vec<(ProjectPackage, Kind)> {
    packages
        .normal_build_order()
        .into_iter()
        .filter(|package| {
            sets.is_none_or(|sets| sets.normal(&package.name, &package.version).is_some())
        })
        .map(|package| (package.clone(), kind_of(packages, package)))
        .collect()
}

/// What `package` is to the project, for how it is read.
fn kind_of(packages: &PackageGraph, package: &ProjectPackage) -> Kind {
    if packages.is_member(package) {
        Kind::Member
    } else if package.source == Source::CratesIo {
        Kind::CratesIo
    } else {
        Kind::Dependency
    }
}

/// The crates of the toolchain's documentation that may implement the trait at `definition`: its
/// own and those that depend on it, of std, alloc, core and `proc_macro` (`test`, which has no
/// implementations of their traits for others to look for, only for its own).
fn std_crates_of(definition: &str) -> &'static [StdCrate] {
    match definition.split("::").next() {
        Some("alloc") => &[StdCrate::Alloc, StdCrate::Std, StdCrate::ProcMacro],
        Some("std") => &[StdCrate::Std, StdCrate::ProcMacro],
        Some("proc_macro") => &[StdCrate::ProcMacro],
        Some("test") => &[StdCrate::Test],
        _ => &[
            StdCrate::Core,
            StdCrate::Alloc,
            StdCrate::Std,
            StdCrate::ProcMacro,
        ],
    }
}

/// The implementations of the trait at `definition` in the crates of std that may have them, of
/// the nightly toolchain that `get_item` reads std from for `target` and the project at `project`
/// (requirement R-STD-1); or the answer to return when std cannot be read. A crate whose archive
/// is not made by `until` is told as still being indexed.
async fn read_std(
    state: &Arc<ToolState>,
    (target, project): (&TargetTriple, Option<&Path>),
    (definition, trait_crate): (&str, &str),
    until: tokio::time::Instant,
    warnings: &[Box<str>],
) -> Result<Vec<Gathered<InCrate>>, Box<CallToolResult>> {
    let at_least = get_item::project_toolchain(state, project).await;
    let host = state.host().await.triple.clone();
    let chosen = match get_item::shared_std(state, target, &host, at_least.as_ref(), until).await {
        Ok(chosen) => chosen,
        Err(StdUnready::Failed(error)) => {
            return Err(Box::new(get_item::dist_error(StdCrate::Std, &error)));
        },
        Err(StdUnready::Stopped) => return Err(Box::new(get_item::stopped())),
        Err(StdUnready::Going(doing)) => {
            return Err(Box::new(get_item::std_building(
                trait_crate,
                target,
                doing,
                warnings,
            )));
        },
    };
    let version = chosen
        .json
        .release()
        .unwrap_or_else(|| Version::new(0, 0, 0));
    let mut gathered = Vec::new();
    for &krate in std_crates_of(definition) {
        let Ok(name) = krate.as_str().parse::<CrateName>() else {
            continue;
        };
        let unread = |outcome| {
            Gathered::Unread(Member {
                name: name.clone(),
                version: version.clone(),
                outcome,
            })
        };
        let archiving = load::archive_std_crate(&state.cache, &chosen, krate);
        let mapped = match tokio::time::timeout_at(until, archiving).await {
            Ok(Some(Ok(mapped))) => mapped,
            Ok(Some(Err(error))) => {
                tracing::warn!(krate = krate.as_str(), %error, "the JSON of std could not be loaded");
                gathered.push(unread(Outcome::Failed(json!({
                    "code": "docs_unavailable",
                    "reason": "unparsable",
                    "retryable": false,
                }))));
                continue;
            },
            Ok(None) => return Err(Box::new(get_item::shutting_down())),
            // Made on: read by the next call (requirement R-ARCH-6).
            Err(_) => {
                gathered.push(unread(Outcome::Going(Doing::Indexing)));
                continue;
            },
        };
        let looking = (
            name.clone(),
            version.clone(),
            Arc::clone(&chosen),
            at_least.clone(),
            Box::<str>::from(definition),
        );
        let looked = crate::task::spawn_blocking(move || {
            look_at_std(&CrateView::new(mapped.get()), looking)
        })
        .await;
        gathered.push(match looked {
            Ok((result, version)) => Gathered::Read(Read {
                name,
                version,
                how: How::Std,
                result,
            }),
            Err(error) => {
                tracing::error!(%error, "looking at a crate of std did not finish");
                unread(Outcome::Failed(
                    json!({ "code": "internal_error", "retryable": true }),
                ))
            },
        });
    }
    Ok(gathered)
}

/// What `gathered` tells of the implementations of the trait of `owner`, of `packages`: those of
/// another version's trait left out (decided in M4's plan, J7), and those whose version is not
/// told kept (decided in M4b, decision 17), sorted (decision 14).
fn collect(
    gathered: Vec<Gathered<InCrate>>,
    packages: &PackageGraph,
    owner: Owner<'_>,
) -> Collected {
    let mut collected = Collected::default();
    for gathered in gathered {
        let read = match gathered {
            Gathered::Read(read) => read,
            Gathered::Unread(member) => {
                if let Outcome::Going(doing) = &member.outcome {
                    collected.to_come = collected.to_come.saturating_add(1);
                    collected.going.push(*doing);
                } else {
                    collected.failed = collected.failed.saturating_add(1);
                }
                // Past the most, told in one warning, not one source each (decided in M4b,
                // decision 27).
                if let Outcome::Failed(failure) = &member.outcome
                    && failure["reason"] == "too_many"
                {
                    collected
                        .too_many
                        .push(format!("{}@{}", member.name.as_str(), member.version));
                } else {
                    collected.sources.extend(gather::unread_source(&member));
                }
                continue;
            },
        };
        collected.crates_read = collected.crates_read.saturating_add(1);
        collected.read_crates.push_str(read.name.as_str());
        collected.read_crates.push('@');
        collected.read_crates.push_str(&read.version.to_string());
        // What it was made from too: a crate built again has other implementors.
        collected.read_crates.push('=');
        collected.read_crates.push_str(&read.result.digest);
        collected.read_crates.push(';');
        if read.how == (How::Built { stale: true }) {
            collected.to_come = collected.to_come.saturating_add(1);
        }
        collected
            .sources
            .extend(stale_source(&read.name, &read.version, &read.how));
        let implementer = packages.packages().iter().find(|package| {
            get_item::same_crate_name(&package.name, read.name.as_str())
                && package.version == read.version
        });
        let own = match owner {
            Owner::Package(trait_package) => implementer == Some(trait_package),
            Owner::Std => read.how == How::Std,
        };
        for (implementor, trait_crate) in read.result.implementors {
            let judged = match owner {
                Owner::Package(trait_package) => judge(
                    &trait_crate,
                    implementer.filter(|_| !own),
                    packages,
                    trait_package,
                ),
                Owner::Std => Judged::Same,
            };
            match judged {
                Judged::Other => {
                    collected.other_version = collected.other_version.saturating_add(1);
                    continue;
                },
                Judged::Unknown => {
                    collected.unknown_version = collected.unknown_version.saturating_add(1);
                },
                Judged::Same => {},
            }
            collected.negative |= implementor.kind == "negative";
            collected.listed.push(Found {
                package: read.name.clone(),
                version: read.version.clone(),
                own,
                implementor,
            });
        }
    }
    // The trait's crate first, then by package and version, then by type and header (decided in
    // M4b, decision 14).
    collected.listed.sort_by(|a, b| {
        let key = |found: &Found| {
            (
                !found.own,
                found.package.as_str().to_owned(),
                found.version.clone(),
                found.implementor.ty.clone(),
                found.implementor.header.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    collected
}

/// What `collected` tells in `warnings`: the implementations of another version's trait left out,
/// those whose version is not known, and the crates not read (requirement R-SR-5).
fn warn(collected: &Collected, the_trait: &TheTrait, warnings: &mut Vec<Box<str>>) {
    if collected.other_version > 0 {
        warnings.push(
            format!(
                "{} implementations of another version's {} are left out",
                collected.other_version, the_trait.definition
            )
            .into(),
        );
    }
    if collected.unknown_version > 0 {
        warnings.push(
            format!(
                "for {} implementations, which version of {} their trait is of is not known; \
                 they are listed",
                collected.unknown_version, the_trait.name
            )
            .into(),
        );
    }
    let going = collected.going.len();
    if going > 0 || collected.failed > 0 {
        // One line for all, as for the search across crates (requirement R-SR-5); those past the
        // most are not in the sources.
        let total = collected
            .crates_read
            .saturating_add(going)
            .saturating_add(collected.failed);
        let too_many = collected.too_many.len();
        let mut told = format!(
            "{} of {total} crates were read: {going} still being loaded, {} not available (see \
             provenance.sources)",
            collected.crates_read,
            collected.failed.saturating_sub(too_many)
        );
        if too_many > 0 {
            told.push_str(", the others past the most read at once");
        }
        warnings.push(told.into());
    }
    if !collected.too_many.is_empty() {
        // The first of them, as the scope orders them: the nearest to the trait's crate.
        let named = first_named(&collected.too_many);
        warnings.push(
            format!(
                "{} crates past the most read at once ({}) were not read: {named}",
                collected.too_many.len(),
                gather::max_crates()
            )
            .into(),
        );
    }
}

/// The first of `names`, joined, with how many more there are.
fn first_named(names: &[String]) -> String {
    /// How many are named, for a warning of one line.
    const NAMED: usize = 20;
    let mut shown: Vec<&str> = names.iter().take(NAMED).map(String::as_str).collect();
    let more = names.len().saturating_sub(NAMED);
    let and_more = format!("and {more} more");
    if more > 0 {
        shown.push(&and_more);
    }
    shown.join(", ")
}

/// The packages that implement the trait of another version of `trait_package` alone, told in
/// `warnings`, with how to list them, as hints (decided in M4's plan, J7).
fn other_versions(
    (packages, sets): (&PackageGraph, Option<&crate::data::features::FeatureSets>),
    trait_package: &ProjectPackage,
    the_trait: &TheTrait,
    warnings: &mut Vec<Box<str>>,
) -> Vec<Box<str>> {
    let path = the_trait
        .response
        .get("data")
        .and_then(|data| data.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut hints = Vec::new();
    for (other, only) in packages.through_other_versions(trait_package) {
        // As the scope is: the normal build, not procedural macros.
        let names: Vec<String> = only
            .iter()
            .filter(|package| {
                !packages.is_proc_macro(package)
                    && sets
                        .is_none_or(|sets| sets.normal(&package.name, &package.version).is_some())
            })
            .map(|package| format!("{}@{}", package.name, package.version))
            .collect();
        if names.is_empty() {
            continue;
        }
        warnings.push(
            format!(
                "{} depend on {} {} and not on this version, so what they implement is its {}, \
                 not listed: {}",
                names.len(),
                other.name,
                other.version,
                the_trait.definition,
                first_named(&names)
            )
            .into(),
        );
        hints.push(
            format!(
                r#"call list_implementors with {{"crate": "{}@{}", "path": "{path}", "scope": "dependency_graph"}} for those"#,
                other.name, other.version,
            )
            .into(),
        );
    }
    hints
}

/// What the crate of std of `view` tells of the implementations of the trait at `definition`, with
/// its version: the one that its JSON says, as `get_item` tells it, else `release`, the
/// toolchain's.
fn look_at_std(
    view: &CrateView<'_>,
    (name, release, chosen, at_least, definition): (
        CrateName,
        Version,
        Arc<crate::data::std_json::Chosen>,
        Option<Version>,
        Box<str>,
    ),
) -> (InCrate, Version) {
    let version = view
        .crate_version()
        .and_then(|version| Version::parse(version).ok())
        .unwrap_or(release);
    let toolchain = chosen.json.toolchain();
    let request = ItemRequest {
        name: &name,
        version: &version,
        path: name.as_str(),
        origin: Origin::Std {
            toolchain: &toolchain,
            std_from: chosen.json.source(),
            why: &chosen.why,
            project_release: at_least.as_ref(),
        },
        detail: Detail::Normal,
        project_features: None,
        follow: false,
        target: None,
    };
    let result = InCrate {
        implementors: implementors_in(view, request, &definition).0,
        digest: view.source_digest().into(),
    };
    (result, version)
}

/// What each crate read is looked at for: the implementations of the trait at `definition`
/// in it, by its reverse index.
fn per_crate(state: &ToolState, definition: Box<str>) -> PerCrate<InCrate> {
    let dependencies = local::dependencies(state);
    Arc::new(move |member: &Member, view| {
        let request = ItemRequest {
            name: &member.name,
            version: &member.version,
            path: member.root(),
            origin: origin_of(member, dependencies.as_ref()),
            detail: Detail::Normal,
            project_features: None,
            follow: false,
            target: None,
        };
        InCrate {
            implementors: implementors_in(view, request, &definition).0,
            digest: view.source_digest().into(),
        }
    })
}

/// Where the documentation of `member` comes from, as the lookups tell it.
fn origin_of<'m>(
    member: &'m Member,
    dependencies: Option<&'m crate::data::source_root::Dependencies>,
) -> Origin<'m> {
    match &member.outcome {
        Outcome::Built(built) | Outcome::Cached(built) | Outcome::Recorded(built, _) => {
            let record = &built.record;
            Origin::Local {
                private: record.private,
                package_source: (!record.private).then(|| local::package_source(&record.package)),
                resolved_from: ResolvedFrom::CargoLock,
                project_from: None,
                toolchain: record.toolchain.as_deref(),
                built_at: &record.built_at,
                features_requested: record.features_requested.as_deref(),
                features_effective: record.features_effective.as_deref(),
                recorded_by: built.recorded_by,
                roots: record
                    .workspace_root
                    .as_deref()
                    .zip(record.package_root.as_deref()),
                source_files: &record.source_files,
                dependencies,
            }
        },
        Outcome::Loaded(_) | Outcome::Failed(_) | Outcome::Going(_) => Origin::DocsRs {
            resolved_from: ResolvedFrom::CargoLock,
            project_from: None,
            build: None,
        },
    }
}

/// Whether an implementation in `implementer`, of the trait of the crate `trait_crate`, is of the
/// trait of `trait_package` (decided in M4's plan, J7). The implementer's own trait is; else the
/// version of the trait's crate that the implementer depends on in the project, if one; else the
/// version that the URL of its documentation tells, which docs.rs resolved when it built it, the
/// same if compatible, as the project cannot have two compatible versions of a crate; else not
/// known. `implementer` is none for the trait's own crate.
fn judge(
    trait_crate: &TraitCrate,
    implementer: Option<&ProjectPackage>,
    packages: &PackageGraph,
    trait_package: &ProjectPackage,
) -> Judged {
    let Some(implementer) = implementer else {
        return Judged::Same;
    };
    if *trait_crate == TraitCrate::Own {
        return Judged::Same;
    }
    if let [depended] = packages
        .normal_dependencies_by_lib(implementer, &trait_package.lib_name)
        .as_slice()
    {
        return if *depended == trait_package {
            Judged::Same
        } else {
            Judged::Other
        };
    }
    match version_of(trait_crate, &trait_package.lib_name) {
        Some(version) if compatible(&version, &trait_package.version) => Judged::Same,
        Some(_) => Judged::Other,
        None => Judged::Unknown,
    }
}

/// The version of the trait's crate `trait_crate`, named `trait_name` (its lib name), when its
/// documentation URL tells it.
fn version_of(trait_crate: &TraitCrate, trait_name: &str) -> Option<Version> {
    let TraitCrate::External {
        name,
        html_root_url: Some(url),
    } = trait_crate
    else {
        return None;
    };
    let (_, version) = docs_rs_release(url)?;
    (normalize(name) == normalize(trait_name)).then_some(version)
}

/// Whether `a` and `b` are compatible, as Cargo takes them: of the same leftmost part that is not
/// zero, of a release.
fn compatible(a: &Version, b: &Version) -> bool {
    if !a.pre.is_empty() || !b.pre.is_empty() {
        return a == b;
    }
    match (a.major, a.minor) {
        (0, 0) => b.major == 0 && b.minor == 0 && a.patch == b.patch,
        (0, minor) => b.major == 0 && b.minor == minor,
        (major, _) => b.major == major,
    }
}

/// The source of a crate read as it was built before, or while it is built again: stale.
fn stale_source(name: &CrateName, version: &Version, how: &How) -> Option<Value> {
    let reason = match how {
        How::Cached => "unchecked",
        How::Built { stale: true } => "rebuilding",
        How::Recorded { changed: true } => "inputs_changed",
        How::Release
        | How::Std
        | How::Built { stale: false }
        | How::Recorded { changed: false } => {
            return None;
        },
    };
    let mut source = json!({
        "crate": name.as_str(),
        "version": version.to_string(),
        "stale": true,
        "stale_reason": reason,
    });
    // Built again: to ask for again (requirement R-OUT-9).
    if reason == "rebuilding"
        && let Some(fields) = source.as_object_mut()
    {
        fields.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
    }
    Some(source)
}

/// How much of the graph was not read, or is to come.
struct Partly {
    /// How many crates are still being loaded, or built again, read as built before.
    to_come: usize,
    /// How many crates failed or cannot be read here.
    failed: usize,
    /// Why those still being loaded are not in yet.
    building_reason: &'static str,
    /// Whether an implementation listed is negative.
    negative: bool,
}

/// `response`, the trait in its crate, with the implementors across the graph, their page, and
/// what was read: `partial` if some crates were not, or are to come, and, if none is listed while
/// some are to come, `status: building` (requirements R-SR-5 and R-OUT-9, as `search_items`). As
/// many implementors are shown as fit in the size of a response, and at least one (requirement
/// R-OUT-3, as `scope: crate`).
fn respond(
    mut response: Map<String, Value>,
    (listed, paging, digest): (Vec<Found>, &Paging, &str),
    (sources, crates_read, partly): (Vec<Value>, usize, Partly),
    (warnings, hints): (Vec<Box<str>>, Vec<Box<str>>),
) -> CallToolResult {
    let data_digest = digest.get(..16).unwrap_or(digest);
    if let Some(cursor_data) = &paging.cursor_data
        && cursor_data.as_ref() != data_digest
    {
        return super::tool_error(
            "invalid_argument",
            "the crates read changed since the cursor was made; list again without the cursor",
            false,
            &["list again without the cursor"],
        );
    }
    let total = listed.len();
    let shown: Vec<Value> = listed
        .into_iter()
        .skip(paging.offset)
        .take(paging.limit)
        .map(|found| {
            let mut entry = serde_json::to_value(&found.implementor).unwrap_or(Value::Null);
            if let Some(entry) = entry.as_object_mut() {
                entry.insert("crate".to_owned(), json!(found.package.as_str()));
                entry.insert("version".to_owned(), json!(found.version.to_string()));
            }
            entry
        })
        .collect();
    if let Some(Value::Object(provenance)) = response.get_mut("provenance") {
        provenance.insert("crates_read".to_owned(), json!(crates_read));
        provenance.insert("sources".to_owned(), Value::Array(sources));
    }
    if let Some(Value::Object(data)) = response.get_mut("data") {
        if partly.negative {
            data.insert("has_negative_impls".to_owned(), json!(true));
        }
        // In its place, before the page.
        data.insert("implementors".to_owned(), Value::Array(Vec::new()));
    }
    // The hints of the trait's crate but that other crates are not listed.
    let mut all_hints: Vec<Value> = response
        .get("hints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|hint| {
            !hint
                .as_str()
                .is_some_and(|hint| hint.starts_with("implementors in other crates"))
        })
        .cloned()
        .collect();
    all_hints.extend(hints.into_iter().map(|hint| json!(hint)));
    if partly.to_come > 0 {
        all_hints.push(json!(format!(
            "some crates are still being loaded or built again: call again in about \
             {RETRY_AFTER_SECS} seconds for them, without the cursor"
        )));
    }
    // In the order of the other responses: the state after the provenance.
    let mut ordered = Map::new();
    if let Some(provenance) = response.remove("provenance") {
        ordered.insert("provenance".to_owned(), provenance);
    }
    if total == 0 && partly.to_come > 0 {
        ordered.insert("status".to_owned(), json!("building"));
        ordered.insert("building_reason".to_owned(), json!(partly.building_reason));
    }
    if partly.to_come > 0 || partly.failed > 0 {
        ordered.insert("partial".to_owned(), json!(true));
    }
    if partly.to_come > 0 {
        ordered.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
    }
    for (name, value) in response {
        if name != "page" && name != "hints" {
            ordered.insert(name, value);
        }
    }
    ordered.insert("page".to_owned(), Value::Null);
    ordered.insert("hints".to_owned(), Value::Array(all_hints));
    let mut result = Value::Object(ordered);
    get_item::add_warnings(&mut result, &warnings);
    fit(&mut result, shown, (paging, total, data_digest));
    CallToolResult::structured(result)
}

/// Puts as many of `shown` into `result` as fit in the size of a response, and at least one, with
/// their page of `total`.
fn fit(
    result: &mut Value,
    mut shown: Vec<Value>,
    (paging, total, data_digest): (&Paging, usize, &str),
) {
    loop {
        let count = shown.len();
        let next = paging.offset.saturating_add(count);
        let mut page = Map::new();
        page.insert("shown".to_owned(), json!(count));
        page.insert("total".to_owned(), json!(total));
        page.insert("has_more".to_owned(), json!(next < total));
        if next < total {
            page.insert(
                "next_cursor".to_owned(),
                json!(format!("{}.{data_digest}.{next}", paging.request_digest)),
            );
        }
        if let Some(fields) = result.as_object_mut() {
            fields.insert("page".to_owned(), Value::Object(page));
        }
        if let Some(Value::Object(data)) = result.get_mut("data") {
            data.insert("implementors".to_owned(), Value::Array(shown));
        }
        let size = serde_json::to_string(&*result).map_or(0, |json| json.chars().count());
        if size <= MAX_RESPONSE_CHARS || count <= 1 {
            return;
        }
        shown = match result.pointer_mut("/data/implementors") {
            Some(Value::Array(shown)) => std::mem::take(shown),
            _ => Vec::new(),
        };
        shown.truncate(count.saturating_sub(count.div_ceil(4).max(1)));
    }
}

/// `name` with `-` as `_`, as crate names compare.
fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The version of the trait's crate is read from the URL of its documentation, for the crate
    /// of its lib name only.
    #[test]
    fn the_version_of_the_traits_crate_is_read_from_its_url() {
        let external = |name: &str, url: Option<&str>| TraitCrate::External {
            name: name.into(),
            html_root_url: url.map(Into::into),
        };
        assert_eq!(
            version_of(
                &external(
                    "tower_service",
                    Some("https://docs.rs/tower-service/0.3.3/x86_64-unknown-linux-gnu/")
                ),
                "tower_service"
            ),
            Some(Version::new(0, 3, 3))
        );
        assert_eq!(
            version_of(&external("tower_service", None), "tower_service"),
            None
        );
        assert_eq!(version_of(&TraitCrate::Own, "tower_service"), None);
        assert_eq!(
            version_of(
                &external("other", Some("https://docs.rs/other/1.0.0/")),
                "tower_service"
            ),
            None
        );
    }

    /// Versions are compatible as Cargo takes them.
    #[test]
    fn versions_are_compatible_as_cargo_takes_them() {
        let version = |text: &str| Version::parse(text).expect("a version");
        for (a, b, same) in [
            ("0.3.2", "0.3.3", true),
            ("0.3.3", "0.2.0", false),
            ("1.2.0", "1.0.5", true),
            ("1.0.0", "2.0.0", false),
            ("0.0.1", "0.0.1", true),
            ("0.0.1", "0.0.2", false),
            ("1.0.0-rc.1", "1.0.0", false),
        ] {
            assert_eq!(compatible(&version(a), &version(b)), same, "{a} {b}");
        }
    }

    /// Whether an implementation is of the trait asked of is told by the version of the trait's
    /// crate that the implementer depends on, else by the URL of the trait's crate, compatibly,
    /// else not known (decided in M4's plan, J7, and in M4b, decision 17).
    #[test]
    fn the_version_implemented_is_told_by_the_dependencies_then_the_url() {
        const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";
        let lock = crate::data::cargo_lock::CargoLock::parse(&format!(
            "[[package]]\nname = \"svc\"\nversion = \"0.3.3\"\nsource = \"{REGISTRY}\"\n\n\
             [[package]]\nname = \"svc\"\nversion = \"0.2.0\"\nsource = \"{REGISTRY}\"\n\n\
             [[package]]\nname = \"new\"\nversion = \"1.0.0\"\nsource = \"{REGISTRY}\"\n\
             dependencies = [\"svc 0.3.3\"]\n\n\
             [[package]]\nname = \"old\"\nversion = \"1.0.0\"\nsource = \"{REGISTRY}\"\n\
             dependencies = [\"svc 0.2.0\"]\n\n\
             [[package]]\nname = \"both\"\nversion = \"1.0.0\"\nsource = \"{REGISTRY}\"\n\
             dependencies = [\"svc 0.2.0\", \"svc 0.3.3\"]\n"
        ))
        .expect("a Cargo.lock");
        let graph = PackageGraph::from_lock(&lock);
        let named = |name: &str, version: &str| {
            graph
                .packages()
                .iter()
                .find(|package| &*package.name == name && package.version.to_string() == version)
                .expect("a package")
        };
        let svc = named("svc", "0.3.3");
        let built_here = TraitCrate::External {
            name: "svc".into(),
            html_root_url: None,
        };
        let on_docs_rs = |version: &str| TraitCrate::External {
            name: "svc".into(),
            html_root_url: Some(format!("https://docs.rs/svc/{version}/").into()),
        };
        let judged = |trait_crate: &TraitCrate, implementer: &str| {
            judge(trait_crate, Some(named(implementer, "1.0.0")), &graph, svc)
        };
        assert_eq!(judged(&built_here, "new"), Judged::Same);
        assert_eq!(judged(&built_here, "old"), Judged::Other);
        // Docs.rs resolved another compatible version when it built it.
        assert_eq!(judged(&on_docs_rs("0.3.3"), "old"), Judged::Other);
        assert_eq!(judged(&built_here, "both"), Judged::Unknown);
        assert_eq!(judged(&on_docs_rs("0.3.1"), "both"), Judged::Same);
        assert_eq!(judged(&on_docs_rs("0.2.0"), "both"), Judged::Other);
        assert_eq!(judged(&TraitCrate::Own, "both"), Judged::Same);
        assert_eq!(judge(&built_here, None, &graph, svc), Judged::Same);
    }

    /// A crate read as it was built before is stale, as unchecked; one read anew is not.
    #[test]
    fn a_crate_read_as_built_before_is_told_as_stale() {
        let name: CrateName = "a".parse().expect("a name");
        let version = Version::new(1, 0, 0);
        assert_eq!(
            stale_source(&name, &version, &How::Cached)
                .map(|source| source["stale_reason"].clone()),
            Some(json!("unchecked"))
        );
        assert_eq!(stale_source(&name, &version, &How::Release), None);
        assert_eq!(
            stale_source(&name, &version, &How::Built { stale: false }),
            None
        );
    }

    /// A trait of std is looked for in the crates of std that depend on its crate.
    #[test]
    fn a_trait_of_std_is_looked_for_where_its_crate_is_depended_on() {
        assert_eq!(std_crates_of("core::fmt::Display"), [
            StdCrate::Core,
            StdCrate::Alloc,
            StdCrate::Std,
            StdCrate::ProcMacro
        ]);
        assert_eq!(std_crates_of("alloc::borrow::ToOwned"), [
            StdCrate::Alloc,
            StdCrate::Std,
            StdCrate::ProcMacro
        ]);
        assert_eq!(std_crates_of("std::io::Read"), [
            StdCrate::Std,
            StdCrate::ProcMacro
        ]);
        assert_eq!(std_crates_of("proc_macro::MultiSpan"), [
            StdCrate::ProcMacro
        ]);
    }

    /// The traits that the compiler implements are known by their definition paths.
    #[test]
    fn the_built_in_markers_are_of_core() {
        assert!(
            BUILT_IN_MARKERS
                .iter()
                .all(|path| path.starts_with("core::marker::"))
        );
        let unique: BTreeSet<&&str> = BUILT_IN_MARKERS.iter().collect();
        assert_eq!(unique.len(), BUILT_IN_MARKERS.len());
    }
}
