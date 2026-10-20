//! The same queries give the same responses in separate processes, whatever was asked before them
//! (requirements R-ARCH-5 and R-OUT-4).
//!
//! Each process hashes with its own random seed, so a `HashMap` whose order reaches a response
//! shows up here as a difference. The crates are those of `testdata/docs-rs-json.txt`, which must
//! be in the cache that `CHATORA_RUSTDOC_MCP_CACHE__DIR` names, as in the `testdata` job of CI.
//! Run it with `cargo test --release --test determinism -- --ignored`.

#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

use std::{
    collections::BTreeMap,
    io::{BufRead as _, BufReader, Write as _},
    process::{Command, Stdio},
};

use serde_json::{Value, json};

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
/// text the client reads. Errors lose their `request_id`, which differs on purpose (requirement
/// R-ERR-2).
fn answers(queries: &[(&'static str, Value)], logs: &std::path::Path) -> BTreeMap<String, String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_chatora-rustdoc-mcp"))
        .arg("serve")
        // The project of the default project is this package, for `list_crates`.
        .env("CLAUDE_PROJECT_DIR", env!("CARGO_MANIFEST_DIR"))
        .env("CHATORA_RUSTDOC_MCP_LOG__DIR", logs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let mut answers = BTreeMap::new();
    for (id, (name, arguments)) in (1_u64..).zip(queries) {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments, "_meta": meta },
        });
        writeln!(stdin, "{request}").expect("the server reads stdin");
        stdin.flush().expect("the server reads stdin");
        let mut line = String::new();
        stdout.read_line(&mut line).expect("a response");
        let response: Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(response["id"], id, "{response}");
        let result = &response["result"];
        let answer = if result["isError"] == true {
            let mut error: Value = result["content"][0]["text"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or_else(|| panic!("an error with JSON in its text: {result}"));
            error
                .as_object_mut()
                .map(|error| error.remove("request_id"));
            json!({ "error": error })
        } else {
            result["structuredContent"].clone()
        };
        // As text: maps that differ only in order are equal as values.
        answers.insert(format!("{name} {arguments}"), answer.to_string());
    }
    drop(stdin);
    assert!(child.wait().expect("the server exits").success());
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
