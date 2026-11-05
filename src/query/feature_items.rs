//! The items that each feature of a crate enables, from the conditions on their canonical paths
//! (requirement §5.9's features, decided in M3d, decisions 43, 44 and 55, §5 P12).
//!
//! An item counts for a feature when its condition, with those of the modules, the re-export and
//! the parent that its canonical path goes through, mentions it, under `not` too. It is an entry
//! of the feature when its own condition, or that of the re-export that names it, does; the
//! others only inherit it.

use std::collections::BTreeMap;

use super::{
    cfg::Cfg,
    get_item::{CarriedCfg, conditions, kind_of},
    search,
    view::CrateView,
};

/// An item whose condition mentions a feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureItem {
    /// Its canonical path.
    pub path: Box<str>,
    /// What it is, as `get_item` names it.
    pub kind: &'static str,
    /// The features that it needs, as `get_item` tells them.
    pub required_features: Option<Box<str>>,
    /// Its other conditions.
    pub other_cfg: Box<[Box<str>]>,
    /// Whether its own condition, or that of the re-export that names it, mentions the feature.
    pub entry: bool,
    /// Whether the feature is mentioned only under `not`: the item is there without it.
    pub negated: bool,
}

/// The items of the crate of `view` that each feature it mentions enables, sorted: the entries
/// first, then by how deep the path is and by the path (decided in M3d, decision 44).
#[must_use]
pub fn feature_items(view: &CrateView<'_>) -> BTreeMap<Box<str>, Vec<FeatureItem>> {
    let mut by_feature: BTreeMap<Box<str>, Vec<FeatureItem>> = BTreeMap::new();
    for (id, parent, path) in search::public_items(view.search_index()) {
        let Some(item) = view.item(id) else {
            continue;
        };
        let CarriedCfg {
            conjuncts, entry, ..
        } = conditions(view, view.index(), id, parent);
        let mentioned: Vec<(&str, bool)> = conjuncts.iter().flat_map(Cfg::features).collect();
        if mentioned.is_empty() {
            continue;
        }
        let in_entry: Vec<(&str, bool)> = entry.iter().flat_map(Cfg::features).collect();
        let (features, other): (Vec<&Cfg>, Vec<&Cfg>) =
            conjuncts.iter().partition(|cfg| cfg.is_features_only());
        let required_features = (!features.is_empty()).then(|| {
            Cfg::all(features.into_iter().cloned().collect())
                .features_text()
                .into()
        });
        let other_cfg: Box<[Box<str>]> = other.iter().map(|cfg| cfg.to_string().into()).collect();
        let kind = kind_of(item, parent.is_some());
        let mut told: Vec<&str> = Vec::new();
        for &(feature, _) in &mentioned {
            if told.contains(&feature) {
                continue;
            }
            told.push(feature);
            // Under `not` alone: no mention of it without.
            let negated = !mentioned.contains(&(feature, false));
            by_feature
                .entry(feature.into())
                .or_default()
                .push(FeatureItem {
                    path: path.into(),
                    kind,
                    required_features: required_features.clone(),
                    other_cfg: other_cfg.clone(),
                    entry: in_entry.iter().any(|&(name, _)| name == feature),
                    negated,
                });
        }
    }
    for items in by_feature.values_mut() {
        items.sort_by(|a, b| {
            (!a.entry, a.path.matches("::").count(), &a.path).cmp(&(
                !b.entry,
                b.path.matches("::").count(),
                &b.path,
            ))
        });
    }
    by_feature
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::rustdoc_json::ParsedCrate,
        query::{
            across::Indexed,
            get_item::tests::{parsed, with_cfgs},
            path_index::PathIndex,
            search::SearchIndex,
            view::tests::archived,
        },
    };

    /// `krate` archived with its index and its search index.
    fn archive(krate: rustdoc_types::Crate) -> rkyv::util::AlignedVec {
        let index = PathIndex::build(&krate);
        let indexed = Indexed {
            search: SearchIndex::build(&krate, &index),
            reverse: crate::query::reverse::ReverseIndex::build(&krate),
            index,
            parsed: ParsedCrate {
                krate,
                newer_than_supported: false,
            },
            source: "ab".repeat(32).into(),
        };
        rkyv::to_bytes::<rkyv::rancor::Error>(&indexed).expect("archived")
    }

    /// The items of each feature: an entry by its own condition or the re-export that names it,
    /// the others inheriting it from a parent; `any` counts for each feature, and a feature under
    /// `not` alone is marked (decided in M3d, decisions 43, 44 and 55).
    #[test]
    fn the_items_of_each_feature_are_found_entries_first() {
        let mut krate = parsed().krate;
        with_cfgs(&mut krate);
        let mut add = |id: u32, text: &str| {
            if let Some(item) = krate.index.get_mut(&rustdoc_types::Id(id)) {
                item.attrs
                    .push(rustdoc_types::Attribute::Other(text.to_owned()));
            }
        };
        // A variant without `legacy`, and one that mentions `a` under `not` but inherits it too.
        add(9, r#"#[doc(cfg(not(feature = "legacy")))]"#);
        add(10, r#"#[doc(cfg(not(feature = "a")))]"#);
        // The `impl` of `Mutex` adds `io` to its method, and has `sync`, which `Mutex` has.
        add(5, r#"#[doc(cfg(all(feature = "io", feature = "sync")))]"#);
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let by_feature = feature_items(&view);
        let told = |feature: &str| -> Vec<(String, bool, bool)> {
            by_feature
                .get(feature)
                .into_iter()
                .flatten()
                .map(|item| (item.path.to_string(), item.entry, item.negated))
                .collect()
        };
        // `Mutex` is re-exported under the condition, which names it; its method inherits it, but
        // `io` is its `impl`'s alone (decision 60).
        assert_eq!(told("sync"), [
            ("my_crate::Mutex".to_owned(), true, false),
            ("my_crate::Mutex::lock".to_owned(), false, false),
        ]);
        // `any(a, b)` of `E` counts for each, and its variants inherit it; `E::B` also mentions
        // `a` itself, under `not`, which makes it an entry.
        assert_eq!(told("a"), [
            ("my_crate::E".to_owned(), true, false),
            ("my_crate::E::B".to_owned(), true, false),
            ("my_crate::E::A".to_owned(), false, false),
        ]);
        assert_eq!(told("b"), [
            ("my_crate::E".to_owned(), true, false),
            ("my_crate::E::A".to_owned(), false, false),
            ("my_crate::E::B".to_owned(), false, false),
        ]);
        let mutex = &by_feature["sync"][0];
        assert_eq!(mutex.required_features.as_deref(), Some("sync"));
        assert_eq!(mutex.kind, "struct");
        let lock = &by_feature["sync"][1];
        // Its `impl`'s `doc(cfg)` is read before its trace of `#[cfg]`.
        assert_eq!(lock.required_features.as_deref(), Some("all(sync, io)"));
        assert!(lock.other_cfg.is_empty());
        assert_eq!(told("io"), [(
            "my_crate::Mutex::lock".to_owned(),
            true,
            false
        )]);
        assert_eq!(told("legacy"), [("my_crate::E::A".to_owned(), true, true)]);
    }
}
