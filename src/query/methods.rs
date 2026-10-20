//! The methods that can be called on a type (requirements R-TI-1, R-LS-3 and R-SH-4).
//!
//! Its own, those of the traits it implements, and those of blanket impls. The methods of the
//! type it dereferences to, and those of primitive types gathered over core, alloc and std, are
//! M4.

use std::borrow::Cow;

use rustdoc_types::{Id, Impl, Item, ItemEnum, Type};
use serde::Serialize;

use super::{
    get_item::{
        GetItemError,
        ItemData,
        ItemRequest,
        MAX_RESPONSE_CHARS,
        Notes,
        Provenance,
        Query,
        kind_of,
        with_root_name,
    },
    impls::{TraitOrigin, find_type, trait_origin},
    list_module::Page,
    path_index::impls_of,
    view::CrateView,
};

/// The traits of core and alloc whose blanket impls every type has (requirement R-SH-4).
const STD_BLANKET: &[&str] = &[
    "From",
    "Into",
    "TryFrom",
    "TryInto",
    "Borrow",
    "BorrowMut",
    "Any",
    "ToOwned",
    "ToString",
    "CloneToUninit",
];

/// The traits of std's prelude, whose methods need no `use` (editions 2021 and 2024).
const PRELUDE: &[&str] = &[
    "AsMut",
    "AsRef",
    "Clone",
    "Copy",
    "Default",
    "DoubleEndedIterator",
    "Drop",
    "Eq",
    "ExactSizeIterator",
    "Extend",
    "Fn",
    "FnMut",
    "FnOnce",
    "From",
    "FromIterator",
    "Future",
    "Into",
    "IntoFuture",
    "IntoIterator",
    "Iterator",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Send",
    "Sized",
    "Sync",
    "ToOwned",
    "ToString",
    "TryFrom",
    "TryInto",
    "Unpin",
];

/// The crates of std.
const STD_CRATES: &[&str] = &["core", "alloc", "std", "proc_macro"];

/// The origins that `from` takes (requirement R-LS-3).
pub const ORIGINS: &[&str] = &["inherent", "trait", "blanket"];

/// How to list the methods.
#[derive(Debug, Clone, Copy)]
pub struct MethodOptions<'a> {
    /// Only methods whose name has this, in any case (requirement R-LS-3).
    pub name: Option<&'a str>,
    /// Only methods from these origins, all if empty.
    pub from: &'a [&'a str],
    /// List the methods of blanket impls of traits other than std's general ones one by one,
    /// rather than by trait (requirement R-SH-4).
    pub expand_blanket: bool,
    /// The most methods in a page.
    pub limit: usize,
    /// Where the page starts.
    pub offset: usize,
    /// What a cursor for the next page starts with (requirement R-OUT-3).
    pub cursor_request: &'a str,
    /// The digest of the data that the cursor was made for, when there is one.
    pub cursor_data: Option<&'a str>,
}

/// The result of `list_methods` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct MethodsResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// The type and its methods.
    pub data: Methods<'a>,
    /// Which part of the methods this is (requirement R-OUT-3).
    pub page: Page,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// A type and its methods.
#[derive(Debug, Serialize)]
pub struct Methods<'a> {
    /// The canonical path of the type.
    pub path: Box<str>,
    /// What the type is.
    pub kind: &'static str,
    /// The methods of the page: its own, then those of traits, then those of blanket impls.
    #[serde(rename = "methods")]
    pub listed: Vec<Method<'a>>,
    /// The methods that std's general traits give it through blanket impls, by name
    /// (`to_string`, `into`; requirement R-SH-4).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub std_blanket: Box<[Box<str>]>,
    /// Without `expand_blanket`, the other blanket impls, one for each trait.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blanket: Vec<BlanketTrait>,
}

