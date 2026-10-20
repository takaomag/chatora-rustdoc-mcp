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
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt,
};

use rustdoc_types::{Crate, Id, Impl, Item, ItemEnum, Type, Use, Visibility};

use super::view::{self, CrateView};

/// Deepest chain of modules and re-exports to follow (requirement R-PR-3). Glob re-exports that
/// refer to each other could otherwise go on without end.
const MAX_DEPTH: usize = 256;

/// How many times one module is walked at most, once for each path that reaches it (requirement
/// R-PR-3). Re-exports of a module under several names multiply the paths of everything below
/// it, so a crate can make the number of paths grow exponentially with the depth of its modules.
/// In the 20 crates of testdata/docs-rs-json.txt, no module was reached more than twice
/// (2026-10-19).
const MAX_MODULE_VISITS: usize = 64;

/// A path that can be `use`d: the crate name, then module and item names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, rkyv::Archive, rkyv::Serialize)]
pub struct PublicPath(Box<[Box<str>]>);

impl PublicPath {
    /// The path written `path`, joined with `::`.
    pub(super) fn parse(path: &str) -> Self {
        Self(path.split("::").map(Into::into).collect())
    }

    fn new(segments: &[&str]) -> Self {
        Self(segments.iter().map(|&segment| segment.into()).collect())
    }

    /// The names that make up the path.
    #[must_use]
    pub fn segments(&self) -> &[Box<str>] {
        &self.0
    }

    /// The path joined with `::`, allocated once at its exact length.
    #[must_use]
    pub fn joined(&self) -> Box<str> {
        self.0.join("::").into_boxed_str()
    }
}

impl fmt::Display for PublicPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, segment) in self.0.iter().enumerate() {
            if position > 0 {
                f.write_str("::")?;
            }
            f.write_str(segment)?;
        }
        Ok(())
    }
}

/// A public path of this crate that re-exports an item of another crate.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize)]
pub struct ExternalReexport {
    /// The path in this crate.
    pub path: PublicPath,
    /// The id of the item, which is not in this crate's `index`.
    pub target: Id,
    /// The definition path of the item in its crate, from `Crate::paths`, if it is there.
    pub definition: Option<PublicPath>,
}

/// A path that goes on in another crate ([`ArchivedPathIndex::crossing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crossing {
    /// The re-export of the other crate's item that the path goes through.
    pub reexport: ExternalReexport,
    /// The path in the other crate, starting with its name.
    pub continued: Box<str>,
}

/// A glob re-export of a module of another crate, which this index does not expand.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize)]
pub struct ExternalGlob {
    /// The path in this crate that the glob re-exports into.
    pub prefix: PublicPath,
    /// The `use` source as written, such as `axum_core::extract::rejection`.
    pub source: Box<str>,
}

/// The public paths of the items of one crate. It is archived with the crate (requirement
/// R-PF-2).
#[derive(Debug, rkyv::Archive, rkyv::Serialize)]
pub struct PathIndex {
    /// The public paths of each item, the canonical one first.
    paths_of: HashMap<Id, Box<[PublicPath]>>,
    /// The modules and `use` items that the canonical path of each item goes through, whose
    /// `cfg` the item has, too (requirement R-FT-2). The root module is left out.
    canonical_via: HashMap<Id, Box<[Id]>>,
    /// The items at each public path, by the path joined with `::`.
    by_public_path: HashMap<Box<str>, Box<[Id]>>,
    /// The items of this crate at each definition path.
    by_definition_path: HashMap<Box<str>, Box<[Id]>>,
    /// Re-exports from other crates, by their public path.
    external_reexports: HashMap<Box<str>, ExternalReexport>,
    /// The public paths one segment below each public path, in order (requirement R-LS-1):
    /// those of items and of re-exports from other crates.
    children: HashMap<Box<str>, Box<[Box<str>]>>,
    /// Glob re-exports from other crates.
    external_globs: Box<[ExternalGlob]>,
    /// Some paths were not followed because a module was reached too many times.
    incomplete: bool,
}

