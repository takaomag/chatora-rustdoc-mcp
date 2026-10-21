//! Searching the items of a crate by their names, `doc(alias)` names, documentation and paths
//! (requirements R-SR-1 and R-SR-3 to R-SR-5).
//!
//! The index is built with the archive (requirement R-PF-2), so that a search reads postings
//! instead of tokenizing every item's documentation.

use std::{
    borrow::Cow,
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use rustdoc_types::{Attribute, Crate, Id, Item, ItemEnum, StabilityLevel, Type, Visibility};
use serde::Serialize;

use super::{
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
        first_paragraph,
        kind_of,
    },
    list_module::Page,
    path_index::{ExternalReexport, PathIndex, PublicPath, impls_of, is_doc_hidden},
    view::{self, CrateView},
};

/// The weight of an item's name (requirement R-SR-3).
const NAME_WEIGHT: f64 = 20.0;

/// The weight of a `doc(alias)` name.
const ALIAS_WEIGHT: f64 = 10.0;

/// The weight of the first paragraph of the documentation.
const FIRST_WEIGHT: f64 = 3.0;

/// BM25's `k1`, with `b = 0`: the length of the documentation does not count.
const K1: f64 = 1.2;

/// Results on a page after the first, or with `limit` unset after the first page.
const DEFAULT_LIMIT: usize = 20;

/// The most results on the first page when the scores decide where it ends.
const MAX_FIRST_PAGE: usize = 50;

/// The index of a crate's items for search.
#[derive(Debug, Default, rkyv::Archive, rkyv::Serialize)]
pub struct SearchIndex {
    /// The items that can be found, with what is shown of them.
    entries: Box<[Entry]>,
    /// For each token, the items that have it and how many times in each field.
    postings: HashMap<Box<str>, Box<[Posting]>>,
    /// The items by their name in lower case.
    names: HashMap<Box<str>, Box<[u32]>>,
    /// The items by their `doc(alias)` names in lower case.
    aliases: HashMap<Box<str>, Box<[u32]>>,
}

/// An item that can be found.
#[derive(Debug, rkyv::Archive, rkyv::Serialize)]
struct Entry {
    /// The item, of this crate or, for a re-export, of another.
    id: Id,
    /// The type, trait or enum that it is a member of.
    parent: Option<Id>,
    /// Its canonical path.
    path: Box<str>,
    /// How many intra-doc links of the crate point to it (requirement R-SR-3).
    referenced: u32,
    /// Its path has `__`, `_private` or `internal`, which rank lower.
    penalized: bool,
    /// It is a re-export of another crate's item, whose documentation is not here.
    external: bool,
    /// It is not public, or it is hidden (`#[doc(hidden)]`): found only in a workspace member,
    /// whose own code it is (requirement R-SR-4, decided in M2's D6).
    restricted: bool,
}

/// How many times a token is in each field of an item.
#[derive(Debug, Clone, Copy, Default, rkyv::Archive, rkyv::Serialize)]
struct Posting {
    entry: u32,
    name: u16,
    alias: u16,
    first: u16,
    rest: u16,
    /// The names of its ancestors in its path, weighted 3, 2 and 1 from the nearest.
    path: u16,
}

/// The fields that a search looks in (requirement R-SR-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one switch for each field, as the `fields` argument has them"
)]
pub struct Fields {
    /// The item's name.
    pub name: bool,
    /// Its `doc(alias)` names.
    pub alias: bool,
    /// Its documentation.
    pub docs: bool,
    /// The names of its ancestors in its path.
    pub path: bool,
}

impl Fields {
    /// Every field.
    pub const ALL: Self = Self {
        name: true,
        alias: true,
        docs: true,
        path: true,
    };
}

impl SearchIndex {
    /// Indexes the items of `krate` that have a public path, their members (the items of their
    /// inherent `impl`s, of a trait, and the variants of an enum), and the re-exports of other
    /// crates' items. Unstable items of std are left out (requirement R-SR-4). With private
    /// items, the others too, at their paths in the crate (`crate::a::Item`), marked as
    /// restricted, as are the members that are not public and the hidden items (decided in M2's
    /// D6).
    #[must_use]
    pub fn build(krate: &Crate, paths: &PathIndex) -> Self {
        let mut referenced: HashMap<Id, u32> = HashMap::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "counting the links does not depend on their order"
        )]
        for item in krate.index.values() {
            #[expect(
                clippy::iter_over_hash_type,
                reason = "counting the links does not depend on their order"
            )]
            for &target in item.links.values() {
                let count = referenced.entry(target).or_default();
                *count = count.saturating_add(1);
            }
        }
        let mut builder = Builder {
            referenced: &referenced,
            entries: Vec::new(),
            postings: HashMap::new(),
            names: HashMap::new(),
            aliases: HashMap::new(),
        };
        let mut seen = HashSet::new();
        let public = paths.public_ids().into_iter().filter_map(|id| {
            let path = paths.canonical(id).map(PublicPath::joined)?;
            Some((id, path, paths.is_hidden(id)))
        });
        // Those without a public path, with private items: in the order of their paths.
        let mut others: Vec<(Id, Box<str>)> = if krate.includes_private {
            krate
                .paths
                .iter()
                .filter(|(id, summary)| summary.crate_id == 0 && paths.canonical(**id).is_none())
                .map(|(&id, summary)| (id, in_crate_path(&summary.path)))
                .collect()
        } else {
            Vec::new()
        };
        others.sort_unstable_by(|a, b| (&a.1, a.0.0).cmp(&(&b.1, b.0.0)));
        let all = public.chain(others.into_iter().map(|(id, path)| (id, path, true)));
        for (id, path, restricted) in all {
            let Some(item) = krate.index.get(&id) else {
                continue;
            };
            if id == krate.root || is_unstable(item) || !seen.insert(id) {
                continue;
            }
            for &member in &members(krate, id, item) {
                if let Some(member_item) = krate.index.get(&member)
                    && let Some(name) = member_item.name.as_deref()
                    && !is_unstable(member_item)
                    && seen.insert(member)
                {
                    let restricted = restricted
                        || !matches!(
                            member_item.visibility,
                            Visibility::Public | Visibility::Default
                        )
                        || is_doc_hidden(member_item)
                        || paths.is_hidden(member);
                    builder.add(
                        member_item,
                        Some(id),
                        &format!("{path}::{name}"),
                        restricted,
                    );
                }
            }
            builder.add(item, None, &path, restricted);
        }
        for reexport in paths.external_reexports() {
            let path = reexport.path.joined();
            builder.add_external(reexport.target, &path, paths.is_hidden_reexport(&path));
        }
        builder.finish()
    }
}

/// The path of an item of the crate in it, `crate::a::Item`, from its definition path.
fn in_crate_path(definition: &[String]) -> Box<str> {
    let mut path = String::from("crate");
    for segment in definition.iter().skip(1) {
        path.push_str("::");
        path.push_str(segment);
    }
    path.into()
}

