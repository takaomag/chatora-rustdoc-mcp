//! How the items of std, alloc and core are shown, by std's paths.
//!
//! Those of std's prelude are shown by their names, and those that std re-exports by a glob at
//! core's paths (requirements R-OUT-2 and R-PR-2, decided in M4b, decisions 18, 30 and 31).
//!
//! std re-exports most of alloc and core under other paths (`alloc::vec::Vec` is
//! `std::vec::Vec`), so an item of theirs, as another crate's JSON names it by its definition
//! path, is looked up in its crate's archive for its canonical path there, and that path is shown
//! at std's path for it, as std re-exports it.

use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;
use rustdoc_types::{Id, ItemKind, ItemSummary};

use super::{
    across::Indexed,
    external::{ExternalPaths, NotCanonical, Reason, Shown},
    search::Reexports,
    view::CrateView,
};
use crate::data::archive::Mapped;

/// std's paths of the items of its prelude, shown by their names (decided in M4b, decision 18):
/// those that a crate names without a `use`. Not by their names alone, as `std::io::Result` and
/// `std::fmt::Error` share theirs with others.
const PRELUDE: &[&str] = &[
    "std::option::Option",
    "std::result::Result",
    "std::vec::Vec",
    "std::string::String",
    "std::boxed::Box",
    "std::borrow::ToOwned",
    "std::clone::Clone",
    "std::cmp::Eq",
    "std::cmp::Ord",
    "std::cmp::PartialEq",
    "std::cmp::PartialOrd",
    "std::convert::AsMut",
    "std::convert::AsRef",
    "std::convert::From",
    "std::convert::Into",
    "std::convert::TryFrom",
    "std::convert::TryInto",
    "std::default::Default",
    "std::future::Future",
    "std::future::IntoFuture",
    "std::iter::DoubleEndedIterator",
    "std::iter::ExactSizeIterator",
    "std::iter::Extend",
    "std::iter::FromIterator",
    "std::iter::IntoIterator",
    "std::iter::Iterator",
    "std::marker::Copy",
    "std::marker::Send",
    "std::marker::Sized",
    "std::marker::Sync",
    "std::marker::Unpin",
    "std::ops::AsyncFn",
    "std::ops::AsyncFnMut",
    "std::ops::AsyncFnOnce",
    "std::ops::Drop",
    "std::ops::Fn",
    "std::ops::FnMut",
    "std::ops::FnOnce",
    "std::string::ToString",
];

/// The definition paths of the items of std's prelude, as a nightly of 2026 gives them, which are
/// shown by their names with std's archives or without them (decided in M4b, decision 35). They
/// move with the toolchain: one that moved is looked up in std's archives.
const PRELUDE_DEFINED: &[&str] = &[
    "core::option::Option",
    "core::result::Result",
    "alloc::vec::Vec",
    "alloc::string::String",
    "alloc::boxed::Box",
    "alloc::borrow::ToOwned",
    "alloc::string::ToString",
    "core::clone::Clone",
    "core::cmp::Eq",
    "core::cmp::Ord",
    "core::cmp::PartialEq",
    "core::cmp::PartialOrd",
    "core::convert::AsMut",
    "core::convert::AsRef",
    "core::convert::From",
    "core::convert::Into",
    "core::convert::TryFrom",
    "core::convert::TryInto",
    "core::default::Default",
    "core::future::future::Future",
    "core::future::into_future::IntoFuture",
    "core::iter::traits::double_ended::DoubleEndedIterator",
    "core::iter::traits::exact_size::ExactSizeIterator",
    "core::iter::traits::collect::Extend",
    "core::iter::traits::collect::FromIterator",
    "core::iter::traits::collect::IntoIterator",
    "core::iter::traits::iterator::Iterator",
    "core::marker::Copy",
    "core::marker::Send",
    "core::marker::Sized",
    "core::marker::Sync",
    "core::marker::Unpin",
    "core::ops::async_function::AsyncFn",
    "core::ops::async_function::AsyncFnMut",
    "core::ops::async_function::AsyncFnOnce",
    "core::ops::drop::Drop",
    "core::ops::function::Fn",
    "core::ops::function::FnMut",
    "core::ops::function::FnOnce",
];

