//! `get_item`: one item with its signature, the first paragraph of its documentation, and where
//! the data came from (requirements R-OUT-2, R-OUT-7 `normal`, R-OUT-8, R-OUT-9, R-SH-1, R-SH-2
//! and R-SH-6).
//!
//! This only reads the parsed crate and its path index. Loading the JSON and turning the result
//! into an MCP tool result are done by the caller.

use std::{
    fmt,
    path::{Component, Path},
};

use rustdoc_types::{
    Attribute,
    Crate,
    Id,
    Item,
    ItemEnum,
    ItemKind,
    MacroKind,
    Struct,
    StructKind,
};
use semver::Version;
use serde::Serialize;

use super::{
    path_index::{ExternalReexport, PathIndex, Resolution},
    render::Renderer,
};
use crate::data::{crate_name::CrateName, rustdoc_json::ParsedCrate};

/// What to look up.
#[derive(Debug, Clone, Copy)]
pub struct ItemRequest<'a> {
    /// The crate, as given (`tokio`).
    pub name: &'a CrateName,
    /// The version, as given.
    pub version: &'a Version,
    /// The path of the item, starting with the crate name (`tokio::sync::Mutex::lock`).
    pub path: &'a str,
}

/// The result of `get_item` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct ItemResponse {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Box<str>]>,
    /// The item.
    pub data: ItemData,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// Where the data came from. The version, target and format are read from the JSON itself.
#[derive(Debug, Serialize)]
pub struct Provenance {
    /// The source that returned the data.
    pub source: Source,
    /// The crate, as requested.
    #[serde(rename = "crate")]
    pub name: Box<str>,
    /// The version in the JSON, or the requested one if the JSON has none.
    pub version: Box<str>,
    /// How the version was chosen.
    pub resolved_from: ResolvedFrom,
    /// The target the JSON was built for.
    pub target: Box<str>,
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
}

/// One item, as of `detail=normal` (requirement R-OUT-7), with the signature before the prose
/// (requirement R-SH-1).
#[derive(Debug, Default, Serialize)]
pub struct ItemData {
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
    pub docs: Option<Box<str>>,
    /// The documentation goes on after the first paragraph.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub docs_truncated: bool,
    /// `#[deprecated]` (requirement R-SH-6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<Deprecated>,
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
    pub source_location: Option<SourceLocation>,
}

/// `#[deprecated]`.
#[derive(Debug, Serialize)]
pub struct Deprecated {
    /// The version that deprecated the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<Box<str>>,
    /// Why, or what to use instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<Box<str>>,
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
pub struct SourceLocation {
    /// The file, relative to the package root.
    pub file: Box<str>,
    /// The first and last lines (`434-466`), or the one line (`301`).
    pub lines: Box<str>,
    /// The file on docs.rs, when it is in the package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Box<str>>,
}

/// Why an item could not be returned.
#[derive(Debug, thiserror::Error)]
pub enum GetItemError {
    /// Nothing has the path: as a public path, a re-export from another crate, a definition path,
    /// or a member of an item at its parent path.
    #[error("no item has the path {path}")]
    NotFound {
        /// The path as requested.
        path: Box<str>,
    },
    /// More than one item has the path (requirement R-PR-7).
    #[error("{} items have the path {path}", candidates.len())]
    Ambiguous {
        /// The path as requested.
        path: Box<str>,
        /// What each item is: its canonical path, or its name and `impl`.
        candidates: Box<[Box<str>]>,
    },
}

/// Looks up the item at `request.path` in `parsed`, whose public paths are in `index`.
///
/// # Errors
///
/// Returns an error when no item or more than one item has the path.
pub fn get_item(
    parsed: &ParsedCrate,
    index: &PathIndex,
    request: ItemRequest<'_>,
) -> Result<ItemResponse, GetItemError> {
    let mut query = Query {
        request,
        krate: &parsed.krate,
        index,
        renderer: Renderer::new(&parsed.krate, index),
        warnings: Vec::new(),
        hints: Vec::new(),
    };
    let provenance = query.provenance(parsed);
    let data = match index.resolve(&parsed.krate, request.path) {
        Resolution::Item(id) => query.item_data(id, None),
        Resolution::Member { parent, member } => query.item_data(member, Some(parent)),
        Resolution::ExternalReexport(reexport) => Some(query.reexport_data(reexport)),
        Resolution::Ambiguous(ids) => {
            return Err(GetItemError::Ambiguous {
                path: request.path.into(),
                candidates: ids.iter().map(|&id| query.describe(id)).collect(),
            });
        },
        Resolution::NotFound => None,
    };
    let Some(data) = data else {
        return Err(GetItemError::NotFound {
            path: request.path.into(),
        });
    };
    let Query {
        renderer,
        mut warnings,
        hints,
        ..
    } = query;
    warnings.extend(
        renderer
            .warnings()
            .iter()
            .map(|warning| warning.as_str().into()),
    );
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
    krate: &'a Crate,
    index: &'a PathIndex,
    renderer: Renderer<'a>,
    warnings: Vec<Box<str>>,
    hints: Vec<Box<str>>,
}

impl Query<'_> {
    /// The provenance, read from the JSON and checked against the request (§7.3).
    fn provenance(&mut self, parsed: &ParsedCrate) -> Provenance {
        let requested = self.request.version.to_string();
        let version = match &self.krate.crate_version {
            Some(version) if *version == requested => requested,
            Some(version) => {
                self.warnings.push(
                    format!("requested version {requested}, but the data is of version {version}")
                        .into(),
                );
                version.clone()
            },
            None => {
                self.warnings
                    .push("the data has no crate version; showing the requested one".into());
                requested
            },
        };
        if parsed.newer_than_supported {
            self.warnings.push(
                format!(
                    "rustdoc JSON format {} is newer than this server supports; the result may be \
                     incomplete",
                    self.krate.format_version
                )
                .into(),
            );
        }
        Provenance {
            source: Source::DocsRs,
            name: self.request.name.as_str().into(),
            version: version.into(),
            resolved_from: ResolvedFrom::Explicit,
            target: self.krate.target.triple.as_str().into(),
            format_version: self.krate.format_version,
        }
    }

