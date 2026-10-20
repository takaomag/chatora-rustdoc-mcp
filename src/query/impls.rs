//! The traits that a type implements, sorted by how (requirements R-TI-2, R-SH-4 and R-TI-5).
//!
//! An `impl` is classified by its fields alone, never by the trait's name (requirement R-TI-2):
//! `is_synthetic` is an auto trait, `blanket_impl` a blanket one, `#[automatically_derived]` a
//! derive, no trait an inherent one, `is_negative` a negative one, the rest explicit.

use std::{borrow::Cow, collections::BTreeMap};

use rustdoc_types::{Attribute, Id, Impl, Item, ItemEnum, StructKind, Type};
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
    path_index::{Resolution, impls_of},
    view::CrateView,
};

/// The traits of core and alloc whose blanket impls every type has, shown by name in one line
/// (requirement R-SH-4).
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

/// Auto traits that are unstable, hidden by default (requirement R-TI-2).
const UNSTABLE_AUTO: &[&str] = &["Freeze", "UnsafeUnpin"];

/// The crates of std, whose impls come last (requirement R-SH-4).
const STD_CRATES: &[&str] = &["core", "alloc", "std", "proc_macro"];

/// How to page the impls.
#[derive(Debug, Clone, Copy)]
pub struct ImplOptions<'a> {
    /// The most impls in a page.
    pub limit: usize,
    /// Where the page starts.
    pub offset: usize,
    /// What a cursor for the next page starts with (requirement R-OUT-3).
    pub cursor_request: &'a str,
    /// The digest of the data that the cursor was made for, when there is one.
    pub cursor_data: Option<&'a str>,
}

/// The result of `list_impls` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct ImplsResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// The type and its impls.
    pub data: Impls,
    /// Which part of the impls this is (requirement R-OUT-3).
    pub page: Page,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// A type and the traits it implements.
#[derive(Debug, Serialize)]
pub struct Impls {
    /// The canonical path of the type.
    pub path: Box<str>,
    /// What the type is.
    pub kind: &'static str,
    /// It is `#[non_exhaustive]` (requirement R-TI-5).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_exhaustive: bool,
    /// It has fields that are not public (requirement R-TI-5).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub has_private_fields: bool,
    /// Each auto trait: `yes`, `no`, or `if` and its conditions (requirement R-TI-2).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub auto_traits: BTreeMap<Box<str>, Box<str>>,
    /// The traits of core and alloc that it has through blanket impls, by name
    /// (requirement R-SH-4).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub std_blanket: Box<[Box<str>]>,
    /// The other impls of the page: those of this crate, then of other crates, then of std.
    #[serde(rename = "impls")]
    pub entries: Vec<ImplEntry>,
}

/// One `impl`.
#[derive(Debug, Serialize)]
pub struct ImplEntry {
    /// `inherent`, `trait`, `derive`, `blanket` or `negative`.
    pub kind: &'static str,
    /// The trait, by the path of its definition.
    #[serde(rename = "trait", skip_serializing_if = "Option::is_none")]
    pub trait_path: Option<Box<str>>,
    /// The crate of the trait, when it is another.
    #[serde(rename = "crate", skip_serializing_if = "Option::is_none")]
    pub trait_crate: Option<Box<str>>,
    /// The `impl` as Rust writes it, with its where clause.
    pub header: Box<str>,
    /// The features and other conditions that the `impl` is written with (requirement R-FT-2).
    #[serde(flatten)]
    pub conditions: Conditions,
}

/// How an `impl` was made (requirement R-TI-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum How {
    Inherent,
    Trait,
    Derive,
    Blanket,
    Negative,
    Auto,
}

impl How {
    fn of(item: &Item, contents: &Impl) -> Self {
        if contents.is_synthetic {
            Self::Auto
        } else if contents.blanket_impl.is_some() {
            Self::Blanket
        } else if contents.trait_.is_none() {
            Self::Inherent
        } else if contents.is_negative {
            Self::Negative
        } else if item.attrs.contains(&Attribute::AutomaticallyDerived) {
            Self::Derive
        } else {
            Self::Trait
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Inherent => "inherent",
            Self::Trait => "trait",
            Self::Derive => "derive",
            Self::Blanket => "blanket",
            Self::Negative => "negative",
            Self::Auto => "auto",
        }
    }
}