/// A method.
#[derive(Debug, Serialize)]
pub struct Method<'a> {
    /// The method as `get_item` shows it at `detail=brief`, at the type's path.
    #[serde(flatten)]
    pub data: ItemData<'a>,
    /// `inherent`, `trait` or `blanket`.
    pub from: &'static str,
    /// The trait it comes from, by the path of its definition.
    #[serde(rename = "trait", skip_serializing_if = "Option::is_none")]
    pub trait_path: Option<Box<str>>,
    /// The trait must be in scope to call it, as it is not in std's prelude.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub needs_use: bool,
    /// Whether the trait gives it a default body (provided) or the impl must write it
    /// (required), when that is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provided: Option<bool>,
}

/// A blanket impl of a trait, with the names of its methods.
#[derive(Debug, Serialize)]
pub struct BlanketTrait {
    /// The trait, by the path of its definition.
    #[serde(rename = "trait")]
    pub trait_path: Box<str>,
    /// The crate of the trait.
    #[serde(rename = "crate", skip_serializing_if = "Option::is_none")]
    pub trait_crate: Option<Box<str>>,
    /// The names of its methods.
    pub methods: Box<[Box<str>]>,
}

/// Where a method comes from, in the order they are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum From {
    Inherent,
    Trait,
    Blanket,
}

impl From {
    const fn name(self) -> &'static str {
        match self {
            Self::Inherent => "inherent",
            Self::Trait => "trait",
            Self::Blanket => "blanket",
        }
    }
}

/// The methods of the type at `request.path`.
///
/// # Errors
///
/// Returns an error when the path names no type, or the cursor does not fit.
pub fn list_methods<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
    options: MethodOptions<'_>,
) -> Result<MethodsResponse<'a>, GetItemError> {
    let digest = view.source_digest();
    let data_digest = digest.get(..16).unwrap_or(digest);
    if let Some(cursor_data) = options.cursor_data
        && cursor_data != data_digest
    {
        return Err(GetItemError::BadCursor(
            "the data changed since the cursor was made; list again without the cursor",
        ));
    }
    let mut query = Query::new(view, request);
    let provenance = query.provenance();
    let with_root = with_root_name(view, request.name, request.path);
    let path = with_root.as_deref().unwrap_or(request.path);
    let ty = find_type(view, &mut query, path, request.follow)?;
    let Some(item) = view.item(ty) else {
        return Err(GetItemError::NotFound);
    };
    let type_path = query.path_of(ty);
    let Collected {
        mut methods,
        mut std_blanket,
        blanket,
    } = collect(
        view,
        &mut query,
        ty,
        item,
        &type_path,
        options.expand_blanket,
    );
    let wanted = options.name.map(str::to_lowercase);
    methods.retain(|method| {
        let name = method.data.path.rsplit("::").next().unwrap_or_default();
        wanted
            .as_deref()
            .is_none_or(|wanted| name.to_lowercase().contains(wanted))
            && (options.from.is_empty() || options.from.contains(&method.from))
    });
    // Its own first, then by trait, then by name (requirement R-OUT-4).
    methods.sort_by(|a, b| {
        let key = |method: &Method<'_>| {
            (
                ORIGINS.iter().position(|origin| *origin == method.from),
                method.trait_path.clone(),
                method.data.path.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    std_blanket.sort_unstable();
    std_blanket.dedup();
    let total = methods.len();
    if options.offset > total {
        return Err(GetItemError::BadCursor(
            "the cursor goes past the end of the methods; list again without the cursor",
        ));
    }
    let shown: Vec<Method<'a>> = methods
        .into_iter()
        .skip(options.offset)
        .take(options.limit)
        .collect();
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
    let mut response = MethodsResponse {
        provenance,
        warnings,
        data: Methods {
            path: type_path,
            kind: kind_of(item, false),
            listed: Vec::new(),
            std_blanket: std_blanket.into(),
            blanket,
        },
        page: page(0),
        hints,
    };
    fit(&mut response, shown, &page);
    Ok(response)
}

/// The methods of a type, sorted out.
struct Collected<'a> {
    methods: Vec<Method<'a>>,
    std_blanket: Vec<Box<str>>,
    blanket: Vec<BlanketTrait>,
}

/// The methods of the impls of the type `ty`, `item`, at `type_path`.
fn collect<'a>(
    view: &'a CrateView<'a>,
    query: &mut Query<'a>,
    ty: Id,
    item: &Item,
    type_path: &str,
    expand_blanket: bool,
) -> Collected<'a> {
    let mut collected = Collected {
        methods: Vec::new(),
        std_blanket: Vec::new(),
        blanket: Vec::new(),
    };
    let is_primitive = matches!(item.inner, ItemEnum::Primitive(_));
    for &impl_id in impls_of(item) {
        let Some(ItemEnum::Impl(contents)) = view.item(impl_id).map(|impl_item| &impl_item.inner)
        else {
            continue;
        };
        let for_this = is_primitive
            || matches!(&contents.for_, Type::ResolvedPath(path) if path.id == ty)
            || contents.blanket_impl.is_some();
        if !for_this || contents.is_synthetic || contents.is_negative {
            continue;
        }
        let Some(trait_ref) = &contents.trait_ else {
            for &member in &contents.items {
                if is_function(view, member)
                    && let Some(data) = query.item_data(member, Some(ty))
                {
                    collected.methods.push(Method {
                        data,
                        from: From::Inherent.name(),
                        trait_path: None,
                        needs_use: false,
                        provided: None,
                    });
                }
            }
            continue;
        };
        let Some(TraitOrigin {
            path: trait_path,
            krate: trait_crate,
        }) = trait_origin(view, trait_ref.id)
        else {
            continue;
        };
        let trait_name = trait_ref
            .path
            .rsplit("::")
            .next()
            .unwrap_or(&trait_ref.path);
        let of_std = trait_crate
            .as_deref()
            .is_some_and(|krate| STD_CRATES.contains(&krate));
        let names = method_names(view, contents);
        if contents.blanket_impl.is_some() && of_std && STD_BLANKET.contains(&trait_name) {
            collected.std_blanket.extend(names);
            continue;
        }
        if contents.blanket_impl.is_some() && !expand_blanket {
            collected.blanket.push(BlanketTrait {
                trait_path: trait_path.unwrap_or_else(|| trait_ref.path.as_str().into()),
                trait_crate,
                methods: names.into(),
            });
            continue;
        }
        let from = if contents.blanket_impl.is_some() {
            From::Blanket
        } else {
            From::Trait
        };
        let needs_use = !(of_std && PRELUDE.contains(&trait_name));
        let trait_path = trait_path.unwrap_or_else(|| trait_ref.path.as_str().into());
        trait_methods(
            view,
            query,
            ty,
            type_path,
            contents,
            trait_ref.id,
            &mut collected,
            &Origin {
                from,
                trait_path: &trait_path,
                needs_use,
            },
        );
    }
    collected
}

