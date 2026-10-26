//! The types that implement a trait of the crate, and what the trait asks of them
//! (requirements R-TI-3 and R-TI-5, §3 Q9 within the crate).
//!
//! Implementors in other crates (`scope: dependency_graph`) are M4.

use std::borrow::Cow;

use rustdoc_types::{GenericBound, Id, Item, ItemEnum};
use serde::Serialize;

use super::{
    get_item::{
        Conditions,
        GetItemError,
        ItemRequest,
        MAX_RESPONSE_CHARS,
        Notes,
        Provenance,
        Query,
        kind_of,
        with_root_name,
    },
    list_module::Page,
    path_index::{Resolution, is_doc_hidden},
    view::CrateView,
};

/// How to page the implementors.
#[derive(Debug, Clone, Copy)]
pub struct ImplementorOptions<'a> {
    /// The most implementors in a page.
    pub limit: usize,
    /// Where the page starts.
    pub offset: usize,
    /// What a cursor for the next page starts with (requirement R-OUT-3).
    pub cursor_request: &'a str,
    /// The digest of the data that the cursor was made for, when there is one.
    pub cursor_data: Option<&'a str>,
}

/// The result of `list_implementors` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct ImplementorsResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// The trait and its implementors.
    pub data: Implementors,
    /// Which part of the implementors this is (requirement R-OUT-3).
    pub page: Page,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// A trait, what it asks of its implementors, and those in its crate.
#[derive(Debug, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a fact of the trait that the JSON shows when it holds"
)]
pub struct Implementors {
    /// The canonical path of the trait.
    pub path: Box<str>,
    /// Its declaration as Rust writes it.
    pub signature: Box<str>,
    /// Its items, required or provided (requirement R-TI-5).
    pub items: Vec<TraitItem>,
    /// Whether `dyn` can be made of it.
    pub dyn_compatible: bool,
    /// It is an auto trait.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    /// It is an `unsafe trait`.
    #[serde(rename = "unsafe", skip_serializing_if = "std::ops::Not::not")]
    pub is_unsafe: bool,
    /// Why no crate but its own can implement it, when that was found: a supertrait that is not
    /// public. Its absence does not mean that others can (requirement R-TI-5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Box<str>>,
    /// Some types are said not to implement it (`impl !Trait for T`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_negative_impls: bool,
    /// The implementors of the page, in its crate only.
    #[serde(rename = "implementors")]
    pub listed: Vec<Implementor>,
}

/// An item of a trait.
#[derive(Debug, Serialize)]
pub struct TraitItem {
    /// Its name.
    pub name: Box<str>,
    /// `method`, `assoc_type` or `assoc_const`.
    pub kind: &'static str,
    /// The trait gives it a default; an implementor need not write it.
    pub provided: bool,
}

/// A type that implements the trait.
#[derive(Debug, Serialize)]
pub struct Implementor {
    /// The type as Rust writes it (`tokio::sync::Mutex<T>`, `&T`, `[u8]`).
    #[serde(rename = "type")]
    pub ty: Box<str>,
    /// `trait`, `derive`, `blanket`, `negative` or `auto`.
    pub kind: &'static str,
    /// The `impl` as Rust writes it, with its where clause.
    pub header: Box<str>,
    /// The features and other conditions that the `impl` is written with (requirement R-FT-2).
    #[serde(flatten)]
    pub conditions: Conditions,
}