/// The members of `item` that a search finds: the items of a trait, the variants of an enum,
/// and the items of the inherent `impl`s of a type.
fn members(krate: &Crate, id: Id, item: &Item) -> Vec<Id> {
    match &item.inner {
        ItemEnum::Trait(contents) => contents.items.clone(),
        ItemEnum::Enum(contents) => {
            let mut members = contents.variants.clone();
            members.extend(inherent_items(krate, id, item));
            members
        },
        ItemEnum::Struct(_) | ItemEnum::Union(_) | ItemEnum::Primitive(_) => {
            inherent_items(krate, id, item)
        },
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
    }
}

/// The items of the inherent `impl`s of the type `id`.
fn inherent_items(krate: &Crate, id: Id, item: &Item) -> Vec<Id> {
    let is_primitive = matches!(item.inner, ItemEnum::Primitive(_));
    impls_of(item)
        .iter()
        .filter_map(|impl_id| {
            let ItemEnum::Impl(contents) = &krate.index.get(impl_id)?.inner else {
                return None;
            };
            let own =
                is_primitive || matches!(&contents.for_, Type::ResolvedPath(path) if path.id == id);
            (contents.trait_.is_none() && own).then(|| contents.items.clone())
        })
        .flatten()
        .collect()
}

/// Whether std marks `item` unstable.
fn is_unstable(item: &Item) -> bool {
    item.stability
        .as_deref()
        .is_some_and(|stability| matches!(stability.level, StabilityLevel::Unstable))
}

/// Collects the entries and postings of an index.
struct Builder<'r> {
    referenced: &'r HashMap<Id, u32>,
    entries: Vec<Entry>,
    postings: HashMap<Box<str>, Vec<Posting>>,
    names: HashMap<Box<str>, Vec<u32>>,
    aliases: HashMap<Box<str>, Vec<u32>>,
}

impl Builder<'_> {
    /// Adds `item`, a member of `parent` if it is one, at `path`, `restricted` if it is not public
    /// or hidden.
    fn add(&mut self, item: &Item, parent: Option<Id>, path: &str, restricted: bool) {
        let Some(name) = item.name.as_deref() else {
            return;
        };
        let entry = self.push_entry(item.id, parent, path, (false, restricted));
        let mut counts: HashMap<String, Posting> = HashMap::new();
        let mut count = |token: String, field: fn(&mut Posting) -> &mut u16, by: u16| {
            let posting = counts.entry(token).or_default();
            let slot = field(posting);
            *slot = slot.saturating_add(by);
        };
        for token in tokens(name) {
            count(token, |posting| &mut posting.name, 1);
        }
        for alias in aliases(item) {
            self.aliases
                .entry(alias.to_lowercase().into())
                .or_default()
                .push(entry);
            for token in tokens(&alias) {
                count(token, |posting| &mut posting.alias, 1);
            }
        }
        if let Some(docs) = item.docs.as_deref() {
            let (first, _) = first_paragraph(docs);
            let rest = docs.get(first.len()..).unwrap_or_default();
            for token in prose_tokens(first) {
                count(token, |posting| &mut posting.first, 1);
            }
            for token in prose_tokens(rest) {
                count(token, |posting| &mut posting.rest, 1);
            }
        }
        for (segment, weight) in path.rsplit("::").skip(1).take(3).zip([3, 2, 1]) {
            for token in tokens(segment) {
                count(token, |posting| &mut posting.path, weight);
            }
        }
        self.names
            .entry(name.to_lowercase().into())
            .or_default()
            .push(entry);
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each token's postings are sorted by entry when the index is finished"
        )]
        for (token, mut posting) in counts {
            posting.entry = entry;
            self.postings.entry(token.into()).or_default().push(posting);
        }
    }

    /// Adds a re-export of another crate's item, `target`, at `path`, `hidden` if it goes through
    /// an item of `#[doc(hidden)]`: found by its name and path, as its documentation is in the
    /// other crate.
    fn add_external(&mut self, target: Id, path: &str, hidden: bool) {
        let name = path.rsplit("::").next().unwrap_or(path);
        let entry = self.push_entry(target, None, path, (true, hidden));
        let mut counts: HashMap<String, Posting> = HashMap::new();
        for token in tokens(name) {
            let posting = counts.entry(token).or_default();
            posting.name = posting.name.saturating_add(1);
        }
        for (segment, weight) in path.rsplit("::").skip(1).take(3).zip([3, 2, 1]) {
            for token in tokens(segment) {
                let posting = counts.entry(token).or_default();
                posting.path = posting.path.saturating_add(weight);
            }
        }
        self.names
            .entry(name.to_lowercase().into())
            .or_default()
            .push(entry);
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each token's postings are sorted by entry when the index is finished"
        )]
        for (token, mut posting) in counts {
            posting.entry = entry;
            self.postings.entry(token.into()).or_default().push(posting);
        }
    }

    /// Adds the entry of `id`, a member of `parent`, at `path`: a re-export of another crate's
    /// item, and not public or hidden, as `(external, restricted)` tells.
    fn push_entry(
        &mut self,
        id: Id,
        parent: Option<Id>,
        path: &str,
        (external, restricted): (bool, bool),
    ) -> u32 {
        let entry = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        let penalized = path.split("::").any(|segment| {
            segment.starts_with("__") || segment.contains("_private") || segment == "internal"
        });
        self.entries.push(Entry {
            id,
            parent,
            path: path.into(),
            referenced: self.referenced.get(&id).copied().unwrap_or_default(),
            penalized,
            external,
            restricted,
        });
        entry
    }

    fn finish(self) -> SearchIndex {
        let boxed = |map: HashMap<Box<str>, Vec<u32>>| {
            map.into_iter()
                .map(|(key, mut entries)| {
                    entries.sort_unstable();
                    entries.dedup();
                    (key, entries.into_boxed_slice())
                })
                .collect()
        };
        SearchIndex {
            entries: self.entries.into_boxed_slice(),
            postings: self
                .postings
                .into_iter()
                .map(|(token, mut postings)| {
                    postings.sort_unstable_by_key(|posting| posting.entry);
                    (token, postings.into_boxed_slice())
                })
                .collect(),
            names: boxed(self.names),
            aliases: boxed(self.aliases),
        }
    }
}

/// The `doc(alias)` names of `item`: `#[doc(alias = "a")]` and `#[doc(alias("a", "b"))]`.
fn aliases(item: &Item) -> Vec<String> {
    let mut aliases = Vec::new();
    for attribute in &item.attrs {
        let Attribute::Other(text) = attribute else {
            continue;
        };
        let Some(rest) = text.strip_prefix("#[doc") else {
            continue;
        };
        let Some(at) = rest.find("alias") else {
            continue;
        };
        let mut quoted = rest.get(at..).unwrap_or_default().split('"');
        quoted.next();
        // Every other piece is inside quotes.
        for (position, piece) in quoted.enumerate() {
            if position % 2 == 0 && !piece.is_empty() {
                aliases.push(piece.to_owned());
            }
        }
    }
    aliases
}

