//! `get_item`: one item with its signature, the first paragraph of its documentation, and where
//! the data came from (requirements R-OUT-2, R-OUT-7 `normal`, R-OUT-8, R-OUT-9, R-SH-1, R-SH-2
//! and R-SH-6).
//!
//! This only reads the crate and its path index, from their archive. Loading them and turning the
//! result into an MCP tool result are done by the caller. The response borrows the strings that it
//! shows as they are from the items it takes out, so it is serialized while the view is there.

use std::{
    borrow::Cow,
    fmt,
    path::{Component, Path},
};

use rustdoc_types::{Attribute, Id, Item, ItemEnum, ItemKind, MacroKind, Struct, StructKind};
use semver::Version;
use serde::Serialize;

use super::{
    path_index::{self, ArchivedPathIndex, ExternalReexport, PublicPath, Resolution},
    render::Renderer,
    view::CrateView,
};
use crate::data::crate_name::CrateName;

/// What to look up.
#[derive(Debug, Clone, Copy)]
pub struct ItemRequest<'a> {
    /// The crate, as given (`tokio`).
    pub name: &'a CrateName,
    /// The version, as given.
    pub version: &'a Version,
    /// The path of the item, starting with the crate name (`tokio::sync::Mutex::lock`).
    pub path: &'a str,
    /// How the version was chosen.
    pub resolved_from: ResolvedFrom,
    /// Where the project that decided the version came from, if one did (§7.3).
    pub project_from: Option<&'static str>,
    /// The target asked for, to warn when the data is for another.
    pub target: Option<&'a str>,
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
    /// The crate, as requested.
    #[serde(rename = "crate")]
    pub name: &'a str,
    /// The version in the JSON, or the requested one if the JSON has none.
    pub version: Cow<'a, str>,
    /// How the version was chosen.
    pub resolved_from: ResolvedFrom,
    /// Where the project that decided the version came from: `argument`, `CLAUDE_PROJECT_DIR` or
    /// `cwd` (requirement R-MCP-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_from: Option<&'static str>,
    /// The target the JSON was built for.
    pub target: &'a str,
    /// The rustdoc JSON format.
    pub format_version: u32,
}

/// The source that returned the data (§7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Source {
    /// The rustdoc JSON that docs.rs built.
    #[serde(rename = "docs.rs")]
    DocsRs,
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
}