/// The archives of std, alloc and core of one toolchain, and std's paths for what it re-exports
/// of the others.
pub struct StdPaths {
    /// std's, alloc's and core's archives, by their names; none when the cache has no std.
    crates: Vec<(&'static str, Arc<Mapped<Indexed>>)>,
    /// The toolchain's release, the version of the crates of std.
    version: Option<Box<str>>,
    /// std's paths for the items of alloc and core that it re-exports.
    reexports: Reexports,
    /// What was found of each item, by its crate and definition path.
    found: Mutex<HashMap<Found, Shown>>,
}

/// An item of std's crates looked up: its crate, its definition path and its kind.
type Found = (Box<str>, Box<str>, ItemKind);

impl std::fmt::Debug for StdPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdPaths")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl StdPaths {
    /// The paths of the archives of std, alloc and core, in that order, of the toolchain of
    /// `version`.
    #[must_use]
    pub fn new([std, alloc, core]: [Arc<Mapped<Indexed>>; 3], version: Option<Box<str>>) -> Self {
        let reexports = {
            let (std, alloc, core) = (
                CrateView::new(std.get()),
                CrateView::new(alloc.get()),
                CrateView::new(core.get()),
            );
            Reexports::new(&std, &[("alloc", &alloc), ("core", &core)])
        };
        Self {
            crates: vec![("std", std), ("alloc", alloc), ("core", core)],
            version,
            reexports,
            found: Mutex::new(HashMap::new()),
        }
    }

    /// The paths of a std that the cache does not have: each item of std's crates is told as not
    /// cached, so that looking one up has std fetched (decided in M4b, decision 30).
    #[must_use]
    pub fn not_cached() -> Self {
        Self {
            crates: Vec::new(),
            version: None,
            reexports: Reexports::default(),
            found: Mutex::new(HashMap::new()),
        }
    }

    /// How the item at `definition` in `krate`, one of std's, is shown: the one of `kind` there,
    /// as a trait and its derive macro share their definition path.
    fn look_up(&self, krate: &'static str, definition: &str, kind: ItemKind) -> Shown {
        let told = |reason| {
            Shown::NotCanonical(NotCanonical {
                ty: Box::default(),
                krate: krate.into(),
                version: self.version.clone(),
                lookup: definition.into(),
                reason,
            })
        };
        // The prelude's by their names, with std or without (decided in M4b, decision 35).
        if PRELUDE_DEFINED.contains(&definition) {
            let name = definition.rsplit("::").next().unwrap_or_default();
            return Shown::Path(name.into());
        }
        let Some((_, mapped)) = self.crates.iter().find(|(name, _)| *name == krate) else {
            return told(Reason::NotCached);
        };
        let there = CrateView::new(mapped.get());
        let index = there.index();
        let of_kind: Vec<Id> = index
            .at_definition_path(definition)
            .iter()
            .copied()
            .filter(|&id| there.summary(id).is_none_or(|summary| summary.kind == kind))
            .collect();
        let [id] = of_kind.as_slice() else {
            return told(Reason::NotFound);
        };
        let id = *id;
        let Some(canonical) = index
            .canonical_path(id)
            .filter(|_| !index.is_hidden(id))
            .map(|canonical| canonical.joined())
        else {
            return told(Reason::Unnameable);
        };
        // At std's path for it, else at its crate's: std re-exports it by a glob, which a lookup
        // does not follow (decided in M4b, decision 31).
        let path = if krate == "std" {
            canonical
        } else {
            self.reexports
                .shown_at(&canonical, Some(definition))
                .unwrap_or(canonical)
        };
        if PRELUDE.contains(&&*path) {
            let name = path.rsplit("::").next().unwrap_or_default();
            return Shown::Path(name.into());
        }
        Shown::Path(path)
    }
}