/// The tokens of a name or of text, in lower case (requirement R-SR-3).
///
/// Each word comes whole and split at `_` and at the changes of case of CamelCase, acronyms
/// included (`HTTPServer` is `http` and `server`). Parts of one letter are left out.
#[must_use]
pub fn tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.is_empty() {
            continue;
        }
        let whole = word.to_lowercase();
        let parts = parts(word);
        if parts.len() > 1 || parts.first().is_some_and(|part| *part != whole) {
            tokens.extend(parts.into_iter().filter(|part| part.chars().count() > 1));
        }
        if whole.chars().count() > 1 {
            tokens.push(whole);
        }
    }
    tokens
}

/// `word` split at `_` and at its changes of case, in lower case.
fn parts(word: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in word.split('_').filter(|piece| !piece.is_empty()) {
        let chars: Vec<char> = piece.chars().collect();
        let mut start = 0;
        for at in 1..chars.len() {
            let (before, here) = (chars.get(at.saturating_sub(1)), chars.get(at));
            let next = chars.get(at.saturating_add(1));
            let boundary = match (before, here) {
                (Some(before), Some(here)) => {
                    (before.is_lowercase() || before.is_numeric()) && here.is_uppercase()
                        || before.is_uppercase()
                            && here.is_uppercase()
                            && next.is_some_and(|next| next.is_lowercase())
                },
                _ => false,
            };
            if boundary {
                parts.push(
                    chars
                        .get(start..at)
                        .unwrap_or_default()
                        .iter()
                        .collect::<String>()
                        .to_lowercase(),
                );
                start = at;
            }
        }
        parts.push(
            chars
                .get(start..)
                .unwrap_or_default()
                .iter()
                .collect::<String>()
                .to_lowercase(),
        );
    }
    parts
}

/// The tokens of documentation, without its code blocks (requirement R-SR-3).
fn prose_tokens(docs: &str) -> Vec<String> {
    let mut tokens_found = Vec::new();
    let mut in_code = false;
    for line in docs.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if !in_code {
            tokens_found.extend(tokens(line));
        }
    }
    tokens_found
}

/// How to search.
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions<'a> {
    /// What to look for.
    pub query: &'a str,
    /// Where to look.
    pub fields: Fields,
    /// The kinds to find, all of them if empty.
    pub kinds: &'a [&'a str],
    /// The most results in a page. Without it, the first page ends where the scores drop
    /// (requirement R-SR-4).
    pub limit: Option<usize>,
    /// Where the page starts.
    pub offset: usize,
    /// What a cursor for the next page starts with (requirement R-OUT-3).
    pub cursor_request: &'a str,
    /// The digest of the data that the cursor was made for, when there is one.
    pub cursor_data: Option<&'a str>,
    /// Which of the crates searched together the others' items are shown through (std, for
    /// alloc and core): at its public paths for them, and without its own re-exports of them
    /// (requirement R-PR-2).
    pub entry: Option<usize>,
}

/// The result of `search_items` (the common shape of §7.3).
#[derive(Debug, Serialize)]
pub struct SearchResponse<'a> {
    /// Where the data came from (requirement R-OUT-9).
    pub provenance: Provenance<'a>,
    /// What the caller should know about the data.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub warnings: Box<[Cow<'static, str>]>,
    /// What was found.
    pub data: SearchData<'a>,
    /// Which part of the results this is (requirement R-OUT-3).
    pub page: Page,
    /// What to call next.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub hints: Box<[Box<str>]>,
}

/// What a search found.
#[derive(Debug, Serialize)]
pub struct SearchData<'a> {
    /// The query, as given.
    pub query: Box<str>,
    /// The items found, best first, each at its canonical path.
    pub items: Vec<ItemData<'a>>,
    /// With no results, names of items that are spelt like the query (requirement R-SR-5).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub did_you_mean: Box<[Box<str>]>,
}

/// How a result matched, best first (requirement R-SR-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    /// The query is a path that the item's path ends with.
    Path,
    /// The query is the item's name.
    Name,
    /// The last word of the query is the item's name.
    LastWord,
    /// The query is a `doc(alias)` name of the item.
    Alias,
    /// Some tokens of the query are in the item.
    Partial,
}

/// One result, before it is shown.
struct Hit {
    /// Which crate of the search it is in.
    source: usize,
    entry: u32,
    tier: Tier,
    /// Its name is the query with the same case, which goes first in its tier.
    same_case: bool,
    score: f64,
}

/// Searches the crate of `view` (requirement R-SR-1).
///
/// # Errors
///
/// Returns an error when the cursor does not fit.
pub fn search_items<'a>(
    view: &'a CrateView<'a>,
    request: ItemRequest<'a>,
    options: SearchOptions<'_>,
) -> Result<SearchResponse<'a>, GetItemError> {
    let found = search_across(&[Searched { view, request }], options)?;
    let Found {
        mut provenances,
        warnings,
        hints,
        data,
        page,
    } = found;
    let Some(provenance) = provenances.pop() else {
        return Err(GetItemError::NotFound);
    };
    Ok(SearchResponse {
        provenance,
        warnings,
        data,
        page,
        hints,
    })
}

/// A crate to search, with the request that it is looked up with.
#[derive(Debug, Clone, Copy)]
pub struct Searched<'a> {
    /// The crate.
    pub view: &'a CrateView<'a>,
    /// Its name, version, origin and how much to show.
    pub request: ItemRequest<'a>,
}

/// What a search of one or more crates found.
#[derive(Debug)]
pub struct Found<'a> {
    /// The provenance of each crate, in the order given.
    pub provenances: Vec<Provenance<'a>>,
    /// What the caller should know about the data.
    pub warnings: Box<[Cow<'static, str>]>,
    /// What to call next.
    pub hints: Box<[Box<str>]>,
    /// What was found.
    pub data: SearchData<'a>,
    /// Which part of the results this is.
    pub page: Page,
}