/// What a path refers to.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
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
    ExternalReexport(ExternalReexport),
    /// More than one item of this crate has this path (requirement R-PR-7).
    Ambiguous(Box<[Id]>),
    /// More than one member of `parent` has this name, in the first group of `impl`s that has
    /// one (requirement R-PR-7).
    AmbiguousMembers {
        /// The type, trait or enum.
        parent: Id,
        /// The members with the name.
        members: Box<[Id]>,
    },
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
            visits: HashMap::new(),
            incomplete: false,
        };
        walker.record(krate.root, vec![crate_name], &[]);
        walker.visit_module(krate.root, &Place::root(crate_name), 0);

        let mut canonical_via = HashMap::new();
        let paths_of: HashMap<Id, Box<[PublicPath]>> = walker
            .paths
            .into_iter()
            .map(|(id, paths)| {
                let name = krate.index.get(&id).and_then(|item| item.name.as_deref());
                let (paths, via) = canonical_first(paths, name);
                if !via.is_empty() {
                    canonical_via.insert(id, via);
                }
                (id, paths)
            })
            .collect();
        let by_public_path = group(
            paths_of
                .iter()
                .flat_map(|(&id, paths)| paths.iter().map(move |path| (path.joined(), id))),
        );
        let by_definition_path = group(
            krate
                .paths
                .iter()
                .filter(|(_, summary)| summary.crate_id == 0)
                .map(|(&id, summary)| (summary.path.join("::").into_boxed_str(), id)),
        );
        let external_reexports: HashMap<Box<str>, ExternalReexport> = walker
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
                (reexport.path.joined(), reexport)
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
        let children = children_of(by_public_path.keys().chain(external_reexports.keys()));
        Self {
            paths_of,
            canonical_via,
            children,
            by_public_path,
            by_definition_path,
            external_reexports,
            external_globs,
            incomplete: walker.incomplete,
        }
    }
}

impl PathIndex {
    /// The canonical path of `id`, before archiving.
    pub(super) fn canonical(&self, id: Id) -> Option<&PublicPath> {
        self.paths_of.get(&id).and_then(|paths| paths.first())
    }

    /// The items with a public path, sorted by id, before archiving.
    pub(super) fn public_ids(&self) -> Vec<Id> {
        let mut ids: Vec<Id> = self.paths_of.keys().copied().collect();
        ids.sort_unstable_by_key(|id| id.0);
        ids
    }

    /// The re-exports of other crates' items, sorted by path, before archiving.
    pub(super) fn external_reexports(&self) -> Vec<&ExternalReexport> {
        let mut reexports: Vec<&ExternalReexport> = self.external_reexports.values().collect();
        reexports.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        reexports
    }
}

impl ArchivedPathIndex {
    /// The canonical path of `id`: among its public paths, one that does not rename the item,
    /// then the shortest, then the first in lexicographic order (requirement R-PR-2).
    #[must_use]
    pub fn canonical_path(&self, id: Id) -> Option<PublicPath> {
        self.archived_paths(id)
            .first()
            .map(PublicPath::from_archived)
    }

    /// Every public path of `id`, the canonical one first.
    #[must_use]
    pub fn public_paths(&self, id: Id) -> Vec<PublicPath> {
        self.archived_paths(id)
            .iter()
            .map(PublicPath::from_archived)
            .collect()
    }

    /// The public paths one segment below `path` (a module, an enum or a crate root), sorted:
    /// those of items and of re-exports from other crates (requirement R-LS-1).
    #[must_use]
    pub fn children(&self, path: &str) -> Vec<Box<str>> {
        let Some(key) = normalize(path) else {
            return Vec::new();
        };
        self.children
            .get(key.as_ref())
            .map(|children| children.iter().map(|child| Box::from(&**child)).collect())
            .unwrap_or_default()
    }

