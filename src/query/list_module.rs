//! Listing a module: its items and the re-exports in it, and with `recursive` those of its
//! submodules, a page at a time (requirements R-LS-1, R-LS-2, R-OUT-3 and R-OUT-4).

use std::{borrow::Cow, collections::HashSet};

use rustdoc_types::{Id, ItemEnum};
use serde::Serialize;

use super::{
    get_item::{
        Detail,
        GetItemError,
        ItemData,
        ItemRequest,
        MAX_RESPONSE_CHARS,
        Notes,
        Provenance,
        Query,
        first_paragraph,
        kind_of,
        summary_kind,
        with_root_name,
    },
    path_index::{ExternalReexport, Resolution},
    view::CrateView,
};

/// The kinds that `kind` takes, as `get_item` names them. Members (`method` and the associated
/// items) are found by `search_items`; `list_module` lists none.
pub const KINDS: &[&str] = &[
    "module",
    "struct",
    "enum",
    "union",
    "trait",
    "trait_alias",
    "function",
    "type_alias",
    "constant",
    "static",
    "macro",
    "proc_attribute",
    "proc_derive",
    "primitive",
    "extern_crate",
    "extern_type",
    "variant",
    "method",
    "assoc_const",
    "assoc_type",
];

/// How to list a module.
#[derive(Debug, Clone, Copy)]
pub struct ListOptions<'a> {
    /// Also list the items of the submodules (requirement R-LS-1).
    pub recursive: bool,
    /// How many levels of submodules below the module to list, with `recursive`.
    pub depth: usize,
    /// The kinds to list, all of them if empty (requirement R-LS-2).
    pub kinds: &'a [&'a str],
    /// The most items in a page.
    pub limit: usize,
    /// Where the page starts.
    pub offset: usize,
    /// What a cursor for the next page starts with: the digest of the request, which the
    /// caller makes. The digest of the data and the offset are added (requirement R-OUT-3).
    pub cursor_request: &'a str,
    /// The digest of the data that the cursor was made for, when there is one.
    pub cursor_data: Option<&'a str>,
}

/// The result of `list_module` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct ModuleResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// The module and its items.
    pub data: ModuleData<'a>,
    /// Which part of the list this is (requirement R-OUT-3).
    pub page: Page,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// A module and its items.
#[derive(Debug, Serialize)]
pub struct ModuleData<'a> {
    /// The path of the module as asked for, or its canonical one if it has no items there; for a
    /// module without a public path, its path in the crate of a workspace member, or its
    /// definition path.
    pub path: Box<str>,
    /// The first paragraph of the module's documentation, with `detail` normal or full.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<Cow<'a, str>>,
    /// The documentation goes on after the first paragraph.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub docs_truncated: bool,
    /// The items of the page, each at the path it is listed at.
    pub items: Vec<ItemData<'a>>,
}

/// Which part of a list a response has (requirement R-OUT-3).
#[derive(Debug, Serialize)]
pub struct Page {
    /// How many items it has.
    pub shown: usize,
    /// How many items the list has, after filtering.
    pub total: usize,
    /// More items come after.
    pub has_more: bool,
    /// What to pass as `cursor` for the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Box<str>>,
}

/// What is at a path in the list.
enum Entry {
    /// An item of this crate.
    Item(Id),
    /// A re-export of another crate's item.
    External(ExternalReexport),
}

