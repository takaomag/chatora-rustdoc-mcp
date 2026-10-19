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

use chatora_rustdoc_mcp::data::{cache::Cache, crate_name::CrateName, format_version};
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

/// Reads the JSON of `entry` from the cache and checks that it is the one in the list.
fn cached_json(cache: &Cache, entry: &Entry) -> Vec<u8> {
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