/// Searches the crates of `searched` together (requirements R-SR-1 to R-SR-4): the statistics of
/// BM25 are over all of them, and their results are ranked as one.
///
/// # Errors
///
/// Returns an error when the cursor does not fit.
pub fn search_across<'a>(
    searched: &[Searched<'a>],
    options: SearchOptions<'_>,
) -> Result<Found<'a>, GetItemError> {
    let data_digest = check_cursor(searched, options)?;
    let indexes: Vec<&ArchivedSearchIndex> = searched
        .iter()
        .map(|source| source.view.search_index())
        .collect();
    let entry = options
        .entry
        .and_then(|entry| EntryCrate::new(searched, entry));
    let mut hits = hits(searched, &indexes, options);
    if let Some(entry) = &entry {
        hits.retain(|hit| !entry.hides(hit, &indexes));
    }
    let total = hits.len();
    if options.offset > total {
        return Err(GetItemError::BadCursor(
            "the cursor goes past the end of the results; search again without the cursor",
        ));
    }
    let shown = match (options.limit, options.offset) {
        (Some(limit), _) => limit,
        (None, 0) => first_page(&hits),
        (None, _) => DEFAULT_LIMIT,
    };
    let mut queries: Vec<Query<'a>> = searched
        .iter()
        .map(|source| {
            Query::new(source.view, ItemRequest {
                detail: match source.request.detail {
                    Detail::Brief => Detail::Brief,
                    Detail::Normal | Detail::Full => Detail::Normal,
                },
                ..source.request
            })
        })
        .collect();
    let provenances = queries.iter_mut().map(Query::provenance).collect();
    let items: Vec<ItemData<'a>> = hits
        .iter()
        .skip(options.offset)
        .take(shown)
        .filter_map(|hit| {
            let source = searched.get(hit.source)?;
            let query = queries.get_mut(hit.source)?;
            let mut item = result(
                source.view,
                query,
                source.view.search_index(),
                hit.entry,
                source.request.detail,
            )?;
            if let Some(entry) = &entry
                && hit.source != entry.source
                && let Some(found) = entry_at(source.view.search_index(), hit.entry)
            {
                entry.show(
                    &mut item,
                    source.view,
                    view::native(&found.id),
                    found.parent.as_ref().map(view::native),
                );
            }
            Some(item)
        })
        .collect();
    let did_you_mean = if total == 0 {
        did_you_mean(searched, options.query)
    } else {
        Box::default()
    };
    let Notes { warnings, hints } = notes(queries);
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
    let mut found = Found {
        provenances,
        warnings,
        hints,
        data: SearchData {
            query: options.query.into(),
            items: Vec::new(),
            did_you_mean,
        },
        page: page(0),
    };
    fit(&mut found, items, &page);
    Ok(found)
}

/// A crate searched together with crates whose items it re-exports, such as std with alloc and
/// core, and the paths at which it re-exports them (requirement R-PR-2).
struct EntryCrate<'a> {
    /// Which crate of the search it is.
    source: usize,
    view: &'a CrateView<'a>,
    /// The other crates, as code writes their names.
    others: Vec<&'a str>,
    /// The path in another crate of each item it re-exports (`alloc::vec`), and the shortest
    /// public path at which it does (`std::vec`).
    reexported: HashMap<Box<str>, Box<str>>,
}

