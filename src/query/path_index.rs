//! The public path index of one crate (requirements R-PR-1 to R-PR-3).
//!
//! The `paths` of rustdoc JSON hold definition paths, which can go through private modules and so
//! cannot be `use`d (`tokio::sync::mutex::Mutex`). This index walks from the crate root through
//! `Module.items` and `Use` items, records every public path of every item, and picks a canonical
//! one.
//!
//! M0a covers the items of the crate itself. Re-exports and globs from other crates are recorded
//! but not followed, and re-exports of primitive types are skipped (R-PR-3 and R-PR-4 in M1).

use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use rustdoc_types::{Crate, Id, Item, ItemEnum, Use, Visibility};

/// Deepest chain of modules and re-exports to follow (requirement R-PR-3). Glob re-exports that
/// refer to each other could otherwise go on without end.
const MAX_DEPTH: usize = 256;

/// A path that can be `use`d: the crate name, then module and item names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicPath(Box<[Box<str>]>);

impl PublicPath {
    fn new(segments: &[&str]) -> Self {
        Self(segments.iter().map(|&segment| segment.into()).collect())
    }

    /// The names that make up the path.
    #[must_use]
    pub fn segments(&self) -> &[Box<str>] {
        &self.0
    }
}

impl fmt::Display for PublicPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("::"))
    }
}

/// A public path of this crate that re-exports an item of another crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReexport {
    /// The path in this crate.
    pub path: PublicPath,
    /// The id of the item, found in `Crate::paths`.
    pub target: Id,
    /// The definition path of the item in its crate, from `Crate::paths`, if it is there.
    pub definition: Option<PublicPath>,
}

/// A glob re-export of a module of another crate, which this index does not expand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalGlob {
    /// The path in this crate that the glob re-exports into.
    pub prefix: PublicPath,
    /// The `use` source as written, such as `axum_core::extract::rejection`.
    pub source: Box<str>,
}

/// The public paths of the items of one crate.
#[derive(Debug)]
pub struct PathIndex {
    /// The public paths of each item, the canonical one first.
    paths_of: HashMap<Id, Box<[PublicPath]>>,
    /// The items at each public path, by the path joined with `::`.
    by_public_path: HashMap<Box<str>, Box<[Id]>>,
    /// The items of this crate at each definition path.
    by_definition_path: HashMap<Box<str>, Box<[Id]>>,
    /// Re-exports from other crates, by their public path.
    external_reexports: HashMap<Box<str>, ExternalReexport>,
    /// Glob re-exports from other crates.
    external_globs: Box<[ExternalGlob]>,
}

/// What a path refers to.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// An item of this crate.
    Item(Id),
    /// An item that belongs to another: a method or associated item of an `impl`, an item of a
    /// trait, or a variant of an enum (requirement R-PR-1).
    Member {
        /// The type, trait or enum.
        parent: Id,
        /// The method, associated item or variant.
        member: Id,
    },
    /// A re-export of an item of another crate, whose documentation is not in this crate.
    ExternalReexport(&'a ExternalReexport),
    /// More than one item has this path (requirement R-PR-7).
    Ambiguous(Box<[Id]>),
    /// Nothing has this path.
    NotFound,
}

impl PathIndex {
    /// Builds the index of `krate`.
    #[must_use]
    pub fn build(krate: &Crate) -> Self {
        let crate_name = krate
            .index
            .get(&krate.root)
            .and_then(|root| root.name.as_deref())
            .unwrap_or("crate");
        let mut walker = Walker {
            krate,
            paths: HashMap::new(),
            external_reexports: Vec::new(),
            external_globs: Vec::new(),
            on_stack: HashSet::new(),
        };
        walker.record(krate.root, vec![crate_name]);
        walker.visit_module(krate.root, &[crate_name], 0);

        let paths_of: HashMap<Id, Box<[PublicPath]>> = walker
            .paths
            .into_iter()
            .map(|(id, paths)| {
                let name = krate.index.get(&id).and_then(|item| item.name.as_deref());
                (id, canonical_first(paths, name))
            })
            .collect();
        let by_public_path = group(
            paths_of
                .iter()
                .flat_map(|(&id, paths)| paths.iter().map(move |path| (path.to_string(), id))),
        );
        let by_definition_path = group(
            krate
                .paths
                .iter()
                .filter(|(_, summary)| summary.crate_id == 0)
                .map(|(&id, summary)| (summary.path.join("::"), id)),
        );
        let external_reexports = walker
            .external_reexports
            .into_iter()
            .map(|(path, target)| {
                let reexport = ExternalReexport {
                    path: PublicPath::new(&path),
                    target,
                    definition: krate.paths.get(&target).map(|summary| {
                        PublicPath(summary.path.iter().map(|s| s.as_str().into()).collect())
                    }),
                };
                (reexport.path.to_string().into_boxed_str(), reexport)
            })
            .collect();
        let external_globs = walker
            .external_globs
            .into_iter()
            .map(|(prefix, source)| ExternalGlob {
                prefix: PublicPath::new(&prefix),
                source: source.into(),
            })
            .collect();
        Self {
            paths_of,
            by_public_path,
            by_definition_path,
            external_reexports,
            external_globs,
        }
    }