    /// The item `id`, which belongs to `parent` if it is a member. `None` if it is not in the
    /// crate, which a resolved path should not be.
    fn item_data(&mut self, id: Id, parent: Option<Id>) -> Option<ItemData> {
        let krate = self.krate;
        let item = krate.index.get(&id)?;
        let name = item.name.as_deref().unwrap_or("_");
        let mut data = ItemData {
            kind: kind_of(item, parent.is_some()),
            signature: Some(self.renderer.signature(item).into()),
            where_predicates: boxed(self.renderer.where_predicates(item)),
            deprecated: item.deprecation.as_ref().map(|deprecation| Deprecated {
                since: deprecation.since.as_deref().map(Into::into),
                note: deprecation.note.as_deref().map(Into::into),
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
                .map(|path| path.to_string().into())
                .collect();
        }
        data.fields = self.fields(item);
        data.variants = self.variants(item);
        if let Some(docs) = item.docs.as_deref() {
            let (first, truncated) = first_paragraph(docs);
            if !first.is_empty() {
                data.docs = Some(first.into());
            }
            data.docs_truncated = truncated;
        }
        Some(data)
    }

    /// A re-export of an item of another crate, with the arguments to look the item up there.
    fn reexport_data(&mut self, reexport: &ExternalReexport) -> ItemData {
        let summary = self.krate.paths.get(&reexport.target);
        let external =
            summary.and_then(|summary| self.krate.external_crates.get(&summary.crate_id));
        let reexport_of = if let (Some(definition), Some(external)) =
            (&reexport.definition, external)
        {
            let name = if let Some((name, version)) =
                external.html_root_url.as_deref().and_then(docs_rs_release)
            {
                format!("{name}@{version}")
            } else {
                self.warnings.push(
                    format!(
                        "the version of {} that is re-exported is unknown",
                        external.name
                    )
                    .into(),
                );
                external.name.clone()
            };
            let path = definition.to_string();
            self.hints.push(
                format!("call get_item with crate {name} and path {path} for its documentation")
                    .into(),
            );
            Some(ReexportOf {
                name: name.into(),
                path: path.into(),
            })
        } else {
            self.warnings
                .push("the re-exported item is not described in this crate's data".into());
            None
        };
        ItemData {
            path: reexport.path.to_string().into(),
            kind: summary.map_or("unknown", |summary| summary_kind(summary.kind)),
            reexport_of,
            ..ItemData::default()
        }
    }

    /// The canonical path of `id`, or its definition path with a warning if it has no public
    /// path.
    fn path_of(&mut self, id: Id) -> Box<str> {
        if let Some(path) = self.index.canonical_path(id) {
            return path.to_string().into();
        }
        let path = self.krate.paths.get(&id).map_or_else(
            || {
                self.krate
                    .index
                    .get(&id)
                    .and_then(|item| item.name.clone())
                    .unwrap_or_else(|| "_".to_owned())
            },
            |summary| summary.path.join("::"),
        );
        self.warnings
            .push(format!("{path} has no public path; it is shown as defined").into());
        path.into()
    }

    /// The header of the `impl` of `parent` that has `member`, with its where clause. `None` for
    /// an item of a trait and a variant.
    fn impl_header(&mut self, parent: Id, member: Id) -> Option<Box<str>> {
        let krate = self.krate;
        let impl_item = impls_of(krate.index.get(&parent)?)
            .iter()
            .filter_map(|id| krate.index.get(id))
            .find(|item| matches!(&item.inner, ItemEnum::Impl(contents) if contents.items.contains(&member)))?;
        let mut header = self.renderer.signature(impl_item);
        let predicates = self.renderer.where_predicates(impl_item);
        if !predicates.is_empty() {
            header.push_str(" where ");
            header.push_str(&predicates.join(", "));
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
            .filter_map(|id| self.krate.index.get(id))
            .map(|item| self.renderer.signature(item).into())
            .collect()
    }

    /// Where `item` is defined, with a docs.rs URL when the file is in the package.
    fn source_location(&self, item: &Item) -> Option<SourceLocation> {
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
        let url = in_package.then(|| {
            let version: &dyn fmt::Display = match &self.krate.crate_version {
                Some(version) => version,
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
            file: file.into(),
            lines: lines.into(),
            url,
        })
    }

    /// What `id` is, to tell apart the items of an ambiguous path.
    fn describe(&mut self, id: Id) -> Box<str> {
        if let Some(path) = self.index.canonical_path(id) {
            return path.to_string().into();
        }
        let krate = self.krate;
        let Some(item) = krate.index.get(&id) else {
            return "an item that is not in the data".into();
        };
        let name = item.name.as_deref().unwrap_or("_");
        let owner = krate.index.values().find(|owner| {
            matches!(&owner.inner, ItemEnum::Impl(contents) if contents.items.contains(&id))
        });
        match owner {
            Some(owner) => format!("{name} in {}", self.renderer.signature(owner)).into(),
            None => name.into(),
        }
    }
}

/// `values` as a boxed slice of boxed strings.
fn boxed(values: Vec<String>) -> Box<[Box<str>]> {
    values.into_iter().map(String::into_boxed_str).collect()
}

/// The `impl`s of a type.
fn impls_of(item: &Item) -> &[Id] {
    match &item.inner {
        ItemEnum::Struct(contents) => &contents.impls,
        ItemEnum::Enum(contents) => &contents.impls,
        ItemEnum::Union(contents) => &contents.impls,
        ItemEnum::Primitive(contents) => &contents.impls,
        ItemEnum::Module(_)
        | ItemEnum::ExternCrate { .. }
        | ItemEnum::Use(_)
        | ItemEnum::StructField(_)
        | ItemEnum::Variant(_)
        | ItemEnum::Function(_)
        | ItemEnum::Trait(_)
        | ItemEnum::TraitAlias(_)
        | ItemEnum::Impl(_)
        | ItemEnum::TypeAlias(_)
        | ItemEnum::Constant { .. }
        | ItemEnum::Static(_)
        | ItemEnum::ExternType
        | ItemEnum::Macro(_)
        | ItemEnum::ProcMacro(_)
        | ItemEnum::AssocConst { .. }
        | ItemEnum::AssocType { .. } => &[],
    }
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
    let components: Vec<_> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect();
    components.join("/")
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
        check: impl FnOnce(Result<ItemResponse, GetItemError>) -> T,
    ) -> T {
        let parsed = parsed();
        let index = PathIndex::build(&parsed.krate);
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::parse(version).expect("a version");
        check(get_item(&parsed, &index, ItemRequest {
            name: &name,
            version: &version,
            path,
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
        assert!(
            matches!(&error, GetItemError::NotFound { path } if &**path == "my_crate::Nothing"),
            "{error:?}"
        );
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