impl ExternalPaths for StdPaths {
    fn shown(&self, view: &CrateView<'_>, _id: Id, summary: &ItemSummary) -> Option<Shown> {
        // Of the crate of the view itself only when it is alloc or core, shown through std.
        let named = |name: Option<&str>| {
            ["std", "alloc", "core"]
                .into_iter()
                .find(|krate| Some(*krate) == name)
        };
        let krate = if summary.crate_id == 0 {
            named(view.item(view.root()).and_then(|root| root.name.as_deref()))
                .filter(|krate| *krate != "std")?
        } else {
            named(Some(view.external_crate(summary.crate_id)?.name.as_str()))?
        };
        let key = (
            Box::from(krate),
            summary.path.join("::").into_boxed_str(),
            summary.kind,
        );
        if let Some(shown) = self.found.lock().get(&key) {
            return Some(shown.clone());
        }
        let shown = self.look_up(krate, &key.1, summary.kind);
        self.found.lock().insert(key, shown.clone());
        Some(shown)
    }
}

#[cfg(test)]
mod tests {
    use rustdoc_types::{
        Crate,
        ExternalCrate,
        Generics,
        Item,
        ItemEnum,
        Module,
        Struct,
        StructKind,
        Target,
        Trait,
        Use,
        Visibility,
    };

    use super::*;
    use crate::data::rustdoc_json::ParsedCrate;

    fn item(id: u32, name: &str, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: (!name.is_empty()).then(|| name.to_owned()),
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

    fn module(id: u32, name: &str, items: &[u32], is_crate: bool) -> Item {
        item(
            id,
            name,
            ItemEnum::Module(Module {
                is_crate,
                items: items.iter().map(|&id| Id(id)).collect(),
                is_stripped: false,
            }),
        )
    }

    fn generics() -> Generics {
        Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        }
    }

    fn summary(crate_id: u32, path: &[&str], kind: ItemKind) -> ItemSummary {
        ItemSummary {
            crate_id,
            path: path.iter().map(|&segment| segment.to_owned()).collect(),
            kind,
        }
    }

    fn krate(items: Vec<Item>, paths: Vec<(u32, ItemSummary)>, external: &[(u32, &str)]) -> Crate {
        Crate {
            root: Id(0),
            crate_version: None,
            includes_private: false,
            index: items.into_iter().map(|item| (item.id, item)).collect(),
            paths: paths
                .into_iter()
                .map(|(id, summary)| (Id(id), summary))
                .collect(),
            external_crates: external
                .iter()
                .map(|&(id, name)| {
                    (id, ExternalCrate {
                        name: name.to_owned(),
                        html_root_url: None,
                        path: name.into(),
                    })
                })
                .collect(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".to_owned(),
                target_features: Vec::new(),
            },
            format_version: rustdoc_types::FORMAT_VERSION,
        }
    }

    fn mapped(krate: Crate) -> Arc<Mapped<Indexed>> {
        let indexed = Indexed::new(ParsedCrate {
            krate,
            newer_than_supported: false,
        });
        Arc::new(Mapped::in_memory(&indexed).expect("archived"))
    }

    fn unit(id: u32, name: &str) -> Item {
        item(
            id,
            name,
            ItemEnum::Struct(Struct {
                kind: StructKind::Unit,
                generics: generics(),
                impls: Vec::new(),
            }),
        )
    }

    fn a_trait(id: u32, name: &str) -> Item {
        item(
            id,
            name,
            ItemEnum::Trait(Trait {
                is_auto: false,
                is_unsafe: false,
                is_dyn_compatible: true,
                items: Vec::new(),
                generics: generics(),
                bounds: Vec::new(),
                implementations: Vec::new(),
            }),
        )
    }

    fn hidden(item: Item) -> Item {
        Item {
            attrs: vec![rustdoc_types::Attribute::Other("#[doc(hidden)]".to_owned())],
            ..item
        }
    }

    fn reexport(id: u32, source: &str, target: u32, is_glob: bool) -> Item {
        let name = source.rsplit("::").next().unwrap_or_default();
        item(
            id,
            "",
            ItemEnum::Use(Use {
                source: source.to_owned(),
                name: name.to_owned(),
                id: Some(Id(target)),
                is_glob,
            }),
        )
    }

