//! `get_item`: one item with its signature, the first paragraph of its documentation, and where
//! the data came from (requirements R-OUT-2, R-OUT-7 `normal`, R-OUT-8, R-OUT-9, R-SH-1, R-SH-2
//! and R-SH-6).
//!
//! This only reads the crate and its path index, from their archive. Loading them and turning the
//! result into an MCP tool result are done by the caller. The response borrows the strings that it
//! shows as they are from the items it takes out, so it is serialized while the view is there.

use std::{
    borrow::Cow,
    collections::BTreeSet,
    fmt,
    path::{Component, Path},
};

use rustdoc_types::{
    Attribute,
    Id,
    Item,
    ItemEnum,
    ItemKind,
    MacroKind,
    Stability,
    StabilityLevel,
    Struct,
    StructKind,
    Visibility,
};
use semver::Version;
use serde::Serialize;

use super::{
    cfg::{Cfg, CfgAttribute, CfgSource},
    docs,
    path_index::{self, ArchivedPathIndex, ExternalReexport, PublicPath, Resolution},
    render::{Renderer, in_crate},
    view::CrateView,
};
use crate::data::{crate_file::DocsRsBuild, crate_name::CrateName, source_root};

/// What to look up.
#[derive(Debug, Clone, Copy)]
pub struct ItemRequest<'a> {
    /// The crate, as given (`tokio`).
    pub name: &'a CrateName,
    /// The version, as given.
    pub version: &'a Version,
    /// The path of the item, starting with the crate name (`tokio::sync::Mutex::lock`).
    pub path: &'a str,
    /// Where the data comes from, and how the version was chosen.
    pub origin: Origin<'a>,
    /// How much to show (requirement R-OUT-7).
    pub detail: Detail,
    /// The features that the project builds the crate with, when the project decided the
    /// version, for `enabled_in_project` (requirement R-FT-2).
    pub project_features: Option<ProjectFeatures<'a>>,
    /// Whether a path that goes on in another crate is returned as
    /// [`GetItemError::Elsewhere`], to be followed there (requirement R-PR-4), rather than as the
    /// re-export with a hint.
    pub follow: bool,
    /// The target asked for, to warn when the data is for another.
    pub target: Option<&'a str>,
}

/// The features that the project's build for the target enables on the crate (requirement
/// R-FT-2).
#[derive(Debug, Clone, Copy)]
pub struct ProjectFeatures<'a> {
    /// In the normal build. `None` when the normal build does not build the crate.
    pub normal: Option<&'a BTreeSet<Box<str>>>,
    /// In the build with the members' dev-dependencies, as in the tests.
    pub with_dev: Option<&'a BTreeSet<Box<str>>>,
}

/// Whether an item is in the project's build: `true`, `false` or `unknown` (requirement R-FT-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enabled {
    /// It is.
    Yes,
    /// It is not.
    No,
    /// It needs a condition that the project may or may not set (`tokio_unstable`).
    Unknown,
}

impl Serialize for Enabled {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Yes => serializer.serialize_bool(true),
            Self::No => serializer.serialize_bool(false),
            Self::Unknown => serializer.serialize_str("unknown"),
        }
    }
}

/// How much of an item to show (requirement R-OUT-7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Detail {
    /// The path, the kind and the signature on one line.
    Brief,
    /// Also the where clause, the fields or variants, the first paragraph of the documentation and
    /// the headings of its other sections, and where the item is defined.
    #[default]
    Normal,
    /// Also the whole documentation, section by section, and the attributes.
    Full,
}

/// Where the data comes from, for the provenance (§7.3).
#[derive(Debug, Clone, Copy)]
pub enum Origin<'a> {
    /// docs.rs, at a version chosen as `resolved_from` says.
    DocsRs {
        /// How the version was chosen.
        resolved_from: ResolvedFrom,
        /// Where the project that decided the version came from, if one did.
        project_from: Option<&'static str>,
        /// How docs.rs built it, if its `.crate` could be read (requirement R-FT-1).
        build: Option<&'a DocsRsBuild>,
    },
    /// A build here: of a workspace member (decided in M2's D2), or of a dependency that docs.rs
    /// does not have (decided in M2's D7).
    Local {
        /// Whether it has the private and hidden items, as a member's does, which are shown.
        private: bool,
        /// Where a dependency came from: `path`, `git`, `registry` or `crates.io`. `None` for a
        /// member (decided in M2's D7).
        package_source: Option<&'static str>,
        /// How the version was chosen.
        resolved_from: ResolvedFrom,
        /// Where the project came from.
        project_from: Option<&'static str>,
        /// The toolchain, as `rustc -vV` gives it on its first line, if known.
        toolchain: Option<&'a str>,
        /// When it was built, in RFC 3339 (requirement R-WS-9).
        built_at: &'a str,
        /// The features asked for with `--no-default-features`, or `None` for cargo's default
        /// (requirement R-FT-3).
        features_requested: Option<&'a [Box<str>]>,
        /// The features that cargo built it with, if its messages told them (requirement
        /// R-FT-3).
        features_effective: Option<&'a [Box<str>]>,
        /// The tool that recorded it, `build_external_docs`, for a build that the external tool
        /// made (decided in M2e's E1b).
        recorded_by: Option<&'static str>,
        /// The workspace's root, where cargo ran, and the package's, which the files of the
        /// definitions are told relative to (requirement R-OUT-8, decided in M3a). `None` for a
        /// record from before.
        roots: Option<(&'a Path, &'a Path)>,
    },
    /// The std of a nightly toolchain (requirement R-STD-1).
    Std {
        /// The toolchain, as `rustc -vV` gives it on its first line.
        toolchain: &'a str,
        /// `dist` or `local`.
        std_from: &'static str,
        /// Why that one was used.
        why: &'a str,
        /// The release of the project's toolchain, to tell what it does not have yet.
        project_release: Option<&'a Version>,
    },
}

/// The result of `get_item` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct ItemResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// The item.
    pub data: ItemData<'a>,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// Where the data came from. The version, target and format are read from the JSON itself.
#[derive(Debug, Serialize)]
pub struct Provenance<'a> {
    /// The source that returned the data.
    pub source: Source,
    /// Where a dependency built here came from: `path`, `git`, `registry` or `crates.io`
    /// (decided in M2's D7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_source: Option<&'static str>,
    /// The crate, as requested.
    #[serde(rename = "crate")]
    pub name: &'a str,
    /// The version in the JSON, or the requested one if the JSON has none.
    pub version: Cow<'a, str>,
    /// How the version was chosen. Not for std.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_from: Option<ResolvedFrom>,
    /// Where the project that decided the version came from: `argument`, `claude_project_dir` or
    /// `cwd` (requirement R-MCP-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_from: Option<&'static str>,
    /// The toolchain that std is from (requirement R-STD-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<&'a str>,
    /// Where std came from: `dist` (the Rust distribution server) or `local` (the nightly
    /// toolchain installed here), and why (requirement R-STD-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub std_from: Option<StdFrom<'a>>,
    /// The target the JSON was built for.
    pub target: &'a str,
    /// The rustdoc JSON format.
    pub format_version: u32,
    /// The features docs.rs built the documentation with, when known and not all of them
    /// (requirement R-FT-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<&'a [Box<str>]>,
    /// docs.rs built it with every feature.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub all_features: bool,
    /// The `--cfg` that docs.rs built it with, other than `docsrs` (`tokio_unstable`).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub cfg: &'a [Box<str>],
    /// When a local build was made (requirement R-WS-9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_at: Option<&'a str>,
    /// The features that a local build asked for (requirement R-FT-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features_requested: Option<&'a [Box<str>]>,
    /// The features that cargo built a local build with (requirement R-FT-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features_effective: Option<&'a [Box<str>]>,
    /// The tool that recorded a local build, `build_external_docs`, which makes it again (decided
    /// in M2e's E1b).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_by: Option<&'static str>,
}

/// What the provenance tells of where the data came from, by its origin.
struct FromWhere<'a> {
    source: Source,
    package_source: Option<&'static str>,
    resolved_from: Option<ResolvedFrom>,
    project_from: Option<&'static str>,
    toolchain: Option<&'a str>,
    std_from: Option<StdFrom<'a>>,
    built_at: Option<&'a str>,
    features_requested: Option<&'a [Box<str>]>,
    features_effective: Option<&'a [Box<str>]>,
    recorded_by: Option<&'static str>,
}

/// [`FromWhere`] of `origin`.
const fn from_where(origin: Origin<'_>) -> FromWhere<'_> {
    match origin {
        Origin::DocsRs {
            resolved_from,
            project_from,
            ..
        } => FromWhere {
            source: Source::DocsRs,
            package_source: None,
            resolved_from: Some(resolved_from),
            project_from,
            toolchain: None,
            std_from: None,
            built_at: None,
            features_requested: None,
            features_effective: None,
            recorded_by: None,
        },
        Origin::Local {
            package_source,
            resolved_from,
            project_from,
            toolchain,
            built_at,
            features_requested,
            features_effective,
            recorded_by,
            ..
        } => FromWhere {
            source: Source::Local,
            package_source,
            resolved_from: Some(resolved_from),
            project_from,
            toolchain,
            std_from: None,
            built_at: Some(built_at),
            features_requested,
            features_effective,
            recorded_by,
        },
        Origin::Std {
            toolchain,
            std_from,
            why,
            ..
        } => FromWhere {
            source: Source::Std,
            package_source: None,
            resolved_from: None,
            project_from: None,
            toolchain: Some(toolchain),
            std_from: Some(StdFrom {
                source: std_from,
                reason: why,
            }),
            built_at: None,
            features_requested: None,
            features_effective: None,
            recorded_by: None,
        },
    }
}

/// Where std came from, and why.
#[derive(Debug, Serialize)]
pub struct StdFrom<'a> {
    /// `dist` or `local`.
    pub source: &'static str,
    /// Why that one was used.
    pub reason: &'a str,
}

/// The source that returned the data (§7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Source {
    /// The rustdoc JSON that docs.rs built.
    #[serde(rename = "docs.rs")]
    DocsRs,
    /// The rustdoc JSON of std that a nightly toolchain ships.
    #[serde(rename = "std")]
    Std,
    /// The rustdoc JSON of a workspace member, built here.
    #[serde(rename = "local")]
    Local,
}

/// How the version was chosen (§7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedFrom {
    /// Given exactly by the caller.
    Explicit,
    /// The largest version that the caller's semver requirement matches.
    ExplicitReq,
    /// The latest version on crates.io, as no version was given (requirement R-VR-3).
    Latest,
    /// The version that the project's Cargo.lock resolves, through `cargo metadata`
    /// (requirement R-VR-2).
    CargoLock,
    /// The version in the project's Cargo.lock, read directly because `cargo metadata` failed
    /// (requirement R-VR-2).
    CargoLockRaw,
    /// The version that the crate which re-exports the item was built with on docs.rs, from its
    /// `html_root_url` (requirement R-PR-4).
    Reexport,
    /// The version of a crate of a git repository at the commit that its ref resolved to, which
    /// `build_external_docs` recorded (decided in M2e's E3b).
    Git,
    /// The version that `cargo metadata` resolved in a workspace outside the default project, as
    /// `build_external_docs` recorded it (requirement R-WS-10, decided in M2e's E4b).
    External,
}

/// The conditions of an entry of a list that is not an item of its own, such as an `impl`.
///
/// They are those that it is written with, without those of the type or trait it is listed for
/// (requirement R-FT-2), and empty when it has none.
#[derive(Debug, Default, Serialize)]
pub struct Conditions {
    /// The features it needs, as [`ItemData::required_features`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_features: Option<Box<str>>,
    /// The other conditions it needs, as [`ItemData::other_cfg`].
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub other_cfg: Box<[Box<str>]>,
    /// Where they were read, as [`ItemData::cfg_source`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfg_source: Option<&'static str>,
    /// Whether the project's normal build has it, as [`ItemData::enabled_in_project`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_in_project: Option<Enabled>,
    /// As [`ItemData::enabled_in_dev_only`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub enabled_in_dev_only: bool,
}

