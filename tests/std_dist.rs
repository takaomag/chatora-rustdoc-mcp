//! Looks std up in the rustdoc JSON that the Rust distribution server publishes (requirements
//! R-STD-1, R-STD-2 and R-STD-6).
//!
//! It fetches archives of about 4 MB each from static.rust-lang.org into the cache, and one test
//! needs a nightly toolchain with the `rust-docs-json` component, so they are ignored by default.
//! Run them with `cargo test --release --test std_dist -- --ignored --nocapture`.

#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

use std::{collections::HashMap, sync::Arc, time::Instant};

use chatora_rustdoc_mcp::{
    data::{
        Network,
        archive::Mapped,
        cache::Cache,
        dist::{self, Dist, DistSettings, NightlyDate, StdCrate, StdDocs},
        rustdoc_json,
        target::TargetTriple,
    },
    query::{
        across::{self, Indexed},
        path_index::Resolution,
        view::CrateView,
    },
};
use rustdoc_types::{Item, StabilityLevel};

fn linux() -> TargetTriple {
    "x86_64-unknown-linux-gnu".parse().expect("a triple")
}

/// The JSON of std of the nightly of `date`, or the latest.
async fn std_docs(date: Option<&str>) -> (Cache, StdDocs) {
    let cache = Cache::open_default().expect("the cache directory is known");
    let dist = Dist::new(&Network::default(), DistSettings::default()).expect("an HTTP client");
    let date: Option<NightlyDate> = date.map(|date| date.parse().expect("a date"));
    let started = Instant::now();
    let docs = dist::std_docs(&cache, &dist, &linux(), date.as_ref())
        .await
        .expect("the JSON of std is had");
    eprintln!("{:?} in {:?}", docs.nightly, started.elapsed());
    (cache, docs)
}

/// The item that `path` leads to, followed through the crates of std as needed, with its crate.
/// `json` reads the JSON of a crate of std.
fn look_up(json: impl Fn(StdCrate) -> Box<[u8]>, path: &str) -> (Found, Item) {
    // The crates of std are loaded as the path goes into them, each once, archived in memory.
    let mut loaded: HashMap<StdCrate, Arc<Mapped<Indexed>>> = HashMap::new();
    let load = |name: &str| {
        let krate = StdCrate::from_name(name)?;
        if let Some(indexed) = loaded.get(&krate) {
            return Some(Arc::clone(indexed));
        }
        let parsed = rustdoc_json::parse(&json(krate)).expect("the crate parses");
        let indexed = Arc::new(Mapped::in_memory(&Indexed::new(parsed)).expect("archived"));
        loaded.insert(krate, Arc::clone(&indexed));
        Some(indexed)
    };
    let followed = across::follow(path, load).expect("std is loaded");
    eprintln!("{path}: through {:?}", followed.through);
    let view = CrateView::new(followed.krate.get());
    let resolution = view.index().resolve(&view, &followed.path);
    let Resolution::Item(id) = resolution else {
        panic!("{path} is one item: {resolution:?}");
    };
    let item = view.item(id).expect("the item").clone();
    let root = view.item(view.root()).and_then(|root| root.name.clone());
    let found = Found {
        root,
        format_version: view.format_version(),
    };
    (found, item)
}

/// What a look-up found besides the item: the crate it is in, and the format of its JSON.
struct Found {
    root: Option<String>,
    format_version: u32,
}

/// [`look_up`] in the JSON from the distribution server.
fn look_up_in(cache: &Cache, docs: &StdDocs, path: &str) -> (Found, Item) {
    look_up(
        |krate| dist::read_std_json(cache, docs, krate).expect("the JSON of the crate"),
        path,
    )
}

/// The paths that are looked up in std, and the crate that defines each.
const PATHS: [(&str, &str); 3] = [
    ("std::vec::Vec", "alloc"),
    ("std::option::Option", "core"),
    ("std::collections::HashMap", "std"),
];

/// Checks that each of [`PATHS`] is found in its crate, with its stability.
fn check_paths(look_up: impl Fn(&str) -> (Found, Item)) {
    for (path, defined_in) in PATHS {
        let (found, item) = look_up(path);
        assert_eq!(found.root.as_deref(), Some(defined_in), "{path}");
        assert!(item.stability.is_some(), "{path} has its stability");
    }
}

#[cfg(feature = "local")]
#[tokio::test]
#[ignore = "needs a nightly toolchain with the rust-docs-json component"]
async fn std_is_read_from_the_nightly_here() {
    use chatora_rustdoc_mcp::data::local_std;

    let dir = tempfile::tempdir().expect("a temporary directory");
    let local = local_std::local_std(dir.path(), None)
        .await
        .expect("a nightly toolchain with rust-docs-json is installed");
    eprintln!("{:?}", local.rustc);
    check_paths(|path| {
        look_up(
            |krate| local_std::read_local_std_json(&local, krate).expect("the JSON of the crate"),
            path,
        )
    });
}

#[tokio::test]
#[ignore = "fetches about 4 MB from static.rust-lang.org"]
async fn std_paths_are_followed_into_core_and_alloc_from_the_distribution_server() {
    let (cache, docs) = std_docs(None).await;
    check_paths(|path| look_up_in(&cache, &docs, path));
}