impl<'a> EntryCrate<'a> {
    fn new(searched: &[Searched<'a>], source: usize) -> Option<Self> {
        let view = searched.get(source)?.view;
        let others: Vec<&str> = searched
            .iter()
            .enumerate()
            .filter(|&(at, _)| at != source)
            .map(|(_, other)| other.request.name.as_str())
            .collect();
        let mut entry = Self {
            source,
            view,
            others,
            reexported: HashMap::new(),
        };
        for reexport in view.index().external_reexports() {
            if let Some(definition) = &reexport.definition
                && is_of(view, reexport.target, &entry.others)
            {
                entry.add(definition.joined(), reexport.path.joined());
            }
        }
        // The others' re-exports of each other's items, at this crate's paths for them: std
        // re-exports `alloc::borrow`, which re-exports `core::borrow::Borrow`. Glob re-exports of
        // other crates' modules (`std::hash` of `core::hash`) are left out, as `get_item` does not
        // follow them (requirement R-PR-3): those items keep the paths of their crates.
        let mut reexported = Vec::new();
        for (at, other) in searched.iter().enumerate() {
            if at == source {
                continue;
            }
            for reexport in other.view.index().external_reexports() {
                if let Some(definition) = &reexport.definition
                    && is_of(other.view, reexport.target, &entry.others)
                    && let Some(path) = entry.path_of(&reexport.path.joined())
                {
                    reexported.push((definition.joined(), path));
                }
            }
        }
        for (definition, path) in reexported {
            entry.add(definition, path);
        }
        Some(entry)
    }

    /// Records that this crate re-exports the item at `definition` in another crate at `path`,
    /// unless it does at a shorter one.
    fn add(&mut self, definition: Box<str>, path: Box<str>) {
        if self
            .reexported
            .get(&definition)
            .is_none_or(|shortest| shorter(&path, shortest))
        {
            self.reexported.insert(definition, path);
        }
    }

    /// `hit` is this crate's re-export of an item of the others, which is found there.
    fn hides(&self, hit: &Hit, indexes: &[&ArchivedSearchIndex]) -> bool {
        hit.source == self.source
            && indexes
                .get(hit.source)
                .and_then(|index| entry_at(index, hit.entry))
                .is_some_and(|found| {
                    found.external && is_of(self.view, view::native(&found.id), &self.others)
                })
    }

    /// Shows `item` (`id`, a member of `parent` if it has one) of the crate of `view`, another
    /// one, at this crate's path for it. This crate re-exports items by their definition paths
    /// (`alloc::collections::vec_deque::VecDeque`), which may differ from the canonical paths
    /// there (`alloc::collections::VecDeque`), so both are tried: the path that is shortest once
    /// the rest is added wins, then the first in lexicographic order.
    fn show(&self, item: &mut ItemData<'_>, view: &CrateView<'_>, id: Id, parent: Option<Id>) {
        let definition = |id| view.summary(id).map(|summary| summary.path.join("::"));
        let name = item.path.rsplit("::").next().unwrap_or_default();
        let defined = parent.map_or_else(
            || definition(id),
            |parent| definition(parent).map(|parent| format!("{parent}::{name}")),
        );
        if let Some(path) = self.shown_at(&item.path, defined.as_deref()) {
            item.path = path;
        }
        if let Some(shown) = &mut item.parent
            && let Some(path) = self.shown_at(shown, parent.and_then(definition).as_deref())
        {
            *shown = path;
        }
    }

    /// This crate's path for the item at `canonical` (and `defined`) in another crate.
    fn shown_at(&self, canonical: &str, defined: Option<&str>) -> Option<Box<str>> {
        let by_definition = defined.and_then(|defined| self.path_of(defined));
        match (self.path_of(canonical), by_definition) {
            (Some(a), Some(b)) => Some(if shorter(&b, &a) { b } else { a }),
            (a, b) => a.or(b),
        }
    }

    /// `path`, of another crate, at this crate's path for it, if it re-exports a prefix of it.
    fn path_of(&self, path: &str) -> Option<Box<str>> {
        let mut best: Option<String> = None;
        let mut end = path.len();
        loop {
            let (prefix, rest) = (path.get(..end)?, path.get(end..)?);
            if let Some(there) = self.reexported.get(prefix) {
                let shown = format!("{there}{rest}");
                if best.as_ref().is_none_or(|best| shorter(&shown, best)) {
                    best = Some(shown);
                }
            }
            match prefix.rfind("::") {
                Some(at) => end = at,
                None => return best.map(String::into_boxed_str),
            }
        }
    }
}

/// The item `id`, which is not in the crate of `view`, is of one of `crates`.
fn is_of(view: &CrateView<'_>, id: Id, crates: &[&str]) -> bool {
    view.summary(id)
        .and_then(|summary| view.external_crate(summary.crate_id))
        .is_some_and(|krate| crates.contains(&krate.name.as_str()))
}

/// `a` has fewer segments than `b`, or as many and comes first.
fn shorter(a: &str, b: &str) -> bool {
    (a.matches("::").count(), a) < (b.matches("::").count(), b)
}

/// The hits in all the crates, of the kinds asked for, best first.
fn hits(
    searched: &[Searched<'_>],
    indexes: &[&ArchivedSearchIndex],
    options: SearchOptions<'_>,
) -> Vec<Hit> {
    let stats = Stats::of(indexes, options.query);
    let mut hits: Vec<Hit> = indexes
        .iter()
        .enumerate()
        .flat_map(|(source, index)| {
            let mut hits = rank(index, options, &stats);
            for hit in &mut hits {
                hit.source = source;
            }
            hits
        })
        .collect();
    hits.retain(|hit| {
        searched.get(hit.source).is_some_and(|source| {
            shows(source, hit.entry)
                && (options.kinds.is_empty()
                    || entry_kind(source.view, source.view.search_index(), hit.entry)
                        .is_some_and(|kind| options.kinds.contains(&kind)))
        })
    });
    let path_of = |hit: &Hit| -> &str {
        indexes
            .get(hit.source)
            .and_then(|index| entry_at(index, hit.entry))
            .map_or("", |found| found.path.as_ref())
    };
    hits.sort_by(|a, b| {
        (
            a.tier,
            !a.same_case,
            Reverse(quantized(a.score)),
            path_of(a),
        )
            .cmp(&(
                b.tier,
                !b.same_case,
                Reverse(quantized(b.score)),
                path_of(b),
            ))
    });
    hits
}

/// Whether the search of `source` shows its entry `entry`: one that is not public or is hidden only
/// in a workspace member (requirements R-SR-4 and R-SH-5, decided in M2's D6).
fn shows(source: &Searched<'_>, entry: u32) -> bool {
    matches!(source.request.origin, Origin::Local { .. })
        || entry_at(source.view.search_index(), entry).is_some_and(|found| !found.restricted)
}

/// The warnings and hints of all the queries, in order.
fn notes(queries: Vec<Query<'_>>) -> Notes {
    let (mut warnings, mut hints) = (Vec::new(), Vec::new());
    for query in queries {
        let notes = query.finish();
        warnings.extend(notes.warnings);
        hints.extend(notes.hints);
    }
    Notes {
        warnings: warnings.into(),
        hints: hints.into(),
    }
}

/// The part of the data's digest that cursors carry, once the cursor in `options`, if any, is
/// found to be for this data: that of the one crate, or a digest of those of all.
fn check_cursor(
    searched: &[Searched<'_>],
    options: SearchOptions<'_>,
) -> Result<String, GetItemError> {
    let digest = if let [one] = searched {
        one.view.source_digest().to_owned()
    } else {
        let joined: Vec<&str> = searched
            .iter()
            .map(|source| source.view.source_digest())
            .collect();
        crate::data::cache::sha256_hex(joined.join(".").as_bytes())
    };
    let digest = digest.get(..16).unwrap_or(&digest).to_owned();
    match options.cursor_data {
        Some(cursor_data) if cursor_data != digest => Err(GetItemError::BadCursor(
            "the data changed since the cursor was made; search again without the cursor",
        )),
        Some(_) | None => Ok(digest),
    }
}

/// The statistics of BM25 over the crates searched (requirement R-SR-3): how many items there
/// are, and how many have each token of the query.
struct Stats {
    total: f64,
    df: HashMap<String, f64>,
}

impl Stats {
    fn of(indexes: &[&ArchivedSearchIndex], query: &str) -> Self {
        let count = |n: usize| f64::from(u32::try_from(n).unwrap_or(u32::MAX));
        let total = indexes.iter().map(|index| count(index.entries.len())).sum();
        let df = tokens(query)
            .into_iter()
            .map(|token| {
                let df = indexes
                    .iter()
                    .filter_map(|index| index.postings.get(token.as_str()))
                    .map(|postings| count(postings.len()))
                    .sum();
                (token, df)
            })
            .collect();
        Self { total, df }
    }
}

/// The BM25 score of each item that has a token of the query, in the fields asked for
/// (requirement R-SR-3).
fn scores(
    index: &ArchivedSearchIndex,
    options: SearchOptions<'_>,
    stats: &Stats,
) -> HashMap<u32, f64> {
    let fields = options.fields;
    let query = options.query.trim();
    let query_tokens: Vec<String> = {
        let mut seen = HashSet::new();
        tokens(query)
            .into_iter()
            .filter(|token| seen.insert(token.clone()))
            .collect()
    };
    let mut scores: HashMap<u32, f64> = HashMap::new();
    for token in &query_tokens {
        let Some(postings) = index.postings.get(token.as_str()) else {
            continue;
        };
        let df = stats.df.get(token).copied().unwrap_or_default();
        let idf = ((stats.total - df + 0.5) / (df + 0.5)).ln_1p();
        for posting in postings.iter() {
            let field = |on: bool, count: u16, weight: f64| {
                if on { f64::from(count) * weight } else { 0.0 }
            };
            let weighted = field(fields.name, posting.name.to_native(), NAME_WEIGHT)
                + field(fields.alias, posting.alias.to_native(), ALIAS_WEIGHT)
                + field(fields.docs, posting.first.to_native(), FIRST_WEIGHT)
                + field(fields.docs, posting.rest.to_native(), 1.0)
                + field(fields.path, posting.path.to_native(), 1.0);
            if weighted > 0.0 {
                let score = scores.entry(posting.entry.to_native()).or_default();
                *score += idf * weighted * (K1 + 1.0) / (weighted + K1);
            }
        }
    }
    scores
}

/// How each item matches the query, for those that match better than by some tokens
/// (requirement R-SR-3).
fn tiers(index: &ArchivedSearchIndex, options: SearchOptions<'_>) -> HashMap<u32, Tier> {
    let fields = options.fields;
    let lower = options.query.trim().to_lowercase();
    let by_name = |name: &str| -> Vec<u32> {
        index
            .names
            .get(name)
            .map(|entries| entries.iter().map(|entry| entry.to_native()).collect())
            .unwrap_or_default()
    };
    let mut tiers: HashMap<u32, Tier> = HashMap::new();
    let mut set = |entries: Vec<u32>, tier: Tier| {
        for entry in entries {
            let best = tiers.entry(entry).or_insert(tier);
            *best = (*best).min(tier);
        }
    };
    let words: Vec<&str> = lower.split_whitespace().collect();
    if fields.name {
        if lower.contains("::") {
            let last = lower.rsplit("::").next().unwrap_or(&lower);
            let wanted = format!("::{lower}");
            let ends = by_name(last)
                .into_iter()
                .filter(|&entry| {
                    entry_at(index, entry).is_some_and(|found| {
                        let path = found.path.to_lowercase();
                        path == lower || path.ends_with(&wanted)
                    })
                })
                .collect();
            set(ends, Tier::Path);
        }
        set(by_name(&lower), Tier::Name);
        // Words that make a name together: `unwrap or default`, `hash map`.
        if let [_, _, ..] = words.as_slice() {
            set(by_name(&words.join("_")), Tier::Name);
            set(by_name(&words.concat()), Tier::Name);
        }
        if let [_, .., last] = words.as_slice() {
            set(by_name(last), Tier::LastWord);
        }
    }
    if fields.alias {
        let aliases = index
            .aliases
            .get(lower.as_str())
            .map(|entries| entries.iter().map(|entry| entry.to_native()).collect())
            .unwrap_or_default();
        set(aliases, Tier::Alias);
    }
    tiers
}

/// The hits for the query, best first: by tier, then by score quantized, then by path, so that
/// the order does not depend on rounding (requirement R-OUT-4).
fn rank(index: &ArchivedSearchIndex, options: SearchOptions<'_>, stats: &Stats) -> Vec<Hit> {
    let scores = scores(index, options, stats);
    let tiers = tiers(index, options);
    let mut candidates: HashSet<u32> = scores.keys().copied().collect();
    candidates.extend(tiers.keys().copied());
    let mut hits: Vec<Hit> = candidates
        .into_iter()
        .filter_map(|entry| {
            let found = entry_at(index, entry)?;
            let authority = f64::from(found.referenced.to_native())
                .ln_1p()
                .mul_add(0.1, 1.0);
            let penalty = if found.penalized { 0.5 } else { 1.0 };
            let score = scores.get(&entry).copied().unwrap_or_default() * authority * penalty;
            let name = found.path.rsplit("::").next().unwrap_or_default();
            Some(Hit {
                source: 0,
                entry,
                tier: tiers.get(&entry).copied().unwrap_or(Tier::Partial),
                same_case: name == options.query.trim(),
                score,
            })
        })
        .collect();
    let path_of =
        |entry: u32| -> &str { entry_at(index, entry).map_or("", |found| found.path.as_ref()) };
    hits.sort_by(|a, b| {
        (
            a.tier,
            !a.same_case,
            Reverse(quantized(a.score)),
            path_of(a.entry),
        )
            .cmp(&(
                b.tier,
                !b.same_case,
                Reverse(quantized(b.score)),
                path_of(b.entry),
            ))
    });
    hits
}

/// The entry `entry` of `index`.
fn entry_at(index: &ArchivedSearchIndex, entry: u32) -> Option<&ArchivedEntry> {
    index.entries.get().get(usize::try_from(entry).ok()?)
}

/// A score rounded to keep the order stable where floating point differs in the last digits.
fn quantized(score: f64) -> i64 {
    // Scores are small positive numbers; the cast saturates on what is out of range.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a score fits in i64 after rounding, and saturates otherwise"
    )]
    let quantized = (score * 1000.0).round() as i64;
    quantized
}

/// How many hits the first page has when the scores decide (requirement R-SR-4): every hit
/// better than a partial match, then partial matches until one scores less than 5% of the best
/// one, or less than half of the one before, or the ones already taken have more than 30% of the
/// total score of all. At least one, at most 50.
fn first_page(hits: &[Hit]) -> usize {
    let exact = hits
        .iter()
        .take_while(|hit| hit.tier != Tier::Partial)
        .count();
    let partial = hits.get(exact..).unwrap_or_default();
    let Some(best) = partial.first().map(|hit| hit.score) else {
        return exact.clamp(1, MAX_FIRST_PAGE);
    };
    let sum: f64 = partial.iter().map(|hit| hit.score).sum();
    let mut taken = 0.0;
    let mut kept: usize = 0;
    let mut previous = best;
    for hit in partial {
        if kept > 0 && (hit.score < best * 0.05 || hit.score < previous * 0.5 || taken > sum * 0.3)
        {
            break;
        }
        taken += hit.score;
        previous = hit.score;
        kept = kept.saturating_add(1);
    }
    exact.saturating_add(kept).clamp(1, MAX_FIRST_PAGE)
}

/// The kind of what an entry is.
fn entry_kind(
    view: &CrateView<'_>,
    index: &ArchivedSearchIndex,
    entry: u32,
) -> Option<&'static str> {
    let found = entry_at(index, entry)?;
    let id = view::native(&found.id);
    if found.external {
        return view
            .summary(id)
            .map(|summary| super::get_item::summary_kind(summary.kind));
    }
    view.item(id)
        .map(|item| kind_of(item, found.parent.as_ref().is_some()))
}

