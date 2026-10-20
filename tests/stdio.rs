//! Runs the server binary over stdio and checks its JSON-RPC behavior (`doc/m0-plan.md`, A2).
//!
//! Every test starts a server process of its own. Responses may arrive in any order, because rmcp
//! handles each request in a task of its own, so they are matched by id.

// Lets clippy treat the helpers below as test code, like the `#[test]` functions
// (`allow-expect-in-tests` and the other settings in clippy.toml).
#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

use std::{
    collections::HashMap,
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use chatora_rustdoc_mcp::data::{
    cache::{Cache, sha256_hex},
    crate_name::CrateName,
};
use flate2::{Compression, write::GzEncoder};
use rustdoc_types::{
    Abi,
    Crate,
    Function,
    FunctionHeader,
    FunctionSignature,
    Generics,
    Id,
    Item,
    ItemEnum,
    Module,
    Target,
    Type,
    Visibility,
};
use semver::Version;
use serde_json::{Value, json};
use tempfile::TempDir;

/// Longest wait for a message the test expects.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The only protocol version the server supports.
const PROTOCOL_VERSION: &str = "2026-07-28";

/// The `_meta` that every 2026-07-28 request carries, plus `extra` fields.
fn meta(extra: Value) -> Value {
    let mut meta = json!({
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    if let (Some(meta), Value::Object(extra)) = (meta.as_object_mut(), extra) {
        meta.extend(extra);
    }
    meta
}

/// A 2026-07-28 request: `params` plus the required `_meta`.
fn request(id: u64, method: &str, mut params: Value) -> Value {
    params["_meta"] = meta(json!({}));
    // Moved in rather than written inside `json!`, which would serialize a copy of it.
    let mut request = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    request["params"] = params;
    request
}

/// A `tools/call` request.
fn call_tool(id: u64, name: &str, arguments: Value) -> Value {
    let mut params = json!({ "name": name });
    params["arguments"] = arguments;
    request(id, "tools/call", params)
}

/// Parses the JSON in `content[0].text` of a tool result.
fn text_json(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .expect("content[0] is text");
    serde_json::from_str(text).expect("the text is JSON")
}

/// The crate in the test cache: `demo` 0.1.0 with `/// Says hello.` `pub fn hello() -> u8`.
fn demo_crate() -> Crate {
    let item = |id, name: &str, docs: Option<&str>, inner| Item {
        id: Id(id),
        crate_id: 0,
        name: Some(name.to_owned()),
        span: None,
        visibility: Visibility::Public,
        docs: docs.map(str::to_owned),
        links: HashMap::new(),
        attrs: Vec::new(),
        deprecation: None,
        stability: None,
        const_stability: None,
        inner,
    };
    let root = item(
        0,
        "demo",
        None,
        ItemEnum::Module(Module {
            is_crate: true,
            items: vec![Id(1)],
            is_stripped: false,
        }),
    );
    let hello = item(
        1,
        "hello",
        Some("Says hello.\n\nMore."),
        ItemEnum::Function(Function {
            sig: FunctionSignature {
                inputs: Vec::new(),
                output: Some(Type::Primitive("u8".to_owned())),
                is_c_variadic: false,
            },
            generics: Generics {
                params: Vec::new(),
                where_predicates: Vec::new(),
            },
            header: FunctionHeader {
                is_const: false,
                is_unsafe: false,
                is_async: false,
                abi: Abi::Rust,
            },
            has_body: true,
            default_unstable: None,
        }),
    );
    Crate {
        root: Id(0),
        crate_version: Some("0.1.0".to_owned()),
        includes_private: false,
        index: [root, hello]
            .into_iter()
            .map(|item| (item.id, item))
            .collect(),
        paths: HashMap::new(),
        external_crates: HashMap::new(),
        target: Target {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            target_features: Vec::new(),
        },
        format_version: rustdoc_types::FORMAT_VERSION,
    }
}

/// The target of the JSON of [`demo_crate`], which the tests ask for, so that they do not depend on
/// the host.
const TARGET: &str = "x86_64-unknown-linux-gnu";

/// A cache directory that holds the JSON of [`demo_crate`], so that `get_item` needs no network.
fn test_cache() -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    let json = serde_json::to_vec(&demo_crate()).expect("serializes");
    let name: CrateName = "demo".parse().expect("a crate name");
    let cache = Cache::at(dir.path());
    cache
        .write_docs_rs_json(
            &cache.docs_rs_dir(
                &name,
                &Version::new(0, 1, 0),
                Some(&TARGET.parse().expect("a target")),
            ),
            rustdoc_types::FORMAT_VERSION,
            &json,
        )
        .expect("the cache is writable");
    // The `.crate`, so that what docs.rs built it with is read without static.crates.io (where
    // a crate called `demo` is).
    let path = cache.crate_file_path(&name, &Version::new(0, 1, 0));
    let contents = demo_crate_file();
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("made");
    std::fs::write(&path, &contents).expect("written");
    std::fs::write(path.with_extension("crate.sha256"), sha256_hex(&contents)).expect("written");
    dir
}

/// The `.crate` of `demo` 0.1.0, whose features docs.rs enabled all.
fn demo_crate_file() -> Vec<u8> {
    let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n[features]\nextra = []\n\
                    [package.metadata.docs.rs]\nall-features = true\n";
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len().try_into().expect("small"));
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "demo-0.1.0/Cargo.toml", manifest.as_bytes())
        .expect("appended");
    builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .expect("finished")
}