    /// The canonical path of `id`: among its public paths, one that does not rename the item,
    /// then the shortest, then the first in order (requirement R-PR-2).
    #[must_use]
    pub fn canonical_path(&self, id: Id) -> Option<&PublicPath> {
        self.public_paths(id).first()
    }

    /// Every public path of `id`, the canonical one first.
    #[must_use]
    pub fn public_paths(&self, id: Id) -> &[PublicPath] {
        self.paths_of.get(&id).map_or(&[], |paths| paths)
    }

    /// Glob re-exports from other crates that this index does not expand.
    #[must_use]
    pub fn external_globs(&self) -> &[ExternalGlob] {
        &self.external_globs
    }

    /// Resolves `path`, which may be a public path or a definition path, and may end in a member
    /// of an item (`tokio::sync::Mutex::lock`). `-` and `_` in the crate name are the same.
    #[must_use]
    pub fn resolve(&self, krate: &Crate, path: &str) -> Resolution<'_> {
        let Some(key) = normalize(path) else {
            return Resolution::NotFound;
        };
        if let Some(resolution) = self.lookup(&key) {
            return resolution;
        }
        // An item in an `impl` has no path of its own: find the parent, then the member in it.
        let Some((parent, member)) = key.rsplit_once("::") else {
            return Resolution::NotFound;
        };
        match self.lookup(parent) {
            Some(Resolution::Item(parent)) => find_member(krate, parent, member),
            _ => Resolution::NotFound,
        }
    }

    /// Looks `key` up as a public path, a re-export from another crate, then a definition path.
    fn lookup(&self, key: &str) -> Option<Resolution<'_>> {
        if let Some(ids) = self.by_public_path.get(key) {
            return Some(one_or_ambiguous(ids));
        }
        if let Some(reexport) = self.external_reexports.get(key) {
            return Some(Resolution::ExternalReexport(reexport));
        }
        self.by_definition_path
            .get(key)
            .map(|ids| one_or_ambiguous(ids))
    }
}

/// Walks the modules and re-exports of a crate, with paths that borrow names from it.
struct Walker<'a> {
    krate: &'a Crate,
    paths: HashMap<Id, Vec<Vec<&'a str>>>,
    external_reexports: Vec<(Vec<&'a str>, Id)>,
    external_globs: Vec<(Vec<&'a str>, &'a str)>,
    /// The modules being walked, to stop at a cycle.
    on_stack: HashSet<Id>,
}

impl<'a> Walker<'a> {
    fn record(&mut self, id: Id, path: Vec<&'a str>) {
        self.paths.entry(id).or_default().push(path);
    }