/// One item, as of `detail=normal` (requirement R-OUT-7), with the signature before the prose
/// (requirement R-SH-1).
#[derive(Debug, Default, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a flag of the JSON that is left out when false"
)]
pub struct ItemData<'a> {
    /// The canonical path; for a member, that of its parent and its name. In a list, the path
    /// it is listed at.
    pub path: Box<str>,
    /// In a list, the canonical path, when the item is listed at another (a re-export).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical: Option<Box<str>>,
    /// What the item is, in the words of rustdoc's `ItemKind`, except that a function in an
    /// `impl` or a trait is a `method`.
    pub kind: &'static str,
    /// In a workspace member, which has its private items, who can use the item, as Rust writes
    /// it: `pub`, `pub(crate)`, `pub(in crate::a)`, or `private` (requirement R-WS-3, decided in
    /// M2's D6). An item of a trait, a variant and an item of an `impl` of a trait have their
    /// parent's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Box<str>>,
    /// In a workspace member, the item is hidden from its documentation (`#[doc(hidden)]`, or
    /// below one), as other crates do not see it (decided in M2's D6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// The type, trait or enum that the item belongs to (requirement R-SH-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<str>>,
    /// The `impl` that the item is in, with its where clause (requirement R-SH-2).
    #[serde(rename = "impl", skip_serializing_if = "Option::is_none")]
    pub impl_header: Option<Box<str>>,
    /// The declaration as Rust (requirement R-OUT-2). Absent for a re-export of another crate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Box<str>>,
    /// The where clause, one predicate each.
    #[serde(rename = "where", skip_serializing_if = "<[_]>::is_empty")]
    pub where_predicates: Box<[Box<str>]>,
    /// The fields of a struct with named fields or of a union, one each.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub fields: Box<[Box<str>]>,
    /// The variants of an enum, one each.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub variants: Box<[Box<str>]>,
    /// The features the item needs, by name, with `any`, `all` and `not` as in `cfg`
    /// (`any(rt, rt-multi-thread)`. Requirement R-FT-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_features: Option<Box<str>>,
    /// The other conditions it needs, as `cfg` writes them (`tokio_unstable`, `unix`).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub other_cfg: Box<[Box<str>]>,
    /// Where the conditions were read: `doc_cfg`, `cfg_trace`, `inherited` (only from the
    /// modules, re-exports, type or `impl` it is in), or `unknown` (some could not be read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfg_source: Option<&'static str>,
    /// Whether the project's normal build for the target has the item, when the project decided
    /// the version (requirement R-FT-2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_in_project: Option<Enabled>,
    /// The item is not in the normal build, but in the build with the dev-dependencies.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub enabled_in_dev_only: bool,
    /// The first paragraph of the documentation, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<Cow<'a, str>>,
    /// The documentation goes on after the first paragraph.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub docs_truncated: bool,
    /// The sections of the documentation after the first paragraph: their headings and how many
    /// lines they have, and with `detail=full` their text (requirement R-OUT-7).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub sections: Box<[Section]>,
    /// `#[deprecated]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<Deprecated<'a>>,
    /// `#[must_use]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub must_use: bool,
    /// `#[non_exhaustive]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_exhaustive: bool,
    /// Since which release of Rust the item is stable, or the feature it needs, as std marks
    /// it (§3 Q17).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: Option<StabilityNote<'a>>,
    /// The attributes, with `detail=full` (requirement R-OUT-7).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub attrs: Box<[Box<str>]>,
    /// The other public paths of the item.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub also_reexported_at: Box<[Box<str>]>,
    /// The item of another crate that this path re-exports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reexport_of: Option<ReexportOf>,
    /// Where the item is defined (requirement R-OUT-8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_location: Option<SourceLocation<'a>>,
    /// Something was cut to keep the response within its budget (requirement R-OUT-3).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// The path of the item `id` of the crate of `view` in it, `crate::a::Item`, from its definition
/// path (requirement R-PR-2, decided in M2's D6). `None` for an item of another crate.
pub(super) fn crate_path(view: &CrateView<'_>, id: Id) -> Option<Box<str>> {
    let summary = view.summary(id).filter(|summary| summary.crate_id == 0)?;
    let mut path = String::from("crate");
    for segment in summary.path.iter().skip(1) {
        path.push_str("::");
        path.push_str(segment);
    }
    Some(path.into())
}

impl ItemData<'_> {
    /// The item as a list shows it: with its visibility only when it is not `pub` (decided in
    /// M2's D6).
    #[must_use]
    pub fn listed(mut self) -> Self {
        if self.visibility.as_deref() == Some("pub") {
            self.visibility = None;
        }
        self
    }
}

/// The stability of an item of std, and of its use in `const`.
#[derive(Debug, Default, Serialize)]
pub struct StabilityNote<'a> {
    /// The release it is stable since.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_since: Option<&'a str>,
    /// The feature that it needs, being unstable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unstable_feature: Option<&'a str>,
    /// The release it is stable in `const` since.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub const_stable_since: Option<&'a str>,
    /// The feature that its use in `const` needs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub const_unstable_feature: Option<&'a str>,
    /// The feature that the default body (or value) of a trait's item needs, when the item itself
    /// is stable but the default is not (requirement R-STD-2): an implementation of the trait
    /// that leaves it out relies on an unstable default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_unstable: Option<&'a str>,
}

/// A section of the documentation.
#[derive(Debug, Serialize)]
pub struct Section {
    /// The heading (`Examples`).
    pub heading: Box<str>,
    /// How many lines the section has.
    pub lines: usize,
    /// The text, with `detail=full`, unless it was cut to fit the budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<Box<str>>,
}

/// `#[deprecated]`.
#[derive(Debug, Serialize)]
pub struct Deprecated<'a> {
    /// The version that deprecated the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<&'a str>,
    /// Why, or what to use instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'a str>,
}

/// An item of another crate, by the arguments that `get_item` takes for it.
#[derive(Debug, Serialize)]
pub struct ReexportOf {
    /// The crate, as `name@version` when the version is known.
    #[serde(rename = "crate")]
    pub name: Box<str>,
    /// The path where that crate defines the item. It may go through private modules, but
    /// `get_item` resolves definition paths too.
    pub path: Box<str>,
}

/// Where an item is defined.
#[derive(Debug, Serialize)]
pub struct SourceLocation<'a> {
    /// The file: relative to the package root, with `/`, when the file is in the package; else as
    /// the span gives it.
    pub file: Cow<'a, str>,
    /// The first and last lines (`434-466`), or the one line (`301`).
    pub lines: Box<str>,
    /// The file on docs.rs, when it is in the package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Box<str>>,
    /// The warning that the file is outside the package, which the query told, to take back
    /// with the location from a response that leaves it out.
    #[serde(skip)]
    pub warning: Option<Box<str>>,
}

/// Why an item could not be returned. The caller has the path it asked for.
#[derive(Debug, thiserror::Error)]
pub enum GetItemError {
    /// Nothing has the path: as a public path, a re-export from another crate, a definition path,
    /// or a member of an item at its parent path.
    #[error("no item has the path")]
    NotFound,
    /// More than one item has the path (requirement R-PR-7).
    #[error("{} items have the path", candidates.len())]
    Ambiguous {
        /// What each item is: its canonical path, or its name and `impl`.
        candidates: Box<[Box<str>]>,
    },
    /// The path goes on in another crate, through a re-export of its item (requirement R-PR-4).
    #[error("the path goes on in {}", .0.path)]
    Elsewhere(Box<Destination>),
    /// The path names an item that is not a module, to list (requirement R-LS-1).
    #[error("the path names a {kind}, not a module")]
    NotAModule {
        /// What the item is.
        kind: &'static str,
    },
    /// The path names an item that is not a trait, whose implementors to list (requirement
    /// R-TI-3).
    #[error("the path names a {kind}, not a trait")]
    NotATrait {
        /// What the item is.
        kind: &'static str,
    },
    /// The path names an item that is not a type, whose impls to list (requirement R-TI-2).
    #[error("the path names a {kind}, not a type")]
    NotAType {
        /// What the item is.
        kind: &'static str,
    },
    /// The cursor is not for this data or goes past its end (requirement R-OUT-3).
    #[error("{0}")]
    BadCursor(&'static str),
}

/// Where a path goes on in another crate (requirement R-PR-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    /// The path asked for, with the crate's name (`reqwest::header::HeaderMap`), at which the
    /// item found there is shown (requirement R-PR-2).
    pub entry: Box<str>,
    /// The path asked for, up to the re-export (`reqwest::header`).
    pub reexport: Box<str>,
    /// What it re-exports, by its path in the other crate (`http::header`).
    pub reexported: Box<str>,
    /// The path to look up in the other crate, which starts with its name as code writes it
    /// (`http::header::HeaderMap`).
    pub path: Box<str>,
    /// The crate as code writes it (`http`).
    pub lib_name: Box<str>,
    /// The crate as crates.io names it and the version that the JSON was built with, from its
    /// `html_root_url` on docs.rs.
    pub release: Option<(CrateName, Version)>,
    /// The crate is one of std, documented with the toolchain.
    pub is_std: bool,
    /// It is through a glob re-export (`pub use other::*;`), which may not have the item: then the
    /// path is not this crate's, and the next of `others`, the other globs it may be through, is
    /// tried (decided in M2's D6).
    pub via_glob: bool,
    /// The other globs that the path may go on through, in the order to try them.
    pub others: Vec<Self>,
}

/// Looks up the item at `request.path` in the crate of `view`.
///
/// # Errors
///
/// Returns an error when no item or more than one item has the path.
pub fn get_item<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
) -> Result<ItemResponse<'a>, GetItemError> {
    let index = view.index();
    let mut query = Query::new(view, request);
    let provenance = query.provenance();
    let with_root = with_root_name(view, request.name, request.path);
    let path = with_root.as_deref().unwrap_or(request.path);
    let resolution = query.resolve(path);
    if request.follow
        && matches!(
            resolution,
            Resolution::ExternalReexport(_) | Resolution::NotFound
        )
        && let Some(destination) = query.destination(path)
    {
        return Err(GetItemError::Elsewhere(Box::new(destination)));
    }
    let data = match resolution {
        Resolution::Item(id) => {
            let data = query.item_data(id, None);
            if let Some(reexport) = index.hidden_reexport(path) {
                query.hint_hidden_reexport(&reexport);
            }
            data
        },
        Resolution::Member { parent, member } => query.item_data(member, Some(parent)),
        Resolution::ExternalReexport(reexport) => Some(query.reexport_data(&reexport)),
        Resolution::Ambiguous(ids) => {
            return Err(GetItemError::Ambiguous {
                candidates: ids.iter().map(|&id| query.describe_item(id)).collect(),
            });
        },
        Resolution::AmbiguousMembers { parent, members } => {
            return Err(GetItemError::Ambiguous {
                candidates: members
                    .iter()
                    .map(|&member| query.describe_member(parent, member))
                    .collect(),
            });
        },
        Resolution::NotFound => None,
    };
    let Some(data) = data else {
        return Err(GetItemError::NotFound);
    };
    let Notes { warnings, hints } = query.finish();
    let mut response = ItemResponse {
        provenance,
        warnings,
        data,
        hints,
    };
    fit(&mut response, MAX_RESPONSE_CHARS);
    Ok(response)
}

/// `path` with its head written as the crate's lib name, the name of its root module, when it is
/// written as the package `name` and the two differ (`md-5::Md5` is `md5::Md5`). When the lib name
/// follows the package name (`md_5::md5::Md5`, as a path given without the crate name gets it),
/// the package name is dropped. `None` when nothing changes.
pub(super) fn with_root_name(view: &CrateView<'_>, name: &CrateName, path: &str) -> Option<String> {
    let root = view.item(view.root())?.name.as_deref()?;
    let (head, rest) = path
        .split_once("::")
        .map_or((path, None), |(head, rest)| (head, Some(rest)));
    let package = name.as_str().replace('-', "_");
    if head == root || !head.replace('-', "_").eq_ignore_ascii_case(&package) {
        return None;
    }
    let rest = rest.and_then(|rest| match rest.split_once("::") {
        Some((next, after)) if next == root => Some(after),
        None if rest == root => None,
        Some(_) | None => Some(rest),
    });
    Some(rest.map_or_else(|| root.to_owned(), |rest| format!("{root}::{rest}")))
}

/// The most characters of a response, as JSON (requirement R-OUT-3: about 8,000 tokens until the
/// estimate is calibrated).
pub const MAX_RESPONSE_CHARS: usize = 32_000;

/// Cuts `response` until its JSON has at most `limit` characters, marking it `truncated`
/// (requirement R-OUT-3): first the text of the last sections of the documentation, then its
/// first part, then the longer of the fields and the variants, with a warning of how many are
/// shown.
fn fit(response: &mut ItemResponse<'_>, limit: usize) {
    let fits = |response: &ItemResponse<'_>| {
        serde_json::to_string(response).map_or(true, |json| json.chars().count() <= limit)
    };
    if fits(response) {
        return;
    }
    response.data.truncated = true;
    for at in (0..response.data.sections.len()).rev() {
        if let Some(section) = response.data.sections.get_mut(at) {
            section.text = None;
        }
        if fits(response) {
            return;
        }
    }
    while let Some(docs) = response.data.docs.as_mut()
        && docs.chars().count() > MIN_DOCS_CHARS
    {
        let cut: String = docs
            .chars()
            .take(docs.chars().count().div_euclid(2))
            .collect();
        *docs = Cow::Owned(format!("{cut}…"));
        if fits(response) {
            return;
        }
    }
    let fields = response.data.fields.len() >= response.data.variants.len();
    // What rustdoc does not show (`/* private fields */`) stays at the end.
    let all = list_mut(&mut response.data, fields).clone();
    let shown_by_rustdoc = all
        .iter()
        .take_while(|entry| !entry.starts_with("/*"))
        .count();
    let (entries, notes) = all.split_at(shown_by_rustdoc);
    let mut kept = entries.len();
    while kept > 1 && !fits(response) {
        kept = kept.div_euclid(2);
        let mut shown = entries.get(..kept).unwrap_or_default().to_vec();
        shown.push(
            format!(
                "/* {} more not shown */",
                entries.len().saturating_sub(kept)
            )
            .into(),
        );
        shown.extend_from_slice(notes);
        *list_mut(&mut response.data, fields) = shown.into();
    }
    if kept < entries.len() {
        let mut warnings = std::mem::take(&mut response.warnings).into_vec();
        warnings.push(Cow::Owned(format!(
            "only {kept} of {} fields or variants are shown, to keep the response small",
            entries.len()
        )));
        response.warnings = warnings.into();
    }
}