    /// core with `option::Option`, `fmt::Formatter`, `hash::Hash`, the trait `clone::Clone` with
    /// its derive macro at the same definition path, the hidden `fmt::Hidden`, and `Secret` in a
    /// private module.
    fn core() -> Crate {
        let derive = item(
            9,
            "Clone",
            ItemEnum::ProcMacro(rustdoc_types::ProcMacro {
                kind: rustdoc_types::MacroKind::Derive,
                helpers: Vec::new(),
            }),
        );
        let private = Item {
            visibility: Visibility::Default,
            ..module(14, "private", &[15], false)
        };
        krate(
            vec![
                module(0, "core", &[1, 3, 5, 7, 14], true),
                module(1, "option", &[2], false),
                unit(2, "Option"),
                module(3, "fmt", &[4, 13], false),
                unit(4, "Formatter"),
                hidden(unit(13, "Hidden")),
                module(5, "hash", &[6], false),
                a_trait(6, "Hash"),
                module(7, "clone", &[8, 9], false),
                a_trait(8, "Clone"),
                derive,
                private,
                unit(15, "Secret"),
            ],
            vec![
                (
                    2,
                    summary(0, &["core", "option", "Option"], ItemKind::Struct),
                ),
                (
                    4,
                    summary(0, &["core", "fmt", "Formatter"], ItemKind::Struct),
                ),
                (6, summary(0, &["core", "hash", "Hash"], ItemKind::Trait)),
                (8, summary(0, &["core", "clone", "Clone"], ItemKind::Trait)),
                (
                    9,
                    summary(0, &["core", "clone", "Clone"], ItemKind::ProcDerive),
                ),
                (13, summary(0, &["core", "fmt", "Hidden"], ItemKind::Struct)),
                (
                    15,
                    summary(0, &["core", "private", "Secret"], ItemKind::Struct),
                ),
            ],
            &[],
        )
    }

    /// alloc with `vec::Vec`.
    fn alloc() -> Crate {
        krate(
            vec![
                module(0, "alloc", &[1], true),
                module(1, "vec", &[2], false),
                unit(2, "Vec"),
            ],
            vec![(2, summary(0, &["alloc", "vec", "Vec"], ItemKind::Struct))],
            &[],
        )
    }

    /// std, which re-exports core's `fmt`, `option` and `clone` and alloc's `vec`, core's `hash` by
    /// a glob, and `core::fmt::Formatter` in a hidden module `aaa`, whose path would win by its
    /// letters; with an `io::Error` of its own.
    fn std() -> Crate {
        krate(
            vec![
                module(0, "std", &[1, 2, 3, 4, 5, 7, 8], true),
                reexport(1, "core::fmt", 10, false),
                reexport(2, "core::option", 11, false),
                reexport(3, "core::clone", 12, false),
                reexport(4, "core::hash", 14, true),
                hidden(module(5, "aaa", &[6], false)),
                reexport(6, "core::fmt::Formatter", 16, false),
                reexport(7, "alloc::vec", 20, false),
                module(8, "io", &[9], false),
                unit(9, "Error"),
            ],
            vec![
                (9, summary(0, &["std", "io", "Error"], ItemKind::Struct)),
                (10, summary(1, &["core", "fmt"], ItemKind::Module)),
                (11, summary(1, &["core", "option"], ItemKind::Module)),
                (12, summary(1, &["core", "clone"], ItemKind::Module)),
                (14, summary(1, &["core", "hash"], ItemKind::Module)),
                (
                    16,
                    summary(1, &["core", "fmt", "Formatter"], ItemKind::Struct),
                ),
                (20, summary(2, &["alloc", "vec"], ItemKind::Module)),
            ],
            &[(1, "core"), (2, "alloc")],
        )
    }

    fn paths() -> StdPaths {
        StdPaths::new(
            [mapped(std()), mapped(alloc()), mapped(core())],
            Some("1.101.0".into()),
        )
    }