/// The traits that the type at `request.path` implements.
///
/// # Errors
///
/// Returns an error when the path names no type, or the cursor does not fit.
pub fn list_impls<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
    options: ImplOptions<'_>,
) -> Result<ImplsResponse<'a>, GetItemError> {
    let digest = view.source_digest();
    let data_digest = digest.get(..16).unwrap_or(digest);
    let mut query = Query::new(view, request);
    let provenance = query.provenance();
    let with_root = with_root_name(view, request.name, request.path);
    let path = with_root.as_deref().unwrap_or(request.path);
    let ty = find_type(view, &mut query, path, request.follow)?;
    // Checked in the crate that the path goes on in, if it does.
    if let Some(cursor_data) = options.cursor_data
        && cursor_data != data_digest
    {
        return Err(GetItemError::BadCursor(
            "the data changed since the cursor was made; list again without the cursor",
        ));
    }
    let Some(item) = view.item(ty) else {
        return Err(GetItemError::NotFound);
    };
    let Classified {
        auto_traits,
        mut std_blanket,
        mut entries,
    } = classify(view, &mut query, item, ty);
    // By where the trait comes from, then how, then the trait, then the header (R-OUT-4).
    entries.sort_by(|a, b| (a.0, a.1, &a.2, &a.3.header).cmp(&(b.0, b.1, &b.2, &b.3.header)));
    std_blanket.sort_unstable();
    std_blanket.dedup();
    let total = entries.len();
    if options.offset > total {
        return Err(GetItemError::BadCursor(
            "the cursor goes past the end of the impls; list again without the cursor",
        ));
    }
    let impls: Vec<ImplEntry> = entries
        .into_iter()
        .skip(options.offset)
        .take(options.limit)
        .map(|(_, _, _, entry)| entry)
        .collect();
    let (non_exhaustive, has_private_fields) = shape_of(item);
    let path = query.path_of(ty);
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
    let mut response = ImplsResponse {
        provenance,
        warnings,
        data: Impls {
            path,
            kind: kind_of(item, false),
            non_exhaustive,
            has_private_fields,
            auto_traits,
            std_blanket: std_blanket.into(),
            entries: Vec::new(),
        },
        page: page(0),
        hints,
    };
    fit(&mut response, impls, &page);
    Ok(response)
}

/// The impls of a type, sorted out (requirements R-TI-2 and R-SH-4).
struct Classified {
    auto_traits: BTreeMap<Box<str>, Box<str>>,
    std_blanket: Vec<Box<str>>,
    /// The other impls, with what they are sorted by: where the trait comes from (this crate,
    /// another, std), how the impl was made, and the trait.
    entries: Vec<(u8, How, Box<str>, ImplEntry)>,
}