    fn visit_module(&mut self, module: Id, prefix: &[&'a str], depth: usize) {
        if depth > MAX_DEPTH || !self.on_stack.insert(module) {
            return;
        }
        if let Some(Item {
            inner: ItemEnum::Module(contents),
            ..
        }) = self.krate.index.get(&module)
        {
            for &member in &contents.items {
                self.visit_member(member, prefix, depth);
            }
        }
        self.on_stack.remove(&module);
    }

    fn visit_member(&mut self, id: Id, prefix: &[&'a str], depth: usize) {
        let Some(item) = self.krate.index.get(&id) else {
            return;
        };
        // Items that are not public have no public path. docs.rs JSON has none, but JSON built
        // with private items does.
        if item.visibility != Visibility::Public {
            return;
        }
        match &item.inner {
            ItemEnum::Use(reexport) => self.visit_use(reexport, prefix, depth),
            // rustdoc strips a module that is not reachable by its own path.
            ItemEnum::Module(contents) if contents.is_stripped => {},
            ItemEnum::Module(_) => {
                let Some(path) = item.name.as_deref().map(|name| extended(prefix, name)) else {
                    return;
                };
                self.visit_module(id, &path, depth.saturating_add(1));
                self.record(id, path);
            },
            ItemEnum::ExternCrate { .. }
            | ItemEnum::Union(_)
            | ItemEnum::Struct(_)
            | ItemEnum::Enum(_)
            | ItemEnum::Function(_)
            | ItemEnum::Trait(_)
            | ItemEnum::TraitAlias(_)
            | ItemEnum::TypeAlias(_)
            | ItemEnum::Constant { .. }
            | ItemEnum::Static(_)
            | ItemEnum::ExternType
            | ItemEnum::Macro(_)
            | ItemEnum::ProcMacro(_)
            | ItemEnum::Primitive(_) => {
                if let Some(name) = item.name.as_deref() {
                    self.record(id, extended(prefix, name));
                }
            },
            // These belong to another item and are found through it (`find_member`).
            ItemEnum::StructField(_)
            | ItemEnum::Variant(_)
            | ItemEnum::Impl(_)
            | ItemEnum::AssocConst { .. }
            | ItemEnum::AssocType { .. } => {},
        }
    }

    fn visit_use(&mut self, reexport: &'a Use, prefix: &[&'a str], depth: usize) {
        // A re-export of a primitive type has no id (R-PR-3 step 1, in M1).
        let Some(target) = reexport.id else {
            return;
        };
        // `pub use Trait as _;` brings a trait into scope without a name, so it adds no path.
        if !reexport.is_glob && reexport.name == "_" {
            return;
        }
        let Some(item) = self.krate.index.get(&target) else {
            // The item is in another crate.
            if reexport.is_glob {
                self.external_globs
                    .push((prefix.to_vec(), &reexport.source));
            } else {
                self.external_reexports
                    .push((extended(prefix, &reexport.name), target));
            }
            return;
        };
        if reexport.is_glob {
            match &item.inner {
                ItemEnum::Module(_) => {
                    self.visit_module(target, prefix, depth.saturating_add(1));
                },
                ItemEnum::Enum(contents) => {
                    for &variant in &contents.variants {
                        if let Some(name) = self
                            .krate
                            .index
                            .get(&variant)
                            .and_then(|variant| variant.name.as_deref())
                        {
                            self.record(variant, extended(prefix, name));
                        }
                    }
                },
                // A glob imports the items of a module or the variants of an enum, nothing else.
                ItemEnum::ExternCrate { .. }
                | ItemEnum::Use(_)
                | ItemEnum::Union(_)
                | ItemEnum::Struct(_)
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
                | ItemEnum::Primitive(_)
                | ItemEnum::AssocConst { .. }
                | ItemEnum::AssocType { .. } => {},
            }
            return;
        }
        let path = extended(prefix, &reexport.name);
        if matches!(item.inner, ItemEnum::Module(_)) {
            self.visit_module(target, &path, depth.saturating_add(1));
        }
        self.record(target, path);
    }
}

/// `prefix` followed by `name`.
fn extended<'a>(prefix: &[&'a str], name: &'a str) -> Vec<&'a str> {
    let mut path = Vec::with_capacity(prefix.len().saturating_add(1));
    path.extend_from_slice(prefix);
    path.push(name);
    path
}

/// The paths of an item without duplicates, the canonical one first (requirement R-PR-2): paths
/// that do not rename the item, then shorter paths, then in order.
fn canonical_first(paths: Vec<Vec<&str>>, name: Option<&str>) -> Box<[PublicPath]> {
    let renames = |path: &[&str]| path.last().copied() != name;
    let mut paths = paths;
    paths.sort_by(|a, b| {
        (renames(a), a.len())
            .cmp(&(renames(b), b.len()))
            .then_with(|| a.cmp(b))
    });
    paths.dedup();
    paths.iter().map(|path| PublicPath::new(path)).collect()
}