/// The fields of `data`, or else its variants.
const fn list_mut<'b>(data: &'b mut ItemData<'_>, fields: bool) -> &'b mut Box<[Box<str>]> {
    if fields {
        &mut data.fields
    } else {
        &mut data.variants
    }
}

/// The documentation is not cut shorter than this many characters.
const MIN_DOCS_CHARS: usize = 200;

/// One lookup, with what it has found to warn about so far.
/// What a query says besides its data.
pub(super) struct Notes {
    /// What the caller should know about the data.
    pub(super) warnings: Box<[Cow<'static, str>]>,
    /// What to call next.
    pub(super) hints: Box<[Box<str>]>,
}

pub(super) struct Query<'a> {
    request: ItemRequest<'a>,
    view: &'a CrateView<'a>,
    index: &'a ArchivedPathIndex,
    renderer: Renderer<'a>,
    warnings: Vec<Cow<'static, str>>,
    hints: Vec<Box<str>>,
}

impl<'a> Query<'a> {
    /// A query of `view` for `request`.
    pub(super) const fn new(view: &'a CrateView<'a>, request: ItemRequest<'a>) -> Self {
        Self {
            request,
            view,
            index: view.index(),
            renderer: Renderer::new(view).public_only(!matches!(request.origin, Origin::Local {
                private: true,
                ..
            })),
            warnings: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// Adds a hint of what to call next.
    pub(super) fn hint(&mut self, hint: String) {
        self.hints.push(hint.into());
    }

    /// The kind of the item `id` of another crate, from its summary.
    pub(super) fn external_kind(&self, id: Id) -> &'static str {
        self.view
            .summary(id)
            .map_or("unknown", |summary| summary_kind(summary.kind))
    }

    /// The warnings and the hints of the query, the renderer's warnings last.
    pub(super) fn finish(self) -> Notes {
        let mut warnings = self.warnings;
        warnings.extend(self.renderer.into_warnings());
        Notes {
            warnings: warnings.into(),
            hints: self.hints.into(),
        }
    }

    /// The provenance, read from the JSON and checked against the request (§7.3).
    pub(super) fn provenance(&mut self) -> Provenance<'a> {
        let view = self.view;
        let build = match self.request.origin {
            Origin::DocsRs { build, .. } => build,
            Origin::Local { .. } | Origin::Std { .. } => None,
        };
        if let Some(build) = build
            && !build.missing.is_empty()
        {
            // Items behind a feature that docs.rs did not enable are not in the JSON (R-FT-1).
            self.warnings.push(Cow::Owned(format!(
                "docs.rs built {} {} without the features [{}]; items that need them are not in \
                 this documentation",
                self.request.name,
                self.request.version,
                build.missing.join(", ")
            )));
        }
        let requested = self.request.version;
        let version = if let Origin::Std { .. } = self.request.origin {
            // std writes its version with the commit (`1.101.0-nightly\t(b6eeb93c9\t…)`); the
            // toolchain's release is what the caller gave.
            Cow::Owned(requested.to_string())
        } else if let Some(version) = view.crate_version() {
            // Compared as versions, so that the requested one is not formatted to compare it.
            if Version::parse(version).ok().as_ref() != Some(requested) {
                self.warnings.push(Cow::Owned(format!(
                    "requested version {requested}, but the data is of version {version}"
                )));
            }
            Cow::Borrowed(version)
        } else {
            self.warnings.push(Cow::Borrowed(
                "the data has no crate version; showing the requested one",
            ));
            Cow::Owned(requested.to_string())
        };
        if view.newer_than_supported() {
            self.warnings.push(Cow::Owned(format!(
                "rustdoc JSON format {} is newer than the supported {}; read as if its changes were \
                 additive, so results unverified",
                view.format_version(),
                rustdoc_types::FORMAT_VERSION
            )));
        }
        let target = view.target_triple();
        if let Some(requested) = self.request.target
            && requested != target
        {
            self.warnings.push(Cow::Owned(format!(
                "the target {requested} was asked for, but the data is for {target}"
            )));
        }
        if self.index.is_incomplete() {
            self.warnings.push(Cow::Borrowed(
                "this crate re-exports its modules through too many paths; some public paths \
                 were not followed, so a path may not be found",
            ));
        }
        let FromWhere {
            source,
            package_source,
            resolved_from,
            project_from,
            toolchain,
            std_from,
            built_at,
            features_requested,
            features_effective,
            recorded_by,
        } = from_where(self.request.origin);
        Provenance {
            source,
            package_source,
            name: self.request.name.as_str(),
            version,
            resolved_from,
            project_from,
            toolchain,
            std_from,
            target,
            format_version: view.format_version(),
            // Every feature is in the list with `all_features`, which says it in fewer words.
            features: build
                .filter(|build| !build.all_features)
                .map(|build| &*build.features),
            all_features: build.is_some_and(|build| build.all_features),
            cfg: build.map_or(&[], |build| &build.cfg),
            built_at,
            features_requested,
            features_effective,
            recorded_by,
        }
    }

