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
        archive::Mapped,
        cache::{Cache, CachedJson},
        crate_name::CrateName,
        format_version,
        rustdoc_json::{self, ParseError},
    },
    query::{
        across::Indexed,
        get_item::{ItemRequest, get_item},
        path_index::Resolution,
        view::CrateView,
    },
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
    let json = match cache
        .read_docs_rs_json(&cache.docs_rs_dir(name, version, None))
        .expect("the cache is readable")
    {
        CachedJson::Found(json) => json,
        CachedJson::Corrupt { .. } | CachedJson::None => panic!(
            "{name}@{version} is not in the cache: run `cargo run --example fetch_docs_rs -- {name}@{version}`"
        ),
    };
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
        // Formats before 54 write attributes as strings; those with a variant become it
        // (requirement R-FV-2).
        if entry.format_version < 54 {
            let unparsed = krate
                .index
                .values()
                .flat_map(|item| &item.attrs)
                .filter(|attr| {
                    matches!(attr, rustdoc_types::Attribute::Other(text)
                        if ["#[non_exhaustive", "#[automatically_derived", "#[macro_export", "#[must_use", "#[repr"]
                            .iter()
                            .any(|prefix| text.starts_with(prefix)))
                })
                .count();
            assert_eq!(
                unparsed, 0,
                "{what}: attributes with a variant left as strings"
            );
        }
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

/// The canonical path and the definition path of what `path` resolves to in the crate of `view`.
fn describe(view: &CrateView<'_>, path: &str) -> String {
    let describe_id = |id: rustdoc_types::Id| {
        let canonical = view
            .index()
            .canonical_path(id)
            .map_or_else(|| "-".to_owned(), |path| path.to_string());
        let definition = view
            .summary(id)
            .map_or_else(|| "-".to_owned(), |summary| summary.path.join("::"));
        format!("canonical {canonical}, definition {definition}")
    };
    match view.index().resolve(view, path) {
        Resolution::Item(id) => format!("item: {}", describe_id(id)),
        Resolution::Member { parent, member } => {
            let name = view
                .item(member)
                .and_then(|member| member.name.clone())
                .unwrap_or_default();
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
        Resolution::AmbiguousMembers { members, .. } => {
            format!("ambiguous among {} members", members.len())
        },
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
        let parsed = rustdoc_json::parse(&json).expect("parses");
        let started = Instant::now();
        let mapped = Mapped::in_memory(&Indexed::new(parsed)).expect("archived");
        let build_time = started.elapsed();
        let view = CrateView::new(mapped.get());
        let found = describe(&view, path);
        eprintln!("{path}: {found} (index built and archived in {build_time:?})");
        assert_eq!(found, expected, "{path}");
    }
}

/// The rustdoc subtree that the response renders: the item, and the fields or variants of a type.
fn rendered_subtree<'v>(
    view: &'v CrateView<'_>,
    id: rustdoc_types::Id,
) -> Vec<&'v rustdoc_types::Item> {
    use rustdoc_types::{ItemEnum, StructKind};

    let item = view.item(id).expect("the item is in the crate");
    let members: &[rustdoc_types::Id] = match &item.inner {
        ItemEnum::Struct(contents) => match &contents.kind {
            StructKind::Plain { fields, .. } => fields,
            StructKind::Unit | StructKind::Tuple(_) => &[],
        },
        ItemEnum::Union(contents) => &contents.fields,
        ItemEnum::Enum(contents) => &contents.variants,
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
        | ItemEnum::Primitive(_)
        | ItemEnum::AssocConst { .. }
        | ItemEnum::AssocType { .. } => &[],
    };
    std::iter::once(item)
        .chain(members.iter().filter_map(|&id| view.item(id)))
        .collect()
}