/// The sources of std of the latest nightly come from its `rust-src` on the distribution server,
/// of the same commit as its JSON, with files of std where they are expected (requirement R-SC-2,
/// decided in M3h, decision 133).
#[tokio::test]
#[ignore = "fetches about 4 MB and the rust-src of about 6 MB from static.rust-lang.org"]
async fn the_sources_of_std_are_had_from_the_distribution_server() {
    let (cache, docs) = std_docs(None).await;
    let dist = Dist::new(&Network::default(), DistSettings::default()).expect("an HTTP client");
    let sources = dist::rust_src(&cache, &dist, &docs.nightly)
        .await
        .expect("the rust-src of the nightly is had");
    assert!(sources.root.starts_with(cache.dir()), "{:?}", sources.root);
    assert!(sources.warnings.is_empty(), "{:?}", sources.warnings);
    for file in [
        "library/core/src/option.rs",
        "library/alloc/src/vec/mod.rs",
        "library/std/src/lib.rs",
    ] {
        assert!(
            sources.root.join(file).is_file(),
            "{file} in {:?}",
            sources.root
        );
    }
}

/// Formats 58 to 60 write `Stability` flat, and format 58 has no `const_stability`. The shim maps
/// them to the latest types (requirement R-FV-2); this checks it on the JSON of std of those
/// formats.
#[tokio::test]
#[ignore = "fetches about 12 MB from static.rust-lang.org"]
async fn stability_of_formats_58_to_60_is_read() {
    for (date, format) in [("2026-06-25", 58), ("2026-06-26", 59), ("2026-07-10", 60)] {
        let (cache, docs) = std_docs(Some(date)).await;
        let (found, vec) = look_up_in(&cache, &docs, "std::vec::Vec");
        assert_eq!(found.format_version, format, "{date}");
        assert_eq!(
            vec.stability.map(|stability| stability.level),
            Some(StabilityLevel::Stable {
                since: Some("1.0.0".to_owned())
            }),
            "{date}"
        );
        // Every item of alloc, where Vec is, is counted in the parsed JSON.
        let json = dist::read_std_json(&cache, &docs, StdCrate::Alloc).expect("alloc.json");
        let krate = rustdoc_json::parse(&json).expect("alloc parses").krate;
        let unstable = krate
            .index
            .values()
            .filter(|item| {
                item.stability
                    .as_ref()
                    .is_some_and(|stability| stability.level == StabilityLevel::Unstable)
            })
            .count();
        let const_stable = krate
            .index
            .values()
            .filter(|item| item.const_stability.is_some())
            .count();
        eprintln!("{date}: format {format}, {unstable} unstable, {const_stable} const stability");
        assert!(unstable > 0, "{date}: alloc has unstable items");
        // `const_stability` came in format 59.
        assert_eq!(const_stable > 0, format >= 59, "{date}");
    }
}

/// The examples in the docs of std, alloc and core are found and ranked in a short time: once for
/// each call of `find_examples` (decided in M3e, decisions 74 and 75, which keeps a memory cache
/// for over 200 ms), the item's own first.
#[tokio::test]
#[ignore = "fetches about 4 MB from static.rust-lang.org"]
async fn the_examples_in_the_docs_of_std_are_found_in_a_short_time() {
    use chatora_rustdoc_mcp::query::{
        examples::{Asked, Entry, Rank, arranged, doc_blocks},
        get_item::{Detail, ItemRequest, Origin, ResolvedFrom, examples_of, source_locations},
    };

    let (cache, docs) = std_docs(None).await;
    for (krate, path, parent) in [
        (
            StdCrate::Std,
            "std::collections::HashMap::insert",
            "HashMap",
        ),
        (StdCrate::Alloc, "alloc::vec::Vec::push", "Vec"),
        (StdCrate::Core, "core::option::Option::map", "Option"),
    ] {
        let json = dist::read_std_json(&cache, &docs, krate).expect("the JSON of the crate");
        let parsed = rustdoc_json::parse(&json).expect("the crate parses");
        let mapped = Mapped::in_memory(&Indexed::new(parsed)).expect("archived");
        let view = CrateView::new(mapped.get());
        let name = path
            .split("::")
            .next()
            .unwrap_or_default()
            .parse()
            .expect("a crate name");
        let version = semver::Version::new(0, 0, 0);
        let request = ItemRequest {
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
        };
        let started = Instant::now();
        let blocks = doc_blocks(&view);
        let of = examples_of(&view, request)
            .expect("found")
            .expect("not the root");
        assert_eq!(of.parent.as_deref(), Some(parent), "{path}");
        let asked = Asked {
            id: of.id,
            path: &of.path,
            name: &of.name,
            parent: of.parent.as_deref(),
        };
        let order = arranged(&[], &blocks, Some(&asked));
        let ids: Vec<_> = order
            .iter()
            .take(50)
            .filter_map(|entry| match entry {
                Entry::Block { block, .. } => Some(block.id),
                Entry::File { .. } => None,
            })
            .collect();
        let (locations, _) = source_locations(&view, request, &ids);
        let took = started.elapsed();
        assert!(locations.iter().any(Option::is_some), "{path}");
        assert_eq!(&*of.path, path);
        assert!(blocks.len() > 100, "{path}: {} blocks", blocks.len());
        eprintln!("{path}: {} blocks in {took:?}", blocks.len());
        assert_eq!(order.first().map(Entry::rank), Some(Rank::Own), "{path}");
        // Over 200 ms, decision 74 keeps them in memory.
        assert!(
            took.as_millis() < 200,
            "{path}: {took:?} for {} blocks",
            blocks.len()
        );
    }
}
