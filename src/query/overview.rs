//! The overview of a crate: its root documentation, how many public items of each kind it has,
//! its top-level modules and items, and its features (§6.3 `crate_overview`, §3 Q3).

use std::{borrow::Cow, collections::BTreeMap};

use rustdoc_types::{Id, ItemEnum};
use serde::Serialize;

use super::{
    docs,
    get_item::{
        Detail,
        GetItemError,
        ItemData,
        ItemRequest,
        MAX_RESPONSE_CHARS,
        Notes,
        Origin,
        Provenance,
        Query,
        Section,
        first_paragraph,
        kind_of,
    },
    path_index::Resolution,
    view::CrateView,
};

/// The most top-level items other than modules in an overview; `list_module` lists them all.
const MAX_ITEMS: usize = 50;

/// The result of `crate_overview` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct OverviewResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// The overview.
    pub data: Overview<'a>,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// The overview of a crate.
#[derive(Debug, Serialize)]
pub struct Overview<'a> {
    /// The crate root, as code writes it.
    pub path: Box<str>,
    /// The first paragraph of the root's documentation, or with `detail=full` what comes before
    /// its first heading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<Cow<'a, str>>,
    /// The documentation goes on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub docs_truncated: bool,
    /// The sections of the root's documentation: headings and lines, and with `detail=full` their
    /// text.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub sections: Box<[Section]>,
    /// How many items with a public path the crate has, of each kind; a workspace member's, all.
    pub counts: BTreeMap<&'static str, usize>,
    /// The modules at the root, each with the first sentence of its documentation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ItemData<'a>>,
    /// The other items at the root, at most 50, by path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ItemData<'a>>,
    /// How many items other than modules the root has.
    pub items_total: usize,
    /// The features, when the crate's `.crate` was read (requirement R-FT-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<FeatureOverview<'a>>,
    /// Something was cut to keep the response within its budget (requirement R-OUT-3).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// The features of a crate.
#[derive(Debug, Serialize)]
pub struct FeatureOverview<'a> {
    /// Every feature, with those that optional dependencies make.
    pub all: &'a [Box<str>],
    /// Those that `default` enables.
    pub default: &'a [Box<str>],
    /// Those that docs.rs did not build the documentation with.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub not_in_docs: &'a [Box<str>],
    /// Those that the project's normal build enables, when the project decided the version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_in_project: Option<Vec<&'a str>>,
}