/// A running server, talking JSON-RPC over its stdin and stdout.
struct Server {
    child: Child,
    /// The server's cache directory, removed when the server is dropped.
    _cache: TempDir,
    stdin: Option<ChildStdin>,
    /// Lines from stdout: a parsed message, or a line that is not a JSON-RPC message.
    lines: Receiver<Result<Value, String>>,
    /// Messages received while waiting for another one.
    pending: Vec<Value>,
}

impl Server {
    fn start() -> Self {
        Self::start_in(None)
    }

    /// A running server whose default project is `project`, or which has none: it then runs in
    /// its cache directory, so that the project these tests run in is not read.
    fn start_in(project: Option<&Path>) -> Self {
        let cache = test_cache();
        let mut command = Command::new(env!("CARGO_BIN_EXE_chatora-rustdoc-mcp"));
        match project {
            Some(project) => command.env("CLAUDE_PROJECT_DIR", project),
            None => command.env_remove("CLAUDE_PROJECT_DIR"),
        };
        let mut child = command
            .arg("serve")
            .current_dir(cache.path())
            // Logging stays on, so that the tests would see a log line that reached stdout
            // (requirement R-SEC-1).
            .env("CHATORA_RUSTDOC_MCP_LOG__FILTER", "debug")
            .env("CHATORA_RUSTDOC_MCP_CACHE__DIR", cache.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                // Stdout carries only JSON-RPC messages, one per line (requirement R-SEC-1).
                let message = serde_json::from_str::<Value>(&line)
                    .ok()
                    .filter(|message| message["jsonrpc"] == "2.0")
                    .ok_or(line);
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            _cache: cache,
            stdin,
            lines,
            pending: Vec::new(),
        }
    }