/// The implementors of the trait at `request.path` in its crate.
///
/// # Errors
///
/// Returns an error when the path names no trait, or the cursor does not fit.
pub fn list_implementors<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
    options: ImplementorOptions<'_>,
) -> Result<ImplementorsResponse<'a>, GetItemError> {
    let digest = view.source_digest();
    let data_digest = digest.get(..16).unwrap_or(digest);
    let mut query = Query::new(view, request);
    let provenance = query.provenance();
    let with_root = with_root_name(view, request.name, request.path);
    let path = with_root.as_deref().unwrap_or(request.path);
    let trait_id = find_trait(view, &mut query, path, request.follow)?;
    // Checked in the crate that the path goes on in, if it does.
    if let Some(cursor_data) = options.cursor_data
        && cursor_data != data_digest
    {
        return Err(GetItemError::BadCursor(
            "the data changed since the cursor was made; list again without the cursor",
        ));
    }
    let Some(item) = view.item(trait_id) else {
        return Err(GetItemError::NotFound);
    };
    let ItemEnum::Trait(contents) = &item.inner else {
        return Err(GetItemError::NotFound);
    };
    // Another crate's hidden items of the trait are not its API (decided in M2's D6).
    let member = query.is_member();
    let items = contents
        .items
        .iter()
        .filter(|&&id| member || !view.index().is_hidden(id))
        .filter_map(|&id| trait_item(view, id))
        .collect();
    let (mut entries, has_negative_impls) =
        implementors_of(view, &mut query, &contents.implementations);
    // By the type, then the header (requirement R-OUT-4).
    entries.sort_by(|a, b| (&a.ty, &a.header).cmp(&(&b.ty, &b.header)));
    let total = entries.len();
    if options.offset > total {
        return Err(GetItemError::BadCursor(
            "the cursor goes past the end of the implementors; list again without the cursor",
        ));
    }
    let shown: Vec<Implementor> = entries
        .into_iter()
        .skip(options.offset)
        .take(options.limit)
        .collect();
    let sealed = sealed(view, contents.bounds.as_slice());
    let path = query.path_of(trait_id);
    let signature = query
        .item_data(trait_id, None)
        .and_then(|mut data| {
            query.drop_source_location(&mut data);
            data.signature
        })
        .unwrap_or_default();
    query.hint(format!(
        "implementors in other crates are not listed; list_impls on a type tells whether it \
         implements {path}"
    ));
    let Notes { warnings, hints } = query.finish();
    let page = |shown: usize| {
        let next = options.offset.saturating_add(shown);
        Page {
            shown,
            total,
            has_more: next < total,
            next_cursor: (next < total)
                .then(|| format!("{}.{data_digest}.{next}", options.cursor_request).into()),
        }
    };
    let mut response = ImplementorsResponse {
        provenance,
        warnings,
        data: Implementors {
            path,
            signature,
            items,
            dyn_compatible: contents.is_dyn_compatible,
            auto: contents.is_auto,
            is_unsafe: contents.is_unsafe,
            sealed,
            has_negative_impls,
            listed: Vec::new(),
        },
        page: page(0),
        hints,
    };
    fit(&mut response, shown, &page);
    Ok(response)
}

/// The implementors that `implementations` (impls of the trait) give, and whether one of them is
/// negative.
fn implementors_of(
    view: &CrateView<'_>,
    query: &mut Query<'_>,
    implementations: &[Id],
) -> (Vec<Implementor>, bool) {
    let mut entries: Vec<Implementor> = Vec::new();
    let mut has_negative_impls = false;
    for &impl_id in implementations {
        let Some(impl_item) = view.item(impl_id) else {
            continue;
        };
        let ItemEnum::Impl(contents) = &impl_item.inner else {
            continue;
        };
        if !query.is_member() && is_doc_hidden(impl_item) {
            continue;
        }
        // A type of this crate without a public path or hidden is not public API (requirement
        // R-SH-5), but in a workspace member, whose own code it is (decided in M2's D6).
        if !query.is_member()
            && let rustdoc_types::Type::ResolvedPath(path) = &contents.for_
            && view.item(path.id).is_some()
            && (view.index().canonical_path(path.id).is_none() || view.index().is_hidden(path.id))
        {
            continue;
        }
        let kind = if contents.is_synthetic {
            "auto"
        } else if contents.is_negative {
            has_negative_impls = true;
            "negative"
        } else if contents.blanket_impl.is_some() {
            "blanket"
        } else if impl_item
            .attrs
            .contains(&rustdoc_types::Attribute::AutomaticallyDerived)
        {
            "derive"
        } else {
            "trait"
        };
        entries.push(Implementor {
            ty: query.type_text(&contents.for_),
            kind,
            header: query.impl_text(impl_item),
            conditions: query.own_conditions(impl_item.id),
        });
    }
    (entries, has_negative_impls)
}