    /// The item `id`, which belongs to `parent` if it is a member. `None` if it is not in the
    /// crate, which a resolved path should not be.
    pub(super) fn item_data(&mut self, id: Id, parent: Option<Id>) -> Option<ItemData<'a>> {
        let item = self.view.item(id)?;
        let name = item.name.as_deref().unwrap_or("_");
        let detail = self.request.detail;
        let signature = self.renderer.signature(item);
        let mut data = ItemData {
            kind: kind_of(item, parent.is_some()),
            visibility: self.visibility_of(item, parent),
            hidden: self.is_member()
                && (self.index.is_hidden(id)
                    || parent.is_some_and(|parent| self.index.is_hidden(parent))),
            ..ItemData::default()
        };
        if let Some(parent) = parent {
            let parent_path = self.path_of(parent);
            data.path = format!("{parent_path}::{name}").into();
            data.parent = Some(parent_path);
        } else {
            data.path = self.path_of(id);
        }
        self.cfg_note(id, parent, &mut data);
        if detail == Detail::Brief {
            // One line: a `macro_rules!` macro shows its rules on the lines after it.
            data.signature = Some(signature.lines().next().unwrap_or_default().into());
            return Some(data);
        }
        data.signature = Some(signature.into());
        data.where_predicates = boxed(self.renderer.where_predicates(item));
        data.deprecated = item.deprecation.as_ref().map(|deprecation| Deprecated {
            since: deprecation.since.as_deref(),
            note: deprecation.note.as_deref(),
        });
        data.must_use = item
            .attrs
            .iter()
            .any(|attr| matches!(attr, Attribute::MustUse { .. }));
        data.non_exhaustive = item.attrs.contains(&Attribute::NonExhaustive);
        data.stability = self.stability(item, &data.path);
        data.source_location = self.source_location(item);
        if let Some(parent) = parent {
            data.impl_header = self.impl_header(parent, id);
        } else {
            data.also_reexported_at = self
                .index
                .public_paths(id)
                .iter()
                .skip(1)
                .map(PublicPath::joined)
                .collect();
        }
        data.fields = self.fields(item);
        data.variants = self.variants(item);
        if let Some(docs) = item.docs.as_deref() {
            let split = docs::split(docs);
            let full = detail == Detail::Full;
            let (first, more) = first_paragraph(&split.intro);
            let intro = if full { split.intro.as_str() } else { first };
            if !intro.is_empty() {
                data.docs = Some(Cow::Owned(intro.to_owned()));
            }
            data.docs_truncated = !full && (more || !split.sections.is_empty());
            data.sections = split
                .sections
                .into_iter()
                .map(|section| Section {
                    heading: section.heading.into(),
                    lines: section.text.lines().count(),
                    text: full.then(|| section.text.into()),
                })
                .collect();
        }
        if detail == Detail::Full {
            data.attrs = item.attrs.iter().filter_map(attribute_text).collect();
        }
        Some(data)
    }

    /// A re-export of an item of another crate, with the arguments to look the item up there.
    fn reexport_data(&mut self, reexport: &ExternalReexport) -> ItemData<'a> {
        let summary = self.view.summary(reexport.target);
        let reexport_of = self.reexport_target(reexport);
        if let Some(target) = &reexport_of {
            self.hints.push(
                format!(
                    "call get_item with crate {} and path {} for its documentation",
                    target.name, target.path
                )
                .into(),
            );
        }
        ItemData {
            path: reexport.path.joined(),
            kind: summary.map_or("unknown", |summary| summary_kind(summary.kind)),
            reexport_of,
            ..ItemData::default()
        }
    }

    /// The stability of `item`, at `path`, with a warning if the project's toolchain does not
    /// have it as stable (§3 Q17).
    fn stability(&mut self, item: &'a Item, path: &str) -> Option<StabilityNote<'a>> {
        let mut note = StabilityNote::default();
        let level = |stability: Option<&'a Stability>| match stability {
            Some(Stability {
                level: StabilityLevel::Stable { since },
                ..
            }) => (since.as_deref(), None),
            Some(Stability {
                feature,
                level: StabilityLevel::Unstable,
            }) => (None, Some(feature.as_str())),
            None => (None, None),
        };
        (note.stable_since, note.unstable_feature) = level(item.stability.as_deref());
        (note.const_stable_since, note.const_unstable_feature) =
            level(item.const_stability.as_deref());
        let default_unstable = match &item.inner {
            ItemEnum::Function(function) => function.default_unstable.as_deref(),
            ItemEnum::AssocConst {
                default_unstable, ..
            }
            | ItemEnum::AssocType {
                default_unstable, ..
            } => default_unstable.as_deref(),
            ItemEnum::Module(_)
            | ItemEnum::ExternCrate { .. }
            | ItemEnum::Use(_)
            | ItemEnum::Union(_)
            | ItemEnum::Struct(_)
            | ItemEnum::StructField(_)
            | ItemEnum::Enum(_)
            | ItemEnum::Variant(_)
            | ItemEnum::Trait(_)
            | ItemEnum::TraitAlias(_)
            | ItemEnum::Impl(_)
            | ItemEnum::TypeAlias(_)
            | ItemEnum::Constant { .. }
            | ItemEnum::Static(_)
            | ItemEnum::ExternType
            | ItemEnum::Macro(_)
            | ItemEnum::ProcMacro(_)
            | ItemEnum::Primitive(_) => None,
        };
        note.default_unstable = default_unstable.map(|default| default.feature.as_str());
        if let Some(feature) = note.default_unstable {
            self.warnings.push(Cow::Owned(format!(
                "the default of {path} is unstable (#![feature({feature})]): an implementation \
                 on stable Rust must write it"
            )));
        }
        let project_release = match self.request.origin {
            Origin::Std {
                project_release, ..
            } => project_release,
            Origin::DocsRs { .. } | Origin::Local { .. } => None,
        };
        if let Some(feature) = note.unstable_feature {
            self.warnings.push(Cow::Owned(format!(
                "{path} is unstable: it needs a nightly toolchain and #![feature({feature})]"
            )));
        } else if let (Some(since), Some(project)) = (note.stable_since, project_release)
            && Version::parse(since).is_ok_and(|since| {
                (since.major, since.minor, since.patch)
                    > (project.major, project.minor, project.patch)
            })
        {
            self.warnings.push(Cow::Owned(format!(
                "{path} is stable since Rust {since}, after the project's toolchain {project}"
            )));
        }
        (note.stable_since.is_some()
            || note.unstable_feature.is_some()
            || note.default_unstable.is_some())
        .then_some(note)
    }

    /// The conditions that the entry `id` of a list is written with (an `impl`), or none.
    pub(super) fn own_conditions(&self, id: Id) -> Conditions {
        let mut data = ItemData::default();
        self.cfg_note(id, None, &mut data);
        if data.required_features.is_none() && data.other_cfg.is_empty() {
            // Only that some could not be read is worth telling.
            return Conditions {
                cfg_source: data.cfg_source.filter(|source| *source == "unknown"),
                ..Conditions::default()
            };
        }
        Conditions {
            required_features: data.required_features,
            other_cfg: data.other_cfg,
            cfg_source: data.cfg_source,
            enabled_in_project: data.enabled_in_project,
            enabled_in_dev_only: data.enabled_in_dev_only,
        }
    }

    /// Fills in the conditions that the item `id` needs (requirement R-FT-2): its own, those of
    /// the `impl`, trait or type it belongs to, and those of the modules and re-exports on the
    /// canonical path to it. rustdoc gives a child item none of its parent's.
    fn cfg_note(&self, id: Id, parent: Option<Id>, data: &mut ItemData<'a>) {
        let mut carriers = parent.map_or_else(
            || self.index.canonical_via(id),
            |parent| {
                let mut carriers = self.index.canonical_via(parent);
                carriers.push(parent);
                carriers.extend(
                    path_index::own_impls(self.view, parent)
                        .find(|(_, contents)| contents.items.contains(&id))
                        .map(|(impl_item, _)| impl_item.id),
                );
                carriers
            },
        );
        carriers.push(id);
        let mut conjuncts: Vec<Cfg> = Vec::new();
        let mut own = None;
        let mut unreadable = false;
        for &carrier in &carriers {
            let Some(item) = self.view.item(carrier) else {
                continue;
            };
            let (mut doc_cfg, mut traces) = (Vec::new(), Vec::new());
            for attribute in &item.attrs {
                let Attribute::Other(text) = attribute else {
                    continue;
                };
                match Cfg::of_attribute(text) {
                    Some(CfgAttribute::Cfg(CfgSource::DocCfg, cfg)) => doc_cfg.push(cfg),
                    Some(CfgAttribute::Cfg(CfgSource::CfgTrace, cfg)) => traces.push(cfg),
                    Some(CfgAttribute::Unreadable) => unreadable = true,
                    None => {},
                }
            }
            // What the crate writes for its documentation comes first (requirement R-FT-2).
            let (source, cfgs) = if doc_cfg.is_empty() {
                (CfgSource::CfgTrace, traces)
            } else {
                (CfgSource::DocCfg, doc_cfg)
            };
            if cfgs.is_empty() {
                continue;
            }
            if carrier == id {
                own = Some(source);
            }
            for conjunct in Cfg::all(cfgs).for_build().conjuncts() {
                if !conjuncts.contains(&conjunct) {
                    conjuncts.push(conjunct);
                }
            }
        }
        if let Some(project) = self.request.project_features {
            let in_build = |features| self.in_build(&conjuncts, features);
            let normal = in_build(project.normal);
            data.enabled_in_project = Some(match normal {
                Some(true) => Enabled::Yes,
                Some(false) => Enabled::No,
                None => Enabled::Unknown,
            });
            data.enabled_in_dev_only =
                normal != Some(true) && in_build(project.with_dev) == Some(true);
        }
        let (features, other): (Vec<_>, Vec<_>) =
            conjuncts.into_iter().partition(Cfg::is_features_only);
        let inherited = !(features.is_empty() && other.is_empty());
        data.cfg_source = if unreadable {
            Some("unknown")
        } else {
            match own {
                Some(CfgSource::DocCfg) => Some("doc_cfg"),
                Some(CfgSource::CfgTrace) => Some("cfg_trace"),
                None => inherited.then_some("inherited"),
            }
        };
        if !features.is_empty() {
            data.required_features = Some(Cfg::all(features).features_text().into());
        }
        data.other_cfg = other.iter().map(|cfg| cfg.to_string().into()).collect();
    }

    /// Whether the conditions `conjuncts` hold in a build of the crate with `features` (`None`
    /// when it is not built), or `None` if that is not known (requirement R-FT-2).
    ///
    /// A condition without features held when docs.rs built the JSON, which is for the target of
    /// the build, so it holds in the build too, unless it needs a `--cfg` that docs.rs set
    /// (`tokio_unstable`), or the JSON is for another target. `doc` and `docsrs` are false.
    fn in_build(&self, conjuncts: &[Cfg], features: Option<&BTreeSet<Box<str>>>) -> Option<bool> {
        let Some(features) = features else {
            return Some(false);
        };
        let docs_rs_cfg: &[Box<str>] = match self.request.origin {
            Origin::DocsRs {
                build: Some(build), ..
            } => &build.cfg,
            Origin::DocsRs { build: None, .. } | Origin::Local { .. } | Origin::Std { .. } => &[],
        };
        let same_target = self
            .request
            .target
            .is_none_or(|target| target == self.view.target_triple());
        let only_in_docs = |name: &str| matches!(name, "doc" | "docsrs");
        let value_of = |cfg: &Cfg| match cfg {
            Cfg::NameValue(name, value) if &**name == "feature" => Some(features.contains(value)),
            Cfg::Name(name) if only_in_docs(name) => Some(false),
            Cfg::Bool(_)
            | Cfg::Name(_)
            | Cfg::NameValue(..)
            | Cfg::All(_)
            | Cfg::Any(_)
            | Cfg::Not(_) => None,
        };
        let mut known = Some(true);
        for conjunct in conjuncts {
            let set_by_docs_rs =
                conjunct.mentions(&|name: &str| docs_rs_cfg.iter().any(|cfg| &**cfg == name));
            let value = if !conjunct.has_no_features() || set_by_docs_rs || !same_target {
                conjunct.eval(&value_of)
            } else if conjunct.mentions(&only_in_docs) {
                // Kept as it is only when it is false in a build (`Cfg::for_build`).
                Some(false)
            } else {
                Some(true)
            };
            match value {
                Some(false) => return Some(false),
                Some(true) => {},
                None => known = None,
            }
        }
        known
    }

    /// Who can use `item`, which belongs to `parent` if it is a member, in a workspace member's
    /// build (requirement R-WS-3, decided in M2's D6): `None` elsewhere, where only public items
    /// are shown, and for a member whose parent is not known.
    fn visibility_of(&self, item: &Item, parent: Option<Id>) -> Option<Box<str>> {
        if !self.is_member() {
            return None;
        }
        Some(match &item.visibility {
            Visibility::Public => "pub".into(),
            Visibility::Crate => "pub(crate)".into(),
            Visibility::Restricted { .. } if self.index.is_module_private(item.id) => {
                "private".into()
            },
            Visibility::Restricted { path, .. } => format!("pub(in {})", in_crate(path)).into(),
            // Its parent's: a trait or an enum; for an item of an `impl` of a trait, the trait's if
            // it is of this crate, else the type's. Unknown without its parent.
            Visibility::Default => {
                let parent = parent?;
                let of_trait = path_index::own_impls(self.view, parent)
                    .find(|(_, contents)| contents.items.contains(&item.id))
                    .and_then(|(_, contents)| contents.trait_.as_ref())
                    .and_then(|path| self.view.item(path.id));
                return self.visibility_of(of_trait.or_else(|| self.view.item(parent))?, None);
            },
        })
    }

    /// What `path` resolves to: in a crate that is not a workspace member, its public items only
    /// (requirement R-SH-5, decided in M2's D6).
    pub(super) fn resolve(&self, path: &str) -> Resolution {
        if self.is_member() {
            self.index.resolve(self.view, path)
        } else {
            self.index.resolve_public(self.view, path)
        }
    }

    /// Whether the re-export of another crate's item at `path` is told hidden: in a workspace
    /// member, when it goes through an item of `#[doc(hidden)]` (decided in M2's D6).
    pub(super) fn hidden_reexport(&self, path: &str) -> bool {
        self.is_member() && self.index.is_hidden_reexport(path)
    }

    /// Whether the crate is a workspace member's, built here with its private items (decided in
    /// M2's D2).
    pub(super) const fn is_member(&self) -> bool {
        matches!(self.request.origin, Origin::Local { private: true, .. })
    }

    /// Whether the crate is built here, a workspace member's or a dependency's, whose version the
    /// project decides (decided in M2's D7).
    const fn is_built_here(&self) -> bool {
        matches!(self.request.origin, Origin::Local { .. })
    }

    /// The canonical path of `id`; in a workspace member, its path in the crate,
    /// `crate::a::Item`, if it has no public path (requirement R-PR-2, decided in M2's D6); or its
    /// definition path with a warning.
    pub(super) fn path_of(&mut self, id: Id) -> Box<str> {
        if let Some(path) = self.index.canonical_path(id) {
            return path.joined();
        }
        if self.is_member()
            && let Some(path) = crate_path(self.view, id)
        {
            return path;
        }
        let path: Box<str> = self.view.summary(id).map_or_else(
            || {
                self.view
                    .item(id)
                    .and_then(|item| item.name.as_deref())
                    .unwrap_or("_")
                    .into()
            },
            |summary| summary.path.join("::").into_boxed_str(),
        );
        self.warnings.push(Cow::Owned(format!(
            "{path} has no public path; it is shown as defined"
        )));
        path
    }

    /// The header of the `impl` of `parent` that has `member`, with its where clause. `None` for
    /// an item of a trait and a variant.
    fn impl_header(&mut self, parent: Id, member: Id) -> Option<Box<str>> {
        let (impl_item, _) = path_index::own_impls(self.view, parent)
            .find(|(_, contents)| contents.items.contains(&member))?;
        Some(self.impl_text(impl_item))
    }

    /// The header of `impl_item`, an `impl`, with its where clause, as Rust writes it.
    pub(super) fn impl_text(&mut self, impl_item: &Item) -> Box<str> {
        let mut header = self.renderer.signature(impl_item);
        let predicates = self.renderer.where_predicates(impl_item);
        for (position, predicate) in predicates.iter().enumerate() {
            header.push_str(if position == 0 { " where " } else { ", " });
            header.push_str(predicate);
        }
        header.into()
    }

    /// The type `ty` as Rust writes it.
    pub(super) fn type_text(&mut self, ty: &rustdoc_types::Type) -> Box<str> {
        self.renderer.render_type(ty).into()
    }

    /// The where predicates of `item`, as Rust writes them.
    pub(super) fn where_text(&mut self, item: &Item) -> Vec<String> {
        self.renderer.where_predicates(item)
    }

    /// The fields of a struct with named fields or of a union, one each. A tuple struct has them
    /// in its signature (requirement R-SH-7).
    fn fields(&mut self, item: &Item) -> Box<[Box<str>]> {
        let (fields, has_stripped_fields) = if let ItemEnum::Struct(Struct {
            kind:
                StructKind::Plain {
                    fields,
                    has_stripped_fields,
                },
            ..
        }) = &item.inner
        {
            (fields, *has_stripped_fields)
        } else if let ItemEnum::Union(contents) = &item.inner {
            (&contents.fields, contents.has_stripped_fields)
        } else {
            return Box::default();
        };
        // Another crate's fields that are not public or are hidden (decided in M2's D6).
        let shown: Vec<Id> = fields
            .iter()
            .copied()
            .filter(|&field| {
                self.view
                    .item(field)
                    .is_none_or(|field| self.renderer.shows_field(field))
            })
            .collect();
        let mut fields_text = self.signatures(&shown);
        if has_stripped_fields || shown.len() < fields.len() {
            fields_text.push("/* private fields */".into());
        }
        let fields = fields_text;
        fields.into()
    }

    /// The variants of an enum, one each.
    fn variants(&mut self, item: &Item) -> Box<[Box<str>]> {
        let ItemEnum::Enum(contents) = &item.inner else {
            return Box::default();
        };
        // Another crate's hidden variants (decided in M2's D6).
        let shown: Vec<Id> = contents
            .variants
            .iter()
            .copied()
            .filter(|&variant| self.is_member() || !self.index.is_hidden(variant))
            .collect();
        let mut variants = self.signatures(&shown);
        if contents.has_stripped_variants || shown.len() < contents.variants.len() {
            variants.push("/* some variants omitted */".into());
        }
        variants.into()
    }

    /// The signatures of the items `ids`, skipping any that are not in the crate.
    fn signatures(&mut self, ids: &[Id]) -> Vec<Box<str>> {
        ids.iter()
            .filter_map(|&id| self.view.item(id))
            .map(|item| self.renderer.signature(item).into())
            .collect()
    }

    /// Where `item` is defined, with a docs.rs URL when the file is in the package. A file outside
    /// it (generated in `OUT_DIR`, or a path on the build host) gets no URL and a warning
    /// (requirement R-OUT-8).
    fn source_location(&mut self, item: &'a Item) -> Option<SourceLocation<'a>> {
        let span = item.span.as_ref()?;
        let file = span.filename.to_str()?;
        let in_package = span
            .filename
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        let lines = if span.begin.0 == span.end.0 {
            span.begin.0.to_string()
        } else {
            format!("{}-{}", span.begin.0, span.end.0)
        };
        // std is built from the rust repository at a commit: `/rustc/<commit>/library/…`.
        if let Origin::Std { .. } = self.request.origin {
            let url = std_source(file).map(|(commit, path)| {
                let anchor = if span.begin.0 == span.end.0 {
                    format!("L{}", span.begin.0)
                } else {
                    format!("L{}-L{}", span.begin.0, span.end.0)
                };
                format!("https://github.com/rust-lang/rust/blob/{commit}/{path}#{anchor}").into()
            });
            return Some(SourceLocation {
                file: Cow::Borrowed(file),
                lines: lines.into(),
                url,
                warning: None,
            });
        }
        let on_docs_rs = matches!(self.request.origin, Origin::DocsRs { .. });
        // A local build's spans are relative to the workspace's root where cargo ran, or absolute
        // outside it, as a dependency's that a derive generated code of (decided in M3a).
        let (file, in_package) = match self.request.origin {
            Origin::Local {
                roots: Some((workspace, package)),
                ..
            } => {
                let full = source_root::lexically(&workspace.join(&span.filename));
                match full.strip_prefix(package) {
                    Ok(inside) => (Cow::Owned(source_root::slash_separated(inside)), true),
                    // As the span gives it, with `/` (decided in M3a).
                    Err(_) if std::path::MAIN_SEPARATOR == '/' => (Cow::Borrowed(file), false),
                    Err(_) => (
                        Cow::Owned(file.replace(std::path::MAIN_SEPARATOR, "/")),
                        false,
                    ),
                }
            },
            Origin::DocsRs { .. } | Origin::Local { .. } | Origin::Std { .. } => {
                (Cow::Borrowed(file), in_package)
            },
        };
        // A local build's record from before has no roots to tell the package's files by.
        let warning = match self.request.origin {
            Origin::DocsRs { .. } if !in_package => Some(format!(
                "{file} is outside the package, so the definition has no docs.rs URL"
            )),
            Origin::Local { roots: Some(_), .. } if !in_package => {
                Some(format!("{file} is outside the package"))
            },
            Origin::DocsRs { .. } | Origin::Local { .. } | Origin::Std { .. } => None,
        };
        if let Some(warning) = &warning {
            self.warnings.push(Cow::Owned(warning.clone()));
        }
        let url = (in_package && on_docs_rs).then(|| {
            let version: &dyn fmt::Display = match self.view.crate_version() {
                Some(version) => &version.to_owned(),
                None => self.request.version,
            };
            format!(
                "https://docs.rs/crate/{}/{version}/source/{}#{lines}",
                self.request.name,
                source_root::slash_separated(&span.filename)
            )
            .into()
        });
        Some(SourceLocation {
            file,
            lines: lines.into(),
            url,
            warning: warning.map(String::into_boxed_str),
        })
    }

    /// Takes the source location out of `data`, for a response that leaves it out, with the
    /// warning it told.
    pub(super) fn drop_source_location(&mut self, data: &mut ItemData<'a>) {
        if let Some(warning) = data
            .source_location
            .take()
            .and_then(|location| location.warning)
            && let Some(at) = self.warnings.iter().rposition(|told| **told == *warning)
        {
            self.warnings.remove(at);
        }
    }

    /// The crate and definition path of the item that `reexport` re-exports, to look it up
    /// there. `None`, with a warning, when the data does not describe it.
    pub(super) fn reexport_target(&mut self, reexport: &ExternalReexport) -> Option<ReexportOf> {
        let summary = self.view.summary(reexport.target);
        let external = summary.and_then(|summary| self.view.external_crate(summary.crate_id));
        let (Some(definition), Some(external)) = (&reexport.definition, external) else {
            self.warnings.push(Cow::Owned(format!(
                "the item that {} re-exports from another crate is not described in this \
                 crate's data",
                reexport.path
            )));
            return None;
        };
        // A crate of std, or of the sysroot that std depends on, which in std's own data have
        // paths of the build of the toolchain.
        let of_std = matches!(self.request.origin, Origin::Std { .. })
            || external
                .html_root_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://doc.rust-lang.org/"))
            || external
                .path
                .components()
                .any(|component| component.as_os_str() == "rustlib");
        let name: Box<str> = if self.is_built_here() {
            // By its crate's name, as the JSON of the crate built here names it, whose version the
            // project decides: the JSON's `html_root_url` may be old (decided in M2's D6 and D7).
            external.name.as_str().into()
        } else if let Some((name, version)) =
            external.html_root_url.as_deref().and_then(docs_rs_release)
        {
            format!("{name}@{version}").into()
        } else if of_std {
            // A crate of std has the version of the toolchain, as `get_item` shows it.
            external.name.as_str().into()
        } else {
            self.warnings.push(Cow::Owned(format!(
                "the version of {} that is re-exported is unknown",
                external.name
            )));
            external.name.as_str().into()
        };
        Some(ReexportOf {
            name,
            path: definition.joined(),
        })
    }

    /// Where the path asked for goes on in another crate: the longest prefix of it that
    /// re-exports another crate's item. `None` when there is none, or the crate is a private
    /// dependency of std (sysroot's `hashbrown`), which is not the crate of crates.io
    /// (requirement R-PR-4).
    pub(super) fn destination(&self, path: &str) -> Option<Destination> {
        let Some(crossing) = self.index.crossing(path) else {
            return self.glob_destination(path);
        };
        let summary = self.view.summary(crossing.reexport.target)?;
        let external = self.view.external_crate(summary.crate_id)?;
        // The JSON of a crate built here has the `html_root_url` that a crate writes itself, if
        // any, which may be old: the project's build tells the version (decided in M2's D6 and
        // D7).
        let release = external
            .html_root_url
            .as_deref()
            .and_then(docs_rs_release)
            .filter(|_| !self.is_built_here());
        let is_std = external
            .html_root_url
            .as_deref()
            .is_some_and(|url| url.starts_with("https://doc.rust-lang.org/"))
            || matches!(
                external.name.as_str(),
                "std" | "core" | "alloc" | "proc_macro" | "test"
            );
        // Formats 54 to 56 have no path, so a private dependency of std cannot be told there.
        let in_sysroot = external
            .path
            .components()
            .any(|component| component.as_os_str() == "rustlib");
        if in_sysroot && !is_std {
            return None;
        }
        Some(Destination {
            entry: path.into(),
            reexport: crossing.reexport.path.joined(),
            reexported: crossing
                .reexport
                .definition
                .as_ref()
                .map_or_else(|| external.name.as_str().into(), PublicPath::joined),
            path: crossing.continued,
            lib_name: external.name.as_str().into(),
            release,
            is_std,
            via_glob: false,
            others: Vec::new(),
        })
    }

    /// Where `path`, which this crate does not have, may go on through a glob re-export of another
    /// crate (`pub use other::*;`): in a workspace member only, which the caller follows into
    /// another member only (decided in M2's D6). Not in another crate, as std (requirement R-PR-3).
    /// The globs with the longest prefix come first, then in the order they are written; each
    /// goes by what it re-exports, as the data has it, not by its `use` source, which may begin
    /// with a local name.
    fn glob_destination(&self, path: &str) -> Option<Destination> {
        if !self.is_member() {
            return None;
        }
        let segments: Vec<&str> = path.split("::").collect();
        let same = |at: usize, want: &str, have: &str| {
            // The crate's name, as `-` and `_` the same.
            want == have || at == 0 && want.replace('-', "_") == have.replace('-', "_")
        };
        let mut found: Vec<(usize, Destination)> = self
            .index
            .external_globs()
            .into_iter()
            .filter_map(|glob| {
                let prefix = glob.prefix.segments();
                let matches = prefix.len() < segments.len()
                    && prefix
                        .iter()
                        .zip(&segments)
                        .enumerate()
                        .all(|(at, (want, have))| same(at, want, have));
                let target = glob.target.as_ref().filter(|_| matches)?;
                let rest = segments.get(prefix.len()..)?.join("::");
                Some((prefix.len(), Destination {
                    entry: path.into(),
                    reexport: glob.prefix.joined(),
                    reexported: target.joined(),
                    path: format!("{}::{rest}", target.joined()).into(),
                    lib_name: target.segments().first()?.clone(),
                    release: None,
                    is_std: false,
                    via_glob: true,
                    others: Vec::new(),
                }))
            })
            .collect();
        // Stable: the order written, among the globs with prefixes as long.
        found.sort_by_key(|(prefix, _)| std::cmp::Reverse(*prefix));
        let mut found = found.into_iter().map(|(_, destination)| destination);
        let mut first = found.next()?;
        first.others = found.collect();
        Some(first)
    }

    /// A hint about the re-export of another crate's item that has the path of the item returned
    /// (`serde::Deserialize` is also `serde_derive`'s derive macro).
    fn hint_hidden_reexport(&mut self, reexport: &ExternalReexport) {
        let kind = self
            .view
            .summary(reexport.target)
            .map_or("item", |summary| summary_kind(summary.kind));
        if let Some(target) = self.reexport_target(reexport) {
            self.hints.push(
                format!(
                    "{} also names a {kind} of another crate; call get_item with crate {} and path {} for it",
                    reexport.path, target.name, target.path
                )
                .into(),
            );
        }
    }

    /// What the item `id` is, to tell apart the items of an ambiguous path: its kind and
    /// canonical path, which may be the same for items of different kinds.
    fn describe_item(&self, id: Id) -> Box<str> {
        let Some(item) = self.view.item(id) else {
            return "an item that is not in the data".into();
        };
        let kind = kind_of(item, false);
        let path = self.index.canonical_path(id).map_or_else(
            || item.name.as_deref().unwrap_or("_").into(),
            |path| path.joined(),
        );
        format!("{kind} {path}").into()
    }

    /// What the member `member` of `parent` is, to tell apart the members of the same name: the
    /// `impl` of `parent` that has it.
    fn describe_member(&mut self, parent: Id, member: Id) -> Box<str> {
        let name = self
            .view
            .item(member)
            .and_then(|item| item.name.as_deref())
            .unwrap_or("_");
        let owner = path_index::own_impls(self.view, parent)
            .find(|(_, contents)| contents.items.contains(&member));
        owner.map_or_else(
            || name.into(),
            |(owner, _)| format!("{name} in {}", self.renderer.signature(owner)).into(),
        )
    }
}