/// One result as the response shows it: at its canonical path, with the first sentence of its
/// documentation with `detail=brief`.
fn result<'a>(
    view: &'a CrateView<'a>,
    query: &mut Query<'a>,
    index: &ArchivedSearchIndex,
    entry: u32,
    detail: Detail,
) -> Option<ItemData<'a>> {
    let found = entry_at(index, entry)?;
    let id = view::native(&found.id);
    if found.external {
        let reexport = ExternalReexport {
            path: PublicPath::parse(&found.path),
            target: id,
            definition: view
                .summary(id)
                .map(|summary| PublicPath::parse(&summary.path.join("::"))),
        };
        return Some(ItemData {
            path: found.path.as_ref().into(),
            kind: query.external_kind(id),
            reexport_of: query.reexport_target(&reexport),
            ..ItemData::default()
        });
    }
    let parent = found.parent.as_ref().map(view::native);
    let mut data = query.item_data(id, parent)?.listed();
    if detail == Detail::Brief {
        data.docs = view
            .item(id)
            .and_then(|item| item.docs.as_deref())
            .map(first_sentence);
    } else {
        data.fields = Box::default();
        data.variants = Box::default();
        data.sections = Box::default();
        data.also_reexported_at = Box::default();
        data.source_location = None;
    }
    Some(data)
}

/// The first sentence of `docs`.
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