/// Groups ids by key, with the ids of each key in a fixed order (requirement R-OUT-4).
fn group(pairs: impl Iterator<Item = (String, Id)>) -> HashMap<Box<str>, Box<[Id]>> {
    let mut groups: HashMap<Box<str>, Vec<Id>> = HashMap::new();
    for (key, id) in pairs {
        groups.entry(key.into_boxed_str()).or_default().push(id);
    }
    groups
        .into_iter()
        .map(|(key, mut ids)| {
            ids.sort_unstable_by_key(|id| id.0);
            ids.dedup();
            (key, ids.into_boxed_slice())
        })
        .collect()
}

fn one_or_ambiguous(ids: &[Id]) -> Resolution<'static> {
    match ids {
        [] => Resolution::NotFound,
        [id] => Resolution::Item(*id),
        _ => Resolution::Ambiguous(ids.into()),
    }
}

/// Trims `path`, drops a leading `::`, and writes the crate name with `_` instead of `-`.
fn normalize(path: &str) -> Option<String> {
    let path = path.trim().trim_start_matches("::");
    let (crate_name, rest) = path.split_once("::").unwrap_or((path, ""));
    if crate_name.is_empty() {
        return None;
    }
    let crate_name = crate_name.replace('-', "_");
    Some(if rest.is_empty() {
        crate_name
    } else {
        format!("{crate_name}::{rest}")
    })
}

/// Finds the member called `name` of `parent`: the variants of an enum, the items of a trait, or
/// the items of the `impl`s of a type, first inherent ones, then trait ones, then blanket ones.
fn find_member(krate: &Crate, parent: Id, name: &str) -> Resolution<'static> {
    let Some(item) = krate.index.get(&parent) else {
        return Resolution::NotFound;
    };
    let groups: Vec<Vec<Id>> = match &item.inner {
        ItemEnum::Enum(contents) => {
            let mut groups = vec![contents.variants.clone()];
            groups.extend(impl_items(krate, &contents.impls));
            groups
        },
        ItemEnum::Struct(contents) => impl_items(krate, &contents.impls).into(),
        ItemEnum::Union(contents) => impl_items(krate, &contents.impls).into(),
        ItemEnum::Primitive(contents) => impl_items(krate, &contents.impls).into(),
        ItemEnum::Trait(contents) => vec![contents.items.clone()],
        // Nothing else has members that are looked up by name.
        ItemEnum::Module(_)
        | ItemEnum::ExternCrate { .. }
        | ItemEnum::Use(_)
        | ItemEnum::StructField(_)
        | ItemEnum::Variant(_)
        | ItemEnum::Function(_)
        | ItemEnum::TraitAlias(_)
        | ItemEnum::Impl(_)
        | ItemEnum::TypeAlias(_)
        | ItemEnum::Constant { .. }
        | ItemEnum::Static(_)
        | ItemEnum::ExternType
        | ItemEnum::Macro(_)
        | ItemEnum::ProcMacro(_)
        | ItemEnum::AssocConst { .. }
        | ItemEnum::AssocType { .. } => Vec::new(),
    };
    for members in groups {
        let found: Vec<Id> = members
            .into_iter()
            .filter(|member| {
                krate
                    .index
                    .get(member)
                    .and_then(|member| member.name.as_deref())
                    == Some(name)
            })
            .collect();
        match found.as_slice() {
            [] => {},
            [member] => {
                return Resolution::Member {
                    parent,
                    member: *member,
                };
            },
            _ => return Resolution::Ambiguous(found.into_boxed_slice()),
        }
    }
    Resolution::NotFound
}

/// The items of `impls`, in three groups: inherent, trait, and blanket `impl`s.
fn impl_items(krate: &Crate, impls: &[Id]) -> [Vec<Id>; 3] {
    let mut groups: [Vec<Id>; 3] = Default::default();
    for impl_id in impls {
        let Some(Item {
            inner: ItemEnum::Impl(contents),
            ..
        }) = krate.index.get(impl_id)
        else {
            continue;
        };
        let group = match (&contents.trait_, &contents.blanket_impl) {
            (None, _) => &mut groups[0],
            (Some(_), None) => &mut groups[1],
            (Some(_), Some(_)) => &mut groups[2],
        };
        group.extend_from_slice(&contents.items);
    }
    groups
}

#[cfg(test)]
mod tests {
    use rustdoc_types::{
        Enum,
        Generics,
        Impl,
        ItemKind,
        ItemSummary,
        Module,
        Struct,
        StructKind,
        Target,
        Type,
        Variant,
        VariantKind,
    };

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