/// Sorts out the impls of the type `ty`, `item`: auto traits, the blanket impls of std's
/// general traits, and the rest.
fn classify(view: &CrateView<'_>, query: &mut Query<'_>, item: &Item, ty: Id) -> Classified {
    let auto_ids = auto_traits_of(view);
    let mut auto_traits = BTreeMap::new();
    let mut std_blanket = Vec::new();
    let mut entries: Vec<(u8, How, Box<str>, ImplEntry)> = Vec::new();
    for &impl_id in impls_of(item) {
        let Some(impl_item) = view.item(impl_id) else {
            continue;
        };
        let ItemEnum::Impl(contents) = &impl_item.inner else {
            continue;
        };
        let is_primitive = matches!(item.inner, ItemEnum::Primitive(_));
        let for_this = is_primitive
            || matches!(&contents.for_, Type::ResolvedPath(path) if path.id == ty)
            || contents.blanket_impl.is_some();
        if !for_this {
            continue;
        }
        let how = How::of(impl_item, contents);
        let trait_id = contents.trait_.as_ref().map(|path| path.id);
        let trait_name = contents.trait_.as_ref().map(|path| {
            path.path
                .rsplit("::")
                .next()
                .unwrap_or(&path.path)
                .to_owned()
        });
        let Some(TraitOrigin {
            path: trait_path,
            krate: trait_crate,
        }) = trait_id.map_or_else(|| Some(TraitOrigin::default()), |id| trait_origin(view, id))
        else {
            continue;
        };
        if let (Some(id), Some(name)) = (trait_id, &trait_name)
            && (how == How::Auto || auto_ids.contains(&id))
        {
            if UNSTABLE_AUTO.contains(&name.as_str()) {
                continue;
            }
            let condition: Box<str> = if how == How::Negative {
                "no".into()
            } else {
                let header = query.impl_text(impl_item);
                let conditions = conditions(&header, &query.where_text(impl_item));
                if conditions.is_empty() {
                    "yes".into()
                } else {
                    format!("if {}", conditions.join(", ")).into()
                }
            };
            auto_traits.insert(name.as_str().into(), condition);
            continue;
        }
        let of_std = trait_crate
            .as_deref()
            .is_some_and(|krate| STD_CRATES.contains(&krate));
        if how == How::Blanket
            && of_std
            && let Some(name) = &trait_name
            && STD_BLANKET.contains(&name.as_str())
        {
            std_blanket.push(Box::from(name.as_str()));
            continue;
        }
        let rank = match trait_crate.as_deref() {
            None => 0,
            Some(_) if of_std => 2,
            Some(_) => 1,
        };
        let header = query.impl_text(impl_item);
        entries.push((
            rank,
            how,
            trait_path.clone().unwrap_or_default(),
            ImplEntry {
                kind: how.name(),
                trait_path,
                trait_crate,
                header,
                conditions: query.own_conditions(impl_id),
            },
        ));
    }
    Classified {
        auto_traits,
        std_blanket,
        entries,
    }
}

/// The conditions of an `impl` whose header is `header`: the bounds of its type parameters
/// (`impl<T: Sync + Send>`) and its where predicates, without `?Sized`, which allows more types
/// rather than fewer.
fn conditions(header: &str, predicates: &[String]) -> Vec<String> {
    let mut conditions = Vec::new();
    if let Some(rest) = header.strip_prefix("impl<") {
        let mut depth = 0_usize;
        let mut start = 0;
        let mut params = Vec::new();
        for (at, c) in rest.char_indices() {
            match c {
                '<' | '(' | '[' => depth = depth.saturating_add(1),
                '>' if depth == 0 => {
                    params.push(rest.get(start..at).unwrap_or_default());
                    break;
                },
                '>' | ')' | ']' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    params.push(rest.get(start..at).unwrap_or_default());
                    start = at.saturating_add(1);
                },
                _ => {},
            }
        }
        conditions.extend(params.iter().filter_map(|param| without_maybe_sized(param)));
    }
    conditions.extend(
        predicates
            .iter()
            .filter_map(|predicate| without_maybe_sized(predicate)),
    );
    conditions
}

/// `bound` (`T: ?Sized + Send`) without `?Sized`, or `None` if nothing else is left.
fn without_maybe_sized(bound: &str) -> Option<String> {
    let (subject, bounds) = bound.trim().split_once(':')?;
    let kept: Vec<&str> = bounds
        .split('+')
        .map(str::trim)
        .filter(|bound| !bound.is_empty() && *bound != "?Sized")
        .collect();
    (!kept.is_empty()).then(|| format!("{}: {}", subject.trim(), kept.join(" + ")))
}