/// Names of items spelt like `query`, when nothing matched (requirement R-SR-5): at most 5, by
/// edit distance in characters, then by name, and only those similar enough. Better none than
/// unrelated ones (requirement R-PR-7).
fn did_you_mean(searched: &[Searched<'_>], query: &str) -> Box<[Box<str>]> {
    let wanted = query
        .trim()
        .rsplit("::")
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let wanted_len = wanted.chars().count();
    if wanted_len < 3 {
        return Box::default();
    }
    let mut close: Vec<(usize, &str, &ArchivedEntry)> = searched
        .iter()
        .flat_map(|source| {
            let (index, wanted) = (source.view.search_index(), wanted.as_str());
            index.names.iter().filter_map(move |(name, entries)| {
                let distance = edit_distance(wanted, name);
                let longer = wanted_len.max(name.chars().count());
                // At least 70% alike, and at most 3 edits.
                if distance > 3 || distance.saturating_mul(10) > longer.saturating_mul(3) {
                    return None;
                }
                // Of the items with the name, one that is not a member, with the shortest path.
                let best = entries
                    .iter()
                    .filter(|entry| shows(source, entry.to_native()))
                    .filter_map(|entry| entry_at(index, entry.to_native()))
                    .min_by_key(|found| {
                        (
                            found.parent.is_some(),
                            found.path.len(),
                            found.path.as_ref(),
                        )
                    })?;
                Some((distance, name.as_ref(), best))
            })
        })
        .collect();
    close
        .sort_unstable_by(|a, b| (a.0, a.1, a.2.path.as_ref()).cmp(&(b.0, b.1, b.2.path.as_ref())));
    close
        .into_iter()
        .take(5)
        .map(|(_, _, found)| found.path.as_ref().into())
        .collect()
}

/// The edit distance between `a` and `b`, counted in characters (requirement R-PR-7).
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, a_char) in a.chars().enumerate() {
        let mut previous = row.first().copied().unwrap_or_default();
        if let Some(first) = row.first_mut() {
            *first = i.saturating_add(1);
        }
        for (j, &b_char) in b.iter().enumerate() {
            let above = row.get(j.saturating_add(1)).copied().unwrap_or_default();
            let left = row.get(j).copied().unwrap_or_default();
            let cost = usize::from(a_char != b_char);
            let value = (above.saturating_add(1))
                .min(left.saturating_add(1))
                .min(previous.saturating_add(cost));
            previous = above;
            if let Some(cell) = row.get_mut(j.saturating_add(1)) {
                *cell = value;
            }
        }
    }
    row.last().copied().unwrap_or_default()
}