    fn structure(id: u32, name: &str, impls: &[u32]) -> Item {
        item(
            id,
            Some(name),
            ItemEnum::Struct(Struct {
                kind: StructKind::Unit,
                generics: no_generics(),
                impls: impls.iter().map(|&id| Id(id)).collect(),
            }),
        )
    }

    fn reexport(id: u32, source: &str, name: &str, target: Option<u32>, is_glob: bool) -> Item {
        item(
            id,
            None,
            ItemEnum::Use(Use {
                source: source.to_owned(),
                name: name.to_owned(),
                id: target.map(Id),
                is_glob,
            }),
        )
    }

    fn inherent_impl(id: u32, items: &[u32]) -> Item {
        item(
            id,
            None,
            ItemEnum::Impl(Impl {
                is_unsafe: false,
                generics: no_generics(),
                provided_trait_methods: Vec::new(),
                trait_: None,
                for_: Type::Infer,
                items: items.iter().map(|&id| Id(id)).collect(),
                is_negative: false,
                is_synthetic: false,
                blanket_impl: None,
            }),
        )
    }

    fn assoc_const(id: u32, name: &str) -> Item {
        item(id, Some(name), ItemEnum::AssocConst {
            type_: Type::Infer,
            value: None,
            default_unstable: None,
        })
    }

    /// A crate named `c` with `items`, and `definitions` as its `paths`.
    fn krate(items: Vec<Item>, definitions: &[(u32, u32, &str)]) -> Crate {
        Crate {
            root: Id(0),
            crate_version: None,
            includes_private: false,
            index: items.into_iter().map(|item| (item.id, item)).collect(),
            paths: definitions
                .iter()
                .map(|&(id, crate_id, path)| {
                    (Id(id), ItemSummary {
                        crate_id,
                        path: path.split("::").map(str::to_owned).collect(),
                        kind: ItemKind::Struct,
                    })
                })
                .collect(),
            external_crates: HashMap::new(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".to_owned(),
                target_features: Vec::new(),
            },
            format_version: rustdoc_types::FORMAT_VERSION,
        }
    }