    /// Writes one message as a line to the server's stdin.
    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{message}").expect("the server reads stdin");
        stdin.flush().expect("the server reads stdin");
    }

    /// Receives the next message, or `None` if none arrives by `deadline`.
    fn receive(&self, deadline: Instant) -> Option<Value> {
        let timeout = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(timeout) {
            Ok(Ok(message)) => Some(message),
            Ok(Err(line)) => panic!("stdout has a line that is not a JSON-RPC message: {line}"),
            Err(_) => None,
        }
    }

    /// Waits for the first message that `matches`, keeping the others for later.
    fn wait_for(&mut self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        if let Some(index) = self.pending.iter().position(&matches) {
            return self.pending.remove(index);
        }
        let deadline = Instant::now()
            .checked_add(TIMEOUT)
            .expect("the deadline fits in an Instant");
        loop {
            let message = self
                .receive(deadline)
                .unwrap_or_else(|| panic!("timed out waiting for {what}"));
            if matches(&message) {
                return message;
            }
            self.pending.push(message);
        }
    }

    /// Waits for the response to the request `id`.
    fn response(&mut self, id: u64) -> Value {
        self.wait_for(&format!("the response to request {id}"), |message| {
            message["id"] == id && message.get("method").is_none()
        })
    }

    /// Waits for a notification with `method`.
    fn notification(&mut self, method: &str) -> Value {
        self.wait_for(&format!("a {method} notification"), |message| {
            message["method"] == method && message.get("id").is_none()
        })
    }

    /// Collects every message that arrives during `duration`, including those kept earlier.
    // Only the test of `sleep` uses it, which is only in debug builds.
    #[cfg(debug_assertions)]
    fn messages_during(&mut self, duration: Duration) -> Vec<Value> {
        let deadline = Instant::now()
            .checked_add(duration)
            .expect("the deadline fits in an Instant");
        let mut messages = std::mem::take(&mut self.pending);
        while let Some(message) = self.receive(deadline) {
            messages.push(message);
        }
        messages
    }

    /// Closes stdin, checks the rest of stdout, and returns how the server exited.
    fn finish(mut self) -> ExitStatus {
        drop(self.stdin.take());
        let deadline = Instant::now()
            .checked_add(TIMEOUT)
            .expect("the deadline fits in an Instant");
        // Every remaining line must be a JSON-RPC message too; `receive` panics otherwise.
        while self.receive(deadline).is_some() {}
        loop {
            if let Some(status) = self.child.try_wait().expect("the server can be waited for") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the server did not exit after stdin closed"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Stop a server that a failed test left running. It is a no-op after the server exits.
        if let Err(error) = self.child.kill() {
            eprintln!("failed to stop the server: {error}");
        }
    }
}

#[test]
fn discover_describes_the_server() {
    let mut server = Server::start();
    server.send(&request(1, "server/discover", json!({})));
    let response = server.response(1);
    let result = &response["result"];
    assert_eq!(result["supportedVersions"], json!([PROTOCOL_VERSION]));
    assert_eq!(result["capabilities"], json!({ "tools": {} }));
    assert_eq!(result["ttlMs"], 3_600_000);
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "chatora-rustdoc-mcp"
    );
    assert!(server.finish().success());
}

#[test]
fn initialize_is_rejected_with_a_message_for_old_clients() {
    let mut server = Server::start();
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" },
        },
    }));
    let response = server.response(1);
    let error = &response["error"];
    assert_eq!(error["code"], -32022);
    let message = error["message"].as_str().expect("the message is a string");
    assert!(message.contains(PROTOCOL_VERSION), "{message}");
    assert!(message.contains("Claude Code v2.1.292"), "{message}");
    assert_eq!(error["data"]["requested"], "2025-11-25");
    assert_eq!(error["data"]["supported"], json!([PROTOCOL_VERSION]));
    // rmcp ends the session after a failed initialize.
    assert_eq!(server.finish().code(), Some(1));
}