/// Where the methods of a trait impl come from.
struct Origin<'p> {
    from: From,
    trait_path: &'p str,
    needs_use: bool,
}

/// Adds the methods of the trait impl `contents` for `ty`: those it writes, and the provided
/// ones it does not override, by name when the trait is in another crate.
#[expect(
    clippy::too_many_arguments,
    reason = "the parts of one impl, which a struct would only rename"
)]
fn trait_methods<'a>(
    view: &'a CrateView<'a>,
    query: &mut Query<'a>,
    ty: Id,
    type_path: &str,
    contents: &Impl,
    trait_id: Id,
    collected: &mut Collected<'a>,
    origin: &Origin<'_>,
) {
    let trait_items = view.item(trait_id).and_then(|trait_item| {
        if let ItemEnum::Trait(contents) = &trait_item.inner {
            Some(&contents.items)
        } else {
            None
        }
    });
    let provided_in_trait = |name: &str| -> Option<bool> {
        let items = trait_items?;
        items.iter().find_map(|&id| {
            let item = view.item(id)?;
            let ItemEnum::Function(function) = &item.inner else {
                return None;
            };
            (item.name.as_deref() == Some(name)).then_some(function.has_body)
        })
    };
    let method = |data: ItemData<'a>, provided: Option<bool>| Method {
        data,
        from: origin.from.name(),
        trait_path: Some(origin.trait_path.into()),
        needs_use: origin.needs_use,
        provided,
    };
    for &member in &contents.items {
        if !is_function(view, member) {
            continue;
        }
        let name = view
            .item(member)
            .and_then(|item| item.name.as_deref())
            .unwrap_or_default();
        let provided = provided_in_trait(name);
        if let Some(mut data) = query.item_data(member, Some(ty)) {
            data.path = format!("{type_path}::{name}").into();
            collected.methods.push(method(data, provided));
        }
    }
    let written: Vec<&str> = contents
        .items
        .iter()
        .filter_map(|&member| view.item(member).and_then(|item| item.name.as_deref()))
        .collect();
    for name in &contents.provided_trait_methods {
        // The impl may override a provided method, which rustdoc lists here too.
        if written.contains(&name.as_str()) {
            continue;
        }
        // A trait of this crate has the method, to show; another's only names it here.
        let in_trait = trait_items.and_then(|items| {
            items.iter().copied().find(|&id| {
                view.item(id)
                    .is_some_and(|item| item.name.as_deref() == Some(name.as_str()))
            })
        });
        let mut data = in_trait
            .and_then(|id| query.item_data(id, Some(trait_id)))
            .unwrap_or_else(|| ItemData {
                kind: "method",
                ..ItemData::default()
            });
        data.path = format!("{type_path}::{name}").into();
        data.parent = None;
        collected.methods.push(method(data, Some(true)));
    }
}

