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
use rustdoc_types::{Id, ItemSummary};

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
    "std::ops::Drop",
    "std::ops::Fn",
    "std::ops::FnMut",
    "std::ops::FnOnce",
    "std::string::ToString",
];

/// The archives of std, alloc and core of one toolchain, and std's paths for what it re-exports
/// of the others.
pub struct StdPaths {
    /// std's, alloc's and core's archives, by their names.
    crates: [(&'static str, Arc<Mapped<Indexed>>); 3],
    /// The toolchain's release, the version of the crates of std.
    version: Option<Box<str>>,
    /// std's paths for the items of alloc and core that it re-exports.
    reexports: Reexports,
    /// What was found of each item, by its crate and definition path.
    found: Mutex<HashMap<Found, Shown>>,
}

/// An item of std's crates looked up: its crate and its definition path.
type Found = (Box<str>, Box<str>);

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
            crates: [("std", std), ("alloc", alloc), ("core", core)],
            version,
            reexports,
            found: Mutex::new(HashMap::new()),
        }
    }

    /// How the item at `definition` in `krate`, one of std's, is shown.
    fn look_up(&self, krate: &'static str, definition: &str) -> Shown {
        let told = |reason| {
            Shown::NotCanonical(NotCanonical {
                ty: Box::default(),
                krate: krate.into(),
                version: self.version.clone(),
                lookup: definition.into(),
                reason,
            })
        };
        let Some((_, mapped)) = self.crates.iter().find(|(name, _)| *name == krate) else {
            return told(Reason::NotCached);
        };
        let there = CrateView::new(mapped.get());
        let index = there.index();
        let Some(id) = index.at_definition_path(definition) else {
            return told(Reason::NotFound);
        };
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
        let krate = if summary.crate_id == 0 {
            view.item(view.root())
                .and_then(|root| root.name.clone())
                .filter(|name| name != "std")?
        } else {
            view.external_crate(summary.crate_id)?.name.clone()
        };
        let krate: &'static str = ["std", "alloc", "core"]
            .into_iter()
            .find(|name| *name == krate)?;
        let definition: Box<str> = summary.path.join("::").into();
        let key = (Box::from(krate), definition);
        if let Some(shown) = self.found.lock().get(&key) {
            return Some(shown.clone());
        }
        let shown = self.look_up(krate, &key.1);
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
        ItemKind,
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

    /// core with `option::Option`, `fmt::Formatter` and `hash::Hash`.
    fn core() -> Crate {
        let unit = || {
            ItemEnum::Struct(Struct {
                kind: StructKind::Unit,
                generics: generics(),
                impls: Vec::new(),
            })
        };
        krate(
            vec![
                module(0, "core", &[1, 3, 5], true),
                module(1, "option", &[2], false),
                item(2, "Option", unit()),
                module(3, "fmt", &[4], false),
                item(4, "Formatter", unit()),
                module(5, "hash", &[6], false),
                item(
                    6,
                    "Hash",
                    ItemEnum::Trait(Trait {
                        is_auto: false,
                        is_unsafe: false,
                        is_dyn_compatible: true,
                        items: Vec::new(),
                        generics: generics(),
                        bounds: Vec::new(),
                        implementations: Vec::new(),
                    }),
                ),
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
            ],
            &[],
        )
    }

    /// std, which re-exports core's `fmt` and `option`, not `hash` (by a glob, in the real one).
    fn std() -> Crate {
        let reexport = |id, name: &str, target| {
            item(
                id,
                "",
                ItemEnum::Use(Use {
                    source: format!("core::{name}"),
                    name: name.to_owned(),
                    id: Some(Id(target)),
                    is_glob: false,
                }),
            )
        };
        krate(
            vec![
                module(0, "std", &[1, 2], true),
                reexport(1, "fmt", 10),
                reexport(2, "option", 11),
            ],
            vec![
                (10, summary(1, &["core", "fmt"], ItemKind::Module)),
                (11, summary(1, &["core", "option"], ItemKind::Module)),
            ],
            &[(1, "core")],
        )
    }

    fn paths() -> StdPaths {
        let alloc = krate(vec![module(0, "alloc", &[], true)], Vec::new(), &[]);
        StdPaths::new(
            [mapped(std()), mapped(alloc), mapped(core())],
            Some("1.101.0".into()),
        )
    }

    /// The items of std's crates are shown at std's paths, those of its prelude by their names,
    /// those that std does not re-export by name at core's, and those that core does not have as
    /// not found; the items of other crates are not looked at (decided in M4b, decisions 18 and
    /// 31).
    #[test]
    fn the_items_of_std_are_shown_at_its_paths() {
        let paths = paths();
        let user = krate(
            vec![module(0, "user", &[], true)],
            vec![
                (
                    50,
                    summary(1, &["core", "option", "Option"], ItemKind::Struct),
                ),
                (
                    51,
                    summary(1, &["core", "fmt", "Formatter"], ItemKind::Struct),
                ),
                (52, summary(1, &["core", "hash", "Hash"], ItemKind::Trait)),
                (53, summary(1, &["core", "nope", "Nope"], ItemKind::Struct)),
                (54, summary(2, &["serde", "Serialize"], ItemKind::Trait)),
            ],
            &[(1, "core"), (2, "serde")],
        );
        let user = mapped(user);
        let view = CrateView::new(user.get());
        let shown = |id: u32| {
            let summary = view.summary(Id(id)).expect("a summary").clone();
            paths.shown(&view, Id(id), &summary)
        };
        assert_eq!(shown(50), Some(Shown::Path("Option".into())));
        assert_eq!(shown(51), Some(Shown::Path("std::fmt::Formatter".into())));
        assert_eq!(shown(52), Some(Shown::Path("core::hash::Hash".into())));
        assert!(
            matches!(
                shown(53),
                Some(Shown::NotCanonical(NotCanonical {
                    reason: Reason::NotFound,
                    ref lookup,
                    ..
                })) if &**lookup == "core::nope::Nope"
            ),
            "{:?}",
            shown(53)
        );
        assert_eq!(shown(54), None);
        // From core itself, its own items are shown through std too.
        let core = mapped(core());
        let view = CrateView::new(core.get());
        let summary = view.summary(Id(4)).expect("a summary").clone();
        assert_eq!(
            paths.shown(&view, Id(4), &summary),
            Some(Shown::Path("std::fmt::Formatter".into()))
        );
    }
}