/// The length of `items` as compact JSON, without what the response does not render the same
/// way: the documentation, its links and the span.
fn signature_tree_len(items: &[&rustdoc_types::Item]) -> usize {
    items
        .iter()
        .map(|&item| {
            let mut value = serde_json::to_value(item).expect("serializes");
            if let Some(object) = value.as_object_mut() {
                for key in ["docs", "links", "span"] {
                    object.remove(key);
                }
            }
            value.to_string().len()
        })
        .sum()
}

/// The length of the rendered signature, where clause, fields and variants of `data` as JSON.
fn signature_strings_len(data: &serde_json::Value) -> usize {
    ["signature", "where", "fields", "variants"]
        .iter()
        .filter_map(|key| data.get(key))
        .map(|value| value.to_string().len())
        .sum()
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache"]
fn representative_items_are_returned_by_get_item() {
    let cache = Cache::open_default().expect("the cache directory is known");
    let cases = [
        ("tokio", "1.53.3", "tokio::sync::Mutex"),
        ("tokio", "1.53.3", "tokio::sync::Mutex::lock"),
        ("tokio", "1.53.3", "tokio::spawn"),
        ("tokio", "1.53.3", "tokio::select"),
        ("tokio", "1.53.3", "tokio::io::AsyncReadExt::read_buf"),
        ("tokio", "1.53.3", "tokio::runtime::RuntimeFlavor"),
        ("axum", "0.8.9", "axum::Router::route"),
        ("axum", "0.8.9", "axum::extract::State"),
        ("axum", "0.8.9", "axum::debug_handler"),
        ("axum", "0.8.9", "axum::body::Body"),
        ("serde", "1.0.228", "serde::Deserialize"),
        // `impl From<ClientBuilder> for blocking::ClientBuilder` and `impl From<Elapsed> for
        // std::io::Error` are among the `impls` of these types, but their `from` is the blanket
        // `impl<T> From<T> for T` (R-PR-7).
        ("reqwest", "0.13.5", "reqwest::ClientBuilder::from"),
        ("tokio", "1.53.3", "tokio::time::error::Elapsed::from"),
    ];
    eprintln!("path | rustdoc subtree | response | signature tree | signature strings");
    for (crate_name, version, path) in cases {
        let entry = entry(crate_name, version);
        let json = cached_json(&cache, &entry);
        let parsed = rustdoc_json::parse(&json).expect("parses");
        let mapped = Mapped::in_memory(&Indexed::new(parsed)).expect("archived");
        let view = CrateView::new(mapped.get());
        let response = get_item(&view, ItemRequest {
            name: &entry.name,
            version: &entry.version,
            path,
        })
        .unwrap_or_else(|error| panic!("{path}: {error}"));
        let response = serde_json::to_value(&response).expect("serializes");
        insta::assert_json_snapshot!(path.replace("::", "__"), response);

        // R-OUT-2: how much larger the rustdoc subtree is than the response.
        let id = match view.index().resolve(&view, path) {
            Resolution::Item(id) | Resolution::Member { member: id, .. } => id,
            Resolution::ExternalReexport(_) => continue,
            other @ (Resolution::Ambiguous(_)
            | Resolution::AmbiguousMembers { .. }
            | Resolution::NotFound) => {
                panic!("{path}: {other:?}")
            },
        };
        let subtree = rendered_subtree(&view, id);
        let subtree_len: usize = subtree
            .iter()
            .map(|item| serde_json::to_string(item).expect("serializes").len())
            .sum();
        let response_len = response.to_string().len();
        let tree_len = signature_tree_len(&subtree);
        let strings_len = signature_strings_len(&response["data"]);
        eprintln!(
            "{path} | {subtree_len} | {response_len} ({:.1}x) | {tree_len} | {strings_len} ({:.1}x)",
            ratio(subtree_len, response_len),
            ratio(tree_len, strings_len)
        );
    }
}

/// `numerator / denominator`, for the report.
#[expect(clippy::cast_precision_loss, reason = "the lengths are far below 2^52")]
fn ratio(numerator: usize, denominator: usize) -> f64 {
    numerator as f64 / denominator as f64
}
