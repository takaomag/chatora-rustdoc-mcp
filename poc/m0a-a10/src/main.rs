//! A10 (`doc/m0-plan.md`): compares the public paths that chatora-rustdoc-mcp's index finds with
//! those of rustdoc_processor 0.1.2, on the same docs.rs rustdoc JSON.
//!
//! rustdoc_processor reads rustdoc-types 0.57 (format 57). The JSON is parsed with this server's
//! parser (0.61 types), written back as JSON with `format_version` 57, and read with the 0.57
//! types. Between formats 57 and 61 only stability fields changed, which the 0.57 types ignore,
//! so the modules and `use` items that paths come from are the same.
//!
//! Usage: `cargo run --release -- <crate>@<version> ...` (the JSON must be in the cache; fetch it
//! with `cargo run --example fetch_docs_rs` in the repository root).

use std::collections::{BTreeMap, BTreeSet};

use chatora_rustdoc_mcp::{
    data::{cache::Cache, crate_name::CrateName, rustdoc_json},
    query::path_index::PathIndex,
};
use rustdoc_processor::{GlobalItemId, crate_data::CrateData, queries::Crate as Indexed};
use rustdoc_types::{Id, ItemEnum};
use semver::Version;

/// How many examples to print for each kind of difference.
const EXAMPLES: usize = 10;

fn main() {
    let releases: Vec<String> = std::env::args().skip(1).collect();
    if releases.is_empty() {
        eprintln!("usage: m0a-a10 <crate>@<version> ...");
        std::process::exit(2);
    }
    let cache = Cache::open_default().expect("the cache directory is known");
    for release in &releases {
        compare(&cache, release);
    }
}