/// One item, as of `detail=normal` (requirement R-OUT-7), with the signature before the prose
/// (requirement R-SH-1).
#[derive(Debug, Default, Serialize)]
pub struct ItemData<'a> {
    /// The canonical path; for a member, that of its parent and its name.
    pub path: Box<str>,
    /// What the item is, in the words of rustdoc's `ItemKind`, except that a function in an
    /// `impl` or a trait is a `method`.
    pub kind: &'static str,
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
    /// The first paragraph of the documentation, as written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<&'a str>,
    /// The documentation goes on after the first paragraph.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub docs_truncated: bool,
    /// `#[deprecated]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<Deprecated<'a>>,
    /// `#[must_use]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub must_use: bool,
    /// `#[non_exhaustive]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_exhaustive: bool,
    /// The other public paths of the item.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub also_reexported_at: Box<[Box<str>]>,
    /// The item of another crate that this path re-exports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reexport_of: Option<ReexportOf>,
    /// Where the item is defined (requirement R-OUT-8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_location: Option<SourceLocation<'a>>,
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
    /// The file as the span gives it: relative to the package root when the file is in the
    /// package.
    pub file: &'a str,
    /// The first and last lines (`434-466`), or the one line (`301`).
    pub lines: Box<str>,
    /// The file on docs.rs, when it is in the package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Box<str>>,
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
    let mut query = Query {
        request,
        view,
        index,
        renderer: Renderer::new(view),
        warnings: Vec::new(),
        hints: Vec::new(),
    };
    let provenance = query.provenance();
    let data = match index.resolve(view, request.path) {
        Resolution::Item(id) => {
            let data = query.item_data(id, None);
            if let Some(reexport) = index.hidden_reexport(request.path) {
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
    let Query {
        renderer,
        mut warnings,
        hints,
        ..
    } = query;
    warnings.extend(renderer.into_warnings());
    Ok(ItemResponse {
        provenance,
        warnings: warnings.into(),
        data,
        hints: hints.into(),
    })
}

/// One lookup, with what it has found to warn about so far.
struct Query<'a> {
    request: ItemRequest<'a>,
    view: &'a CrateView<'a>,
    index: &'a ArchivedPathIndex,
    renderer: Renderer<'a>,
    warnings: Vec<Cow<'static, str>>,
    hints: Vec<Box<str>>,
}

impl<'a> Query<'a> {
    /// The provenance, read from the JSON and checked against the request (§7.3).
    fn provenance(&mut self) -> Provenance<'a> {
        let view = self.view;
        let requested = self.request.version;
        let version = if let Some(version) = view.crate_version() {
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
        Provenance {
            source: Source::DocsRs,
            name: self.request.name.as_str(),
            version,
            resolved_from: self.request.resolved_from,
            project_from: self.request.project_from,
            target,
            format_version: view.format_version(),
        }
    }

    /// The item `id`, which belongs to `parent` if it is a member. `None` if it is not in the
    /// crate, which a resolved path should not be.
    fn item_data(&mut self, id: Id, parent: Option<Id>) -> Option<ItemData<'a>> {
        let item = self.view.item(id)?;
        let name = item.name.as_deref().unwrap_or("_");
        let mut data = ItemData {
            kind: kind_of(item, parent.is_some()),
            signature: Some(self.renderer.signature(item).into()),
            where_predicates: boxed(self.renderer.where_predicates(item)),
            deprecated: item.deprecation.as_ref().map(|deprecation| Deprecated {
                since: deprecation.since.as_deref(),
                note: deprecation.note.as_deref(),
            }),
            must_use: item
                .attrs
                .iter()
                .any(|attr| matches!(attr, Attribute::MustUse { .. })),
            non_exhaustive: item.attrs.contains(&Attribute::NonExhaustive),
            source_location: self.source_location(item),
            ..ItemData::default()
        };
        if let Some(parent) = parent {
            let parent_path = self.path_of(parent);
            data.path = format!("{parent_path}::{name}").into();
            data.parent = Some(parent_path);
            data.impl_header = self.impl_header(parent, id);
        } else {
            data.path = self.path_of(id);
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
            let (first, truncated) = first_paragraph(docs);
            if !first.is_empty() {
                data.docs = Some(first);
            }
            data.docs_truncated = truncated;
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

    /// The canonical path of `id`, or its definition path with a warning if it has no public
    /// path.
    fn path_of(&mut self, id: Id) -> Box<str> {
        if let Some(path) = self.index.canonical_path(id) {
            return path.joined();
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
        let mut header = self.renderer.signature(impl_item);
        let predicates = self.renderer.where_predicates(impl_item);
        for (position, predicate) in predicates.iter().enumerate() {
            header.push_str(if position == 0 { " where " } else { ", " });
            header.push_str(predicate);
        }
        Some(header.into())
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
        let mut fields = self.signatures(fields);
        if has_stripped_fields {
            fields.push("/* private fields */".into());
        }
        fields.into()
    }

    /// The variants of an enum, one each.
    fn variants(&mut self, item: &Item) -> Box<[Box<str>]> {
        let ItemEnum::Enum(contents) = &item.inner else {
            return Box::default();
        };
        let mut variants = self.signatures(&contents.variants);
        if contents.has_stripped_variants {
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
        if !in_package {
            self.warnings.push(Cow::Owned(format!(
                "{file} is outside the package, so the definition has no docs.rs URL"
            )));
        }
        let url = in_package.then(|| {
            let version: &dyn fmt::Display = match self.view.crate_version() {
                Some(version) => &version.to_owned(),
                None => self.request.version,
            };
            format!(
                "https://docs.rs/crate/{}/{version}/source/{}#{lines}",
                self.request.name,
                slash_separated(&span.filename)
            )
            .into()
        });
        Some(SourceLocation {
            file,
            lines: lines.into(),
            url,
        })
    }

    /// The crate and definition path of the item that `reexport` re-exports, to look it up
    /// there. `None`, with a warning, when the data does not describe it.
    fn reexport_target(&mut self, reexport: &ExternalReexport) -> Option<ReexportOf> {
        let summary = self.view.summary(reexport.target);
        let external = summary.and_then(|summary| self.view.external_crate(summary.crate_id));
        let (Some(definition), Some(external)) = (&reexport.definition, external) else {
            self.warnings.push(Cow::Borrowed(
                "the re-exported item is not described in this crate's data",
            ));
            return None;
        };
        let name: Box<str> = if let Some((name, version)) =
            external.html_root_url.as_deref().and_then(docs_rs_release)
        {
            format!("{name}@{version}").into()
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

/// `values` as a boxed slice of boxed strings.
fn boxed(values: Vec<String>) -> Box<[Box<str>]> {
    values.into_iter().map(String::into_boxed_str).collect()
}

/// The kind of `item`, as rustdoc's `ItemKind` names it, except that a function that belongs to
/// a type or trait is a `method`.
const fn kind_of(item: &Item, is_member: bool) -> &'static str {
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
const fn summary_kind(kind: ItemKind) -> &'static str {
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

/// `path` with `/` between its components, whatever the platform.
fn slash_separated(path: &Path) -> String {
    let mut joined = String::with_capacity(path.as_os_str().len());
    for (position, component) in path.components().enumerate() {
        if position > 0 {
            joined.push('/');
        }
        joined.push_str(&component.as_os_str().to_string_lossy());
    }
    joined
}

/// The first paragraph of `docs`, and whether anything follows it. A blank line inside a code
/// block does not end the paragraph.
fn first_paragraph(docs: &str) -> (&str, bool) {
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
mod tests {
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
        Visibility,
    };
    use serde_json::json;

    use super::*;
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
    fn parsed() -> ParsedCrate {
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
            resolved_from: ResolvedFrom::Explicit,
            project_from: None,
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
            resolved_from: ResolvedFrom::Explicit,
            project_from: None,
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
}
