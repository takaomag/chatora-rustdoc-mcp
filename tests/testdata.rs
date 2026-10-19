//! Checks the library against the rustdoc JSON listed in `testdata/docs-rs-json.txt`.
//!
//! The JSON is in the cache, not in the repository, so these tests are ignored by default. Fetch
//! the JSON with `cargo run --example fetch_docs_rs -- <crate>@<version> ...`, then run
//! `cargo test --release --test testdata -- --ignored --nocapture`.

// Lets clippy treat the helpers below as test code, like the `#[test]` functions.
#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

use std::{fmt::Write as _, time::Instant};

use chatora_rustdoc_mcp::{
    data::{
        cache::Cache,
        crate_name::CrateName,
        format_version,
        rustdoc_json::{self, ParseError},
    },
    query::path_index::{PathIndex, Resolution},
};
use semver::Version;
use sha2::{Digest as _, Sha256};

/// One line of `testdata/docs-rs-json.txt`.
struct Entry {
    name: CrateName,
    version: Version,
    format_version: u32,
    sha256: &'static str,
}

fn entries() -> Vec<Entry> {
    include_str!("../testdata/docs-rs-json.txt")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            Entry {
                name: fields[0].parse().expect("a crate name"),
                version: Version::parse(fields[1]).expect("a version"),
                format_version: fields[2].parse().expect("a format version"),
                sha256: fields[4],
            }
        })
        .collect()
}

/// The entry of `crate_name` at `version` in the list.
fn entry(crate_name: &str, version: &str) -> Entry {
    entries()
        .into_iter()
        .find(|entry| entry.name.as_str() == crate_name && entry.version.to_string() == version)
        .unwrap_or_else(|| panic!("{crate_name}@{version} is not in the list"))
}