/// Compares the two indexes of one release and prints the result.
fn compare(cache: &Cache, release: &str) {
    let (name, version) = release.split_once('@').expect("crate@version");
    let name: CrateName = name.parse().expect("a crate name");
    let version = Version::parse(version).expect("a version");
    let json = cache
        .read_docs_rs_json(&name, &version)
        .expect("the cache is readable")
        .unwrap_or_else(|| panic!("{release} is not in the cache"));
    let krate = rustdoc_json::parse(&json).expect("parses").krate;

    // This server's index.
    let ours = PathIndex::build(&krate);

    // rustdoc_processor's index, from the same crate written as format 57.
    let mut value = serde_json::to_value(&krate).expect("serializes");
    value["format_version"] = 57.into();
    let old: rustdoc_types_057::Crate = serde_json::from_value(value).expect("reads as format 57");
    let package_id = guppy::PackageId::new(release);
    let theirs = Indexed::index_without_visitor(
        CrateData {
            root_item_id: old.root,
            external_crates: old.external_crates,
            paths: rustdoc_processor::crate_data::CrateItemPaths::Eager(
                rustdoc_processor::crate_data::EagerCrateItemPaths { paths: old.paths },
            ),
            format_version: old.format_version,
            index: rustdoc_processor::crate_data::CrateItemIndex::Eager(
                rustdoc_processor::crate_data::EagerCrateItemIndex { index: old.index },
            ),
        },
        package_id.clone(),
    );

    // The public paths of each item in rustdoc_processor's index (modules and items), and its
    // canonical path: the one rustdoc_processor picks for an item, and for a module the shortest,
    // then the first in lexicographic order.
    let mut their_paths: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    let mut their_canonical: BTreeMap<u32, String> = BTreeMap::new();
    for (id, entry) in theirs
        .import_index
        .modules
        .iter()
        .chain(theirs.import_index.items.iter())
    {
        if entry.public_paths.is_empty() {
            continue;
        }
        their_paths.insert(
            id.0,
            entry
                .public_paths
                .iter()
                .map(|path| path.0.join("::"))
                .collect(),
        );
    }
    for id in their_paths.keys() {
        if theirs
            .import_index
            .items
            .contains_key(&rustdoc_types_057::Id(*id))
        {
            let global = GlobalItemId::new(rustdoc_types_057::Id(*id), package_id.clone());
            if let Ok(path) = theirs.get_canonical_path(&global) {
                their_canonical.insert(*id, path.join("::"));
            }
        } else if let Some(first) = their_paths[id]
            .iter()
            .min_by(|a, b| (a.matches("::").count(), *a).cmp(&(b.matches("::").count(), *b)))
        {
            their_canonical.insert(*id, first.clone());
        }
    }

    // This server's public paths of every item that has one.
    let our_paths: BTreeMap<u32, BTreeSet<String>> = krate
        .index
        .keys()
        .filter_map(|&id| {
            let paths = ours.public_paths(id);
            (!paths.is_empty()).then(|| (id.0, paths.iter().map(ToString::to_string).collect()))
        })
        .collect();

    let kind = |id: u32| {
        krate
            .index
            .get(&Id(id))
            .map_or("missing", |item| kind_name(&item.inner))
    };
    let name_of = |id: u32| {
        krate
            .index
            .get(&Id(id))
            .and_then(|item| item.name.clone())
            .unwrap_or_default()
    };

    let mut same = 0;
    let mut same_canonical_other_paths = Vec::new();
    let mut canonical_rename = Vec::new();
    let mut canonical_other = Vec::new();
    let mut only_ours: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut only_theirs: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (id, our_set) in &our_paths {
        let Some(their_set) = their_paths.get(id) else {
            only_ours.entry(kind(*id)).or_default().push(
                ours.canonical_path(Id(*id))
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            );
            continue;
        };
        let our_canonical = ours
            .canonical_path(Id(*id))
            .map(ToString::to_string)
            .unwrap_or_default();
        let their = their_canonical.get(id).cloned().unwrap_or_default();
        if our_canonical != their {
            let line = format!(
                "{} {}: ours {our_canonical} of {our_set:?}, theirs {their} of {their_set:?}",
                kind(*id),
                id
            );
            // This server avoids paths that rename the item; rustdoc_processor takes the
            // shortest.
            if their.rsplit("::").next() != Some(name_of(*id).as_str()) {
                canonical_rename.push(line);
            } else {
                canonical_other.push(line);
            }
        } else if our_set != their_set {
            same_canonical_other_paths.push(format!(
                "{} {}: only ours {:?}, only theirs {:?}",
                kind(*id),
                our_canonical,
                our_set.difference(their_set).collect::<Vec<_>>(),
                their_set.difference(our_set).collect::<Vec<_>>()
            ));
        } else {
            same += 1;
        }
    }
    for (id, their_set) in &their_paths {
        if !our_paths.contains_key(id) {
            only_theirs
                .entry(kind(*id))
                .or_default()
                .push(their_set.iter().next().cloned().unwrap_or_default());
        }
    }

    println!("## {release}");
    println!(
        "items with public paths: ours {}, theirs {}",
        our_paths.len(),
        their_paths.len()
    );
    println!("same canonical path and same set of public paths: {same}");
    print_list(
        "same canonical path, different sets of public paths",
        &same_canonical_other_paths,
    );
    print_list(
        "different canonical path, theirs renames the item",
        &canonical_rename,
    );
    print_list("different canonical path, otherwise", &canonical_other);
    for (kind, paths) in &only_ours {
        print_list(&format!("only ours: {kind}"), paths);
    }
    for (kind, paths) in &only_theirs {
        print_list(&format!("only theirs: {kind}"), paths);
    }
    println!();
}

fn print_list(title: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    println!("{title}: {}", lines.len());
    for line in lines.iter().take(EXAMPLES) {
        println!("  {line}");
    }
    if lines.len() > EXAMPLES {
        println!("  ... and {} more", lines.len() - EXAMPLES);
    }
}

fn kind_name(inner: &ItemEnum) -> &'static str {
    match inner {
        ItemEnum::Module(_) => "module",
        ItemEnum::ExternCrate { .. } => "extern_crate",
        ItemEnum::Use(_) => "use",
        ItemEnum::Union(_) => "union",
        ItemEnum::Struct(_) => "struct",
        ItemEnum::StructField(_) => "struct_field",
        ItemEnum::Enum(_) => "enum",
        ItemEnum::Variant(_) => "variant",
        ItemEnum::Function(_) => "function",
        ItemEnum::Trait(_) => "trait",
        ItemEnum::TraitAlias(_) => "trait_alias",
        ItemEnum::Impl(_) => "impl",
        ItemEnum::TypeAlias(_) => "type_alias",
        ItemEnum::Constant { .. } => "constant",
        ItemEnum::Static(_) => "static",
        ItemEnum::ExternType => "extern_type",
        ItemEnum::Macro(_) => "macro",
        ItemEnum::ProcMacro(_) => "proc_macro",
        ItemEnum::Primitive(_) => "primitive",
        ItemEnum::AssocConst { .. } => "assoc_const",
        ItemEnum::AssocType { .. } => "assoc_type",
    }
}