/// The trait at `path`.
fn find_trait(
    view: &CrateView<'_>,
    query: &mut Query<'_>,
    path: &str,
    follow: bool,
) -> Result<Id, GetItemError> {
    let is_trait = |id: Id| {
        view.item(id)
            .is_some_and(|item| matches!(item.inner, ItemEnum::Trait(_)))
    };
    let id = match query.resolve(path) {
        Resolution::Item(id) => id,
        Resolution::Ambiguous(ids) => {
            let traits: Vec<Id> = ids.iter().copied().filter(|&id| is_trait(id)).collect();
            match traits.as_slice() {
                [found] => *found,
                _ => {
                    return Err(GetItemError::Ambiguous {
                        candidates: ids.iter().map(|&id| query.path_of(id)).collect(),
                    });
                },
            }
        },
        Resolution::Member { .. } | Resolution::AmbiguousMembers { .. } => {
            return Err(GetItemError::NotATrait { kind: "member" });
        },
        Resolution::ExternalReexport(_) | Resolution::NotFound => {
            return Err(follow
                .then(|| query.destination(path))
                .flatten()
                .map_or(GetItemError::NotFound, |destination| {
                    GetItemError::Elsewhere(Box::new(destination))
                }));
        },
    };
    if is_trait(id) {
        Ok(id)
    } else {
        Err(GetItemError::NotATrait {
            kind: view.item(id).map_or("item", |item| kind_of(item, false)),
        })
    }
}

/// An item of a trait, with whether the trait gives it a default.
fn trait_item(view: &CrateView<'_>, id: Id) -> Option<TraitItem> {
    let item: &Item = view.item(id)?;
    let (kind, provided) = match &item.inner {
        ItemEnum::Function(function) => ("method", function.has_body),
        ItemEnum::AssocType { type_, .. } => ("assoc_type", type_.is_some()),
        ItemEnum::AssocConst { value, .. } => ("assoc_const", value.is_some()),
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
        | ItemEnum::Primitive(_) => return None,
    };
    Some(TraitItem {
        name: item.name.as_deref().unwrap_or("_").into(),
        kind,
        provided,
    })
}

/// Why the trait is sealed, when one of its supertraits (`bounds`) is a trait of this crate that
/// has no public path, which no other crate can implement (requirement R-TI-5).
fn sealed(view: &CrateView<'_>, bounds: &[GenericBound]) -> Option<Box<str>> {
    bounds.iter().find_map(|bound| {
        let GenericBound::TraitBound { trait_, .. } = bound else {
            return None;
        };
        let summary = view.summary(trait_.id)?;
        let private = summary.crate_id == 0 && view.index().canonical_path(trait_.id).is_none();
        private.then(|| {
            format!(
                "it requires the supertrait {}, which is not public",
                summary.path.join("::")
            )
            .into()
        })
    })
}