/// The type at `path`: a struct, an enum, a union or a primitive type.
pub(super) fn find_type(
    view: &CrateView<'_>,
    query: &mut Query<'_>,
    path: &str,
    follow: bool,
) -> Result<Id, GetItemError> {
    let is_type = |id: Id| {
        view.item(id).is_some_and(|item| {
            matches!(
                item.inner,
                ItemEnum::Struct(_)
                    | ItemEnum::Enum(_)
                    | ItemEnum::Union(_)
                    | ItemEnum::Primitive(_)
            )
        })
    };
    let id = match view.index().resolve(view, path) {
        Resolution::Item(id) => id,
        Resolution::Ambiguous(ids) => {
            let types: Vec<Id> = ids.iter().copied().filter(|&id| is_type(id)).collect();
            match types.as_slice() {
                [ty] => *ty,
                _ => {
                    return Err(GetItemError::Ambiguous {
                        candidates: ids.iter().map(|&id| query.path_of(id)).collect(),
                    });
                },
            }
        },
        Resolution::Member { member, .. } => {
            return Err(GetItemError::NotAType {
                kind: view
                    .item(member)
                    .map_or("member", |item| kind_of(item, true)),
            });
        },
        Resolution::AmbiguousMembers { .. } => {
            return Err(GetItemError::NotAType { kind: "member" });
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
    if is_type(id) {
        Ok(id)
    } else {
        Err(GetItemError::NotAType {
            kind: view.item(id).map_or("item", |item| kind_of(item, false)),
        })
    }
}

/// The traits that the crate's data shows to be auto traits: those of its synthetic impls, and
/// its own traits marked auto.
fn auto_traits_of(view: &CrateView<'_>) -> Vec<Id> {
    let mut ids = Vec::new();
    for id in view.index().public_items() {
        let Some(item) = view.item(id) else {
            continue;
        };
        if let ItemEnum::Trait(contents) = &item.inner {
            if contents.is_auto {
                ids.push(id);
            }
            continue;
        }
        for &impl_id in impls_of(item) {
            if let Some(Item {
                inner: ItemEnum::Impl(contents),
                ..
            }) = view.item(impl_id)
                && contents.is_synthetic
                && let Some(path) = &contents.trait_
                && !ids.contains(&path.id)
            {
                ids.push(path.id);
            }
        }
    }
    ids
}

/// The trait of an impl, as a response names it.
#[derive(Debug, Default)]
pub(super) struct TraitOrigin {
    /// For a trait of this crate its canonical path, which can be `use`d; for another's the
    /// path where it is defined.
    pub(super) path: Option<Box<str>>,
    /// Its crate, when that is another.
    pub(super) krate: Option<Box<str>>,
}

/// The trait `id` as a response names it. `None` for a trait of this crate without a public
/// path, which is not public API (std's `SpecExtend`; requirement R-TI-2).
pub(super) fn trait_origin(view: &CrateView<'_>, id: Id) -> Option<TraitOrigin> {
    let Some(summary) = view.summary(id) else {
        return Some(TraitOrigin::default());
    };
    if summary.crate_id == 0 {
        let canonical = view.index().canonical_path(id)?;
        return Some(TraitOrigin {
            path: Some(canonical.joined()),
            krate: None,
        });
    }
    Some(TraitOrigin {
        path: Some(summary.path.join("::").into()),
        krate: view
            .external_crate(summary.crate_id)
            .map(|external| external.name.as_str().into()),
    })
}

/// Whether the type is `#[non_exhaustive]`, and whether it has fields that are not public.
fn shape_of(item: &Item) -> (bool, bool) {
    let non_exhaustive = item.attrs.contains(&Attribute::NonExhaustive);
    let private = match &item.inner {
        ItemEnum::Struct(contents) => match &contents.kind {
            StructKind::Plain {
                has_stripped_fields,
                ..
            } => *has_stripped_fields,
            StructKind::Tuple(fields) => fields.iter().any(Option::is_none),
            StructKind::Unit => false,
        },
        ItemEnum::Union(contents) => contents.has_stripped_fields,
        ItemEnum::Module(_)
        | ItemEnum::ExternCrate { .. }
        | ItemEnum::Use(_)
        | ItemEnum::StructField(_)
        | ItemEnum::Enum(_)
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
        | ItemEnum::Primitive(_)
        | ItemEnum::AssocConst { .. }
        | ItemEnum::AssocType { .. } => false,
    };
    (non_exhaustive, private)
}

/// Puts as many of `impls` into `response` as fit in the budget, and at least one.
fn fit(response: &mut ImplsResponse<'_>, mut impls: Vec<ImplEntry>, page: &impl Fn(usize) -> Page) {
    let mut shown = impls.len();
    loop {
        response.data.entries = impls;
        response.page = page(shown);
        let size = serde_json::to_string(&*response).map_or(0, |json| json.chars().count());
        if size <= MAX_RESPONSE_CHARS || shown <= 1 {
            return;
        }
        impls = std::mem::take(&mut response.data.entries);
        shown = shown.saturating_sub(shown.div_ceil(4).max(1));
        impls.truncate(shown);
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
            get_item::{
                Detail,
                Origin,
                ResolvedFrom,
                tests::{parsed, with_cfgs},
            },
            view::tests::{archive, archived},
        },
    };

    #[test]
    fn a_type_lists_its_impls_and_a_module_is_not_a_type() {
        let bytes = archive(parsed().krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let request = |path| ItemRequest {
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
        };
        let options = ImplOptions {
            limit: 50,
            offset: 0,
            cursor_request: "c1.req",
            cursor_data: None,
        };
        let response = list_impls(&view, request("my_crate::Mutex"), options).expect("listed");
        let data = serde_json::to_value(&response).expect("serializes")["data"].clone();
        assert_eq!(
            data,
            json!({
                "path": "my_crate::Mutex",
                "kind": "struct",
                "has_private_fields": true,
                "impls": [{ "kind": "inherent", "header": "impl my_crate::Mutex" }],
            })
        );
        assert!(matches!(
            list_impls(&view, request("my_crate::sync"), options),
            Err(GetItemError::NotAType { kind: "module" })
        ));
    }

    #[test]
    fn an_impl_tells_its_own_cfg_but_not_that_of_its_type() {
        let mut krate = parsed().krate;
        with_cfgs(&mut krate);
        let bytes = archive(krate);
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
            detail: Detail::Normal,
            project_features: None,
            follow: false,
            target: None,
        };
        let options = ImplOptions {
            limit: 50,
            offset: 0,
            cursor_request: "c1.req",
            cursor_data: None,
        };
        let response = list_impls(&view, request, options).expect("listed");
        let data = serde_json::to_value(&response).expect("serializes")["data"].clone();
        // `sync`, which the re-export of `Mutex` needs, is in `get_item` of the type.
        assert_eq!(
            data["impls"],
            json!([{
                "kind": "inherent",
                "header": "impl my_crate::Mutex",
                "other_cfg": ["tokio_unstable"],
                "cfg_source": "cfg_trace",
            }])
        );
    }

    #[test]
    fn a_cursor_is_checked_in_the_crate_that_the_path_goes_on_in() {
        use crate::query::{
            implementors::{ImplementorOptions, list_implementors},
            list_module::{ListOptions, list_module},
            methods::{MethodOptions, list_methods},
        };

        let bytes = archive(parsed().krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        // `Thing` is a re-export of another crate's item, and the cursor is of that crate's data.
        let request = ItemRequest {
            name: &name,
            version: &version,
            path: "my_crate::Thing",
            origin: Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            detail: Detail::Normal,
            project_features: None,
            follow: true,
            target: None,
        };
        let (cursor_request, cursor_data) = ("c1.req", Some("0123456789abcdef"));
        let results = [
            list_impls(&view, request, ImplOptions {
                limit: 50,
                offset: 10,
                cursor_request,
                cursor_data,
            })
            .err(),
            list_methods(&view, request, MethodOptions {
                name: None,
                from: &[],
                expand_blanket: false,
                limit: 50,
                offset: 10,
                cursor_request,
                cursor_data,
            })
            .err(),
            list_implementors(&view, request, ImplementorOptions {
                limit: 50,
                offset: 10,
                cursor_request,
                cursor_data,
            })
            .err(),
            list_module(&view, request, ListOptions {
                recursive: false,
                depth: 1,
                kinds: &[],
                limit: 50,
                offset: 10,
                cursor_request,
                cursor_data,
            })
            .err(),
        ];
        for result in results {
            assert!(
                matches!(result, Some(GetItemError::Elsewhere(_))),
                "{result:?}"
            );
        }
    }

    #[test]
    fn conditions_come_from_parameters_and_where_clauses_but_not_maybe_sized() {
        assert_eq!(
            conditions(
                "impl<T: ?Sized + Sync + Send, A: Allocator + Send> Send for Arc<T, A>",
                &[]
            ),
            ["T: Sync + Send", "A: Allocator + Send"]
        );
        assert_eq!(
            conditions(
                "impl<T> Send for Mutex<T>",
                &["T: ?Sized + Send".to_owned()]
            ),
            ["T: Send"]
        );
        assert_eq!(
            conditions("impl<T: ?Sized> Unpin for Box<T>", &[]),
            Vec::<String>::new()
        );
        assert_eq!(
            conditions("impl<K: Hash<X, Y>, V> Sync for Map<K, V>", &[]),
            ["K: Hash<X, Y>"]
        );
    }
}