    /// Every item that has a public path, in no particular order.
    pub fn public_items(&self) -> impl Iterator<Item = Id> + '_ {
        self.paths_of.keys().map(view::native)
    }

    /// The items at the public path `path`, or the re-export of another crate's item there.
    #[must_use]
    pub fn at_public_path(&self, path: &str) -> Option<Resolution> {
        self.lookup_public(path)
    }

    /// The modules and `use` items that the canonical path of `id` goes through, from the root
    /// down, without the root module and `id` itself.
    #[must_use]
    pub fn canonical_via(&self, id: Id) -> Vec<Id> {
        self.canonical_via
            .get(&view::archived_id(id))
            .map_or_else(Vec::new, |via| via.iter().map(view::native).collect())
    }

    fn archived_paths(&self, id: Id) -> &[ArchivedPublicPath] {
        self.paths_of
            .get(&view::archived_id(id))
            .map_or(&[], |paths| paths)
    }

    /// Whether some public paths were not followed, because a module was reached through more
    /// paths than the index follows (64). An item may then lack some of its public paths, even
    /// its canonical one, since modules are walked depth first.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    /// Glob re-exports from other crates that this index does not expand.
    #[must_use]
    pub fn external_globs(&self) -> Vec<ExternalGlob> {
        self.external_globs
            .iter()
            .map(|glob| ExternalGlob {
                prefix: PublicPath::from_archived(&glob.prefix),
                source: glob.source.as_ref().into(),
            })
            .collect()
    }

    /// Resolves `path`, which may be a public path or a definition path, and may end in a member
    /// of an item (`tokio::sync::Mutex::lock`). `-` and `_` in the crate name are the same.
    #[must_use]
    pub fn resolve(&self, view: &CrateView<'_>, path: &str) -> Resolution {
        let Some(key) = normalize(path) else {
            return Resolution::NotFound;
        };
        if let Some(resolution) = self.lookup_public(&key) {
            return resolution;
        }
        // An item in an `impl` has no path of its own: find the parent, then the member in it.
        // This comes before the definition paths, which list the methods of some crates (std):
        // found as a member, an item keeps its parent and `impl`.
        if let Some((parent, member)) = key.rsplit_once("::")
            && let Some(Resolution::Item(parent)) = self.lookup(parent)
        {
            let found = find_member(view, parent, member);
            if found != Resolution::NotFound {
                return found;
            }
        }
        self.lookup_definition(&key).unwrap_or(Resolution::NotFound)
    }

    /// Where `path` goes on in another crate: the longest prefix of `path` that re-exports an
    /// item of another crate, and the path in that crate, its definition path followed by the rest
    /// of `path` (`std::vec::Vec` goes on as `alloc::vec::Vec`). `None` when no prefix is such a
    /// re-export, or the definition path of the one that is is not known.
    #[must_use]
    pub fn crossing(&self, path: &str) -> Option<Crossing> {
        let key = normalize(path)?;
        let segments: Vec<&str> = key.split("::").collect();
        (1..=segments.len()).rev().find_map(|end| {
            let (prefix, rest) = segments.split_at(end);
            let reexport = self.external_reexport(&prefix.join("::"))?;
            let mut continued = reexport.definition.as_ref()?.joined().into_string();
            for segment in rest {
                continued.push_str("::");
                continued.push_str(segment);
            }
            Some(Crossing {
                reexport,
                continued: continued.into(),
            })
        })
    }

    /// The re-export of another crate's item at `path` that an item of this crate hides: both
    /// have the path, and [`Self::resolve`] returns this crate's item (`serde::Deserialize` is a
    /// trait of serde and a derive macro of `serde_derive`).
    #[must_use]
    pub fn hidden_reexport(&self, path: &str) -> Option<ExternalReexport> {
        let key = normalize(path)?;
        if !self.by_public_path.contains_key(key.as_ref()) {
            return None;
        }
        self.external_reexport(&key)
    }

    /// Every re-export of another crate's item, in no particular order.
    pub fn external_reexports(&self) -> impl Iterator<Item = ExternalReexport> + '_ {
        self.external_reexports
            .values()
            .map(|reexport| ExternalReexport {
                path: PublicPath::from_archived(&reexport.path),
                target: view::native(&reexport.target),
                definition: reexport.definition.as_ref().map(PublicPath::from_archived),
            })
    }

    /// The re-export of another crate's item at the public path `key`.
    fn external_reexport(&self, key: &str) -> Option<ExternalReexport> {
        let reexport = self.external_reexports.get(key)?;
        Some(ExternalReexport {
            path: PublicPath::from_archived(&reexport.path),
            target: view::native(&reexport.target),
            definition: reexport.definition.as_ref().map(PublicPath::from_archived),
        })
    }

    /// Looks `key` up as a public path, a re-export from another crate, then a definition path.
    /// An item of this crate comes before a re-export at the same path ([`Self::hidden_reexport`]).
    fn lookup(&self, key: &str) -> Option<Resolution> {
        self.lookup_public(key)
            .or_else(|| self.lookup_definition(key))
    }

    /// Looks `key` up as a public path, then a re-export from another crate.
    fn lookup_public(&self, key: &str) -> Option<Resolution> {
        if let Some(ids) = self.by_public_path.get(key) {
            return Some(one_or_ambiguous(ids));
        }
        self.external_reexport(key)
            .map(Resolution::ExternalReexport)
    }

    /// Looks `key` up as a definition path of this crate.
    fn lookup_definition(&self, key: &str) -> Option<Resolution> {
        self.by_definition_path
            .get(key)
            .map(|ids| one_or_ambiguous(ids))
    }
}