/// Lists the module at `request.path` in the crate of `view`.
///
/// # Errors
///
/// Returns an error when nothing or more than one item has the path, the item is not a module,
/// the path goes on in another crate (with `request.follow`), or the cursor does not fit.
pub fn list_module<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
    options: ListOptions<'_>,
) -> Result<ModuleResponse<'a>, GetItemError> {
    let index = view.index();
    let data_digest = digest_part(view.source_digest());
    // The items are listed as `brief` or `normal` would show them; `full` shows each as
    // `get_item` does by default.
    let item_detail = match request.detail {
        Detail::Brief => Detail::Brief,
        Detail::Normal | Detail::Full => Detail::Normal,
    };
    let mut query = Query::new(view, ItemRequest {
        detail: item_detail,
        ..request
    });
    let provenance = query.provenance();
    let with_root = with_root_name(view, request.name, request.path);
    let path = with_root.as_deref().unwrap_or(request.path);
    let module = find_module(view, &mut query, path, request.follow)?;
    // Another crate's hidden module is not its API (requirement R-SH-5, decided in M2's D6).
    if !query.is_member() && index.is_hidden(module) {
        return Err(GetItemError::NotFound);
    }
    // Checked in the crate that the path goes on in, if it does.
    if let Some(cursor_data) = options.cursor_data
        && cursor_data != data_digest
    {
        return Err(GetItemError::BadCursor(
            "the data changed since the cursor was made; list again without the cursor",
        ));
    }
    let Some(item) = view.item(module) else {
        return Err(GetItemError::NotFound);
    };
    // The module is listed under the path asked for, if it has items there (a module re-exported
    // under another name has them under each name), or else under its canonical path.
    let listed_at: Box<str> = if index.children(path).is_empty() {
        query.path_of(module)
    } else {
        path.into()
    };
    let mut entries = Vec::new();
    let depth = if options.recursive { options.depth } else { 1 };
    let mut visited = HashSet::from([module]);
    // A workspace member's items without a public path too (decided in M2's D6).
    let mut members = query.is_member().then_some(&mut query);
    collect(
        view,
        (module, &listed_at),
        depth,
        (&mut visited, &mut members),
        &mut entries,
    );
    entries.retain(|(_, entry)| {
        options.kinds.is_empty() || options.kinds.contains(&entry_kind(view, entry))
    });
    let total = entries.len();
    if options.offset > total {
        return Err(GetItemError::BadCursor(
            "the cursor goes past the end of the list; list again without the cursor",
        ));
    }
    let items: Vec<ItemData<'a>> = entries
        .iter()
        .skip(options.offset)
        .take(options.limit)
        .map(|(path, entry)| entry_data(&mut query, path, entry, request.detail))
        .collect();
    let (docs, docs_truncated) = match (request.detail, item.docs.as_deref()) {
        (Detail::Normal | Detail::Full, Some(docs)) => {
            let (first, more) = first_paragraph(docs);
            (Some(Cow::Borrowed(first)), more)
        },
        (Detail::Brief, _) | (_, None) => (None, false),
    };
    let Notes { warnings, hints } = query.finish();
    let cursor = |offset: usize| -> Box<str> {
        format!("{}.{data_digest}.{offset}", options.cursor_request).into()
    };
    let page = |shown: usize| {
        let next = options.offset.saturating_add(shown);
        Page {
            shown,
            total,
            has_more: next < total,
            next_cursor: (next < total).then(|| cursor(next)),
        }
    };
    let mut response = ModuleResponse {
        provenance,
        warnings,
        data: ModuleData {
            path: listed_at,
            docs,
            docs_truncated,
            items: Vec::new(),
        },
        page: page(0),
        hints,
    };
    fit(&mut response, items, &page);
    Ok(response)
}