/// The names of the methods of the impl `contents`: those it writes and the provided ones.
fn method_names(view: &CrateView<'_>, contents: &Impl) -> Vec<Box<str>> {
    let mut names: Vec<Box<str>> = contents
        .items
        .iter()
        .filter(|&&member| is_function(view, member))
        .filter_map(|&member| view.item(member).and_then(|item| item.name.as_deref()))
        .map(Into::into)
        .collect();
    names.extend(
        contents
            .provided_trait_methods
            .iter()
            .map(|name| name.as_str().into()),
    );
    names.sort_unstable();
    names.dedup();
    names
}

/// Whether `id` is a function.
fn is_function(view: &CrateView<'_>, id: Id) -> bool {
    view.item(id)
        .is_some_and(|item| matches!(item.inner, ItemEnum::Function(_)))
}

/// Puts as many of `methods` into `response` as fit in the budget, and at least one.
fn fit<'a>(
    response: &mut MethodsResponse<'a>,
    mut methods: Vec<Method<'a>>,
    page: &impl Fn(usize) -> Page,
) {
    let mut shown = methods.len();
    loop {
        response.data.listed = methods;
        response.page = page(shown);
        let size = serde_json::to_string(&*response).map_or(0, |json| json.chars().count());
        if size <= MAX_RESPONSE_CHARS || shown <= 1 {
            return;
        }
        methods = std::mem::take(&mut response.data.listed);
        shown = shown.saturating_sub(shown.div_ceil(4).max(1));
        methods.truncate(shown);
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_type_lists_its_own_methods() {
        let bytes = archive(parsed().krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let request = ItemRequest {
            name: &name,
            version: &version,
            path: "my_crate::Mutex",
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            detail: Detail::Brief,
            project_features: None,
            follow: false,
            target: None,
        };
        let options = MethodOptions {
            name: None,
            from: &[],
            expand_blanket: false,
            limit: 50,
            offset: 0,
            cursor_request: "c1.req",
            cursor_data: None,
        };
        let response = list_methods(&view, request, options).expect("listed");
        let data = serde_json::to_value(&response).expect("serializes")["data"].clone();
        assert_eq!(
            data["methods"],
            json!([{
                "path": "my_crate::Mutex::lock",
                "kind": "method",
                "parent": "my_crate::Mutex",
                "signature": "pub fn lock(&self) -> u8",
                "from": "inherent",
            }])
        );
        let only_traits = MethodOptions {
            from: &["trait"],
            ..options
        };
        let response = list_methods(&view, request, only_traits).expect("listed");
        assert_eq!(response.page.total, 0);
        let by_name = MethodOptions {
            name: Some("LOC"),
            ..options
        };
        let response = list_methods(&view, request, by_name).expect("listed");
        assert_eq!(response.page.total, 1);
    }
}