/// Puts as many of `items` into `found` as fit in the budget, and at least one
/// (requirement R-OUT-3).
fn fit<'a>(found: &mut Found<'a>, mut items: Vec<ItemData<'a>>, page: &impl Fn(usize) -> Page) {
    let mut shown = items.len();
    loop {
        found.data.items = items;
        found.page = page(shown);
        let parts = (
            &found.provenances,
            &found.warnings,
            &found.data,
            &found.page,
            &found.hints,
        );
        let size = serde_json::to_string(&parts).map_or(0, |json| json.chars().count());
        if size <= MAX_RESPONSE_CHARS || shown <= 1 {
            return;
        }
        items = std::mem::take(&mut found.data.items);
        shown = shown.saturating_sub(shown.div_ceil(4).max(1));
        items.truncate(shown);
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
            across::Indexed,
            get_item::{ResolvedFrom, tests::parsed},
        },
    };

    #[test]
    fn names_are_split_at_case_and_underscores() {
        assert_eq!(tokens("HTTPServer"), ["http", "server", "httpserver"]);
        assert_eq!(tokens("spawn_blocking"), [
            "spawn",
            "blocking",
            "spawn_blocking"
        ]);
        assert_eq!(tokens("Mutex"), ["mutex"]);
        assert_eq!(tokens("a Vec<u8>"), ["vec", "u8"]);
    }

    /// The paths that `search_items` finds for `query` in the test crate of `get_item`, after
    /// `change`, and what it suggests when it finds none.
    fn found(query: &str, change: impl FnOnce(&mut Crate)) -> (Vec<String>, serde_json::Value) {
        found_in(
            Origin::DocsRs {
                resolved_from: ResolvedFrom::Explicit,
                project_from: None,
                build: None,
            },
            query,
            change,
        )
    }

    /// [`found`], of a crate from `origin`.
    fn found_in(
        origin: Origin<'_>,
        query: &str,
        change: impl FnOnce(&mut Crate),
    ) -> (Vec<String>, serde_json::Value) {
        let mut parsed = parsed();
        change(&mut parsed.krate);
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&Indexed::new(parsed)).expect("archived");
        let view = CrateView::new(
            rkyv::access::<crate::query::across::ArchivedIndexed, rkyv::rancor::Error>(&bytes)
                .expect("checked"),
        );
        let name: CrateName = "my-crate".parse().expect("a crate name");
        let version = Version::new(1, 2, 3);
        let response = search_items(
            &view,
            ItemRequest {
                name: &name,
                version: &version,
                path: "my_crate",
                origin,
                detail: Detail::Brief,
                project_features: None,
                follow: false,
                target: None,
            },
            SearchOptions {
                query,
                fields: Fields::ALL,
                kinds: &[],
                limit: Some(20),
                offset: 0,
                cursor_request: "c1.req",
                cursor_data: None,
                entry: None,
            },
        )
        .expect("searched");
        let response = serde_json::to_value(response).expect("serializes");
        let paths = response["data"]["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["path"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        (paths, response["data"]["did_you_mean"].clone())
    }

    #[test]
    fn what_is_not_public_or_hidden_is_found_only_in_a_workspace_member() {
        // A build with private items, in which `lock` is the crate's, and `Mutex` is hidden.
        let change = |krate: &mut Crate| {
            krate.includes_private = true;
            krate
                .index
                .values_mut()
                .for_each(|item| match item.name.as_deref() {
                    Some("lock") => item.visibility = Visibility::Crate,
                    Some("Mutex") => item.attrs.push(Attribute::Other("#[doc(hidden)]".into())),
                    _ => {},
                });
        };
        let (paths, _) = found("lock", change);
        assert!(
            !paths.iter().any(|path| path.ends_with("::lock")),
            "{paths:?}"
        );
        let (paths, _) = found("Mutex", change);
        assert!(
            !paths.iter().any(|path| path.ends_with("::Mutex")),
            "{paths:?}"
        );
        let member = Origin::Local {
            resolved_from: ResolvedFrom::Explicit,
            project_from: None,
            toolchain: None,
            built_at: "2026-10-21T00:00:00Z",
            features_requested: None,
            features_effective: None,
        };
        let (paths, _) = found_in(member, "lock", change);
        assert_eq!(
            paths.first().map(String::as_str),
            Some("my_crate::Mutex::lock")
        );
    }

    #[test]
    fn a_name_is_found_before_what_merely_mentions_it() {
        let (paths, _) = found("Mutex", |_| {});
        assert_eq!(paths.first().map(String::as_str), Some("my_crate::Mutex"));
        // A member is found at its parent's path.
        let (paths, _) = found("lock", |_| {});
        assert_eq!(
            paths.first().map(String::as_str),
            Some("my_crate::Mutex::lock")
        );
        // So is a re-export of another crate's item, by its name.
        let (paths, _) = found("thing", |_| {});
        assert_eq!(paths, ["my_crate::Thing"]);
    }

    #[test]
    fn words_that_make_a_name_find_it() {
        let (paths, _) = found("mu tex", |_| {});
        assert_eq!(paths.first().map(String::as_str), Some("my_crate::Mutex"));
    }

    #[test]
    fn an_alias_finds_its_item() {
        let (paths, _) = found("padlock", |krate| {
            if let Some(mutex) = krate.index.get_mut(&Id(3)) {
                mutex
                    .attrs
                    .push(Attribute::Other(r#"#[doc(alias = "padlock")]"#.to_owned()));
            }
        });
        assert_eq!(paths, ["my_crate::Mutex"]);
    }

    #[test]
    fn nothing_found_suggests_similar_names() {
        let (paths, suggested) = found("Mutx", |_| {});
        assert!(paths.is_empty(), "{paths:?}");
        assert_eq!(suggested, json!(["my_crate::Mutex"]));
        // Nothing unrelated is suggested.
        let (_, suggested) = found("Elephant", |_| {});
        assert_eq!(suggested, json!(null));
    }

    #[test]
    fn the_first_page_ends_where_the_scores_drop() {
        let hit = |tier, score| Hit {
            source: 0,
            entry: 0,
            tier,
            same_case: false,
            score,
        };
        let hits = [
            hit(Tier::Name, 1.0),
            hit(Tier::Partial, 10.0),
            hit(Tier::Partial, 9.0),
            hit(Tier::Partial, 3.0),
            hit(Tier::Partial, 2.9),
        ];
        // The exact match, then 10, which alone is over 30% of the 24.9 of all.
        assert_eq!(first_page(&hits), 2);
        // Without a dominant one, until the ones taken have more than 30% of all.
        let even = [
            hit(Tier::Partial, 4.0),
            hit(Tier::Partial, 3.9),
            hit(Tier::Partial, 3.8),
            hit(Tier::Partial, 1.0),
            hit(Tier::Partial, 0.9),
            hit(Tier::Partial, 0.9),
            hit(Tier::Partial, 0.9),
            hit(Tier::Partial, 0.9),
            hit(Tier::Partial, 0.9),
            hit(Tier::Partial, 0.9),
        ];
        assert_eq!(first_page(&even), 2);
        // And it stops where a score is less than half of the one before.
        let drop = [hit(Tier::Partial, 1.0), hit(Tier::Partial, 0.4)];
        assert_eq!(first_page(&drop), 1);
        assert_eq!(first_page(&[]), 1);
    }

    #[test]
    fn crates_searched_together_are_ranked_as_one() {
        let archive = |name: &str| {
            let mut parsed = parsed();
            if let Some(root) = parsed.krate.index.get_mut(&Id(0)) {
                root.name = Some(name.to_owned());
            }
            rkyv::to_bytes::<rkyv::rancor::Error>(&Indexed::new(parsed)).expect("archived")
        };
        let (first, second) = (archive("first"), archive("second"));
        let access = |bytes| {
            CrateView::new(
                rkyv::access::<crate::query::across::ArchivedIndexed, rkyv::rancor::Error>(bytes)
                    .expect("checked"),
            )
        };
        let views = [access(&first), access(&second)];
        let names: [CrateName; 2] = [
            "first".parse().expect("a name"),
            "second".parse().expect("a name"),
        ];
        let version = Version::new(1, 2, 3);
        let searched: Vec<Searched<'_>> = views
            .iter()
            .zip(&names)
            .map(|(view, name)| Searched {
                view,
                request: ItemRequest {
                    name,
                    version: &version,
                    path: name.as_str(),
                    origin: Origin::DocsRs {
                        resolved_from: ResolvedFrom::CargoLock,
                        project_from: None,
                        build: None,
                    },
                    detail: Detail::Brief,
                    project_features: None,
                    follow: false,
                    target: None,
                },
            })
            .collect();
        let found = search_across(&searched, SearchOptions {
            query: "Mutex",
            fields: Fields::ALL,
            kinds: &["struct"],
            limit: None,
            offset: 0,
            cursor_request: "c1.req",
            cursor_data: None,
            entry: None,
        })
        .expect("searched");
        let paths: Vec<&str> = found.data.items.iter().map(|item| &*item.path).collect();
        assert_eq!(paths, ["first::Mutex", "second::Mutex"]);
        assert_eq!(found.provenances.len(), 2);
    }

    #[test]
    fn items_reexported_by_the_entry_crate_are_shown_at_its_paths() {
        // `first` re-exports `Mutex` of `second` as `first::Thing`, by its definition path, which
        // is not its canonical path there (as std re-exports
        // `alloc::collections::vec_deque::VecDeque`).
        let defined = || {
            ["second", "sync", "mutex", "Mutex"]
                .map(ToOwned::to_owned)
                .to_vec()
        };
        let archive = |name: &str| {
            let mut parsed = parsed();
            let krate = &mut parsed.krate;
            if let Some(root) = krate.index.get_mut(&Id(0)) {
                root.name = Some(name.to_owned());
            }
            if name == "first" {
                if let Some(thing) = krate.paths.get_mut(&Id(20)) {
                    thing.path = defined();
                    thing.kind = rustdoc_types::ItemKind::Struct;
                }
                if let Some(other) = krate.external_crates.get_mut(&1) {
                    "second".clone_into(&mut other.name);
                }
            } else if let Some(mutex) = krate.paths.get_mut(&Id(3)) {
                mutex.path = defined();
            }
            rkyv::to_bytes::<rkyv::rancor::Error>(&Indexed::new(parsed)).expect("archived")
        };
        let (first, second) = (archive("first"), archive("second"));
        let access = |bytes| {
            CrateView::new(
                rkyv::access::<crate::query::across::ArchivedIndexed, rkyv::rancor::Error>(bytes)
                    .expect("checked"),
            )
        };
        let views = [access(&first), access(&second)];
        let names: [CrateName; 2] = [
            "first".parse().expect("a name"),
            "second".parse().expect("a name"),
        ];
        let version = Version::new(1, 2, 3);
        let searched: Vec<Searched<'_>> = views
            .iter()
            .zip(&names)
            .map(|(view, name)| Searched {
                view,
                request: ItemRequest {
                    name,
                    version: &version,
                    path: name.as_str(),
                    origin: Origin::DocsRs {
                        resolved_from: ResolvedFrom::CargoLock,
                        project_from: None,
                        build: None,
                    },
                    detail: Detail::Brief,
                    project_features: None,
                    follow: false,
                    target: None,
                },
            })
            .collect();
        let paths = |query, entry| {
            let found = search_across(&searched, SearchOptions {
                query,
                fields: Fields::ALL,
                kinds: &[],
                limit: Some(10),
                offset: 0,
                cursor_request: "c1.req",
                cursor_data: None,
                entry,
            })
            .expect("searched");
            found
                .data
                .items
                .iter()
                .map(|item| {
                    (
                        item.path.to_string(),
                        item.parent.as_deref().map(str::to_owned),
                    )
                })
                .collect::<Vec<_>>()
        };
        let at = |path: &str, parent: Option<&str>| (path.to_owned(), parent.map(str::to_owned));
        assert!(paths("lock", Some(0)).contains(&at("first::Thing::lock", Some("first::Thing"))));
        assert!(paths("lock", None).contains(&at("second::Mutex::lock", Some("second::Mutex"))));
        // The re-export itself is left out: the item is found in its crate.
        assert!(paths("Thing", None).contains(&at("first::Thing", None)));
        assert!(!paths("Thing", Some(0)).contains(&at("first::Thing", None)));
    }
}