    fn paths(index: &PathIndex, id: u32) -> Vec<String> {
        index
            .public_paths(Id(id))
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn a_type_in_a_private_module_is_found_through_its_reexport() {
        // `pub mod sync { mod mutex { pub struct Mutex; } pub use mutex::Mutex; }`. docs.rs JSON
        // has no `mutex` module.
        let krate = krate(
            vec![
                module(0, "c", &[1]),
                module(1, "sync", &[2]),
                reexport(2, "mutex::Mutex", "Mutex", Some(3), false),
                structure(3, "Mutex", &[]),
            ],
            &[(3, 0, "c::sync::mutex::Mutex")],
        );
        let index = PathIndex::build(&krate);
        assert_eq!(paths(&index, 3), ["c::sync::Mutex"]);
        assert_eq!(
            index.resolve(&krate, "c::sync::Mutex"),
            Resolution::Item(Id(3))
        );
        // The definition path is accepted too.
        assert_eq!(
            index.resolve(&krate, "c::sync::mutex::Mutex"),
            Resolution::Item(Id(3))
        );
    }

    #[test]
    fn the_canonical_path_avoids_renames_then_prefers_the_shortest() {
        // `pub mod x { pub struct Foo; pub struct Baz; } pub use x::Foo as Bar; pub use x::Baz;`
        let krate = krate(
            vec![
                module(0, "c", &[1, 4, 5]),
                module(1, "x", &[2, 3]),
                structure(2, "Foo", &[]),
                structure(3, "Baz", &[]),
                reexport(4, "x::Foo", "Bar", Some(2), false),
                reexport(5, "x::Baz", "Baz", Some(3), false),
            ],
            &[],
        );
        let index = PathIndex::build(&krate);
        assert_eq!(paths(&index, 2), ["c::x::Foo", "c::Bar"]);
        assert_eq!(paths(&index, 3), ["c::Baz", "c::x::Baz"]);
    }

    #[test]
    fn globs_that_refer_to_each_other_stop() {
        // `pub mod a { pub use super::b::*; pub struct A; }` and the same with `a` and `b` swapped.
        let krate = krate(
            vec![
                module(0, "c", &[1, 2]),
                module(1, "a", &[3, 4]),
                module(2, "b", &[5, 6]),
                reexport(3, "super::b", "b", Some(2), true),
                structure(4, "A", &[]),
                reexport(5, "super::a", "a", Some(1), true),
                structure(6, "B", &[]),
            ],
            &[],
        );
        let index = PathIndex::build(&krate);
        assert_eq!(paths(&index, 4), ["c::a::A", "c::b::A"]);
        assert_eq!(paths(&index, 6), ["c::a::B", "c::b::B"]);
    }

    #[test]
    fn a_glob_of_an_enum_reexports_its_variants() {
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
        let krate = krate(
            vec![
                module(0, "c", &[1, 4]),
                item(
                    1,
                    Some("E"),
                    ItemEnum::Enum(Enum {
                        generics: no_generics(),
                        has_stripped_variants: false,
                        variants: vec![Id(2), Id(3)],
                        impls: Vec::new(),
                    }),
                ),
                variant(2, "A"),
                variant(3, "B"),
                reexport(4, "E", "E", Some(1), true),
            ],
            &[],
        );
        let index = PathIndex::build(&krate);
        assert_eq!(paths(&index, 2), ["c::A"]);
        // A variant is also found as a member of its enum.
        assert_eq!(index.resolve(&krate, "c::E::B"), Resolution::Member {
            parent: Id(1),
            member: Id(3),
        });
    }

    #[test]
    fn reexports_from_other_crates_are_recorded_but_not_followed() {
        let krate = krate(
            vec![
                module(0, "c", &[1, 2]),
                reexport(1, "other::Thing", "Thing", Some(9), false),
                reexport(2, "other::prelude", "prelude", Some(8), true),
            ],
            &[(9, 1, "other::inner::Thing"), (8, 1, "other::prelude")],
        );
        let index = PathIndex::build(&krate);
        let Resolution::ExternalReexport(reexport) = index.resolve(&krate, "c::Thing") else {
            panic!("not an external re-export");
        };
        assert_eq!(reexport.target, Id(9));
        assert_eq!(
            reexport
                .definition
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("other::inner::Thing")
        );
        assert_eq!(index.external_globs(), [ExternalGlob {
            prefix: PublicPath::new(&["c"]),
            source: "other::prelude".into(),
        }]);
    }

    #[test]
    fn a_member_of_an_impl_is_found_through_its_type() {
        let krate = krate(
            vec![
                module(0, "my_crate", &[1]),
                structure(1, "S", &[2]),
                inherent_impl(2, &[3]),
                assoc_const(3, "lock"),
            ],
            &[],
        );
        let index = PathIndex::build(&krate);
        let lock = Resolution::Member {
            parent: Id(1),
            member: Id(3),
        };
        assert_eq!(index.resolve(&krate, "my_crate::S::lock"), lock);
        // `-` in the crate name is the same as `_`.
        assert_eq!(index.resolve(&krate, "my-crate::S::lock"), lock);
        assert_eq!(
            index.resolve(&krate, "my_crate::S::unlock"),
            Resolution::NotFound
        );
    }

    #[test]
    fn a_reexport_as_underscore_adds_no_path() {
        // `pub mod x { pub struct T; } pub mod prelude { pub use crate::x::T as _; }`, as with
        // `tracing_subscriber::prelude`.
        let krate = krate(
            vec![
                module(0, "c", &[1, 3]),
                module(1, "x", &[2]),
                structure(2, "T", &[]),
                module(3, "prelude", &[4]),
                reexport(4, "crate::x::T", "_", Some(2), false),
            ],
            &[],
        );
        let index = PathIndex::build(&krate);
        assert_eq!(paths(&index, 2), ["c::x::T"]);
        assert_eq!(index.resolve(&krate, "c::prelude::_"), Resolution::NotFound);
    }

    #[test]
    fn two_items_at_the_same_path_are_ambiguous() {
        // A function and a macro may share a name.
        let krate = krate(
            vec![
                module(0, "c", &[1, 2]),
                structure(1, "same", &[]),
                item(2, Some("same"), ItemEnum::Macro(String::new())),
            ],
            &[],
        );
        let index = PathIndex::build(&krate);
        assert_eq!(
            index.resolve(&krate, "c::same"),
            Resolution::Ambiguous(vec![Id(1), Id(2)].into())
        );
    }
}