#[test]
fn tools_list_is_cacheable_and_not_paginated() {
    let mut server = Server::start();
    server.send(&request(1, "tools/list", json!({})));
    server.send(&request(2, "tools/list", json!({ "cursor": "x" })));
    let response = server.response(1);
    let result = &response["result"];
    let names: Vec<&Value> = result["tools"]
        .as_array()
        .expect("tools is an array")
        .iter()
        .map(|tool| &tool["name"])
        .collect();
    // `list_crates` needs local features, and `sleep` is only in debug builds.
    let mut expected = vec!["crate_overview", "get_item"];
    if cfg!(feature = "local") {
        expected.push("list_crates");
    }
    expected.extend([
        "list_implementors",
        "list_impls",
        "list_methods",
        "list_module",
        "resolve_version",
        "search_items",
    ]);
    if cfg!(debug_assertions) {
        expected.push("sleep");
    }
    assert_eq!(names, expected);
    let get_item = result["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "get_item"))
        .expect("get_item is listed");
    assert_eq!(get_item["annotations"]["readOnlyHint"], false);
    assert_eq!(get_item["annotations"]["openWorldHint"], true);
    assert_eq!(
        get_item["outputSchema"]["required"],
        json!(["provenance", "data"])
    );
    assert_eq!(result["ttlMs"], 3_600_000);
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(server.response(2)["error"]["code"], -32602);
    assert!(server.finish().success());
}

#[test]
fn get_item_returns_structured_content_and_tool_errors() {
    let mut server = Server::start();
    server.send(&call_tool(
        1,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET }),
    ));
    server.send(&call_tool(
        2,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "demo::nothing", "target": TARGET }),
    ));
    server.send(&call_tool(
        3,
        "get_item",
        json!({ "crate": "demo@not a version", "path": "hello" }),
    ));
    server.send(&call_tool(4, "nope", json!({})));

    let response = server.response(1);
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(
        result["structuredContent"],
        json!({
            "provenance": {
                "source": "docs.rs",
                "crate": "demo",
                "version": "0.1.0",
                "resolved_from": "explicit",
                "target": "x86_64-unknown-linux-gnu",
                "format_version": rustdoc_types::FORMAT_VERSION,
                "all_features": true,
            },
            "data": {
                "path": "demo::hello",
                "kind": "function",
                "signature": "pub fn hello() -> u8",
                "docs": "Says hello.",
                "docs_truncated": true,
            },
        })
    );
    assert_eq!(text_json(result), result["structuredContent"]);

    // A tool error has its JSON in the text and no structured content (requirements R-ERR-2 and
    // R-OUT-1).
    let response = server.response(2);
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert!(result.get("structuredContent").is_none());
    let error = text_json(result);
    assert_eq!(error["code"], "item_not_found");
    assert_eq!(error["retryable"], false);

    // Bad arguments are a tool error too (requirement R-ARG-1).
    assert_eq!(
        text_json(&server.response(3)["result"])["code"],
        "invalid_argument"
    );
    // Only an unknown tool is a JSON-RPC error.
    assert_eq!(server.response(4)["error"]["code"], -32602);
    assert!(server.finish().success());
}

/// `list_module` lists a module of a cached crate a page at a time (requirements R-LS-1 and
/// R-OUT-3).
#[test]
fn list_module_lists_a_cached_crate() {
    let mut server = Server::start();
    server.send(&call_tool(
        1,
        "list_module",
        json!({ "crate": "demo@0.1.0", "target": TARGET }),
    ));
    server.send(&call_tool(
        2,
        "list_module",
        json!({ "crate": "demo@0.1.0", "target": TARGET, "cursor": "c1.x.y.0" }),
    ));
    let response = server.response(1);
    let result = &response["result"];
    assert_eq!(result["isError"], false, "{result}");
    let content = &result["structuredContent"];
    assert_eq!(content["data"]["path"], "demo");
    assert_eq!(
        content["data"]["items"],
        json!([{ "path": "demo::hello", "kind": "function", "signature": "pub fn hello() -> u8" }])
    );
    assert_eq!(
        content["page"],
        json!({ "shown": 1, "total": 1, "has_more": false })
    );
    let error = text_json(&server.response(2)["result"]);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert!(server.finish().success());
}