/// Reads the JSON of `entry` from the cache and checks that it is the one in the list.
fn cached_json(cache: &Cache, entry: &Entry) -> Box<[u8]> {
    let (name, version) = (&entry.name, &entry.version);
    let json = cache
        .read_docs_rs_json(name, version)
        .expect("the cache is readable")
        .unwrap_or_else(|| {
            panic!("{name}@{version} is not in the cache: run `cargo run --example fetch_docs_rs -- {name}@{version}`")
        });
    let mut sha256 = String::new();
    for byte in Sha256::digest(&json) {
        write!(sha256, "{byte:02x}").expect("writing to a String does not fail");
    }
    assert_eq!(
        sha256, entry.sha256,
        "{name}@{version} changed on docs.rs: fetch it again and update the list"
    );
    json
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache"]
fn format_version_is_read_from_the_end_of_every_format() {
    let cache = Cache::open_default().expect("the cache directory is known");
    for entry in entries() {
        let json = cached_json(&cache, &entry);
        let started = Instant::now();
        let peeked = format_version::peek(&json);
        let peek_time = started.elapsed();
        let started = Instant::now();
        let scanned = format_version::scan(&json).expect("the JSON has a format_version");
        let scan_time = started.elapsed();
        assert_eq!(
            peeked,
            Some(entry.format_version),
            "{}@{}",
            entry.name,
            entry.version
        );
        assert_eq!(
            scanned, entry.format_version,
            "{}@{}",
            entry.name, entry.version
        );
        eprintln!(
            "{}@{}: format {}, {} bytes, peek {peek_time:?}, scan {scan_time:?}",
            entry.name,
            entry.version,
            entry.format_version,
            json.len()
        );
    }
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache"]
fn every_supported_format_is_parsed_into_the_latest_types() {
    let cache = Cache::open_default().expect("the cache directory is known");
    for entry in entries() {
        let json = cached_json(&cache, &entry);
        let what = format!("{}@{}", entry.name, entry.version);
        let started = Instant::now();
        let parsed = rustdoc_json::parse(&json);
        let parse_time = started.elapsed();
        if !rustdoc_json::SUPPORTED.contains(&entry.format_version) {
            assert!(
                matches!(parsed, Err(ParseError::UnsupportedFormat { found, .. }) if found == entry.format_version),
                "{what}: {parsed:?}"
            );
            eprintln!("{what}: format {} is unsupported", entry.format_version);
            continue;
        }
        let krate = parsed
            .unwrap_or_else(|error| panic!("{what}: {error}"))
            .krate;
        assert_eq!(krate.format_version, entry.format_version, "{what}");
        assert_eq!(
            krate.crate_version.as_deref(),
            Some(entry.version.to_string().as_str()),
            "{what}"
        );
        assert!(
            krate.index.contains_key(&krate.root),
            "{what}: the root is not in the index"
        );
        // Formats before 57 have no path for external crates; it reads as an empty path.
        let empty_paths = krate
            .external_crates
            .values()
            .filter(|external_crate| external_crate.path.as_os_str().is_empty())
            .count();
        if entry.format_version < 57 {
            assert_eq!(empty_paths, krate.external_crates.len(), "{what}");
        } else {
            assert_eq!(empty_paths, 0, "{what}");
        }
        eprintln!(
            "{what}: format {}, {} bytes, {} items, {} paths, {} external crates, parse {parse_time:?}",
            entry.format_version,
            json.len(),
            krate.index.len(),
            krate.paths.len(),
            krate.external_crates.len()
        );
    }
}

/// The canonical path and the definition path of what `path` resolves to in `index`.
fn describe(krate: &rustdoc_types::Crate, index: &PathIndex, path: &str) -> String {
    let describe_id = |id: rustdoc_types::Id| {
        let canonical = index
            .canonical_path(id)
            .map_or_else(|| "-".to_owned(), ToString::to_string);
        let definition = krate
            .paths
            .get(&id)
            .map_or_else(|| "-".to_owned(), |summary| summary.path.join("::"));
        format!("canonical {canonical}, definition {definition}")
    };
    match index.resolve(krate, path) {
        Resolution::Item(id) => format!("item: {}", describe_id(id)),
        Resolution::Member { parent, member } => {
            let name = krate.index[&member].name.clone().unwrap_or_default();
            format!("member {name} of {}", describe_id(parent))
        },
        Resolution::ExternalReexport(reexport) => format!(
            "re-export of {}",
            reexport
                .definition
                .as_ref()
                .map_or_else(|| "?".to_owned(), ToString::to_string)
        ),
        Resolution::Ambiguous(ids) => format!("ambiguous among {} items", ids.len()),
        Resolution::NotFound => "not found".to_owned(),
    }
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache"]
fn representative_items_are_found_by_their_public_paths() {
    let cache = Cache::open_default().expect("the cache directory is known");
    let cases = [
        (
            "tokio",
            "1.53.3",
            "tokio::sync::Mutex",
            "item: canonical tokio::sync::Mutex, definition tokio::sync::mutex::Mutex",
        ),
        (
            "tokio",
            "1.53.3",
            "tokio::sync::mutex::Mutex",
            "item: canonical tokio::sync::Mutex, definition tokio::sync::mutex::Mutex",
        ),
        (
            "tokio",
            "1.53.3",
            "tokio::sync::Mutex::lock",
            "member lock of canonical tokio::sync::Mutex, definition tokio::sync::mutex::Mutex",
        ),
        (
            "axum",
            "0.8.9",
            "axum::Json",
            "item: canonical axum::Json, definition axum::json::Json",
        ),
        (
            "axum",
            "0.8.9",
            "axum::Router",
            "item: canonical axum::Router, definition axum::routing::Router",
        ),
        (
            "axum",
            "0.8.9",
            "axum::body::Body",
            "re-export of axum_core::body::Body",
        ),
        (
            "serde",
            "1.0.228",
            "serde::Deserialize",
            "item: canonical serde::Deserialize, definition serde::de::Deserialize",
        ),
    ];
    for (crate_name, version, path, expected) in cases {
        let json = cached_json(&cache, &entry(crate_name, version));
        let krate = rustdoc_json::parse(&json).expect("parses").krate;
        let started = Instant::now();
        let index = PathIndex::build(&krate);
        let build_time = started.elapsed();
        let found = describe(&krate, &index, path);
        eprintln!("{path}: {found} (index built in {build_time:?})");
        assert_eq!(found, expected, "{path}");
    }
}