/// How many items the crate of `view` has, of each kind: with a public path, but the hidden ones;
/// of a workspace member, `member`, all that have a definition path (decided in M2's D6).
fn counts(view: &CrateView<'_>, member: bool) -> BTreeMap<&'static str, usize> {
    let (index, root) = (view.index(), view.root());
    let counted: Vec<Id> = if member {
        index.defined_items().collect()
    } else {
        index
            .public_items()
            .filter(|&id| !index.is_hidden(id))
            .collect()
    };
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for id in counted {
        if id == root {
            continue;
        }
        // Of a member's items, those that a module has: as many as with a public path.
        if let Some(item) = view.item(id)
            && !(member
                && matches!(
                    item.inner,
                    ItemEnum::Variant(_) | ItemEnum::StructField(_) | ItemEnum::Impl(_)
                ))
        {
            let count = counts.entry(kind_of(item, false)).or_default();
            *count = count.saturating_add(1);
        }
    }
    counts
}

/// Adds the item `id` at the root, at `child` if it is listed there, to the modules or the other
/// items.
fn add<'a>(
    (view, query): (&'a CrateView<'a>, &mut Query<'a>),
    id: Id,
    child: Option<&str>,
    (modules, items): (&mut Vec<ItemData<'a>>, &mut Vec<ItemData<'a>>),
) {
    let Some(mut data) = query.item_data(id, None).map(ItemData::listed) else {
        return;
    };
    if let Some(child) = child
        && *data.path != *child
    {
        data.canonical = Some(std::mem::replace(&mut data.path, child.into()));
    }
    if data.kind == "module" {
        data.docs = view
            .item(id)
            .and_then(|item| item.docs.as_deref())
            .map(first_sentence);
        modules.push(data);
    } else {
        items.push(data);
    }
}

/// The overview of the crate of `view`, whose root is at `request.path`.
///
/// # Errors
///
/// Returns an error when the crate has no root module, which a rustdoc JSON always has.
pub fn crate_overview<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
) -> Result<OverviewResponse<'a>, GetItemError> {
    let index = view.index();
    let root = view.root();
    let Some(root_item) = view.item(root) else {
        return Err(GetItemError::NotFound);
    };
    let mut query = Query::new(view, ItemRequest {
        detail: Detail::Brief,
        ..request
    });
    let provenance = query.provenance();
    let path: Box<str> = root_item.name.as_deref().unwrap_or("crate").into();
    let member = query.is_member();
    let counts = counts(view, member);
    let (mut modules, mut items) = (Vec::new(), Vec::new());
    for child in index.children(&path) {
        let ids = match index.at_public_path(&child) {
            Some(Resolution::Item(id)) => vec![id],
            Some(Resolution::Ambiguous(ids)) => ids.into_vec(),
            Some(Resolution::ExternalReexport(reexport)) => {
                if !member && index.is_hidden_reexport(&child) {
                    continue;
                }
                let kind = query.external_kind(reexport.target);
                let data = ItemData {
                    path: child.clone(),
                    kind,
                    hidden: query.hidden_reexport(&child),
                    reexport_of: query.reexport_target(&reexport),
                    ..ItemData::default()
                };
                if kind == "module" {
                    modules.push(data);
                } else {
                    items.push(data);
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
        for id in ids.into_iter().filter(|&id| member || !index.is_hidden(id)) {
            add(
                (view, &mut query),
                id,
                Some(&child),
                (&mut modules, &mut items),
            );
        }
    }
    // The root's items without a public path, in a workspace member.
    if member && let Some(ItemEnum::Module(contents)) = view.item(root).map(|item| &item.inner) {
        for &id in &contents.items {
            let unlisted = view.item(id).is_some_and(|item| {
                !matches!(item.inner, ItemEnum::Impl(_) | ItemEnum::Use(_))
                    && index.canonical_path(id).is_none()
            });
            if unlisted {
                add((view, &mut query), id, None, (&mut modules, &mut items));
            }
        }
    }
    let items_total = items.len();
    if items_total > MAX_ITEMS {
        items.truncate(MAX_ITEMS);
        query.hint(format!(
            "call list_module with path {path} for all {items_total} items at the root"
        ));
    }
    let features = features(request);
    let (docs, docs_truncated, sections) = root_docs(root_item.docs.as_deref(), request.detail);
    let Notes { warnings, hints } = query.finish();
    let mut response = OverviewResponse {
        provenance,
        warnings,
        data: Overview {
            path,
            docs,
            docs_truncated,
            sections,
            counts,
            modules,
            items,
            items_total,
            features,
            truncated: false,
        },
        hints,
    };
    fit(&mut response);
    Ok(response)
}

/// The features from how docs.rs built the crate, and the project's build of it.
fn features(request: ItemRequest<'_>) -> Option<FeatureOverview<'_>> {
    let Origin::DocsRs {
        build: Some(build), ..
    } = request.origin
    else {
        return None;
    };
    let enabled_in_project = request.project_features.map(|project| {
        build
            .declared
            .iter()
            .filter(|feature| {
                project
                    .normal
                    .is_some_and(|enabled| enabled.contains(*feature))
            })
            .map(|feature| &**feature)
            .collect()
    });
    Some(FeatureOverview {
        all: &build.declared,
        default: &build.default,
        not_in_docs: &build.missing,
        enabled_in_project,
    })
}

/// The root's documentation as `detail` shows it: the first paragraph, with `normal` the
/// headings of the sections, and with `full` all of it before the first heading and the text of
/// each section. When it starts with a heading (`# Serde`), the first paragraph is the first
/// section's.
fn root_docs(docs: Option<&str>, detail: Detail) -> (Option<Cow<'_, str>>, bool, Box<[Section]>) {
    let Some(docs) = docs else {
        return (None, false, Box::default());
    };
    let split = docs::split(docs);
    let full = detail == Detail::Full;
    let opening = if split.intro.is_empty() {
        split
            .sections
            .first()
            .map_or("", |section| section.text.as_str())
    } else {
        split.intro.as_str()
    };
    let (first, more) = first_paragraph(opening);
    let (text, truncated) = if full && !split.intro.is_empty() {
        (split.intro.as_str(), false)
    } else {
        (first, more || !split.sections.is_empty())
    };
    let sections = if detail == Detail::Brief {
        Box::default()
    } else {
        split
            .sections
            .iter()
            .map(|section| Section {
                heading: section.heading.as_str().into(),
                lines: section.text.lines().count(),
                text: full.then(|| section.text.as_str().into()),
            })
            .collect()
    };
    let docs = (!text.is_empty()).then(|| Cow::Owned(text.to_owned()));
    (docs, truncated && !full, sections)
}

/// The first sentence of `docs`: up to the first `.` that ends a sentence, in its first
/// paragraph.
fn first_sentence(docs: &str) -> Cow<'_, str> {
    let (paragraph, _) = first_paragraph(docs);
    let end = paragraph
        .char_indices()
        .find(|&(at, c)| {
            c == '.'
                && paragraph
                    .get(at.saturating_add(1)..)
                    .is_none_or(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        })
        .map_or(paragraph.len(), |(at, _)| at.saturating_add(1));
    Cow::Borrowed(paragraph.get(..end).unwrap_or(paragraph))
}

/// Cuts `response` to its budget (requirement R-OUT-3): the text of the sections, the items, the
/// docs of the modules, then the root's documentation.
fn fit(response: &mut OverviewResponse<'_>) {
    let fits = |response: &OverviewResponse<'_>| {
        serde_json::to_string(response)
            .map_or(true, |json| json.chars().count() <= MAX_RESPONSE_CHARS)
    };
    if fits(response) {
        return;
    }
    let data = &mut response.data;
    data.truncated = true;
    for section in &mut data.sections {
        section.text = None;
    }
    while !fits(response) && !response.data.items.is_empty() {
        let keep = response.data.items.len().div_euclid(2);
        response.data.items.truncate(keep);
    }
    if fits(response) {
        return;
    }
    for module in &mut response.data.modules {
        module.docs = None;
    }
    if !fits(response)
        && let Some(docs) = &response.data.docs
    {
        let (first, _) = first_paragraph(docs);
        let first = first.to_owned();
        response.data.docs = Some(Cow::Owned(first));
        response.data.docs_truncated = true;
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
            get_item::{ResolvedFrom, tests::parsed},
            view::tests::{archive, archived},
        },
    };

    #[test]
    fn the_overview_has_counts_modules_and_items() {
        let bytes = archive(parsed().krate);
        let view = CrateView::new(archived(&bytes));
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let response = crate_overview(&view, ItemRequest {
            name: &name,
            version: &version,
            path: "my_crate",
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
        .expect("an overview");
        let data = serde_json::to_value(response).expect("serializes")["data"].clone();
        assert_eq!(data["path"], "my_crate");
        assert_eq!(
            data["counts"],
            json!({ "enum": 1, "module": 1, "struct": 1 })
        );
        assert_eq!(data["modules"][0]["path"], "my_crate::sync");
        let items: Vec<&serde_json::Value> = data["items"]
            .as_array()
            .map(|items| items.iter().map(|item| &item["path"]).collect())
            .unwrap_or_default();
        assert_eq!(items, ["my_crate::E", "my_crate::Mutex", "my_crate::Thing"]);
        assert_eq!(data["items_total"], 3);
    }

    #[test]
    fn the_first_sentence_ends_at_a_full_stop() {
        assert_eq!(
            first_sentence("Async I/O. More here.\n\nNext."),
            "Async I/O."
        );
        assert_eq!(first_sentence("Version 1.2 of it"), "Version 1.2 of it");
        assert_eq!(first_sentence("No stop"), "No stop");
    }
}