/// `search_items` finds an item of a cached crate by name, and suggests a spelling when nothing
/// matches (requirements R-SR-1 and R-SR-5).
#[test]
fn search_items_searches_a_cached_crate() {
    let mut server = Server::start();
    server.send(&call_tool(
        1,
        "search_items",
        json!({ "crate": "demo@0.1.0", "target": TARGET, "query": "hello" }),
    ));
    server.send(&call_tool(
        2,
        "search_items",
        json!({ "crate": "demo@0.1.0", "target": TARGET, "query": "helo" }),
    ));
    let response = server.response(1);
    let content = &response["result"]["structuredContent"];
    assert_eq!(
        content["data"]["items"][0]["path"], "demo::hello",
        "{response}"
    );
    assert_eq!(content["page"]["total"], 1);
    let response = server.response(2);
    let content = &response["result"]["structuredContent"];
    assert_eq!(content["data"]["items"], json!([]), "{response}");
    assert_eq!(content["data"]["did_you_mean"], json!(["demo::hello"]));
    assert!(server.finish().success());
}

/// Without `crate`, `search_items` searches the project's dependencies, which needs a project.
#[test]
fn search_items_without_a_crate_needs_a_project() {
    let mut server = Server::start();
    server.send(&call_tool(1, "search_items", json!({ "query": "hello" })));
    let error = text_json(&server.response(1)["result"]);
    assert_eq!(error["code"], "invalid_argument", "{error}");
    assert!(server.finish().success());
}

/// `list_crates` lists the crates of the project at their resolved versions (§3 Q1).
#[cfg(feature = "local")]
#[test]
fn list_crates_lists_the_project() {
    let project = demo_workspace();
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(1, "list_crates", json!({ "target": TARGET })));
    let response = server.response(1);
    let content = &response["result"]["structuredContent"];
    assert_eq!(content["provenance"]["source"], "project", "{response}");
    assert_eq!(content["data"]["crates"][0]["crate"], "demo", "{response}");
    assert_eq!(content["data"]["crates"][0]["member"], true);
    assert_eq!(content["data"]["crates"][0]["source"], "path");
    assert!(server.finish().success());
}

/// `resolve_version` answers without the network when there is nothing to resolve: std comes
/// with the toolchain, and an argument that is not valid is a tool error (requirement R-ARG-1).
#[test]
fn resolve_version_needs_no_network_for_std_and_bad_arguments() {
    let mut server = Server::start();
    server.send(&call_tool(1, "resolve_version", json!({ "crate": "std" })));
    server.send(&call_tool(
        2,
        "resolve_version",
        json!({ "crate": "tokio@x.y" }),
    ));
    for id in [1, 2] {
        let result = &server.response(id)["result"];
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(text_json(result)["code"], "invalid_argument", "{result}");
    }
    assert!(server.finish().success());
}

/// A workspace with the package `demo` and its Cargo.lock, in a new temporary directory. crates.io
/// has a crate named demo too, which must not be looked for in its place.
#[cfg(feature = "local")]
fn demo_workspace() -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    std::fs::create_dir_all(dir.path().join("src")).expect("made");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("written");
    std::fs::write(dir.path().join("src/lib.rs"), "").expect("written");
    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(dir.path())
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("cargo runs");
    assert!(status.success(), "cargo generate-lockfile");
    dir
}

/// Without a version, the project decides it (requirements R-MCP-3 and R-VR-2). The project here
/// is the package itself, which docs.rs does not have, and one outside the default project is not
/// read with cargo. Without local features, no project is read.
#[cfg(feature = "local")]
#[test]
fn without_a_version_the_project_decides_it() {
    let project = demo_workspace();
    let elsewhere = demo_workspace();
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(
        1,
        "get_item",
        json!({ "crate": "demo", "path": "hello", "target": TARGET }),
    ));
    server.send(&call_tool(
        2,
        "get_item",
        json!({
            "crate": "demo",
            "path": "hello",
            "target": TARGET,
            "project": elsewhere.path(),
        }),
    ));
    let error = text_json(&server.response(1)["result"]);
    assert_eq!(error["code"], "docs_unavailable");
    assert_eq!(error["reason"], "local_build_required", "{error}");
    let error = text_json(&server.response(2)["result"]);
    assert_eq!(error["reason"], "no_record", "{error}");
    assert!(server.finish().success());
}

