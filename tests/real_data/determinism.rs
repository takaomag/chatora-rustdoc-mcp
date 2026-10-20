//! The same queries give the same responses in separate processes, whatever was asked before them
//! (requirements R-ARCH-5 and R-OUT-4).
//!
//! Each process hashes with its own random seed, so a `HashMap` whose order reaches a response
//! shows up here as a difference. The crates are those of `testdata/docs-rs-json.txt`, which must
//! be in the cache that `CHATORA_RUSTDOC_MCP_CACHE__DIR` names, as in the `testdata` job of CI.

use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use crate::client::Client;

/// The queries that are compared: every tool, responses with maps (`auto_traits`, `counts`,
/// `features`), lists that are cut to a page, std, and an error.
fn queries() -> Vec<(&'static str, Value)> {
    let mut queries = vec![
        (
            "get_item",
            json!({ "crate": "tokio@1.53.3", "path": "sync::Mutex", "detail": "full" }),
        ),
        (
            "get_item",
            json!({ "crate": "axum@0.8.9", "path": "Router" }),
        ),
        (
            "get_item",
            json!({ "crate": "serde@1.0.228", "path": "Serialize" }),
        ),
        (
            "get_item",
            json!({ "crate": "std", "path": "collections::HashMap" }),
        ),
        (
            "get_item",
            json!({ "crate": "tokio@1.53.3", "path": "no_such_thing" }),
        ),
        (
            "list_module",
            json!({ "crate": "tokio@1.53.3", "path": "tokio::sync", "recursive": true, "limit": 30 }),
        ),
        ("crate_overview", json!({ "crate": "tokio@1.53.3" })),
        ("crate_overview", json!({ "crate": "axum@0.8.9" })),
        (
            "search_items",
            json!({ "crate": "tokio@1.53.3", "query": "mutex" }),
        ),
        (
            "search_items",
            json!({ "crate": "std", "query": "hashmap" }),
        ),
        (
            "list_impls",
            json!({ "crate": "tokio@1.53.3", "path": "sync::Mutex" }),
        ),
        (
            "list_methods",
            json!({ "crate": "tokio@1.53.3", "path": "sync::Mutex" }),
        ),
        (
            "list_implementors",
            json!({ "crate": "tokio@1.53.3", "path": "io::AsyncRead" }),
        ),
        ("resolve_version", json!({ "crate": "tokio@1.53.3" })),
    ];
    if cfg!(feature = "local") {
        queries.push(("list_crates", json!({ "scope": "all", "limit": 50 })));
    }
    queries
}

/// Queries that the second process answers between the compared ones.
fn other_queries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "get_item",
            json!({ "crate": "clap@4.6.7", "path": "Parser" }),
        ),
        (
            "search_items",
            json!({ "crate": "serde@1.0.228", "query": "deserialize" }),
        ),
        ("list_module", json!({ "crate": "axum@0.8.9" })),
    ]
}

/// Starts a server, asks it `queries` one at a time, and returns its responses by query, as the
/// text the client reads. Results are checked against their `outputSchema` on the way.
fn answers(queries: &[(&'static str, Value)], logs: &Path) -> BTreeMap<String, String> {
    // The project is this package, for `list_crates`.
    let mut client = Client::start(Some(Path::new(env!("CARGO_MANIFEST_DIR"))), logs);
    let answers = queries
        .iter()
        .map(|(name, arguments)| {
            (
                format!("{name} {arguments}"),
                client.call(name, arguments).text(),
            )
        })
        .collect();
    client.finish();
    answers
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache, and the network once"]
fn the_same_queries_give_the_same_responses_in_other_processes() {
    let logs = tempfile::tempdir().expect("a temporary directory");
    let queries = queries();
    // The first run fetches what the cache lacks (the index, `.crate` files, std) and archives
    // the crates, so that the compared runs read the same data.
    let warm = answers(&queries, logs.path());
    let first = answers(&queries, logs.path());
    let mut reordered: Vec<(&'static str, Value)> = Vec::new();
    for (query, other) in queries
        .iter()
        .rev()
        .zip(other_queries().into_iter().cycle())
    {
        reordered.push(other);
        reordered.push(query.clone());
    }
    let mut second = answers(&reordered, logs.path());
    second.retain(|key, _| first.contains_key(key));
    for (key, answer) in &first {
        // A query that failed for want of data is not a test of determinism.
        let parsed: Value = serde_json::from_str(answer).expect("JSON");
        if let Some(code) = parsed["error"]["code"].as_str() {
            assert!(
                code == "item_not_found",
                "{key} failed: {answer}; is the test data in the cache?"
            );
        }
        assert_eq!(
            Some(answer),
            second.get(key),
            "{key} differs between processes"
        );
    }
    assert_eq!(first.len(), second.len());
    // The responses of the warm-up may say that something was fetched; the rest are the same.
    assert_eq!(warm.len(), first.len());
}
