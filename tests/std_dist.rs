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
        cache::Cache,
        dist::{self, Dist, NightlyDate, StdCrate, StdDocs},
        rustdoc_json,
        target::TargetTriple,
    },
    query::{
        across::{self, Indexed},
        path_index::Resolution,
    },
};
use rustdoc_types::{Item, StabilityLevel};

fn linux() -> TargetTriple {
    "x86_64-unknown-linux-gnu".parse().expect("a triple")
}

/// The JSON of std of the nightly of `date`, or the latest.
async fn std_docs(date: Option<&str>) -> (Cache, StdDocs) {
    let cache = Cache::open_default().expect("the cache directory is known");
    let dist = Dist::new().expect("an HTTP client");
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
fn look_up(json: impl Fn(StdCrate) -> Box<[u8]>, path: &str) -> (Arc<Indexed>, Item) {
    // The crates of std are loaded as the path goes into them, each once.
    let mut loaded: HashMap<StdCrate, Arc<Indexed>> = HashMap::new();
    let load = |name: &str| {
        let krate = StdCrate::from_name(name)?;
        if let Some(indexed) = loaded.get(&krate) {
            return Some(Arc::clone(indexed));
        }
        let parsed = rustdoc_json::parse(&json(krate)).expect("the crate parses");
        let indexed = Arc::new(Indexed::new(parsed));
        loaded.insert(krate, Arc::clone(&indexed));
        Some(indexed)
    };
    let followed = across::follow(path, load).expect("std is loaded");
    eprintln!("{path}: through {:?}", followed.through);
    let krate = &followed.krate.parsed.krate;
    let resolution = followed.krate.index.resolve(krate, &followed.path);
    let Resolution::Item(id) = resolution else {
        panic!("{path} is one item: {resolution:?}");
    };
    let item = krate.index[&id].clone();
    (followed.krate, item)
}

/// [`look_up`] in the JSON from the distribution server.
fn look_up_in(cache: &Cache, docs: &StdDocs, path: &str) -> (Arc<Indexed>, Item) {
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
fn check_paths(look_up: impl Fn(&str) -> (Arc<Indexed>, Item)) {
    for (path, defined_in) in PATHS {
        let (krate, item) = look_up(path);
        let root = &krate.parsed.krate.index[&krate.parsed.krate.root];
        assert_eq!(root.name.as_deref(), Some(defined_in), "{path}");
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

/// Formats 58 to 60 write `Stability` flat, and format 58 has no `const_stability`. The shim maps
/// them to the latest types (requirement R-FV-2); this checks it on the JSON of std of those
/// formats.
#[tokio::test]
#[ignore = "fetches about 12 MB from static.rust-lang.org"]
async fn stability_of_formats_58_to_60_is_read() {
    for (date, format) in [("2026-06-25", 58), ("2026-06-26", 59), ("2026-07-10", 60)] {
        let (cache, docs) = std_docs(Some(date)).await;
        let (krate, vec) = look_up_in(&cache, &docs, "std::vec::Vec");
        let krate = &krate.parsed.krate;
        assert_eq!(krate.format_version, format, "{date}");
        assert_eq!(
            vec.stability.map(|stability| stability.level),
            Some(StabilityLevel::Stable {
                since: Some("1.0.0".to_owned())
            }),
            "{date}"
        );
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