/// The module at `path`: an item that is a module, or the one module among the items there.
fn find_module(
    view: &CrateView<'_>,
    query: &mut Query<'_>,
    path: &str,
    follow: bool,
) -> Result<Id, GetItemError> {
    let module = match query.resolve(path) {
        Resolution::Item(id) => id,
        Resolution::Ambiguous(ids) => {
            // A module and a macro (or a function) may have the same path.
            let modules: Vec<Id> = ids
                .iter()
                .copied()
                .filter(|&id| is_module(view, id))
                .collect();
            match modules.as_slice() {
                [module] => *module,
                _ => {
                    return Err(GetItemError::Ambiguous {
                        candidates: ids.iter().map(|id| query.path_of(*id)).collect(),
                    });
                },
            }
        },
        Resolution::Member { member, .. } => {
            return Err(GetItemError::NotAModule {
                kind: view
                    .item(member)
                    .map_or("member", |item| kind_of(item, true)),
            });
        },
        Resolution::AmbiguousMembers { .. } => {
            return Err(GetItemError::NotAModule { kind: "member" });
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
    match view.item(module) {
        Some(item) if !matches!(item.inner, ItemEnum::Module(_)) => Err(GetItemError::NotAModule {
            kind: kind_of(item, false),
        }),
        Some(_) | None => Ok(module),
    }
}

/// Puts as many of `items` into `response` as fit in the budget, and at least one
/// (requirement R-OUT-3), with the page that `page` makes for that many.
fn fit<'a>(
    response: &mut ModuleResponse<'a>,
    mut items: Vec<ItemData<'a>>,
    page: &impl Fn(usize) -> Page,
) {
    let mut shown = items.len();
    loop {
        response.data.items = items;
        response.page = page(shown);
        let size = serde_json::to_string(&*response).map_or(0, |json| json.chars().count());
        if size <= MAX_RESPONSE_CHARS || shown <= 1 {
            return;
        }
        items = std::mem::take(&mut response.data.items);
        shown = shown.saturating_sub(shown.div_ceil(4).max(1));
        items.truncate(shown);
    }
}

/// The part of a digest that cursors carry.
fn digest_part(digest: &str) -> &str {
    digest.get(..16).unwrap_or(digest)
}

/// Whether `id` is a module.
fn is_module(view: &CrateView<'_>, id: Id) -> bool {
    view.item(id)
        .is_some_and(|item| matches!(item.inner, ItemEnum::Module(_)))
}

/// Puts the items below `path`, the path of `module`, into `entries`, in order, and with `depth`
/// above 1 those of the modules among them, each right after its module. A module is listed once,
/// against cycles of re-exports. With `query`, of a workspace member, its items without a public
/// path follow, at their paths in the crate, and, in a module without a public path, all of them
/// (decided in M2's D6); else hidden items are left out (requirement R-SH-5).
fn collect(
    view: &CrateView<'_>,
    (module, path): (Id, &str),
    depth: usize,
    (visited, query): (&mut HashSet<Id>, &mut Option<&mut Query<'_>>),
    entries: &mut Vec<(Box<str>, Entry)>,
) {
    let index = view.index();
    for child in index.children(path) {
        let ids = match index.at_public_path(&child) {
            Some(Resolution::Item(id)) => vec![id],
            Some(Resolution::Ambiguous(ids)) => ids.into_vec(),
            Some(Resolution::ExternalReexport(reexport)) => {
                if query.is_some() || !index.is_hidden_reexport(&child) {
                    entries.push((child, Entry::External(reexport)));
                }
                continue;
            },
            Some(
                Resolution::Member { .. }
                | Resolution::AmbiguousMembers { .. }
                | Resolution::NotFound,
            )
            | None => continue,
        };
        for id in ids {
            if query.is_none() && index.is_hidden(id) {
                continue;
            }
            entries.push((child.clone(), Entry::Item(id)));
            if depth > 1 && is_module(view, id) && visited.insert(id) {
                collect(
                    view,
                    (id, &child),
                    depth.saturating_sub(1),
                    (visited, query),
                    entries,
                );
            }
        }
    }
    let Some(query) = query.as_deref_mut() else {
        return;
    };
    let Some(ItemEnum::Module(contents)) = view.item(module).map(|item| &item.inner) else {
        return;
    };
    let public = index.canonical_path(module).is_some();
    // Not again what the list has, as an item of a module without a public path that a public
    // module re-exports with a glob.
    let listed: HashSet<Id> = entries
        .iter()
        .filter_map(|(_, entry)| match entry {
            Entry::Item(id) => Some(*id),
            Entry::External(_) => None,
        })
        .collect();
    let unlisted: Vec<Id> = contents
        .items
        .iter()
        .copied()
        .filter(|&id| {
            !listed.contains(&id)
                && view.item(id).is_some_and(|item| {
                    !matches!(item.inner, ItemEnum::Impl(_) | ItemEnum::Use(_))
                        && (!public || index.canonical_path(id).is_none())
                })
        })
        .collect();
    for id in unlisted {
        let at = query.path_of(id);
        entries.push((at.clone(), Entry::Item(id)));
        if depth > 1 && is_module(view, id) && visited.insert(id) {
            let mut query = Some(&mut *query);
            collect(
                view,
                (id, &at),
                depth.saturating_sub(1),
                (visited, &mut query),
                entries,
            );
        }
    }
}

/// The kind of what is at a path.
fn entry_kind(view: &CrateView<'_>, entry: &Entry) -> &'static str {
    match entry {
        Entry::Item(id) => view
            .item(*id)
            .map_or("unknown", |item| kind_of(item, false)),
        Entry::External(reexport) => view
            .summary(reexport.target)
            .map_or("unknown", |summary| summary_kind(summary.kind)),
    }
}

/// One item of the list, at the path it is listed at, as much of it as `detail` shows.
fn entry_data<'a>(
    query: &mut Query<'a>,
    path: &str,
    entry: &Entry,
    detail: Detail,
) -> ItemData<'a> {
    match entry {
        Entry::External(reexport) => {
            let kind = query.external_kind(reexport.target);
            ItemData {
                path: path.into(),
                kind,
                hidden: query.hidden_reexport(path),
                reexport_of: query.reexport_target(reexport),
                ..ItemData::default()
            }
        },
        Entry::Item(id) => {
            let Some(mut data) = query.item_data(*id, None).map(ItemData::listed) else {
                return ItemData {
                    path: path.into(),
                    kind: "unknown",
                    ..ItemData::default()
                };
            };
            if detail == Detail::Normal {
                // The rest is for `get_item`; a list keeps one line or so of each.
                data.fields = Box::default();
                data.variants = Box::default();
                data.sections = Box::default();
                data.also_reexported_at = Box::default();
                data.source_location = None;
            }
            if *data.path != *path {
                data.canonical = Some(std::mem::replace(&mut data.path, path.into()));
            }
            data
        },
    }
}

#[cfg(test)]
mod tests {
    use rustdoc_types::Crate;
    use semver::Version;
    use serde_json::json;

    use super::*;
    use crate::{
        data::crate_name::CrateName,
        query::{
            get_item::{Origin, ResolvedFrom, tests::parsed},
            view::tests::{archive, archived},
        },
    };

    /// What `list_module` gives for a path: the path, kind and canonical path of each item, and
    /// the page; or its error.
    enum Listed {
        Ok(serde_json::Value, serde_json::Value),
        Err(GetItemError),
    }

    impl Listed {
        fn expect(self, what: &str) -> (serde_json::Value, serde_json::Value) {
            match self {
                Self::Ok(items, page) => (items, page),
                Self::Err(error) => panic!("{what}: {error:?}"),
            }
        }
    }

    /// What `list_module` gives for `path` in the test crate of `get_item`.
    fn listed(path: &str, options: ListOptions<'_>) -> Listed {
        listed_with(path, options, |_| {})
    }

    /// [`listed`], after `change`.
    fn listed_with(
        path: &str,
        options: ListOptions<'_>,
        change: impl FnOnce(&mut Crate),
    ) -> Listed {
        let mut krate: Crate = parsed().krate;
        change(&mut krate);
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let listed = list_module(
            &view,
            ItemRequest {
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
                follow: false,
                target: None,
            },
            options,
        );
        let response = match listed {
            Ok(response) => serde_json::to_value(response).expect("serializes"),
            Err(error) => return Listed::Err(error),
        };
        let items: Vec<serde_json::Value> = response["data"]["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| json!([item["path"], item["kind"], item["canonical"]]))
                    .collect()
            })
            .unwrap_or_default();
        Listed::Ok(json!(items), response["page"].clone())
    }

    fn options() -> ListOptions<'static> {
        ListOptions {
            recursive: false,
            depth: 1,
            kinds: &[],
            limit: 50,
            offset: 0,
            cursor_request: "c1.req",
            cursor_data: None,
        }
    }

    #[test]
    fn a_module_lists_its_items_and_reexports_in_order() {
        let (items, page) = listed("my_crate", options()).expect("listed");
        assert_eq!(
            items,
            json!([
                ["my_crate::E", "enum", null],
                ["my_crate::Mutex", "struct", null],
                ["my_crate::Thing", "trait", null],
                ["my_crate::sync", "module", null],
            ])
        );
        assert_eq!(page, json!({ "shown": 4, "total": 4, "has_more": false }));
    }

    #[test]
    fn another_crates_hidden_items_are_not_listed() {
        // `sync` and the re-export of `Thing` hidden, with `--document-hidden-items`.
        let hide = |krate: &mut Crate| {
            krate
                .index
                .values_mut()
                .filter(|item| {
                    item.name.as_deref() == Some("sync")
                        || matches!(&item.inner, ItemEnum::Use(reexport) if reexport.name == "Thing")
                })
                .for_each(|item| {
                    item.attrs
                        .push(rustdoc_types::Attribute::Other("#[doc(hidden)]".into()));
                });
        };
        let (items, _) = listed_with("my_crate", options(), hide).expect("listed");
        // `Mutex` is still public at the root, which is not hidden.
        assert_eq!(
            items,
            json!([["my_crate::E", "enum", null], [
                "my_crate::Mutex",
                "struct",
                null
            ],])
        );
        assert!(matches!(
            listed_with("my_crate::sync", options(), hide),
            Listed::Err(GetItemError::NotFound)
        ));
    }

    #[test]
    fn recursive_lists_submodules_after_their_module() {
        let (items, _) = listed("my_crate", ListOptions {
            recursive: true,
            depth: 3,
            ..options()
        })
        .expect("listed");
        assert_eq!(
            items[4],
            json!(["my_crate::sync::Mutex", "struct", "my_crate::Mutex"])
        );
        let (modules, _) = listed("my_crate", ListOptions {
            recursive: true,
            depth: 3,
            kinds: &["module"],
            ..options()
        })
        .expect("listed");
        assert_eq!(modules, json!([["my_crate::sync", "module", null]]));
    }

    #[test]
    fn pages_go_on_with_a_cursor_for_the_same_data() {
        let (items, page) = listed("my_crate", ListOptions {
            limit: 3,
            ..options()
        })
        .expect("listed");
        assert_eq!(items.as_array().map(Vec::len), Some(3));
        assert_eq!(
            page,
            json!({
                "shown": 3,
                "total": 4,
                "has_more": true,
                "next_cursor": "c1.req.abababababababab.3",
            })
        );
        let (rest, _) = listed("my_crate", ListOptions {
            offset: 3,
            cursor_data: Some("abababababababab"),
            ..options()
        })
        .expect("listed");
        assert_eq!(rest, json!([["my_crate::sync", "module", null]]));
        assert!(matches!(
            listed("my_crate", ListOptions {
                offset: 3,
                cursor_data: Some("0000000000000000"),
                ..options()
            }),
            Listed::Err(GetItemError::BadCursor(_))
        ));
        assert!(matches!(
            listed("my_crate", ListOptions {
                offset: 5,
                ..options()
            }),
            Listed::Err(GetItemError::BadCursor(_))
        ));
    }

    #[test]
    fn only_a_module_is_listed() {
        assert!(matches!(
            listed("my_crate::Mutex", options()),
            Listed::Err(GetItemError::NotAModule { kind: "struct" })
        ));
        assert!(matches!(
            listed("my_crate::Nothing", options()),
            Listed::Err(GetItemError::NotFound)
        ));
    }
}