/// The commit and the path in the rust repository of a file of std, which rustdoc writes as
/// `/rustc/<commit>/library/alloc/src/vec/mod.rs`.
fn std_source(file: &str) -> Option<(&str, &str)> {
    let (commit, path) = file.strip_prefix("/rustc/")?.split_once('/')?;
    (commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some((commit, path))
}

/// An attribute as Rust writes it, for `detail=full`. The traces of `cfg` and `doc(cfg)` are left
/// to the feature notes (requirement R-FT-2), `#[doc = "…"]` is the documentation itself, and the
/// attributes internal to rustc (`#[attr = RustcDiagnosticItem("Vec")]`) say nothing to a user.
fn attribute_text(attribute: &Attribute) -> Option<Box<str>> {
    let text = match attribute {
        Attribute::NonExhaustive => "#[non_exhaustive]".to_owned(),
        Attribute::MustUse { reason: None } => "#[must_use]".to_owned(),
        Attribute::MustUse {
            reason: Some(reason),
        } => format!("#[must_use = {reason:?}]"),
        Attribute::MacroExport => "#[macro_export]".to_owned(),
        Attribute::ExportName(name) => format!("#[export_name = {name:?}]"),
        Attribute::LinkSection(name) => format!("#[link_section = {name:?}]"),
        Attribute::AutomaticallyDerived => "#[automatically_derived]".to_owned(),
        Attribute::Repr(repr) => {
            let mut hints = vec![
                match repr.kind {
                    rustdoc_types::ReprKind::Rust => "Rust",
                    rustdoc_types::ReprKind::C => "C",
                    rustdoc_types::ReprKind::Transparent => "transparent",
                    rustdoc_types::ReprKind::Simd => "simd",
                }
                .to_owned(),
            ];
            hints.extend(repr.int.clone());
            hints.extend(repr.align.map(|align| format!("align({align})")));
            hints.extend(repr.packed.map(|packed| format!("packed({packed})")));
            format!("#[repr({})]", hints.join(", "))
        },
        Attribute::NoMangle => "#[no_mangle]".to_owned(),
        Attribute::TargetFeature { enable } => {
            format!("#[target_feature(enable = {:?})]", enable.join(","))
        },
        Attribute::Other(text) => {
            let left_out = [
                "#[<cfg",
                "#[attr = Cfg",
                "#[doc(cfg",
                "#[doc =",
                "#[attr = Rustc",
            ]
            .iter()
            .any(|prefix| text.starts_with(prefix));
            if left_out {
                return None;
            }
            text.clone()
        },
    };
    Some(text.into())
}

/// `values` as a boxed slice of boxed strings.
fn boxed(values: Vec<String>) -> Box<[Box<str>]> {
    values.into_iter().map(String::into_boxed_str).collect()
}

/// The kind of `item`, as rustdoc's `ItemKind` names it, except that a function that belongs to
/// a type or trait is a `method`.
pub(super) const fn kind_of(item: &Item, is_member: bool) -> &'static str {
    match &item.inner {
        ItemEnum::Function(_) if is_member => "method",
        ItemEnum::Function(_) => "function",
        ItemEnum::Module(_) => "module",
        ItemEnum::ExternCrate { .. } => "extern_crate",
        ItemEnum::Use(_) => "use",
        ItemEnum::Union(_) => "union",
        ItemEnum::Struct(_) => "struct",
        ItemEnum::StructField(_) => "struct_field",
        ItemEnum::Enum(_) => "enum",
        ItemEnum::Variant(_) => "variant",
        ItemEnum::Trait(_) => "trait",
        ItemEnum::TraitAlias(_) => "trait_alias",
        ItemEnum::Impl(_) => "impl",
        ItemEnum::TypeAlias(_) => "type_alias",
        ItemEnum::Constant { .. } => "constant",
        ItemEnum::Static(_) => "static",
        ItemEnum::ExternType => "extern_type",
        ItemEnum::Macro(_) => "macro",
        ItemEnum::ProcMacro(proc_macro) => match proc_macro.kind {
            MacroKind::Bang => "macro",
            MacroKind::Attr => "proc_attribute",
            MacroKind::Derive => "proc_derive",
        },
        ItemEnum::Primitive(_) => "primitive",
        ItemEnum::AssocConst { .. } => "assoc_const",
        ItemEnum::AssocType { .. } => "assoc_type",
    }
}