impl PublicPath {
    /// The path that `archived` holds.
    fn from_archived(archived: &ArchivedPublicPath) -> Self {
        Self(
            archived
                .0
                .iter()
                .map(|segment| Box::from(&**segment))
                .collect(),
        )
    }
}

/// Walks the modules and re-exports of a crate, with paths that borrow names from it.
struct Walker<'a> {
    krate: &'a Crate,
    /// The paths of each item, with the modules and `use` items that each goes through.
    paths: HashMap<Id, Vec<Place<'a>>>,
    external_reexports: Vec<(Vec<&'a str>, Id)>,
    external_globs: Vec<(Vec<&'a str>, &'a str)>,
    /// The modules being walked, to stop at a cycle.
    on_stack: HashSet<Id>,
    /// How many times each module has been walked.
    visits: HashMap<Id, usize>,
    /// A module was reached more than [`MAX_MODULE_VISITS`] times.
    incomplete: bool,
}

/// Where the walk is: the path so far, and the modules and `use` items it went through.
#[derive(Clone)]
struct Place<'a> {
    path: Vec<&'a str>,
    via: Vec<Id>,
}

impl<'a> Place<'a> {
    fn root(crate_name: &'a str) -> Self {
        Self {
            path: vec![crate_name],
            via: Vec::new(),
        }
    }

    /// The place one step further, at `name` (if any) through `through`.
    fn then(&self, name: Option<&'a str>, through: &[Id]) -> Self {
        let mut next = self.clone();
        next.path.extend(name);
        next.via.extend_from_slice(through);
        next
    }
}

impl<'a> Walker<'a> {
    fn record(&mut self, id: Id, path: Vec<&'a str>, via: &[Id]) {
        self.paths.entry(id).or_default().push(Place {
            path,
            via: via.to_vec(),
        });
    }

    fn visit_module(&mut self, module: Id, place: &Place<'a>, depth: usize) {
        if depth > MAX_DEPTH || self.on_stack.contains(&module) {
            return;
        }
        let visits = self.visits.entry(module).or_default();
        if *visits >= MAX_MODULE_VISITS {
            self.incomplete = true;
            return;
        }
        *visits = visits.saturating_add(1);
        self.on_stack.insert(module);
        if let Some(Item {
            inner: ItemEnum::Module(contents),
            ..
        }) = self.krate.index.get(&module)
        {
            for &member in &contents.items {
                self.visit_member(member, place, depth);
            }
        }
        self.on_stack.remove(&module);
    }