/// Puts as many of `shown` into `response` as fit in the budget, and at least one.
fn fit(
    response: &mut ImplementorsResponse<'_>,
    mut shown: Vec<Implementor>,
    page: &impl Fn(usize) -> Page,
) {
    let mut count = shown.len();
    loop {
        response.data.listed = shown;
        response.page = page(count);
        let size = serde_json::to_string(&*response).map_or(0, |json| json.chars().count());
        if size <= MAX_RESPONSE_CHARS || count <= 1 {
            return;
        }
        shown = std::mem::take(&mut response.data.listed);
        count = count.saturating_sub(count.div_ceil(4).max(1));
        shown.truncate(count);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rustdoc_types::{
        Function,
        FunctionHeader,
        FunctionSignature,
        Generics,
        Impl,
        ItemKind,
        ItemSummary,
        Path,
        Trait,
        TraitBoundModifier,
        Type,
        Visibility,
    };
    use semver::Version;
    use serde_json::json;

    use super::*;
    use crate::{
        data::crate_name::CrateName,
        query::{
            get_item::{Detail, Origin, ResolvedFrom, tests::parsed},
            view::tests::{archive, archived},
        },
    };

    fn bare(id: u32, name: Option<&str>, inner: ItemEnum) -> Item {
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

    fn path(id: u32, name: &str) -> Path {
        Path {
            path: name.to_owned(),
            id: Id(id),
            args: None,
        }
    }

    /// The test crate of `get_item` with `pub trait Lock: Sealed { fn lock(&self); fn unlock(&self)
    /// {} }`, where `Sealed` is not public, and `impl Lock for Mutex`.
    fn with_lock() -> rustdoc_types::Crate {
        let mut krate = parsed().krate;
        let method = |id, name, has_body| {
            bare(
                id,
                Some(name),
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: Vec::new(),
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: no_generics(),
                    header: FunctionHeader {
                        is_const: false,
                        is_unsafe: false,
                        is_async: false,
                        abi: rustdoc_types::Abi::Rust,
                    },
                    has_body,
                    default_unstable: None,
                }),
            )
        };
        let lock = bare(
            30,
            Some("Lock"),
            ItemEnum::Trait(Trait {
                is_auto: false,
                is_unsafe: false,
                is_dyn_compatible: true,
                items: vec![Id(33), Id(34)],
                generics: no_generics(),
                bounds: vec![GenericBound::TraitBound {
                    trait_: path(32, "Sealed"),
                    generic_params: Vec::new(),
                    modifier: TraitBoundModifier::None,
                }],
                implementations: vec![Id(31)],
            }),
        );
        let implementation = bare(
            31,
            None,
            ItemEnum::Impl(Impl {
                is_unsafe: false,
                generics: no_generics(),
                provided_trait_methods: vec!["unlock".to_owned()],
                trait_: Some(path(30, "Lock")),
                for_: Type::ResolvedPath(path(3, "Mutex")),
                items: Vec::new(),
                is_negative: false,
                is_synthetic: false,
                blanket_impl: None,
            }),
        );
        let sealed = bare(
            32,
            Some("Sealed"),
            ItemEnum::Trait(Trait {
                is_auto: false,
                is_unsafe: false,
                is_dyn_compatible: true,
                items: Vec::new(),
                generics: no_generics(),
                bounds: Vec::new(),
                implementations: Vec::new(),
            }),
        );
        for item in [
            lock,
            implementation,
            sealed,
            method(33, "lock", false),
            method(34, "unlock", true),
        ] {
            krate.index.insert(item.id, item);
        }
        if let Some(ItemEnum::Module(root)) =
            krate.index.get_mut(&Id(0)).map(|root| &mut root.inner)
        {
            root.items.push(Id(30));
        }
        krate.paths.insert(Id(32), ItemSummary {
            crate_id: 0,
            path: vec!["my_crate".into(), "private".into(), "Sealed".into()],
            kind: ItemKind::Trait,
        });
        krate
    }

    #[test]
    fn a_trait_lists_its_implementors_and_whether_it_is_sealed() {
        let krate = with_lock();
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let response = list_implementors(
            &view,
            ItemRequest {
                name: &name,
                version: &version,
                path: "my_crate::Lock",
                origin: Origin::DocsRs {
                    resolved_from: ResolvedFrom::Explicit,
                    project_from: None,
                    build: None,
                },
                detail: Detail::Normal,
                project_features: None,
                follow: false,
                target: None,
            },
            ImplementorOptions {
                limit: 50,
                offset: 0,
                cursor_request: "c1.req",
                cursor_data: None,
            },
        )
        .expect("listed");
        let data = serde_json::to_value(&response).expect("serializes")["data"].clone();
        assert_eq!(
            data["items"],
            json!([
                { "name": "lock", "kind": "method", "provided": false },
                { "name": "unlock", "kind": "method", "provided": true },
            ])
        );
        assert_eq!(
            data["sealed"],
            "it requires the supertrait my_crate::private::Sealed, which is not public"
        );
        assert_eq!(
            data["implementors"],
            json!([{ "type": "my_crate::Mutex", "kind": "trait", "header": "impl my_crate::Lock for my_crate::Mutex" }])
        );
    }
}