/// The kind of an item of another crate, from its summary in `paths`.
pub(super) const fn summary_kind(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Module => "module",
        ItemKind::ExternCrate => "extern_crate",
        ItemKind::Use => "use",
        ItemKind::Struct => "struct",
        ItemKind::StructField => "struct_field",
        ItemKind::Union => "union",
        ItemKind::Enum => "enum",
        ItemKind::Variant => "variant",
        ItemKind::Function => "function",
        ItemKind::TypeAlias => "type_alias",
        ItemKind::Constant => "constant",
        ItemKind::Trait => "trait",
        ItemKind::TraitAlias => "trait_alias",
        ItemKind::Impl => "impl",
        ItemKind::Static => "static",
        ItemKind::ExternType => "extern_type",
        ItemKind::Macro => "macro",
        ItemKind::ProcAttribute => "proc_attribute",
        ItemKind::ProcDerive => "proc_derive",
        ItemKind::AssocConst => "assoc_const",
        ItemKind::AssocType => "assoc_type",
        ItemKind::Primitive => "primitive",
        ItemKind::Keyword => "keyword",
        ItemKind::Attribute => "attribute",
    }
}

/// The crate name and version of a docs.rs `html_root_url`
/// (`https://docs.rs/axum-macros/0.5.1/x86_64-unknown-linux-gnu/`), if both are valid.
fn docs_rs_release(url: &str) -> Option<(CrateName, Version)> {
    let mut segments = url.strip_prefix("https://docs.rs/")?.split('/');
    let name = segments.next()?.parse().ok()?;
    let version = Version::parse(segments.next()?).ok()?;
    Some((name, version))
}

/// The first paragraph of `docs`, and whether anything follows it. A blank line inside a code
/// block does not end the paragraph.
pub(super) fn first_paragraph(docs: &str) -> (&str, bool) {
    let docs = docs.trim_start_matches(['\n', '\r']);
    let mut in_code = false;
    let mut end = docs.len();
    let mut offset = 0;
    for line in docs.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
        } else if trimmed.is_empty() && !in_code {
            end = offset;
            break;
        }
        offset = offset.saturating_add(line.len());
    }
    let (first, rest) = docs.split_at_checked(end).unwrap_or((docs, ""));
    (first.trim_end(), !rest.trim().is_empty())
}