#[test]
fn protocol_errors_follow_the_2026_07_28_rules() {
    let mut server = Server::start();
    // The first request with `_meta` starts the session.
    server.send(&request(1, "tools/list", json!({})));
    server.response(1);
    server.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    server.send(&request(3, "ping", json!({})));
    server.send(&request(4, "logging/setLevel", json!({ "level": "info" })));
    assert_eq!(server.response(2)["error"]["code"], -32602);
    assert_eq!(server.response(3)["error"]["code"], -32601);
    assert_eq!(server.response(4)["error"]["code"], -32601);
    assert!(server.finish().success());
}

/// rmcp behaviors that differ from the requirements and cannot be changed from the handler. If
/// one of these assertions fails, rmcp has changed: update R-MCP-2 and R-ARG-1 in the requirements.
#[test]
fn rmcp_deviations_from_the_requirements() {
    let mut server = Server::start();
    // R-MCP-2 wants -32601: rmcp answers a `ping` before the session starts itself.
    server.send(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }));
    assert_eq!(server.response(1)["result"], json!({}));
    // R-ARG-1 wants -32602: rmcp fails to parse the request and answers -32601.
    server.send(&request(
        2,
        "tools/call",
        json!({ "name": "get_item", "arguments": "hi" }),
    ));
    assert_eq!(server.response(2)["error"]["code"], -32601);
    // The same for a request without `name` (checked in M1, 2026-10-19).
    server.send(&request(3, "tools/call", json!({ "arguments": {} })));
    assert_eq!(server.response(3)["error"]["code"], -32601);
    assert!(server.finish().success());
}

/// `sleep` is only in debug builds.
#[cfg(debug_assertions)]
#[test]
fn progress_is_reported_and_a_cancelled_request_gets_no_response() {
    let mut server = Server::start();
    let mut sleep = call_tool(1, "sleep", json!({ "seconds": 5 }));
    sleep["params"]["_meta"] = meta(json!({ "progressToken": "p1" }));
    server.send(&sleep);

    let progress = server.notification("notifications/progress");
    assert_eq!(progress["params"]["progressToken"], "p1");
    assert_eq!(progress["params"]["progress"], 1.0);
    assert_eq!(progress["params"]["total"], 5.0);

    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 1, "reason": "test" },
    }));
    // The server keeps working for other requests.
    server.send(&request(2, "tools/list", json!({})));
    assert!(server.response(2)["result"]["tools"].is_array());
    // Nothing is sent for the cancelled request: no result, and no more than one more progress
    // notification that may have been on its way (requirement R-WS-6).
    let later = server.messages_during(Duration::from_millis(1500));
    assert!(
        later.iter().all(|message| message["id"] != 1),
        "the cancelled request got a response: {later:?}"
    );
    assert!(
        later.len() <= 1,
        "progress went on after cancellation: {later:?}"
    );
    assert!(server.finish().success());
}

#[test]
fn subscriptions_listen_is_acknowledged_with_no_notifications() {
    let mut server = Server::start();
    server.send(&request(
        5,
        "subscriptions/listen",
        json!({ "notifications": { "toolsListChanged": true } }),
    ));
    let acknowledged = server.notification("notifications/subscriptions/acknowledged");
    assert_eq!(
        acknowledged["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        5
    );
    assert_eq!(acknowledged["params"]["notifications"], json!({}));
    // The subscription does not block other requests.
    server.send(&request(6, "tools/list", json!({})));
    assert!(server.response(6)["result"]["tools"].is_array());
    // A client cancels the subscription before it leaves. Otherwise rmcp waits 5 seconds for it
    // to end after stdin closes, as it does for every request still in progress.
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 5 },
    }));
    assert!(server.finish().success());
}