    fn visit_member(&mut self, id: Id, place: &Place<'a>, depth: usize) {
        let Some(item) = self.krate.index.get(&id) else {
            return;
        };
        // Items that are not public have no public path. docs.rs JSON has none, but JSON built
        // with private items does.
        if item.visibility != Visibility::Public {
            return;
        }
        match &item.inner {
            ItemEnum::Use(reexport) => self.visit_use(id, reexport, place, depth),
            // A stripped module is not public API. Items re-exported from it are reached through
            // their `use`.
            ItemEnum::Module(contents) if contents.is_stripped => {},
            ItemEnum::Module(_) => {
                let Some(name) = item.name.as_deref() else {
                    return;
                };
                let inside = place.then(Some(name), &[id]);
                self.visit_module(id, &inside, depth.saturating_add(1));
                self.record(id, extended(&place.path, name), &place.via);
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
                    self.record(id, extended(&place.path, name), &place.via);
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

    fn visit_use(&mut self, use_id: Id, reexport: &'a Use, place: &Place<'a>, depth: usize) {
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
                    .push((place.path.clone(), &reexport.source));
            } else {
                self.external_reexports
                    .push((extended(&place.path, &reexport.name), target));
            }
            return;
        };
        if reexport.is_glob {
            match &item.inner {
                ItemEnum::Module(_) => {
                    let inside = place.then(None, &[use_id, target]);
                    self.visit_module(target, &inside, depth.saturating_add(1));
                },
                ItemEnum::Enum(contents) => {
                    let via = place.then(None, &[use_id, target]).via;
                    for &variant in &contents.variants {
                        if let Some(name) = self
                            .krate
                            .index
                            .get(&variant)
                            .and_then(|variant| variant.name.as_deref())
                        {
                            self.record(variant, extended(&place.path, name), &via);
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
        let through = place.then(Some(&reexport.name), &[use_id]);
        if matches!(item.inner, ItemEnum::Module(_)) {
            let inside = through.then(None, &[target]);
            self.visit_module(target, &inside, depth.saturating_add(1));
        }
        self.record(target, through.path, &through.via);
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
/// that do not rename the item, then shorter paths, then in lexicographic order. Also returns
/// what the canonical path goes through, the first way that it was reached.
fn canonical_first(paths: Vec<Place<'_>>, name: Option<&str>) -> (Box<[PublicPath]>, Box<[Id]>) {
    let renames = |path: &[&str]| path.last().copied() != name;
    let mut paths = paths;
    // Stable, so that equal paths stay in the order they were reached.
    paths.sort_by(|a, b| {
        (renames(&a.path), a.path.len())
            .cmp(&(renames(&b.path), b.path.len()))
            .then_with(|| a.path.cmp(&b.path))
    });
    paths.dedup_by(|a, b| a.path == b.path);
    let via = paths
        .first()
        .map(|place| place.via.clone().into_boxed_slice())
        .unwrap_or_default();
    (
        paths
            .iter()
            .map(|place| PublicPath::new(&place.path))
            .collect(),
        via,
    )
}

/// The public paths one segment below each of `paths`, sorted, by the path above them.
fn children_of<'p>(
    paths: impl Iterator<Item = &'p Box<str>>,
) -> HashMap<Box<str>, Box<[Box<str>]>> {
    let mut children: HashMap<Box<str>, Vec<Box<str>>> = HashMap::new();
    for path in paths {
        if let Some((parent, _)) = path.rsplit_once("::") {
            children
                .entry(parent.into())
                .or_default()
                .push(path.clone());
        }
    }
    children
        .into_iter()
        .map(|(parent, mut paths)| {
            paths.sort_unstable();
            paths.dedup();
            (parent, paths.into_boxed_slice())
        })
        .collect()
}

/// Groups ids by key, with the ids of each key in a fixed order (requirement R-OUT-4).
fn group(pairs: impl Iterator<Item = (Box<str>, Id)>) -> HashMap<Box<str>, Box<[Id]>> {
    let mut groups: HashMap<Box<str>, Vec<Id>> = HashMap::new();
    for (key, id) in pairs {
        groups.entry(key).or_default().push(id);
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

fn one_or_ambiguous(ids: &[rustdoc_types::ArchivedId]) -> Resolution {
    match ids {
        [] => Resolution::NotFound,
        [id] => Resolution::Item(view::native(id)),
        _ => Resolution::Ambiguous(ids.iter().map(view::native).collect()),
    }
}

/// Trims `path`, drops a leading `::`, and writes the crate name with `_` instead of `-`. The
/// path is borrowed unless its crate name has a `-`.
fn normalize(path: &str) -> Option<Cow<'_, str>> {
    let path = path.trim().trim_start_matches("::");
    let (crate_name, rest) = path.split_once("::").unwrap_or((path, ""));
    if crate_name.is_empty() {
        return None;
    }
    if !crate_name.contains('-') {
        return Some(Cow::Borrowed(path));
    }
    let crate_name = crate_name.replace('-', "_");
    Some(Cow::Owned(if rest.is_empty() {
        crate_name
    } else {
        format!("{crate_name}::{rest}")
    }))
}

/// Finds the member called `name` of `parent`: the variants of an enum, the items of a trait, or
/// the items of the `impl`s of a type, first inherent ones, then trait ones, then blanket ones.
fn find_member(view: &CrateView<'_>, parent: Id, name: &str) -> Resolution {
    let own = view.item(parent).map_or(&[][..], own_members);
    if let Some(found) = named(view, parent, own.iter(), name) {
        return found;
    }
    for kind in [ImplKind::Inherent, ImplKind::Trait, ImplKind::Blanket] {
        let members = own_impls(view, parent)
            .filter(|(_, contents)| ImplKind::of(contents) == kind)
            .flat_map(|(_, contents)| contents.items.iter());
        if let Some(found) = named(view, parent, members, name) {
            return found;
        }
    }
    Resolution::NotFound
}

/// The members that `item` has itself: the variants of an enum and the items of a trait.
fn own_members(item: &Item) -> &[Id] {
    match &item.inner {
        ItemEnum::Enum(contents) => &contents.variants,
        ItemEnum::Trait(contents) => &contents.items,
        ItemEnum::Module(_)
        | ItemEnum::ExternCrate { .. }
        | ItemEnum::Use(_)
        | ItemEnum::Union(_)
        | ItemEnum::Struct(_)
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
        | ItemEnum::Primitive(_)
        | ItemEnum::AssocConst { .. }
        | ItemEnum::AssocType { .. } => &[],
    }
}

/// The `impl`s of the type `parent` that are for it, in the order of the data (requirement
/// R-PR-7).
///
/// rustdoc also lists among the `impls` of a type the `impl`s for other types that mention it,
/// such as `impl From<Elapsed> for std::io::Error` among those of `Elapsed`; their items are not
/// members of the type. A primitive type keeps all of its `impl`s, which are for its many forms
/// (`[T]`, `&str`).
pub fn own_impls<'v>(
    view: &'v CrateView<'_>,
    parent: Id,
) -> impl Iterator<Item = (&'v Item, &'v Impl)> {
    let parent_item = view.item(parent);
    let is_primitive = matches!(
        parent_item,
        Some(Item {
            inner: ItemEnum::Primitive(_),
            ..
        })
    );
    parent_item
        .map_or(&[][..], impls_of)
        .iter()
        .filter_map(move |&id| {
            let item = view.item(id)?;
            let ItemEnum::Impl(contents) = &item.inner else {
                return None;
            };
            let is_own = is_primitive
                || matches!(&contents.for_, Type::ResolvedPath(path) if path.id == parent);
            is_own.then_some((item, contents))
        })
}

/// The `impl`s that rustdoc lists for a type.
pub(super) fn impls_of(item: &Item) -> &[Id] {
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

/// The member of `parent` among `members` called `name`, ambiguous if there are several, or
/// `None` if there is none.
fn named<'a>(
    view: &CrateView<'_>,
    parent: Id,
    members: impl Iterator<Item = &'a Id>,
    name: &str,
) -> Option<Resolution> {
    let found: Vec<Id> = members
        .filter(|&&member| {
            view.item(member).and_then(|member| member.name.as_deref()) == Some(name)
        })
        .copied()
        .collect();
    match found.as_slice() {
        [] => None,
        [member] => Some(Resolution::Member {
            parent,
            member: *member,
        }),
        _ => Some(Resolution::AmbiguousMembers {
            parent,
            members: found.into_boxed_slice(),
        }),
    }
}

/// The groups of `impl`s whose items are searched in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImplKind {
    Inherent,
    Trait,
    Blanket,
}

impl ImplKind {
    const fn of(contents: &Impl) -> Self {
        match (&contents.trait_, &contents.blanket_impl) {
            (None, _) => Self::Inherent,
            (Some(_), None) => Self::Trait,
            (Some(_), Some(_)) => Self::Blanket,
        }
    }
}

#[cfg(test)]
mod tests {
    use rustdoc_types::{
        Enum,
        Generics,
        ItemKind,
        ItemSummary,
        Module,
        Struct,
        StructKind,
        Target,
        Variant,
        VariantKind,
    };

    use super::*;
    use crate::query::view::tests::{archive, archived};

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

    /// An `impl` for the type `for_`, of the trait `trait_` if there is one.
    fn an_impl(id: u32, for_: u32, trait_: Option<u32>, items: &[u32]) -> Item {
        let path = |id: u32| rustdoc_types::Path {
            path: String::new(),
            id: Id(id),
            args: None,
        };
        item(
            id,
            None,
            ItemEnum::Impl(Impl {
                is_unsafe: false,
                generics: no_generics(),
                provided_trait_methods: Vec::new(),
                trait_: trait_.map(path),
                for_: Type::ResolvedPath(path(for_)),
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

    fn paths(index: &ArchivedPathIndex, id: u32) -> Vec<String> {
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
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(paths(index, 3), ["c::sync::Mutex"]);
        assert_eq!(
            index.resolve(&view, "c::sync::Mutex"),
            Resolution::Item(Id(3))
        );
        // The definition path is accepted too.
        assert_eq!(
            index.resolve(&view, "c::sync::mutex::Mutex"),
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
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(paths(index, 2), ["c::x::Foo", "c::Bar"]);
        assert_eq!(paths(index, 3), ["c::Baz", "c::x::Baz"]);
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
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(paths(index, 4), ["c::a::A", "c::b::A"]);
        assert_eq!(paths(index, 6), ["c::a::B", "c::b::B"]);
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
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(paths(index, 2), ["c::A"]);
        // A variant is also found as a member of its enum.
        assert_eq!(index.resolve(&view, "c::E::B"), Resolution::Member {
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
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        let Resolution::ExternalReexport(reexport) = index.resolve(&view, "c::Thing") else {
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
    fn a_path_through_a_reexport_goes_on_in_the_other_crate() {
        let krate = krate(
            vec![
                module(0, "std", &[1, 2]),
                reexport(1, "alloc::vec", "vec", Some(9), false),
                reexport(2, "core::option::Option", "Option", Some(8), false),
            ],
            &[(9, 1, "alloc::vec"), (8, 2, "core::option::Option")],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        let continued = |path: &str| index.crossing(path).map(|crossing| crossing.continued);
        assert_eq!(
            continued("std::vec::Vec").as_deref(),
            Some("alloc::vec::Vec")
        );
        assert_eq!(
            continued("std::vec::Vec::push").as_deref(),
            Some("alloc::vec::Vec::push")
        );
        assert_eq!(continued("std::vec").as_deref(), Some("alloc::vec"));
        assert_eq!(
            continued("std::Option").as_deref(),
            Some("core::option::Option")
        );
        assert_eq!(continued("std::collections::HashMap"), None);
    }

    #[test]
    fn a_member_with_a_definition_path_is_found_as_a_member() {
        // The std of a nightly lists methods among its definition paths.
        let krate = krate(
            vec![
                module(0, "c", &[1]),
                structure(1, "S", &[2]),
                an_impl(2, 1, None, &[3]),
                assoc_const(3, "get"),
            ],
            &[(1, 0, "c::S"), (3, 0, "c::S::get")],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        assert_eq!(
            view.index().resolve(&view, "c::S::get"),
            Resolution::Member {
                parent: Id(1),
                member: Id(3),
            }
        );
    }

    #[test]
    fn a_member_of_an_impl_is_found_through_its_type() {
        let krate = krate(
            vec![
                module(0, "my_crate", &[1]),
                structure(1, "S", &[2]),
                an_impl(2, 1, None, &[3]),
                assoc_const(3, "lock"),
            ],
            &[],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        let lock = Resolution::Member {
            parent: Id(1),
            member: Id(3),
        };
        assert_eq!(index.resolve(&view, "my_crate::S::lock"), lock);
        // `-` in the crate name is the same as `_`.
        assert_eq!(index.resolve(&view, "my-crate::S::lock"), lock);
        assert_eq!(
            index.resolve(&view, "my_crate::S::unlock"),
            Resolution::NotFound
        );
    }

    #[test]
    fn a_member_of_an_impl_for_another_type_is_not_a_member() {
        // `impl From<S> for Other { fn from(..) }` is among the `impls` of `S` too, as with
        // `impl From<Elapsed> for std::io::Error` in tokio.
        let krate = krate(
            vec![
                module(0, "c", &[1, 4]),
                structure(1, "S", &[2]),
                an_impl(2, 4, Some(5), &[3]),
                assoc_const(3, "from"),
                structure(4, "Other", &[2]),
            ],
            &[],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(index.resolve(&view, "c::S::from"), Resolution::NotFound);
        assert_eq!(index.resolve(&view, "c::Other::from"), Resolution::Member {
            parent: Id(4),
            member: Id(3),
        });
    }

    #[test]
    fn members_of_the_same_name_in_one_group_are_ambiguous() {
        // Two trait `impl`s of `S` with an item each called `get`.
        let krate = krate(
            vec![
                module(0, "c", &[1]),
                structure(1, "S", &[2, 4]),
                an_impl(2, 1, Some(6), &[3]),
                assoc_const(3, "get"),
                an_impl(4, 1, Some(7), &[5]),
                assoc_const(5, "get"),
            ],
            &[],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(
            index.resolve(&view, "c::S::get"),
            Resolution::AmbiguousMembers {
                parent: Id(1),
                members: vec![Id(3), Id(5)].into(),
            }
        );
    }

    #[test]
    fn an_item_hides_a_reexport_at_the_same_path() {
        // `pub trait Deserialize {}` and `pub use other::Deserialize;` (a derive macro), as in
        // serde.
        let krate = krate(
            vec![
                module(0, "c", &[1, 2]),
                item(1, Some("Deserialize"), ItemEnum::Macro(String::new())),
                reexport(2, "other::Deserialize", "Deserialize", Some(3), false),
            ],
            &[(3, 1, "other::Deserialize")],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(
            index.resolve(&view, "c::Deserialize"),
            Resolution::Item(Id(1))
        );
        let hidden = index
            .hidden_reexport("c::Deserialize")
            .expect("the re-export is found");
        assert_eq!(hidden.target, Id(3));
        assert_eq!(index.hidden_reexport("c::Other"), None);
    }

    #[test]
    fn modules_reexported_in_many_ways_are_walked_a_bounded_number_of_times() {
        // Module `l{i}` re-exports `l{i-1}` as both `a` and `b`, so `l0` and its struct `S` have
        // 2^40 paths from the root through `l40`.
        const LAYERS: u32 = 40;
        let module_id = |layer: u32| 10 + layer;
        let mut items = vec![
            module(0, "c", &[module_id(LAYERS)]),
            module(module_id(0), "l0", &[1]),
            structure(1, "S", &[]),
        ];
        for layer in 1..=LAYERS {
            let (a, b) = (1000 + 2 * layer, 1001 + 2 * layer);
            items.push(module(module_id(layer), &format!("l{layer}"), &[a, b]));
            for (id, name) in [(a, "a"), (b, "b")] {
                items.push(reexport(
                    id,
                    "super::lower",
                    name,
                    Some(module_id(layer - 1)),
                    false,
                ));
            }
        }
        let krate = krate(items, &[]);
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert!(index.is_incomplete());
        let paths_of_s = index.public_paths(Id(1));
        assert_ne!(paths_of_s, []);
        assert!(paths_of_s.len() <= MAX_MODULE_VISITS);
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
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert!(!index.is_incomplete());
        assert_eq!(paths(index, 2), ["c::x::T"]);
        assert_eq!(index.resolve(&view, "c::prelude::_"), Resolution::NotFound);
    }

    #[test]
    fn two_items_at_the_same_path_are_ambiguous() {
        // A struct and a macro may share a name.
        let krate = krate(
            vec![
                module(0, "c", &[1, 2]),
                structure(1, "same", &[]),
                item(2, Some("same"), ItemEnum::Macro(String::new())),
            ],
            &[],
        );
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let index = view.index();
        assert_eq!(
            index.resolve(&view, "c::same"),
            Resolution::Ambiguous(vec![Id(1), Id(2)].into())
        );
    }
}