#[cfg(test)]
pub(super) mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use rustdoc_types::{
        Crate,
        Deprecation,
        Enum,
        ExternalCrate,
        Function,
        FunctionHeader,
        FunctionSignature,
        Generics,
        Impl,
        ItemSummary,
        Module,
        Span,
        Target,
        Type,
        Use,
        Variant,
        VariantKind,
    };
    use serde_json::json;

    use super::*;

    #[test]
    fn std_sources_are_in_the_rust_repository() {
        let commit = "b6eeb93c992027a03e1fd120b6652546396b142c";
        assert_eq!(
            std_source(&format!("/rustc/{commit}/library/alloc/src/vec/mod.rs")),
            Some((commit, "library/alloc/src/vec/mod.rs"))
        );
        assert_eq!(std_source("/rustc/short/library/x.rs"), None);
        assert_eq!(std_source("src/lib.rs"), None);
    }
    use crate::{
        data::rustdoc_json::ParsedCrate,
        query::view::tests::{archive, archived},
    };

    fn item(id: u32, name: Option<&str>, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: name.map(str::to_owned),
            span: None,
            visibility: Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner,
        }
    }

    fn no_generics() -> Generics {
        Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        }
    }

    fn module(id: u32, name: &str, items: &[u32]) -> Item {
        item(
            id,
            Some(name),
            ItemEnum::Module(Module {
                is_crate: id == 0,
                items: items.iter().map(|&id| Id(id)).collect(),
                is_stripped: false,
            }),
        )
    }

    fn reexport(id: u32, source: &str, name: &str, target: u32) -> Item {
        item(
            id,
            None,
            ItemEnum::Use(Use {
                source: source.to_owned(),
                name: name.to_owned(),
                id: Some(Id(target)),
                is_glob: false,
            }),
        )
    }

    fn primitive(name: &str) -> Type {
        Type::Primitive(name.to_owned())
    }

    /// `#[must_use] pub struct Mutex { pub inner: u8, /* private */ }`, documented.
    fn mutex_struct() -> Item {
        Item {
            docs: Some("A lock.\n\nMore.".to_owned()),
            attrs: vec![Attribute::MustUse { reason: None }],
            span: Some(Span {
                filename: PathBuf::from("src/sync/mutex.rs"),
                begin: (10, 1),
                end: (12, 2),
            }),
            ..item(
                3,
                Some("Mutex"),
                ItemEnum::Struct(Struct {
                    kind: StructKind::Plain {
                        fields: vec![Id(4)],
                        has_stripped_fields: true,
                    },
                    generics: no_generics(),
                    impls: vec![Id(5)],
                }),
            )
        }
    }

    /// `#[deprecated(since = "1.1.0")] pub fn lock(&self) -> u8`.
    fn lock_method() -> Item {
        Item {
            deprecation: Some(Deprecation {
                since: Some("1.1.0".to_owned()),
                note: None,
            }),
            ..item(
                6,
                Some("lock"),
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![("self".to_owned(), Type::BorrowedRef {
                            lifetime: None,
                            is_mutable: false,
                            type_: Box::new(Type::Generic("Self".to_owned())),
                        })],
                        output: Some(primitive("u8")),
                        is_c_variadic: false,
                    },
                    generics: no_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: rustdoc_types::Abi::Rust,
                    },
                    has_body: true,
                    default_unstable: None,
                }),
            )
        }
    }

    /// The crate `my-crate` 1.2.3:
    ///
    /// ```text
    /// pub mod sync {
    ///     mod mutex {
    ///         /// A lock.
    ///         ///
    ///         /// More.
    ///         #[must_use]
    ///         pub struct Mutex { pub inner: u8, /* private */ }
    ///         impl Mutex { #[deprecated(since = "1.1.0")] pub fn lock(&self) -> u8 }
    ///     }
    ///     pub use mutex::Mutex;
    /// }
    /// pub use sync::Mutex;
    /// #[non_exhaustive] pub enum E { A, B }
    /// pub use other::Thing;
    /// ```
    pub(in crate::query) fn parsed() -> ParsedCrate {
        let variant = |id, name| {
            item(
                id,
                Some(name),
                ItemEnum::Variant(Variant {
                    kind: VariantKind::Plain,
                    discriminant: None,
                }),
            )
        };
        let items = vec![
            module(0, "my_crate", &[1, 7, 8, 11]),
            module(1, "sync", &[2]),
            reexport(2, "mutex::Mutex", "Mutex", 3),
            mutex_struct(),
            item(4, Some("inner"), ItemEnum::StructField(primitive("u8"))),
            item(
                5,
                None,
                ItemEnum::Impl(Impl {
                    is_unsafe: false,
                    generics: no_generics(),
                    provided_trait_methods: Vec::new(),
                    trait_: None,
                    for_: Type::ResolvedPath(rustdoc_types::Path {
                        path: "Mutex".to_owned(),
                        id: Id(3),
                        args: None,
                    }),
                    items: vec![Id(6)],
                    is_negative: false,
                    is_synthetic: false,
                    blanket_impl: None,
                }),
            ),
            lock_method(),
            reexport(7, "sync::Mutex", "Mutex", 3),
            Item {
                attrs: vec![Attribute::NonExhaustive],
                ..item(
                    8,
                    Some("E"),
                    ItemEnum::Enum(Enum {
                        generics: no_generics(),
                        has_stripped_variants: false,
                        variants: vec![Id(9), Id(10)],
                        impls: Vec::new(),
                    }),
                )
            },
            variant(9, "A"),
            variant(10, "B"),
            reexport(11, "other::Thing", "Thing", 20),
        ];
        let krate = Crate {
            root: Id(0),
            crate_version: Some("1.2.3".to_owned()),
            includes_private: false,
            index: items.into_iter().map(|item| (item.id, item)).collect(),
            paths: HashMap::from([
                (Id(3), ItemSummary {
                    crate_id: 0,
                    path: vec![
                        "my_crate".into(),
                        "sync".into(),
                        "mutex".into(),
                        "Mutex".into(),
                    ],
                    kind: ItemKind::Struct,
                }),
                (Id(20), ItemSummary {
                    crate_id: 1,
                    path: vec!["other".into(), "inner".into(), "Thing".into()],
                    kind: ItemKind::Trait,
                }),
            ]),
            external_crates: HashMap::from([(1, ExternalCrate {
                name: "other".to_owned(),
                html_root_url: Some(
                    "https://docs.rs/other/0.5.1/x86_64-unknown-linux-gnu/".to_owned(),
                ),
                path: PathBuf::new(),
            })]),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".to_owned(),
                target_features: Vec::new(),
            },
            format_version: rustdoc_types::FORMAT_VERSION,
        };
        ParsedCrate {
            krate,
            newer_than_supported: false,
        }
    }

    /// What `check` makes of `get_item` on [`parsed`] for `path`, requested as
    /// `my-crate@version`.
    fn lookup<T>(
        version: &str,
        path: &str,
        check: impl FnOnce(Result<ItemResponse<'_>, GetItemError>) -> T,
    ) -> T {
        let bytes = archive(parsed().krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::parse(version).expect("a version");
        check(get_item(&view, ItemRequest {
            name: &name,
            version: &version,
            path,
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            detail: Detail::Normal,
            project_features: None,
            follow: false,
            target: None,
        }))
    }

    /// The response of `get_item` for `path`, as JSON.
    fn found(version: &str, path: &str) -> serde_json::Value {
        lookup(version, path, |result| {
            serde_json::to_value(result.expect("found")).expect("serializes")
        })
    }

    #[test]
    fn a_struct_has_its_fields_docs_and_location() {
        let response = found("1.2.3", "my_crate::sync::mutex::Mutex");
        assert_eq!(
            response,
            json!({
                "provenance": {
                    "source": "docs.rs",
                    "crate": "my-crate",
                    "version": "1.2.3",
                    "resolved_from": "explicit",
                    "target": "x86_64-unknown-linux-gnu",
                    "format_version": rustdoc_types::FORMAT_VERSION,
                },
                "data": {
                    "path": "my_crate::Mutex",
                    "kind": "struct",
                    "signature": "pub struct Mutex",
                    "fields": ["pub inner: u8", "/* private fields */"],
                    "docs": "A lock.",
                    "docs_truncated": true,
                    "must_use": true,
                    "also_reexported_at": ["my_crate::sync::Mutex"],
                    "source_location": {
                        "file": "src/sync/mutex.rs",
                        "lines": "10-12",
                        "url": "https://docs.rs/crate/my-crate/1.2.3/source/src/sync/mutex.rs#10-12",
                    },
                },
            })
        );
        // `Value` equality ignores the order of keys. The order is part of the response: the
        // signature comes before the prose (requirements R-OUT-1 and R-SH-1).
        let keys = |value: &serde_json::Value| -> Vec<String> {
            value
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(keys(&response), ["provenance", "data"]);
        assert_eq!(keys(&response["data"]), [
            "path",
            "kind",
            "signature",
            "fields",
            "docs",
            "docs_truncated",
            "must_use",
            "also_reexported_at",
            "source_location",
        ]);
    }

    #[test]
    fn a_method_names_its_parent_and_impl() {
        let response = found("1.2.3", "my-crate::sync::Mutex::lock");
        assert_eq!(
            response["data"],
            json!({
                "path": "my_crate::Mutex::lock",
                "kind": "method",
                "parent": "my_crate::Mutex",
                "impl": "impl my_crate::Mutex",
                "signature": "pub fn lock(&self) -> u8",
                "deprecated": { "since": "1.1.0" },
            })
        );
    }

    #[test]
    fn a_definition_outside_the_package_has_no_url_and_a_warning() {
        let mut parsed = parsed();
        let generated = "/build/out/generated.rs";
        if let Some(span) = parsed
            .krate
            .index
            .get_mut(&Id(3))
            .and_then(|item| item.span.as_mut())
        {
            span.filename = PathBuf::from(generated);
        }
        let bytes = archive(parsed.krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::parse("1.2.3").expect("a version");
        let response = get_item(&view, ItemRequest {
            name: &name,
            version: &version,
            path: "my_crate::sync::Mutex",
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            detail: Detail::Normal,
            project_features: None,
            follow: false,
            target: None,
        })
        .expect("found");
        let response = serde_json::to_value(response).expect("serializes");
        assert_eq!(
            response["data"]["source_location"],
            json!({ "file": generated, "lines": "10-12" })
        );
        assert_eq!(
            response["warnings"],
            json!([format!(
                "{generated} is outside the package, so the definition has no docs.rs URL"
            )])
        );
    }

    /// The file of a definition in a crate built here is relative to its package's root, as the
    /// spans relative to the workspace's root where cargo ran, or absolute outside it, tell; one
    /// outside the package, as of code that a derive of another crate generated, is as the span
    /// gives it, with a warning (requirement R-OUT-8, decided in M3a).
    #[test]
    fn a_definition_built_here_is_told_relative_to_its_package() {
        let told = |filename: &str, roots: Option<(&str, &str)>| {
            let mut parsed = parsed();
            if let Some(span) = parsed
                .krate
                .index
                .get_mut(&Id(3))
                .and_then(|item| item.span.as_mut())
            {
                span.filename = PathBuf::from(filename);
            }
            let bytes = archive(parsed.krate);
            let view = CrateView::new(archived(&bytes));
            let name: CrateName = "my-crate".parse().expect("a crate name");
            let version = Version::parse("1.2.3").expect("a version");
            let response = get_item(&view, ItemRequest {
                name: &name,
                version: &version,
                path: "my_crate::sync::Mutex",
                origin: Origin::Local {
                    private: true,
                    package_source: None,
                    resolved_from: ResolvedFrom::Explicit,
                    project_from: None,
                    toolchain: None,
                    built_at: "2026-10-26T00:00:00Z",
                    features_requested: None,
                    features_effective: None,
                    recorded_by: None,
                    roots: roots
                        .map(|(workspace, package)| (Path::new(workspace), Path::new(package))),
                },
                detail: Detail::Normal,
                project_features: None,
                follow: false,
                target: None,
            })
            .expect("found");
            serde_json::to_value(response).expect("serializes")
        };
        // A member of a virtual workspace, the root package, a path dependency outside the
        // workspace, whose spans are absolute, and spans that go through `.` and `..`.
        for (filename, roots) in [
            ("one/src/lib.rs", ("/ws", "/ws/one")),
            ("src/lib.rs", ("/ws", "/ws")),
            ("/deps/watcher/src/lib.rs", ("/ws", "/deps/watcher")),
            ("./one/../one/src/lib.rs", ("/ws", "/ws/one")),
            ("../watcher/src/lib.rs", ("/ws", "/watcher")),
        ] {
            let response = told(filename, Some(roots));
            assert_eq!(
                response["data"]["source_location"],
                json!({ "file": "src/lib.rs", "lines": "10-12" }),
                "{filename}"
            );
            assert!(response.get("warnings").is_none(), "{response}");
        }
        let generated = "/home/u/.cargo/registry/src/index/dep-1.0.0/src/lib.rs";
        let response = told(generated, Some(("/ws", "/ws/one")));
        assert_eq!(
            response["data"]["source_location"],
            json!({ "file": generated, "lines": "10-12" })
        );
        assert_eq!(
            response["warnings"],
            json!([format!("{generated} is outside the package")])
        );
        // A record from before has no roots: the span as it is, without a warning.
        let response = told(generated, None);
        assert_eq!(
            response["data"]["source_location"],
            json!({ "file": generated, "lines": "10-12" })
        );
        assert!(response.get("warnings").is_none(), "{response}");
    }

    #[test]
    fn an_enum_lists_its_variants() {
        let response = found("1.2.3", "my_crate::E");
        assert_eq!(response["data"]["variants"], json!(["A", "B"]));
        assert_eq!(response["data"]["non_exhaustive"], json!(true));
    }

    #[test]
    fn a_reexport_of_another_crate_points_to_it() {
        let response = found("1.2.3", "my_crate::Thing");
        assert_eq!(
            response["data"],
            json!({
                "path": "my_crate::Thing",
                "kind": "trait",
                "reexport_of": { "crate": "other@0.5.1", "path": "other::inner::Thing" },
            })
        );
        assert_eq!(
            response["hints"],
            json!([
                "call get_item with crate other@0.5.1 and path other::inner::Thing for its \
                 documentation"
            ])
        );
    }

    #[test]
    fn a_version_other_than_requested_is_warned_about() {
        let response = found("1.2.4", "my_crate::E");
        assert_eq!(response["provenance"]["version"], json!("1.2.3"));
        assert_eq!(
            response["warnings"],
            json!(["requested version 1.2.4, but the data is of version 1.2.3"])
        );
    }

    #[test]
    fn an_unknown_path_is_not_found() {
        let error = lookup("1.2.3", "my_crate::Nothing", |result| {
            result.expect_err("not found")
        });
        assert!(matches!(error, GetItemError::NotFound), "{error:?}");
    }

    #[test]
    fn the_first_paragraph_does_not_end_inside_a_code_block() {
        assert_eq!(
            first_paragraph("\nOne\ntwo.\n\nThree."),
            ("One\ntwo.", true)
        );
        assert_eq!(first_paragraph("Only.\n"), ("Only.", false));
        assert_eq!(
            first_paragraph("```\na\n\nb\n```\n\nNext."),
            ("```\na\n\nb\n```", true)
        );
        assert_eq!(first_paragraph(""), ("", false));
    }

    #[test]
    fn a_docs_rs_root_url_gives_the_release() {
        let (name, version) =
            docs_rs_release("https://docs.rs/axum-macros/0.5.1/x86_64-unknown-linux-gnu/")
                .expect("a release");
        assert_eq!(
            (name.as_str(), version.to_string().as_str()),
            ("axum-macros", "0.5.1")
        );
        assert!(docs_rs_release("https://doc.rust-lang.org/nightly/").is_none());
        assert!(docs_rs_release("https://docs.rs/serde/latest/").is_none());
    }

    /// The response for `path` in [`parsed`] with `detail`, after `change` is made to the crate.
    fn at_detail(path: &str, detail: Detail, change: impl FnOnce(&mut Crate)) -> serde_json::Value {
        let mut krate = parsed().krate;
        change(&mut krate);
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let response = get_item(&view, ItemRequest {
            name: &name,
            version: &version,
            path,
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            detail,
            project_features: None,
            follow: false,
            target: None,
        })
        .expect("found");
        serde_json::to_value(response).expect("serializes")
    }

    /// The mutex documented with an example that has a hidden line, and with more attributes.
    fn with_example(krate: &mut Crate) {
        if let Some(mutex) = krate.index.get_mut(&Id(3)) {
            mutex.docs = Some(
                "A lock.\n\nMore.\n\n# Examples\n\n```\n# use my_crate::Mutex;\nlet m = Mutex;\n```"
                    .to_owned(),
            );
            mutex.attrs.extend([
                Attribute::Repr(rustdoc_types::AttributeRepr {
                    kind: rustdoc_types::ReprKind::C,
                    align: Some(8),
                    packed: None,
                    int: None,
                }),
                Attribute::Other(
                    "#[attr = CfgTrace([NameValue { name: \"feature\", value: Some(\"sync\"), \
                     span: src/lib.rs:1:1: 1:2 (#0) }])]"
                        .to_owned(),
                ),
                Attribute::Other("#[doc(alias = \"lock\")]".to_owned()),
                Attribute::Other("#[attr = RustcDiagnosticItem(\"Mutex\")]".to_owned()),
            ]);
        }
    }

    #[test]
    fn brief_is_the_signature_alone() {
        let response = at_detail("my_crate::Mutex", Detail::Brief, with_example);
        assert_eq!(
            response["data"],
            json!({
                "path": "my_crate::Mutex",
                "kind": "struct",
                "signature": "pub struct Mutex",
                "required_features": "sync",
                "cfg_source": "cfg_trace",
            })
        );
    }

    #[test]
    fn normal_lists_the_sections_and_full_has_their_text() {
        let normal = at_detail("my_crate::Mutex", Detail::Normal, with_example);
        assert_eq!(normal["data"]["docs"], "A lock.");
        assert_eq!(normal["data"]["docs_truncated"], true);
        assert_eq!(
            normal["data"]["sections"],
            json!([{ "heading": "Examples", "lines": 3 }])
        );
        assert!(normal["data"].get("attrs").is_none(), "{normal}");

        let full = at_detail("my_crate::Mutex", Detail::Full, with_example);
        assert_eq!(full["data"]["docs"], "A lock.\n\nMore.");
        assert!(full["data"].get("docs_truncated").is_none(), "{full}");
        assert_eq!(
            full["data"]["sections"],
            json!([{
                "heading": "Examples",
                "lines": 3,
                "text": "```\nlet m = Mutex;\n```",
            }])
        );
        assert_eq!(
            full["data"]["attrs"],
            json!([
                "#[must_use]",
                "#[repr(C, align(8))]",
                "#[doc(alias = \"lock\")]"
            ])
        );
    }

    #[test]
    fn a_large_response_is_cut_to_its_budget() {
        let words = "word ".repeat(2_000);
        let (size, cut) = lookup("1.2.3", "my_crate::Mutex", |result| {
            let mut response = result.expect("found");
            response.data.docs = Some(Cow::Owned(words.clone()));
            response.data.sections = (0..3)
                .map(|n| Section {
                    heading: format!("S{n}").into(),
                    lines: 1,
                    text: Some(words.clone().into()),
                })
                .collect();
            response.data.fields = (0..1_000).map(|n| format!("pub f{n}: u8").into()).collect();
            fit(&mut response, 8_000);
            let json = serde_json::to_value(&response).expect("serializes");
            (json.to_string().chars().count(), json)
        });
        assert!(size <= 8_000, "{size}");
        let data = &cut["data"];
        assert_eq!(data["truncated"], true);
        assert!(
            data["sections"].as_array().is_some_and(|sections| sections
                .iter()
                .all(|section| section.get("text").is_none())),
            "{data}"
        );
        assert!(
            data["docs"]
                .as_str()
                .is_some_and(|docs| docs.ends_with('…'))
        );
        let fields = data["fields"].as_array().map_or(0, Vec::len);
        assert!(fields < 1_000, "{fields}");
        assert!(
            cut["warnings"][0]
                .as_str()
                .is_some_and(|warning| warning.contains("of 1000 fields")),
            "{cut}"
        );
    }

    #[test]
    fn a_small_response_is_not_cut() {
        let cut = lookup("1.2.3", "my_crate::Mutex", |result| {
            let mut response = result.expect("found");
            fit(&mut response, MAX_RESPONSE_CHARS);
            serde_json::to_value(&response).expect("serializes")
        });
        assert!(cut["data"].get("truncated").is_none(), "{cut}");
    }

    /// The conditions of the re-export of `Mutex`, of the `impl` of `lock`, and of the enum.
    pub(in crate::query) fn with_cfgs(krate: &mut Crate) {
        let mut add = |id: u32, text: &str| {
            if let Some(item) = krate.index.get_mut(&Id(id)) {
                item.attrs.push(Attribute::Other(text.to_owned()));
            }
        };
        add(7, r#"#[doc(cfg(feature = "sync"))]"#);
        add(
            5,
            r#"#[attr = CfgTrace([NameValue { name: "tokio_unstable", value: None, span: src/lib.rs:1:1: 1:2 (#0) }])]"#,
        );
        add(8, r#"#[<cfg>(any(feature = "a", feature = "b"))]"#);
        add(8, "#[<cfg>(any(all(doc, docsrs), unix))]");
    }

    #[test]
    fn conditions_are_inherited_from_reexports_impls_and_parents() {
        let note = |path: &str| {
            let data = at_detail(path, Detail::Brief, with_cfgs)["data"].clone();
            (
                data["required_features"].clone(),
                data["other_cfg"].clone(),
                data["cfg_source"].clone(),
            )
        };
        assert_eq!(
            note("my_crate::Mutex"),
            (json!("sync"), json!(null), json!("inherited"))
        );
        assert_eq!(
            note("my_crate::Mutex::lock"),
            (json!("sync"), json!(["tokio_unstable"]), json!("inherited"))
        );
        // The trace is read when there is no `doc(cfg)`, and `doc` and `docsrs` are false in a
        // build.
        assert_eq!(
            note("my_crate::E"),
            (json!("any(a, b)"), json!(["unix"]), json!("cfg_trace"))
        );
        assert_eq!(
            note("my_crate::E::A"),
            (json!("any(a, b)"), json!(["unix"]), json!("inherited"))
        );
        // The definition path does not go through the re-export, but the canonical path does.
        assert_eq!(
            note("my_crate::sync::mutex::Mutex"),
            (json!("sync"), json!(null), json!("inherited"))
        );
    }

    #[test]
    fn an_item_without_conditions_has_no_note() {
        let data = at_detail("my_crate::Mutex", Detail::Normal, |_| {})["data"].clone();
        assert!(data.get("cfg_source").is_none(), "{data}");
        assert!(data.get("required_features").is_none(), "{data}");
    }

    /// `enabled_in_project` and `enabled_in_dev_only` of `path` in [`parsed`] with
    /// [`with_cfgs`], when the project builds the crate with `normal` and `with_dev` (`None` when
    /// it does not), docs.rs set `docs_rs_cfg`, and the target asked for is `target`.
    fn in_project(
        path: &str,
        normal: Option<&[&str]>,
        with_dev: Option<&[&str]>,
        docs_rs_cfg: &[&str],
        target: Option<&str>,
    ) -> (serde_json::Value, serde_json::Value) {
        let mut krate = parsed().krate;
        with_cfgs(&mut krate);
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let set = |features: Option<&[&str]>| {
            features.map(|features| {
                features
                    .iter()
                    .map(|&feature| Box::from(feature))
                    .collect::<BTreeSet<_>>()
            })
        };
        let (normal, with_dev) = (set(normal), set(with_dev));
        let build = DocsRsBuild {
            cfg: docs_rs_cfg.iter().map(|&cfg| Box::from(cfg)).collect(),
            ..DocsRsBuild::default()
        };
        let response = get_item(&view, ItemRequest {
            name: &name,
            version: &version,
            path,
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::CargoLock,
                project_from: Some("cwd"),
                build: Some(&build),
            },
            detail: Detail::Brief,
            project_features: Some(ProjectFeatures {
                normal: normal.as_ref(),
                with_dev: with_dev.as_ref(),
            }),
            follow: false,
            target,
        })
        .expect("found");
        let data = serde_json::to_value(response).expect("serializes")["data"].clone();
        (
            data["enabled_in_project"].clone(),
            data["enabled_in_dev_only"].clone(),
        )
    }

    #[test]
    fn an_item_is_in_the_build_if_its_features_are() {
        // `lock` needs `sync` (from the re-export) and `tokio_unstable` (from its `impl`), which
        // held when docs.rs built it.
        let lock = "my_crate::Mutex::lock";
        assert_eq!(
            in_project(lock, Some(&["sync"]), Some(&["sync"]), &[], None),
            (json!(true), json!(null))
        );
        assert_eq!(
            in_project(lock, Some(&[]), Some(&[]), &[], None),
            (json!(false), json!(null))
        );
        // Unless docs.rs set it with `--cfg`: the project may not.
        assert_eq!(
            in_project(lock, Some(&["sync"]), None, &["tokio_unstable"], None),
            (json!("unknown"), json!(null))
        );
        // A missing feature decides it all the same.
        assert_eq!(
            in_project(lock, Some(&[]), None, &["tokio_unstable"], None),
            (json!(false), json!(null))
        );
    }

    #[test]
    fn an_item_only_in_the_tests_is_enabled_in_dev_only() {
        let lock = "my_crate::Mutex::lock";
        assert_eq!(
            in_project(lock, Some(&[]), Some(&["sync"]), &[], None),
            (json!(false), json!(true))
        );
        // A crate that only the dev-dependencies build.
        assert_eq!(
            in_project("my_crate::sync", None, Some(&[]), &[], None),
            (json!(false), json!(true))
        );
        assert_eq!(
            in_project("my_crate::sync", None, None, &[], None),
            (json!(false), json!(null))
        );
    }

    #[test]
    fn conditions_other_than_features_are_unknown_for_another_target() {
        // `E` needs `any(a, b)` and `unix`.
        assert_eq!(
            in_project("my_crate::E", Some(&["b"]), None, &[], None),
            (json!(true), json!(null))
        );
        assert_eq!(
            in_project(
                "my_crate::E",
                Some(&["b"]),
                None,
                &[],
                Some("aarch64-apple-darwin")
            ),
            (json!("unknown"), json!(null))
        );
    }

    /// The error of `get_item` with `follow` for `path` in [`parsed`], after `change`, or `None`
    /// when it finds the item.
    fn followed(path: &str, change: impl FnOnce(&mut Crate)) -> Option<GetItemError> {
        let mut krate = parsed().krate;
        change(&mut krate);
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        get_item(&view, ItemRequest {
            name: &name,
            version: &version,
            path,
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            detail: Detail::Brief,
            project_features: None,
            follow: true,
            target: None,
        })
        .err()
    }

    #[test]
    fn another_crates_items_that_are_not_public_are_not_shown() {
        // A build with private items, in which `lock` and the field `inner` are the crate's.
        let private = |krate: &mut Crate| {
            krate.includes_private = true;
            for id in [Id(4), Id(6)] {
                if let Some(item) = krate.index.get_mut(&id) {
                    item.visibility = Visibility::Crate;
                }
            }
        };
        assert!(matches!(
            followed("my_crate::Mutex::lock", private),
            Some(GetItemError::NotFound)
        ));
        let mutex = at_detail("my_crate::Mutex", Detail::Normal, private);
        assert_eq!(
            mutex["data"]["fields"],
            serde_json::json!(["/* private fields */"]),
            "{mutex}"
        );
        // A module that is not public is not found, though what it has is, where it is public.
        let private_module = |krate: &mut Crate| {
            private(krate);
            if let Some(module) = krate.index.get_mut(&Id(1)) {
                module.visibility = Visibility::Crate;
            }
        };
        assert!(matches!(
            followed("my_crate::sync", private_module),
            Some(GetItemError::NotFound)
        ));
        assert_eq!(
            at_detail("my_crate::Mutex", Detail::Brief, private_module)["data"]["path"],
            "my_crate::Mutex"
        );
        // A variant's fields are `Default`, as public as the enum.
        let with_field = |krate: &mut Crate| {
            private(krate);
            let mut field = krate.index.get(&Id(4)).cloned().expect("a field");
            (field.id, field.name, field.visibility) =
                (Id(50), Some("0".into()), Visibility::Default);
            krate.index.insert(Id(50), field);
            if let Some(ItemEnum::Variant(variant)) =
                krate.index.get_mut(&Id(9)).map(|item| &mut item.inner)
            {
                variant.kind = VariantKind::Tuple(vec![Some(Id(50))]);
            }
        };
        let enumeration = at_detail("my_crate::E", Detail::Normal, with_field);
        assert_eq!(enumeration["data"]["variants"][0], "A(u8)", "{enumeration}");
        // The public ones are, as before, besides those that rustdoc left out.
        let shown = at_detail("my_crate::Mutex", Detail::Normal, |_| {});
        assert_eq!(
            shown["data"]["fields"],
            serde_json::json!(["pub inner: u8", "/* private fields */"]),
            "{shown}"
        );
    }

    #[test]
    fn a_path_into_another_crate_goes_on_there() {
        let destination = |path| match followed(path, |_| {}) {
            Some(GetItemError::Elsewhere(destination)) => *destination,
            other => panic!("{path}: {other:?}"),
        };
        let other = || Some(("other".parse().expect("a name"), Version::new(0, 5, 1)));
        assert_eq!(destination("my_crate::Thing"), Destination {
            entry: "my_crate::Thing".into(),
            reexport: "my_crate::Thing".into(),
            reexported: "other::inner::Thing".into(),
            path: "other::inner::Thing".into(),
            lib_name: "other".into(),
            release: other(),
            is_std: false,
            via_glob: false,
            others: Vec::new(),
        });
        // A path under the re-export goes on under its target, and is shown where it was asked.
        let under = destination("my_crate::Thing::method");
        assert_eq!(
            (under.entry, under.path),
            (
                "my_crate::Thing::method".into(),
                "other::inner::Thing::method".into()
            )
        );
        // The items of this crate are found here.
        assert!(followed("my_crate::Mutex", |_| {}).is_none());
        assert!(matches!(
            followed("my_crate::Nothing", |_| {}),
            Some(GetItemError::NotFound)
        ));
    }

    #[test]
    fn a_private_dependency_of_std_is_not_followed() {
        // The sysroot's copy is not the crate of crates.io with that name.
        let in_sysroot = |krate: &mut Crate| {
            if let Some(other) = krate.external_crates.get_mut(&1) {
                other.path = PathBuf::from(
                    "/toolchain/lib/rustlib/x86_64-unknown-linux-gnu/lib/libother-0123.rlib",
                );
            }
        };
        assert!(followed("my_crate::Thing", in_sysroot).is_none());
        // A crate of std goes on with the toolchain.
        let std = |krate: &mut Crate| {
            if let Some(other) = krate.external_crates.get_mut(&1) {
                other.html_root_url = Some("https://doc.rust-lang.org/nightly/".to_owned());
            }
        };
        assert!(matches!(
            followed("my_crate::Thing", std),
            Some(GetItemError::Elsewhere(destination)) if destination.is_std && destination.release.is_none()
        ));
    }

    #[test]
    fn std_tells_since_when_an_item_is_stable() {
        let mut krate = parsed().krate;
        let stable = |since: &str| {
            Some(Box::new(Stability {
                feature: "f".to_owned(),
                level: StabilityLevel::Stable {
                    since: Some(since.to_owned()),
                },
            }))
        };
        if let Some(mutex) = krate.index.get_mut(&Id(3)) {
            mutex.stability = stable("1.100.0");
        }
        if let Some(lock) = krate.index.get_mut(&Id(6)) {
            lock.stability = Some(Box::new(Stability {
                feature: "mutex_lock".to_owned(),
                level: StabilityLevel::Unstable,
            }));
            lock.const_stability = stable("1.90.0");
            if let ItemEnum::Function(function) = &mut lock.inner {
                function.default_unstable =
                    Some(Box::new(rustdoc_types::ProvidedDefaultUnstable {
                        feature: "lock_default".to_owned(),
                    }));
            }
        }
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let project = Version::new(1, 99, 1);
        let look_up = |path| {
            let response = get_item(&view, ItemRequest {
                name: &name,
                version: &version,
                path,
                origin: Origin::Std {
                    toolchain: "rustc 1.101.0-nightly",
                    std_from: "dist",
                    why: "x",
                    project_release: Some(&project),
                },
                detail: Detail::Normal,
                project_features: None,
                follow: false,
                target: None,
            })
            .expect("found");
            let response = serde_json::to_value(response).expect("serializes");
            (
                response["data"]["stability"].clone(),
                response["warnings"].clone(),
            )
        };
        assert_eq!(
            look_up("my_crate::Mutex"),
            (
                json!({ "stable_since": "1.100.0" }),
                json!([
                    "my_crate::Mutex is stable since Rust 1.100.0, after the project's toolchain 1.99.1"
                ])
            )
        );
        assert_eq!(
            look_up("my_crate::Mutex::lock"),
            (
                json!({
                    "unstable_feature": "mutex_lock",
                    "const_stable_since": "1.90.0",
                    "default_unstable": "lock_default",
                }),
                json!([
                    "the default of my_crate::Mutex::lock is unstable (#![feature(lock_default)]): an implementation on stable Rust must write it",
                    "my_crate::Mutex::lock is unstable: it needs a nightly toolchain and #![feature(mutex_lock)]"
                ])
            )
        );
    }

    #[test]
    fn a_path_may_start_with_the_package_name_of_a_lib_named_otherwise() {
        let mut krate = parsed().krate;
        if let Some(root) = krate.index.get_mut(&Id(0)) {
            root.name = Some("mine".to_owned());
        }
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let with_root = |path| with_root_name(&view, &name, path);
        assert_eq!(with_root("my-crate::Mutex").as_deref(), Some("mine::Mutex"));
        assert_eq!(
            with_root("my_crate::mine::Mutex").as_deref(),
            Some("mine::Mutex")
        );
        assert_eq!(with_root("my_crate").as_deref(), Some("mine"));
        assert_eq!(with_root("mine::Mutex"), None);
        assert_eq!(with_root("other::Mutex"), None);
    }
}