    /// The items of std's crates are shown at std's paths, but under a hidden module; those of its
    /// prelude by their names, a trait apart from its derive macro; those that std re-exports by a
    /// glob or not at all at core's; those that core does not have as not found, and those without
    /// a public path that is not hidden as unnameable; the items of other crates are not looked at
    /// (decided in M4b, decisions 18, 21 and 31).
    #[test]
    fn the_items_of_std_are_shown_at_its_paths() {
        let paths = paths();
        let of_core = |id, path: &[&str], kind| (id, summary(1, path, kind));
        let user = krate(
            vec![module(0, "user", &[], true)],
            vec![
                of_core(50, &["core", "option", "Option"], ItemKind::Struct),
                of_core(51, &["core", "fmt", "Formatter"], ItemKind::Struct),
                of_core(52, &["core", "hash", "Hash"], ItemKind::Trait),
                of_core(53, &["core", "nope", "Nope"], ItemKind::Struct),
                (54, summary(4, &["serde", "Serialize"], ItemKind::Trait)),
                of_core(55, &["core", "clone", "Clone"], ItemKind::Trait),
                (56, summary(2, &["alloc", "vec", "Vec"], ItemKind::Struct)),
                of_core(57, &["core", "fmt", "Hidden"], ItemKind::Struct),
                of_core(58, &["core", "private", "Secret"], ItemKind::Struct),
                (59, summary(3, &["std", "io", "Error"], ItemKind::Struct)),
            ],
            &[(1, "core"), (2, "alloc"), (3, "std"), (4, "serde")],
        );
        let user = mapped(user);
        let view = CrateView::new(user.get());
        let shown = |id: u32| {
            let summary = view.summary(Id(id)).expect("a summary").clone();
            paths.shown(&view, Id(id), &summary)
        };
        let path = |path: &str| Some(Shown::Path(path.into()));
        let reason = |id| match shown(id) {
            Some(Shown::NotCanonical(told)) => Some(told.reason),
            Some(Shown::Path(_)) | None => None,
        };
        assert_eq!(shown(50), path("Option"));
        assert_eq!(shown(51), path("std::fmt::Formatter"));
        assert_eq!(shown(52), path("core::hash::Hash"));
        assert_eq!(reason(53), Some(Reason::NotFound));
        assert_eq!(shown(54), None);
        assert_eq!(shown(55), path("Clone"));
        assert_eq!(shown(56), path("Vec"));
        assert_eq!(reason(57), Some(Reason::Unnameable));
        assert_eq!(reason(58), Some(Reason::Unnameable));
        assert_eq!(shown(59), path("std::io::Error"));
        // Without std, its prelude by the names, the rest not cached (decided in M4b, decision
        // 35).
        let none = StdPaths::not_cached();
        let without = |id: u32| {
            let summary = view.summary(Id(id)).expect("a summary").clone();
            none.shown(&view, Id(id), &summary)
        };
        assert_eq!(without(50), path("Option"));
        assert_eq!(without(55), path("Clone"));
        assert!(
            matches!(
                without(51),
                Some(Shown::NotCanonical(NotCanonical {
                    reason: Reason::NotCached,
                    ..
                }))
            ),
            "{:?}",
            without(51)
        );
        // Found once, then remembered.
        let found = paths.found.lock().len();
        assert_eq!(shown(51), path("std::fmt::Formatter"));
        assert_eq!(paths.found.lock().len(), found);
        // From core itself, its own items are shown through std too; from std, its own as they
        // are.
        let core = mapped(core());
        let view = CrateView::new(core.get());
        let summary = view.summary(Id(4)).expect("a summary").clone();
        assert_eq!(
            paths.shown(&view, Id(4), &summary),
            path("std::fmt::Formatter")
        );
        let std = mapped(std());
        let view = CrateView::new(std.get());
        let summary = view.summary(Id(9)).expect("a summary").clone();
        assert_eq!(paths.shown(&view, Id(9), &summary), None);
    }
}
