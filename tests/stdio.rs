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

#[path = "common/schema.rs"]
mod schema;

// The test server for docs.rs, which only a debug build takes.
#[cfg(debug_assertions)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::{
    collections::HashMap,
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
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

/// Longest wait for a message the test expects: a bound only, which a test that passes does not
/// wait for, long enough for a loaded machine, as a CI runner (5 seconds was not on Windows).
const TIMEOUT: Duration = Duration::from_secs(30);

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

/// The directory of the server's logs, in its cache directory.
const LOG_DIR: &str = "logs";

/// A running server, talking JSON-RPC over its stdin and stdout.
struct Server {
    child: Child,
    /// The server's cache directory, removed when the server is dropped.
    cache: TempDir,
    stdin: Option<ChildStdin>,
    /// Lines from stdout: a parsed message, or a line that is not a JSON-RPC message.
    lines: Receiver<Result<Value, String>>,
    /// Messages received while waiting for another one.
    pending: Vec<Value>,
    /// The tools that the `tools/call` requests sent call, by request id, to check their results
    /// against the tools' `outputSchema`.
    calls: HashMap<u64, String>,
    /// How many `tools/call` requests were sent, each of which the log records.
    sent_calls: usize,
}

impl Server {
    fn start() -> Self {
        Self::start_in(None)
    }

    /// A running server whose default project is `project`, or which has none: it then runs in
    /// its cache directory, so that the project these tests run in is not read.
    fn start_in(project: Option<&Path>) -> Self {
        Self::start_with(project, &[])
    }

    /// [`Self::start_in`], with more environment variables.
    fn start_with(project: Option<&Path>, env: &[(&str, &str)]) -> Self {
        Self::start_with_cache(test_cache(), project, env)
    }

    /// [`Self::start_with`], with the cache `cache`, which [`test_cache`] made.
    fn start_with_cache(cache: TempDir, project: Option<&Path>, env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_chatora-rustdoc-mcp"));
        // Not the config file of the user who runs the tests (requirement R-DIST-3).
        let config = cache.path().join("config.toml");
        std::fs::write(&config, "").expect("written");
        command.env("CHATORA_RUSTDOC_MCP_CONFIG", config);
        // The tool for testing (`sleep`) in a debug build.
        command.env("CHATORA_RUSTDOC_TEST_TOOLS", "1");
        command.envs(env.iter().copied());
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
            .env("CHATORA_RUSTDOC_MCP_LOG__DIR", cache.path().join(LOG_DIR))
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
            cache,
            stdin,
            lines,
            pending: Vec::new(),
            calls: HashMap::new(),
            sent_calls: 0,
        }
    }

    /// The lines of the server's log files, oldest first (requirement R-OBS-1).
    fn log(&self) -> Vec<Value> {
        // A thread of its own writes the log, a moment after the response: its lines are taken
        // once each call has its record and no more come for a while, or after ten seconds.
        let started = Instant::now();
        let mut before = self.log_now();
        loop {
            thread::sleep(Duration::from_millis(200));
            let now = self.log_now();
            let calls = now.iter().filter(|line| line["kind"] == "call").count();
            if (calls >= self.sent_calls && now == before)
                || started.elapsed() > Duration::from_secs(10)
            {
                return now;
            }
            before = now;
        }
    }

    /// The lines of the server's log files as they are now.
    fn log_now(&self) -> Vec<Value> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.cache.path().join(LOG_DIR))
            .expect("the log directory")
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect();
        files.sort();
        files
            .iter()
            .flat_map(|path| {
                let text = std::fs::read_to_string(path).expect("readable");
                // A last line without its newline is still being written.
                let whole = text.rsplit_once('\n').map_or("", |(whole, _)| whole);
                whole
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("a line of JSON"))
                    .collect::<Vec<Value>>()
            })
            .collect()
    }

    /// Writes one message as a line to the server's stdin.
    fn send(&mut self, message: &Value) {
        if message["method"] == "tools/call"
            && let (Some(id), Some(tool)) =
                (message["id"].as_u64(), message["params"]["name"].as_str())
        {
            self.calls.insert(id, tool.to_owned());
            self.sent_calls = self.sent_calls.saturating_add(1);
        }
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
        let response = self.wait_for(&format!("the response to request {id}"), |message| {
            message["id"] == id && message.get("method").is_none()
        });
        // Every result of a tool matches its `outputSchema` (requirement R-OUT-1).
        if let (Some(tool), Some(structured)) = (
            self.calls.get(&id),
            response["result"].get("structuredContent"),
        ) {
            schema::check_output(tool, structured);
        }
        response
    }

    /// Waits for a notification with `method`.
    fn notification(&mut self, method: &str) -> Value {
        self.wait_for(&format!("a {method} notification"), |message| {
            message["method"] == method && message.get("id").is_none()
        })
    }

    /// Collects every message that arrives during `duration`, including those kept earlier.
    // Only the tests of `sleep` and of the test server for docs.rs use it, which are only in debug
    // builds.
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
    // The order of calls and the rules for versions (requirement R-MCP-6).
    let instructions = result["instructions"].as_str().expect("instructions");
    assert!(instructions.contains("`get_item`"), "{instructions}");
    assert!(server.finish().success());
}

/// The server answers at once, even when rustc, which it runs to find the host target, is slow
/// to start (requirement R-MCP-11).
#[cfg(unix)]
#[test]
fn discover_does_not_wait_for_a_slow_rustc() {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = TempDir::new().expect("a temporary directory");
    let rustc = bin.path().join("rustc");
    std::fs::write(&rustc, "#!/bin/sh\nsleep 5\nexit 1\n").expect("written");
    std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).expect("runnable");
    let path = std::env::join_paths(std::iter::once(bin.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("a PATH");
    let mut server = Server::start_with(None, &[("PATH", path.to_str().expect("UTF-8"))]);
    let started = Instant::now();
    server.send(&request(1, "server/discover", json!({})));
    assert!(server.response(1)["result"]["supportedVersions"].is_array());
    let took = started.elapsed();
    assert!(took < Duration::from_secs(2), "discover took {took:?}");
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
    // Only a build that reads projects may run cargo (requirement R-MCP-4).
    assert_eq!(
        get_item["annotations"]["readOnlyHint"],
        !cfg!(feature = "local")
    );
    assert_eq!(get_item["annotations"]["openWorldHint"], true);
    // A response while the crate is being fetched has no `data` (requirement R-OUT-9).
    assert_eq!(get_item["outputSchema"]["required"], json!(["provenance"]));
    assert_eq!(
        get_item["outputSchema"]["properties"]["status"]["enum"],
        json!(["building"])
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

/// Each tool call has a line in the log file with its `request_id`, which a tool error carries
/// too, so that a reported error leads to its line (requirements R-OBS-1 and R-ERR-2).
#[test]
fn errors_carry_the_request_id_of_their_log_line() {
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
    let found = server.response(1);
    let not_found = text_json(&server.response(2)["result"]);
    let invalid = text_json(&server.response(3)["result"]);
    assert_eq!(server.response(4)["error"]["code"], -32602);
    // A successful result carries no request_id, so that it stays the same for the same input
    // (requirement R-ARCH-5).
    assert!(
        found["result"]["structuredContent"]
            .get("request_id")
            .is_none()
    );
    let (Some(not_found_id), Some(invalid_id)) = (
        not_found["request_id"].as_str(),
        invalid["request_id"].as_str(),
    ) else {
        panic!("the errors have a request_id: {not_found} {invalid}");
    };
    assert_ne!(not_found_id, invalid_id);

    // Each call has a record, written a moment after its response.
    let log = server.log();
    assert_eq!(log[0]["kind"], "start", "{log:?}");
    let calls: Vec<&Value> = log.iter().filter(|line| line["kind"] == "call").collect();
    assert_eq!(calls.len(), 4, "{log:?}");
    let call = |request_id: &str| {
        calls
            .iter()
            .copied()
            .find(|call| call["request_id"] == request_id)
            .unwrap_or_else(|| panic!("a record of {request_id}: {log:?}"))
    };
    let record = call(not_found_id);
    assert_eq!(record["tool"], "get_item");
    assert_eq!(record["error"], "item_not_found");
    assert_eq!(record["arguments"]["path"], "demo::nothing");
    assert_eq!(call(invalid_id)["error"], "invalid_argument");
    // The crate was in the cache: nothing was fetched.
    let found_record = calls
        .iter()
        .find(|call| call.get("error").is_none())
        .expect("the record of the call that succeeded");
    assert_eq!(found_record["sources"], json!(["docs.rs"]));
    assert_eq!(found_record["cache"], "hit");
    assert!(
        found_record["bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(
        calls
            .iter()
            .any(|call| call["tool"] == "nope" && call["error"] == "jsonrpc -32602")
    );
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
    workspace_of("demo")
}

/// A workspace with the package `name` 0.1.0, whose lib is empty, and its Cargo.lock, in a new
/// temporary directory.
#[cfg(feature = "local")]
fn workspace_of(name: &str) -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    std::fs::create_dir_all(dir.path().join("src")).expect("made");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
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

/// Puts in `project` a `.cargo/config.toml` whose `build.rustc` is a script that writes `marker`
/// before it runs rustc: a trap for a cargo started there.
#[cfg(all(unix, feature = "local"))]
fn set_trap(project: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let script = project.join("trap-rustc");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec rustc \"$@\"\n",
            marker.display()
        ),
    )
    .expect("written");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("made runnable");
    std::fs::create_dir_all(project.join(".cargo")).expect("made");
    std::fs::write(
        project.join(".cargo/config.toml"),
        format!("[build]\nrustc = '{}'\n", script.display()),
    )
    .expect("written");
}

/// The lookup tools start no process for a project outside the default one, whose settings could
/// run anything; in the default project, the same trap goes off (requirements R-MCP-3 and R-QA-5).
#[cfg(all(unix, feature = "local"))]
#[test]
fn lookups_start_nothing_for_a_project_outside_the_default_one() {
    let markers = TempDir::new().expect("a temporary directory");
    let (inside_marker, outside_marker) = (
        markers.path().join("inside"),
        markers.path().join("outside"),
    );
    let project = demo_workspace();
    let outside = demo_workspace();
    set_trap(project.path(), &inside_marker);
    set_trap(outside.path(), &outside_marker);
    let mut server = Server::start_in(Some(project.path()));
    let elsewhere = outside.path().to_str().expect("UTF-8");
    let calls = [
        ("list_crates", json!({ "project": elsewhere })),
        (
            "resolve_version",
            json!({ "crate": "demo", "project": elsewhere }),
        ),
        (
            "get_item",
            json!({ "crate": "demo", "path": "hello", "project": elsewhere }),
        ),
        (
            "crate_overview",
            json!({ "crate": "demo", "project": elsewhere }),
        ),
        (
            "search_items",
            json!({ "query": "hello", "project": elsewhere }),
        ),
    ];
    for (id, (tool, arguments)) in (1_u64..).zip(calls) {
        server.send(&call_tool(id, tool, arguments));
        let result = server.response(id)["result"].clone();
        assert_eq!(result["isError"], true, "{tool}: {result}");
    }
    assert!(
        !outside_marker.exists(),
        "a process was started outside the default project"
    );
    // The control: the trap works.
    server.send(&call_tool(9, "list_crates", json!({})));
    assert_eq!(server.response(9)["result"]["isError"], false);
    assert!(
        inside_marker.exists(),
        "the trap did not go off in the default project"
    );
    assert!(server.finish().success());
}

/// Without a version, the project decides it (requirements R-MCP-3 and R-VR-2). The project here
/// is the package itself, which docs.rs does not have: its documentation is built here, for the
/// host, while every line of stdout stays a JSON-RPC message (requirement R-SEC-1, decided in
/// M2's D2). One outside the default project is not read with cargo. Without local features, no
/// project is read.
#[cfg(feature = "local")]
#[test]
fn without_a_version_the_project_decides_it() {
    let project = demo_workspace();
    std::fs::write(
        project.path().join("src/lib.rs"),
        "/// Says hello.\npub fn hello() {}\n",
    )
    .expect("written");
    let elsewhere = demo_workspace();
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(
        1,
        "get_item",
        json!({
            "crate": "demo",
            "path": "hello",
            "target": TARGET,
            "project": elsewhere.path(),
        }),
    ));
    let error = text_json(&server.response(1)["result"]);
    assert_eq!(error["reason"], "no_record", "{error}");
    // Called again while it is built past the deadline of a call.
    let mut found = Value::Null;
    for id in 2..20 {
        server.send(&call_tool(
            id,
            "get_item",
            json!({ "crate": "demo", "path": "hello" }),
        ));
        found = server.response(id)["result"].clone();
        if found["structuredContent"]["status"] != "building" {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    assert_ne!(found["isError"], true, "{found}");
    let provenance = &found["structuredContent"]["provenance"];
    assert_eq!(provenance["source"], "local", "{found}");
    assert_eq!(provenance["resolved_from"], "cargo_lock", "{found}");
    assert!(provenance["built_at"].is_string(), "{found}");
    assert_eq!(found["structuredContent"]["data"]["path"], "demo::hello");
    // The member at its version is the member, not the crate of that name on crates.io
    // (requirement R-VR-1).
    server.send(&call_tool(
        30,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "hello" }),
    ));
    let found = server.response(30)["result"]["structuredContent"].clone();
    assert_eq!(found["provenance"]["source"], "local", "{found}");
    assert_eq!(found["provenance"]["resolved_from"], "explicit", "{found}");
    assert!(warns(&found, "may differ"), "{found}");
    assert!(server.finish().success());
}

/// A project that builds its own std, with `build-std` and no `build.target`, builds for a target
/// of its own too: its member is not built here, `target_unsupported`, without starting cargo for
/// it (requirement R-TG-1, decided in M2's D7; it was built for the host since M2's D2).
#[cfg(feature = "local")]
#[test]
fn a_member_of_a_project_that_builds_its_own_std_is_not_built() {
    let project = demo_workspace();
    set_cargo_settings(project.path(), "[unstable]\nbuild-std = [\"core\"]\n");
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(
        1,
        "get_item",
        json!({ "crate": "demo", "path": "hello" }),
    ));
    let result = server.response(1)["result"].clone();
    assert_eq!(result["isError"], true, "{result}");
    let error = text_json(&result);
    assert_eq!(error["reason"], "target_unsupported", "{result}");
    assert!(error.to_string().contains("build-std"), "{result}");
    assert!(server.finish().success());
}

/// A lookup that gives the version reads the release from docs.rs when the project cannot tell
/// whether it is its member: without Cargo.lock, or a project outside the default one (decided in
/// M2's D2).
#[cfg(feature = "local")]
#[test]
fn a_given_version_is_looked_up_when_the_project_cannot_be_read() {
    let project = workspace_of("user");
    std::fs::remove_file(project.path().join("Cargo.lock")).expect("removed");
    let elsewhere = demo_workspace();
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(
        1,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET }),
    ));
    server.send(&call_tool(
        2,
        "get_item",
        json!({
            "crate": "demo@0.1.0",
            "path": "hello",
            "target": TARGET,
            "project": elsewhere.path(),
        }),
    ));
    for id in [1, 2] {
        let found = server.response(id)["result"].clone();
        assert_ne!(found["isError"], true, "{found}");
        assert_eq!(
            found["structuredContent"]["provenance"]["source"], "docs.rs",
            "{found}"
        );
    }
    assert!(server.finish().success());
}

/// A member asked for with `_` for the `-` of its package name is that package, whose features
/// tell `enabled_in_project` (requirement R-FT-2, decided in M2's D2).
#[cfg(feature = "local")]
#[test]
fn a_member_is_found_by_the_name_that_code_writes() {
    let project = workspace_of("hello-world");
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"hello-world\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [features]\ndefault = [\"extra\"]\nextra = []\n",
    )
    .expect("written");
    std::fs::write(
        project.path().join("src/lib.rs"),
        "/// Only with `extra`.\n#[cfg(feature = \"extra\")]\npub fn hi() {}\n",
    )
    .expect("written");
    let mut server = Server::start_in(Some(project.path()));
    let mut found = Value::Null;
    for id in 1..20 {
        server.send(&call_tool(
            id,
            "get_item",
            json!({ "crate": "hello_world", "path": "hi" }),
        ));
        found = server.response(id)["result"].clone();
        if found["structuredContent"]["status"] != "building" {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    assert_ne!(found["isError"], true, "{found}");
    let found = &found["structuredContent"];
    assert_eq!(found["provenance"]["source"], "local", "{found}");
    assert_eq!(found["provenance"]["crate"], "hello-world", "{found}");
    assert_eq!(found["data"]["path"], "hello_world::hi", "{found}");
    assert_eq!(found["data"]["enabled_in_project"], true, "{found}");
    assert!(server.finish().success());
}

/// A member is built with its features in the build of the whole workspace, which another member
/// may enable, as rust-analyzer checks it and `enabled_in_project` tells (requirement R-FT-3,
/// decided in M2's D3).
#[cfg(feature = "local")]
#[test]
fn a_member_is_built_with_the_features_of_the_workspace() {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"front\", \"back\"]\nresolver = \"3\"\n",
    );
    write(
        "front/Cargo.toml",
        "[package]\nname = \"front\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [features]\nextra = []\n",
    );
    write(
        "front/src/lib.rs",
        "/// Only with `extra`.\n#[cfg(feature = \"extra\")]\npub fn extra() {}\n",
    );
    write(
        "back/Cargo.toml",
        "[package]\nname = \"back\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nfront = { path = \"../front\", features = [\"extra\"] }\n",
    );
    write("back/src/lib.rs", "");
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
    let mut server = Server::start_in(Some(dir.path()));
    let mut found = Value::Null;
    for id in 1..20 {
        server.send(&call_tool(
            id,
            "get_item",
            json!({ "crate": "front", "path": "extra" }),
        ));
        found = server.response(id)["result"].clone();
        if found["structuredContent"]["status"] != "building" {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    assert_ne!(found["isError"], true, "{found}");
    let found = &found["structuredContent"];
    assert_eq!(found["provenance"]["source"], "local", "{found}");
    assert_eq!(
        found["provenance"]["features_requested"],
        json!(["extra"]),
        "{found}"
    );
    assert_eq!(found["data"]["enabled_in_project"], true, "{found}");
    assert!(server.finish().success());
}

/// With feature resolver 1 and a package at the root, `--features` is of the package that cargo
/// runs in unless `--manifest-path` names the member, which the build does (decided in M2's D3).
#[cfg(feature = "local")]
#[test]
fn a_member_under_a_root_package_of_resolver_1_gets_its_features() {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2018\"\n\n\
         [workspace]\nmembers = [\"b\"]\n\n\
         [dependencies]\nb = { path = \"b\", features = [\"x\"] }\n",
    );
    write("src/lib.rs", "");
    write(
        "b/Cargo.toml",
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2018\"\n\n\
         [features]\nx = []\n",
    );
    write(
        "b/src/lib.rs",
        "/// Only with `x`.\n#[cfg(feature = \"x\")]\npub fn x() {}\n",
    );
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
    let mut server = Server::start_in(Some(dir.path()));
    let mut found = Value::Null;
    for id in 1..20 {
        server.send(&call_tool(
            id,
            "get_item",
            json!({ "crate": "b", "path": "x" }),
        ));
        found = server.response(id)["result"].clone();
        if found["structuredContent"]["status"] != "building" {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    assert_ne!(found["isError"], true, "{found}");
    let found = &found["structuredContent"];
    assert_eq!(found["provenance"]["source"], "local", "{found}");
    assert_eq!(
        found["provenance"]["features_requested"],
        json!(["x"]),
        "{found}"
    );
    assert!(server.finish().success());
}

/// While a member is built again past `build.wait_secs`, the call gets the build before, marked
/// stale, with when to call again and the LSP for the code edited since; the new build comes after
/// (requirements R-WS-4, R-WS-9 and R-OUT-9, decided in M2's D4).
#[cfg(feature = "local")]
#[test]
fn a_member_being_built_again_is_given_as_it_was_built() {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"slow\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    // A build of some seconds, longer than the call waits for it.
    write(
        "build.rs",
        "fn main() { std::thread::sleep(std::time::Duration::from_secs(6)); }\n",
    );
    write("src/lib.rs", "/// First.\npub fn first() {}\n");
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
    let mut server = Server::start_with(Some(dir.path()), &[(
        "CHATORA_RUSTDOC_MCP_BUILD__WAIT_SECS",
        "1",
    )]);
    let first = json!({ "crate": "slow", "path": "first" });
    let found = built_item(&mut server, 1, &first);
    assert_eq!(
        found["structuredContent"]["provenance"]["source"], "local",
        "{found}"
    );
    write(
        "src/lib.rs",
        "/// First.\npub fn first() {}\n/// Second.\npub fn second() {}\n",
    );
    let mut stale = Value::Null;
    for id in 100..160 {
        server.send(&call_tool(id, "get_item", first.clone()));
        let result = server.response(id)["result"].clone();
        if result["structuredContent"]["provenance"]["stale"] == true {
            stale = result["structuredContent"].clone();
            break;
        }
        thread::sleep(Duration::from_millis(300));
    }
    assert_eq!(stale["provenance"]["stale_reason"], "rebuilding", "{stale}");
    assert!(stale["provenance"]["built_at"].is_string(), "{stale}");
    assert!(stale["retry_after_secs"].is_u64(), "{stale}");
    assert!(stale.get("status").is_none(), "{stale}");
    let text = stale.to_string();
    assert!(text.contains("LSP"), "{stale}");
    assert!(text.contains("the build of slow made at"), "{stale}");
    assert!(text.contains("`span` may have moved"), "{stale}");
    // The item just added, which the build before has not: not an error, but the build to wait
    // for.
    server.send(&call_tool(
        180,
        "get_item",
        json!({ "crate": "slow", "path": "second" }),
    ));
    let added = server.response(180)["result"].clone();
    assert_ne!(added["isError"], true, "{added}");
    assert_eq!(added["structuredContent"]["status"], "building", "{added}");
    // The new build, once it is made: until then, the build before has no `second`.
    let mut second = Value::Null;
    for id in 200..300 {
        server.send(&call_tool(
            id,
            "get_item",
            json!({ "crate": "slow", "path": "second" }),
        ));
        second = server.response(id)["result"].clone();
        if second["structuredContent"]["data"]["path"] == "slow::second" {
            break;
        }
        thread::sleep(Duration::from_millis(300));
    }
    let second = &second["structuredContent"];
    assert_ne!(second["provenance"]["stale"], true, "{second}");
    assert_eq!(second["data"]["path"], "slow::second", "{second}");
    assert!(server.finish().success());
}

/// When a member's build fails past `build.wait_secs`, a call gets `build_failed`, though it got the
/// build before while it was made (requirement R-WS-7, decided in M2's D4).
#[cfg(feature = "local")]
#[test]
fn a_member_whose_build_fails_again_is_told_failed() {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"failing\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write("build.rs", "fn main() {}\n");
    write("src/lib.rs", "/// First.\npub fn first() {}\n");
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
    let mut server = Server::start_with(Some(dir.path()), &[(
        "CHATORA_RUSTDOC_MCP_BUILD__WAIT_SECS",
        "1",
    )]);
    let first = json!({ "crate": "failing", "path": "first" });
    let found = built_item(&mut server, 1, &first);
    assert_eq!(
        found["structuredContent"]["provenance"]["source"], "local",
        "{found}"
    );
    // A build script that fails after longer than a call waits.
    write(
        "build.rs",
        "fn main() {\n    std::thread::sleep(std::time::Duration::from_secs(4));\n    \
         panic!(\"broken\");\n}\n",
    );
    let mut failed = Value::Null;
    for id in 100..200 {
        server.send(&call_tool(id, "get_item", first.clone()));
        let result = server.response(id)["result"].clone();
        if result["isError"] == true {
            failed = result;
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    let text = failed["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("build_failed"), "{failed}");
    // Kept: told again at once, with how to try again once fixed (requirement R-WS-7).
    let body = |result: &Value| -> Value {
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap_or_default())
            .unwrap_or_default()
    };
    let told = body(&failed);
    server.send(&call_tool(300, "get_item", first.clone()));
    let again = body(&server.response(300)["result"]);
    assert_eq!(again["failed_at"], told["failed_at"], "{again}");
    assert!(
        again["suggestions"].to_string().contains("refresh"),
        "{again}"
    );
    // Asked to refresh, it is built again (§6.4, decided in M2's D5).
    let mut refreshed = Value::Null;
    let refreshing = json!({ "crate": "failing", "path": "first", "refresh": true });
    for id in 400..500 {
        server.send(&call_tool(id, "get_item", refreshing.clone()));
        let result = server.response(id)["result"].clone();
        if result["isError"] == true {
            refreshed = body(&result);
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(refreshed["code"], "build_failed", "{refreshed}");
    assert_ne!(refreshed["failed_at"], told["failed_at"], "{refreshed}");
    assert!(server.finish().success());
}

/// The lib of a member with items of each visibility, for
/// [`a_members_items_tell_their_visibility_and_their_path_in_the_crate`].
#[cfg(feature = "local")]
const VISIBILITIES: &str = r"//! Demo.
pub mod open {
    /// Shared.
    pub mod inner {
        /// Up.
        pub(super) fn up() {}
        /// Alone.
        fn alone() {}
    }
}
mod hidden {
    /// Hidden.
    pub struct Hidden {
        /// Open.
        pub open: u8,
        count: u8,
        pub(crate) shared: u8,
    }
    /// Wide.
    pub(crate) fn wide() {}
    /// Act.
    pub(crate) trait Act {
        /// Acts.
        fn act(&self);
    }
    /// Kind.
    pub(crate) enum Kind {
        /// One.
        One,
    }
    impl Hidden {
        /// Secret.
        fn secret(&self) {}
    }
    impl Act for Hidden {
        fn act(&self) {}
    }
}
/// At the root.
fn rooted() {}
/// Concealed.
#[doc(hidden)]
pub fn concealed() {}
/// Shown.
pub mod shown {
    /// Deep.
    pub struct DeepShown;
}
#[doc(hidden)]
pub use shown::DeepShown;
/// Plain.
pub struct Plain;
#[doc(hidden)]
impl Plain {
    /// In a hidden impl.
    pub fn in_hidden_impl(&self) {}
}
#[doc(hidden)]
pub use std::vec::Vec as HiddenVec;
mod globbed {
    /// From a glob.
    pub fn from_glob() {}
}
pub use globbed::*;
";

/// A workspace member's items have their visibility as Rust writes it, and one without a public
/// path its path in the crate, which is taken as it is shown (requirements R-WS-3 and R-PR-2,
/// decided in M2's D6).
#[cfg(feature = "local")]
#[test]
fn a_members_items_tell_their_visibility_and_their_path_in_the_crate() {
    let project = demo_workspace();
    std::fs::write(project.path().join("src/lib.rs"), VISIBILITIES).expect("written");
    let mut server = Server::start_in(Some(project.path()));
    let mut ids = 1..;
    let mut item = |path: &str| -> Value {
        let id = ids.next().expect("an id");
        let found = built_item(
            &mut server,
            id * 100,
            &json!({ "crate": "demo", "path": path, "detail": "brief" }),
        );
        found["structuredContent"].clone()
    };
    // Without a public path: in the crate, as given, and without a warning of it.
    let hidden = item("crate::hidden::Hidden");
    assert_eq!(hidden["data"]["path"], "crate::hidden::Hidden", "{hidden}");
    assert_eq!(hidden["data"]["visibility"], "pub", "{hidden}");
    assert!(!warns(&hidden, "no public path"), "{hidden}");
    let alone = item("open::inner::alone");
    assert_eq!(alone["data"]["visibility"], "private", "{alone}");
    assert_eq!(alone["data"]["signature"], "fn alone()", "{alone}");
    let up = item("open::inner::up");
    assert_eq!(up["data"]["visibility"], "pub(in crate::open)", "{up}");
    assert_eq!(
        up["data"]["signature"], "pub(in crate::open) fn up()",
        "{up}"
    );
    assert_eq!(
        item("crate::hidden::wide")["data"]["visibility"],
        "pub(crate)"
    );
    // As its parent: the trait's and the enum's.
    assert_eq!(
        item("crate::hidden::Act::act")["data"]["visibility"],
        "pub(crate)"
    );
    assert_eq!(
        item("crate::hidden::Kind::One")["data"]["visibility"],
        "pub(crate)"
    );
    // A method of an inherent `impl`, which no module lists, and one of an `impl` of a trait of
    // the crate, which has the trait's.
    let secret = item("crate::hidden::Hidden::secret");
    assert_eq!(secret["data"]["visibility"], "private", "{secret}");
    assert_eq!(secret["data"]["signature"], "fn secret(&self)", "{secret}");
    let act = item("crate::hidden::Hidden::act");
    assert_eq!(act["data"]["visibility"], "pub(crate)", "{act}");
    // At the root, rustdoc tells it as the crate's.
    assert_eq!(item("crate::rooted")["data"]["visibility"], "pub(crate)");
    // The fields, as written.
    let normal = built_item(
        &mut server,
        5_000,
        &json!({ "crate": "demo", "path": "crate::hidden::Hidden" }),
    );
    assert_eq!(
        normal["structuredContent"]["data"]["fields"],
        json!(["pub open: u8", "count: u8", "pub(crate) shared: u8"]),
        "{normal}"
    );
    server.send(&call_tool(
        6_000,
        "list_impls",
        json!({ "crate": "demo", "path": "crate::hidden::Hidden" }),
    ));
    let impls = server.response(6_000)["result"]["structuredContent"].clone();
    assert_eq!(impls["data"]["has_private_fields"], true, "{impls}");
    // A module without a public path, with its items (decided in M2's D6).
    server.send(&call_tool(
        7_000,
        "list_module",
        json!({ "crate": "demo", "path": "crate::hidden" }),
    ));
    let listed = server.response(7_000)["result"]["structuredContent"].clone();
    assert_eq!(listed["page"]["total"], 4, "{listed}");
    assert!(server.finish().success());
}

/// A workspace member's search and lists have its items without a public path and its hidden
/// ones, with their visibility when it is not `pub` (requirement R-SR-4, decided in M2's D6).
#[cfg(feature = "local")]
#[test]
fn a_members_private_and_hidden_items_are_searched_and_listed() {
    let project = demo_workspace();
    std::fs::write(project.path().join("src/lib.rs"), VISIBILITIES).expect("written");
    let mut server = Server::start_in(Some(project.path()));
    // Built first.
    built_item(
        &mut server,
        1,
        &json!({ "crate": "demo", "path": "crate::rooted" }),
    );
    let mut ids = 100..;
    let mut call = |tool: &str, arguments: Value| -> Value {
        let id = ids.next().expect("an id");
        server.send(&call_tool(id, tool, arguments));
        server.response(id)["result"]["structuredContent"].clone()
    };
    let found = |result: &Value, path: &str| -> Value {
        result["data"]["items"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["path"] == path))
            .cloned()
            .unwrap_or_default()
    };
    let searched = call(
        "search_items",
        json!({ "crate": "demo", "query": "secret" }),
    );
    let secret = found(&searched, "crate::hidden::Hidden::secret");
    assert_eq!(secret["visibility"], "private", "{searched}");
    let searched = call(
        "search_items",
        json!({ "crate": "demo", "query": "concealed" }),
    );
    let concealed = found(&searched, "demo::concealed");
    assert_eq!(concealed["hidden"], true, "{searched}");
    assert!(
        concealed.get("visibility").is_none(),
        "pub is not told: {concealed}"
    );
    // A module without a public path, with all its items.
    let listed = call(
        "list_module",
        json!({ "crate": "demo", "path": "crate::hidden" }),
    );
    assert!(!warns(&listed, "no public path"), "{listed}");
    assert_eq!(
        found(&listed, "crate::hidden::wide")["visibility"],
        "pub(crate)",
        "{listed}"
    );
    assert!(
        found(&listed, "crate::hidden::Hidden").is_object(),
        "{listed}"
    );
    // The root, with what has no public path.
    let root = call("list_module", json!({ "crate": "demo", "path": "demo" }));
    assert_eq!(
        found(&root, "crate::rooted")["visibility"],
        "pub(crate)",
        "{root}"
    );
    assert!(found(&root, "crate::hidden").is_object(), "{root}");
    // Hidden through a hidden `impl` and a hidden re-export of another crate's item, and not
    // through a hidden re-export of what has a path that is not.
    let searched = call(
        "search_items",
        json!({ "crate": "demo", "query": "in_hidden_impl" }),
    );
    assert_eq!(
        found(&searched, "demo::Plain::in_hidden_impl")["hidden"],
        true,
        "{searched}"
    );
    assert_eq!(found(&root, "demo::HiddenVec")["hidden"], true, "{root}");
    let deep = call("get_item", json!({ "crate": "demo", "path": "DeepShown" }));
    assert_eq!(deep["data"]["path"], "demo::shown::DeepShown", "{deep}");
    assert!(deep["data"].get("hidden").is_none(), "{deep}");
    // Once, though a module without a public path is re-exported with a glob.
    let all = call(
        "list_module",
        json!({ "crate": "demo", "path": "demo", "recursive": true, "limit": 200 }),
    );
    let globbed = all["data"]["items"].as_array().map_or(0, |items| {
        items
            .iter()
            .filter(|item| {
                item["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("::from_glob"))
            })
            .count()
    });
    assert_eq!(globbed, 1, "{all}");
    let overview = call("crate_overview", json!({ "crate": "demo" }));
    assert!(
        overview["data"]["modules"]
            .as_array()
            .is_some_and(|modules| modules
                .iter()
                .any(|module| module["path"] == "crate::hidden")),
        "{overview}"
    );
    assert!(
        overview["data"]["counts"]["function"].as_u64() >= Some(5),
        "up, alone, wide, rooted and concealed: {overview}"
    );
    assert!(server.finish().success());
}

/// A workspace member's methods, impls and implementors have its private ones (decided in M2's
/// D6).
#[cfg(feature = "local")]
#[test]
fn a_members_other_lists_have_its_private_items() {
    let project = demo_workspace();
    std::fs::write(project.path().join("src/lib.rs"), VISIBILITIES).expect("written");
    let mut server = Server::start_in(Some(project.path()));
    built_item(
        &mut server,
        1,
        &json!({ "crate": "demo", "path": "crate::rooted" }),
    );
    let mut ids = 100..;
    let mut call = |tool: &str, arguments: Value| -> Value {
        let id = ids.next().expect("an id");
        server.send(&call_tool(id, tool, arguments));
        server.response(id)["result"]["structuredContent"].clone()
    };
    let methods = call(
        "list_methods",
        json!({ "crate": "demo", "path": "crate::hidden::Hidden" }),
    )
    .to_string();
    assert!(
        methods.contains(r#""path":"crate::hidden::Hidden::secret""#)
            && methods.contains(r#""visibility":"private""#),
        "{methods}"
    );
    let impls = call(
        "list_impls",
        json!({ "crate": "demo", "path": "crate::hidden::Hidden" }),
    )
    .to_string();
    assert!(impls.contains("crate::hidden::Act"), "{impls}");
    let implementors = call(
        "list_implementors",
        json!({ "crate": "demo", "path": "crate::hidden::Act" }),
    )
    .to_string();
    assert!(
        implementors.contains(r#""type":"Hidden""#),
        "{implementors}"
    );
    assert!(server.finish().success());
}

/// The result of `get_item` with `arguments`, called again while it answers `status: building`,
/// with the ids from `first` on.
#[cfg(feature = "local")]
fn built_item(server: &mut Server, first: u64, arguments: &Value) -> Value {
    let mut found = Value::Null;
    for id in first..first.saturating_add(20) {
        server.send(&call_tool(id, "get_item", arguments.clone()));
        found = server.response(id)["result"].clone();
        if found["structuredContent"]["status"] != "building" {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    found
}

/// A workspace in `ws/` of a new temporary directory, whose members' `[package.metadata.docs.rs]`
/// asks for: features within the project's build, with `--cfg my_cfg` (`asked`); only `--cfg
/// my_cfg` (`argsonly`); all its features, which clash (`clash`); all its features, which build an
/// optional dependency outside (`wide`).
#[cfg(feature = "local")]
fn docs_rs_workspace() -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    let package = |name: &str, rest: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n{rest}")
    };
    write(
        "ws/Cargo.toml",
        "[workspace]\nmembers = [\"asked\", \"argsonly\", \"clash\", \"wide\"]\nresolver = \"3\"\n",
    );
    write(
        "ws/asked/Cargo.toml",
        &package(
            "asked",
            "[features]\nextra = []\n\n[package.metadata.docs.rs]\nfeatures = [\"extra\"]\n\
             rustdoc-args = [\"--cfg\", \"my_cfg\"]\n",
        ),
    );
    write(
        "ws/asked/src/lib.rs",
        "#![allow(unexpected_cfgs)]\n/// With `extra`.\n#[cfg(feature = \"extra\")]\npub fn extra() {}\n\
         /// With `my_cfg`.\n#[cfg(my_cfg)]\npub fn cfged() {}\n",
    );
    write(
        "ws/clash/Cargo.toml",
        &package(
            "clash",
            "[features]\na = []\nb = []\n\n[package.metadata.docs.rs]\nall-features = true\n",
        ),
    );
    write(
        "ws/clash/src/lib.rs",
        "#[cfg(all(feature = \"a\", feature = \"b\"))]\ncompile_error!(\"a and b\");\n\
         /// Always.\npub fn always() {}\n",
    );
    write(
        "ws/wide/Cargo.toml",
        &package(
            "wide",
            "[dependencies]\nopt = { path = \"../../opt\", optional = true }\n\n\
             [package.metadata.docs.rs]\nall-features = true\n",
        ),
    );
    write("ws/wide/src/lib.rs", "/// Always.\npub fn always() {}\n");
    // Only `rustdoc-args`: no features are asked for, and the project's are built with.
    write(
        "ws/argsonly/Cargo.toml",
        &package(
            "argsonly",
            "[features]\ndefault = [\"on\"]\non = []\n\n[package.metadata.docs.rs]\n\
             rustdoc-args = [\"--cfg\", \"my_cfg\"]\n",
        ),
    );
    write(
        "ws/argsonly/src/lib.rs",
        "#![allow(unexpected_cfgs)]\n/// With `on`.\n#[cfg(feature = \"on\")]\npub fn on() {}\n\
         /// With `my_cfg`.\n#[cfg(my_cfg)]\npub fn cfged() {}\n",
    );
    write("opt/Cargo.toml", &package("opt", ""));
    write("opt/src/lib.rs", "");
    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(dir.path().join("ws"))
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("cargo runs");
    assert!(status.success(), "cargo generate-lockfile");
    dir
}

/// A member's `[package.metadata.docs.rs]` is built with, with the `--cfg` of its `rustdoc-args`,
/// unless its features break the build or build packages that the project's build does not:
/// then the project's features are (requirements R-FT-3 and R-MCP-3, decided in M2's D3).
#[cfg(feature = "local")]
#[test]
fn a_member_is_built_as_its_docs_rs_metadata_asks_when_it_may() {
    let dir = docs_rs_workspace();
    let mut server = Server::start_in(Some(&dir.path().join("ws")));
    let asked = built_item(
        &mut server,
        1,
        &json!({ "crate": "asked", "path": "extra" }),
    );
    assert_ne!(asked["isError"], true, "{asked}");
    let asked = &asked["structuredContent"];
    assert_eq!(
        asked["provenance"]["features_requested"],
        json!(["extra"]),
        "{asked}"
    );
    assert!(warns(asked, "as docs.rs builds it"), "{asked}");
    let cfged = built_item(
        &mut server,
        30,
        &json!({ "crate": "asked", "path": "cfged" }),
    );
    assert_ne!(cfged["isError"], true, "{cfged}");
    let clash = built_item(
        &mut server,
        60,
        &json!({ "crate": "clash", "path": "always" }),
    );
    assert_ne!(clash["isError"], true, "{clash}");
    let clash = &clash["structuredContent"];
    assert_eq!(
        clash["provenance"]["features_requested"],
        json!([]),
        "{clash}"
    );
    assert!(warns(clash, "broke the build"), "{clash}");
    // `--cfg docsrs`, which did not break it, is back for the build with the project's features.
    assert!(!warns(clash, "without --cfg docsrs"), "{clash}");
    let wide = built_item(
        &mut server,
        90,
        &json!({ "crate": "wide", "path": "always" }),
    );
    assert_ne!(wide["isError"], true, "{wide}");
    let wide = &wide["structuredContent"];
    assert!(warns(wide, "would build opt 0.1.0"), "{wide}");
    assert_eq!(
        wide["provenance"]["features_requested"],
        json!([]),
        "{wide}"
    );
    let on = built_item(
        &mut server,
        120,
        &json!({ "crate": "argsonly", "path": "on" }),
    );
    assert_ne!(on["isError"], true, "{on}");
    let on = &on["structuredContent"];
    assert_eq!(
        on["provenance"]["features_requested"],
        json!(["default", "on"]),
        "{on}"
    );
    assert!(!warns(on, "as docs.rs builds it"), "{on}");
    let cfged = built_item(
        &mut server,
        150,
        &json!({ "crate": "argsonly", "path": "cfged" }),
    );
    assert_ne!(cfged["isError"], true, "{cfged}");
    assert!(server.finish().success());
}

/// A project in `ws/` of a new temporary directory, which depends on the path dependencies
/// `libdep`, whose `Shown` it re-exports, and `buildonly`, for its build script; with the feature
/// resolver `resolver`.
#[cfg(feature = "local")]
fn path_dependencies(resolver: &str) -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    let package = |name: &str, rest: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n{rest}")
    };
    write(
        "ws/Cargo.toml",
        &package(
            "app",
            &format!(
                "resolver = \"{resolver}\"\n\n[dependencies]\nlibdep = {{ path = \"../libdep\" }}\n\n\
                 [build-dependencies]\nbuildonly = {{ path = \"../buildonly\" }}\n"
            ),
        ),
    );
    write("ws/src/lib.rs", "pub use libdep::Shown;\n");
    write("ws/build.rs", "fn main() {}\n");
    write("libdep/Cargo.toml", &package("libdep", ""));
    write(
        "libdep/src/lib.rs",
        "/// Shown.\npub struct Shown;\n/// Hidden.\nfn kept_inside() {}\n",
    );
    write("buildonly/Cargo.toml", &package("buildonly", ""));
    write(
        "buildonly/src/lib.rs",
        "/// Only for the build.\npub fn build_helper() {}\n",
    );
    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(dir.path().join("ws"))
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("cargo runs");
    assert!(status.success(), "cargo generate-lockfile");
    dir
}

/// A path dependency outside the workspace is built here, its public items only, and told so, and
/// a member's re-export goes on into it; one that only a build script uses, which `cargo rustdoc
/// -p` cannot name, is not (requirement R-WS-5, decided in M2's D7).
#[cfg(feature = "local")]
#[test]
fn a_path_dependency_is_built_here_with_its_public_items() {
    let dir = path_dependencies("3");
    let mut server = Server::start_in(Some(&dir.path().join("ws")));
    let shown = built_item(
        &mut server,
        1,
        &json!({ "crate": "libdep", "path": "Shown" }),
    );
    assert_ne!(shown["isError"], true, "{shown}");
    let shown = &shown["structuredContent"];
    assert_eq!(shown["provenance"]["source"], "local", "{shown}");
    assert_eq!(shown["provenance"]["package_source"], "path", "{shown}");
    assert!(
        shown["data"].get("visibility").is_none(),
        "public only: {shown}"
    );
    // Not its private items, as another crate sees it.
    let inside = built_item(
        &mut server,
        100,
        &json!({ "crate": "libdep", "path": "kept_inside" }),
    );
    assert!(inside.to_string().contains("item_not_found"), "{inside}");
    let only = built_item(
        &mut server,
        200,
        &json!({ "crate": "buildonly", "path": "build_helper" }),
    );
    assert!(only.to_string().contains("build_unsupported"), "{only}");
    // The member's re-export goes on into the dependency, built here.
    let followed = built_item(
        &mut server,
        300,
        &json!({ "crate": "app", "path": "Shown" }),
    );
    assert_ne!(followed["isError"], true, "{followed}");
    let followed = &followed["structuredContent"];
    assert_eq!(followed["provenance"]["crate"], "libdep", "{followed}");
    assert_eq!(
        followed["provenance"]["package_source"], "path",
        "{followed}"
    );
    // The version given is the project's: its package, not the release of crates.io
    // (requirement R-VR-1, decided in M2's D7).
    let given = json!({ "crate": "libdep@0.1.0", "path": "Shown" });
    let given = built_item(&mut server, 400, &given);
    assert_eq!(
        given["structuredContent"]["provenance"]["package_source"], "path",
        "{given}"
    );
    assert!(given.to_string().contains("is the project's"), "{given}");
    server.send(&call_tool(
        500,
        "resolve_version",
        json!({ "crate": "libdep@0.1.0" }),
    ));
    let resolved = server.response(500)["result"]["structuredContent"].clone();
    assert_eq!(resolved["provenance"]["source"], "project", "{resolved}");
    assert!(
        resolved.to_string().contains("is the project's"),
        "{resolved}"
    );
    assert!(server.finish().success());
}

/// The search of `query` in the project's crates, called again from `first` until it is not
/// `partial`, while its crates are loaded or built: the last answer.
#[cfg(feature = "local")]
fn searched_in_full(server: &mut Server, first: u64, query: &str) -> Value {
    let mut searched = Value::Null;
    for id in first..first.saturating_add(40) {
        server.send(&call_tool(id, "search_items", json!({ "query": query })));
        searched = server.response(id)["result"]["structuredContent"].clone();
        if searched.get("partial").is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    searched
}

/// The search of a project without `crate` searches its workspace members and its path
/// dependency too, built here, the dependency's public items only, waiting while they are built
/// (requirements R-SR-2 and R-SR-5, decided in M2's D7).
#[cfg(feature = "local")]
#[test]
fn a_search_of_the_project_searches_its_members_and_dependencies_built_here() {
    let dir = path_dependencies("3");
    let mut server = Server::start_in(Some(&dir.path().join("ws")));
    let searched = searched_in_full(&mut server, 1, "Shown");
    assert!(searched.get("partial").is_none(), "{searched}");
    let sources = searched["provenance"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let of = |krate: &str| {
        sources
            .iter()
            .find(|source| source["crate"] == krate)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(of("app")["source"], "local", "{searched}");
    assert_eq!(of("libdep")["package_source"], "path", "{searched}");
    assert!(
        searched["data"].to_string().contains("libdep::Shown"),
        "{searched}"
    );
    // Not the dependency's private items.
    server.send(&call_tool(
        100,
        "search_items",
        json!({ "query": "kept_inside" }),
    ));
    let inside = server.response(100)["result"]["structuredContent"].clone();
    assert_eq!(inside["data"]["items"], json!([]), "{inside}");
    assert!(server.finish().success());
}

/// A member whose package's name has a `-`, unlike its lib's, is searched with the features of the
/// project's build, which go by the package's name: its items are in the build (requirement
/// R-FT-2, decided in M2's D7).
#[cfg(feature = "local")]
#[test]
fn a_search_tells_a_members_items_in_the_build_by_its_package() {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"dash-name\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write("src/lib.rs", "/// Shown here.\npub fn shown_here() {}\n");
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
    let mut server = Server::start_in(Some(dir.path()));
    let searched = searched_in_full(&mut server, 1, "shown_here");
    let item = &searched["data"]["items"][0];
    assert_eq!(item["path"], "dash_name::shown_here", "{searched}");
    assert_ne!(item["enabled_in_project"], false, "{searched}");
    assert_eq!(
        searched["provenance"]["sources"][0]["crate"], "dash-name",
        "{searched}"
    );
    assert!(server.finish().success());
}

/// With the feature resolver 1, a build dependency is built, for the host, which the project
/// builds it for; a member or a dependency is not built for a target that the project does not
/// build for (requirement R-MCP-3, decided in M2's D7).
#[cfg(feature = "local")]
#[test]
fn a_build_dependency_is_built_for_the_host() {
    let dir = path_dependencies("1");
    std::fs::create_dir_all(dir.path().join("ws/.cargo")).expect("made");
    std::fs::write(
        dir.path().join("ws/.cargo/config.toml"),
        "[build]\ntarget = \"wasm32-unknown-unknown\"\n",
    )
    .expect("written");
    let mut server = Server::start_in(Some(&dir.path().join("ws")));
    let for_host = built_item(
        &mut server,
        1,
        &json!({ "crate": "buildonly", "path": "build_helper" }),
    );
    assert_ne!(for_host["isError"], true, "{for_host}");
    assert!(
        for_host
            .to_string()
            .contains("wasm32-unknown-unknown was asked for, but the data is for"),
        "{for_host}"
    );
    // Asked for the host, it is built too, as what the project's build builds for the host.
    let host = host_triple();
    let asked_host = built_item(
        &mut server,
        50,
        &json!({ "crate": "buildonly", "path": "build_helper", "target": host }),
    );
    assert_ne!(asked_host["isError"], true, "{asked_host}");
    // Not a member, nor a dependency for the target, for another target: nor the member of the
    // version given, which is not taken for a release of crates.io of its name.
    for (id, krate, target) in [
        (100, "libdep", "x86_64-unknown-none"),
        (200, "app", "x86_64-unknown-none"),
        (300, "app@0.1.0", "x86_64-unknown-none"),
        (400, "libdep", host.as_str()),
    ] {
        let other = built_item(
            &mut server,
            id,
            &json!({ "crate": krate, "path": "Shown", "target": target }),
        );
        assert_eq!(other["isError"], true, "{other}");
        let error = text_json(&other);
        assert_eq!(error["reason"], "no_json", "{krate} {target}: {other}");
        assert_eq!(error["retryable"], false, "{other}");
        assert!(
            error
                .to_string()
                .contains("the project builds for wasm32-unknown-unknown"),
            "{other}"
        );
    }
    assert!(server.finish().success());
}

/// The host's target, as `rustc -vV` tells it.
#[cfg(feature = "local")]
fn host_triple() -> String {
    let output = Command::new("rustc")
        .arg("-vV")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .output()
        .expect("rustc runs");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc tells the host")
        .to_owned()
}

/// In a new temporary directory, the git repository `repo/` with the package `dep` on its branch
/// `main`, the workspace `ws/` whose members are `dep`, of the same name and version, and `app`,
/// which re-exports the git one's `FromGit` and depends on `libdep/`, which re-exports it as
/// `Again`; with the environment for cargo and git, which keeps them in the directory.
#[cfg(feature = "local")]
fn git_dependency_of_a_members_name() -> (TempDir, [(&'static str, String); 5]) {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    let package = |name: &str, rest: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n{rest}")
    };
    write("gitconfig", "");
    let cargo_home = dir.path().join("cargo-home");
    let gitconfig = dir.path().join("gitconfig");
    let env = [
        ("CARGO_HOME", cargo_home.display().to_string()),
        ("GIT_CONFIG_GLOBAL", gitconfig.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM", "1".to_owned()),
        ("GIT_TERMINAL_PROMPT", "0".to_owned()),
        ("RUSTUP_AUTO_INSTALL", "0".to_owned()),
    ];
    write("repo/Cargo.toml", &package("dep", ""));
    write(
        "repo/src/lib.rs",
        "/// From git.\npub struct FromGit;\n/// Not public.\nfn inside() {}\n",
    );
    for args in [
        &["init", "--quiet", "--initial-branch=main"][..],
        &["add", "--all"],
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "--message=fixture",
        ],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path().join("repo"))
            .envs(env.clone())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }
    let url = format!(
        "file://{}{}",
        if cfg!(windows) { "/" } else { "" },
        dir.path()
            .join("repo")
            .display()
            .to_string()
            .replace('\\', "/")
    );
    let git = format!("{{ package = \"dep\", git = \"{url}\", branch = \"main\" }}");
    write(
        "ws/Cargo.toml",
        "[workspace]\nmembers = [\"dep\", \"app\"]\nresolver = \"3\"\n",
    );
    write("ws/dep/Cargo.toml", &package("dep", ""));
    write("ws/dep/src/lib.rs", "pub struct FromMember;\n");
    write(
        "ws/app/Cargo.toml",
        &package(
            "app",
            &format!("[dependencies]\nfromgit = {git}\nlibdep = {{ path = \"../../libdep\" }}\n"),
        ),
    );
    write("ws/app/src/lib.rs", "pub use fromgit::FromGit;\n");
    write(
        "libdep/Cargo.toml",
        &package("libdep", &format!("[dependencies]\nfromgit = {git}\n")),
    );
    write("libdep/src/lib.rs", "pub use fromgit::FromGit as Again;\n");
    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(dir.path().join("ws"))
        .envs(env.clone())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("cargo runs");
    assert!(status.success(), "cargo generate-lockfile");
    (dir, env)
}

/// A git dependency of the name and version of a member is built here, by the re-export of a
/// member and by the re-export of a path dependency, its public items only, and told so (decided
/// in M2's D7). The repository is reached by `file://`, and cargo keeps it in a cargo home of the
/// test.
#[cfg(feature = "local")]
#[test]
fn a_git_dependency_of_a_members_name_is_built_here() {
    let (dir, env) = git_dependency_of_a_members_name();
    let env: Vec<(&str, &str)> = env
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let mut server = Server::start_with(Some(&dir.path().join("ws")), &env);
    for (first, arguments) in [
        (1, json!({ "crate": "app", "path": "FromGit" })),
        (100, json!({ "crate": "libdep", "path": "Again" })),
    ] {
        // While it is built, as when it is not yet, it is told a dependency from git.
        let mut found = Value::Null;
        for id in first..first + 30 {
            server.send(&call_tool(id, "get_item", arguments.clone()));
            found = server.response(id)["result"]["structuredContent"].clone();
            if found["provenance"]["crate"] == "dep" {
                assert_eq!(found["provenance"]["package_source"], "git", "{found}");
            }
            if found["status"] != "building" {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
        assert_eq!(found["provenance"]["crate"], "dep", "{arguments}: {found}");
        assert_eq!(found["provenance"]["source"], "local", "{found}");
        assert_eq!(found["provenance"]["package_source"], "git", "{found}");
        assert!(found["data"].get("visibility").is_none(), "{found}");
        assert!(found.to_string().contains("From git."), "{found}");
    }
    assert!(server.finish().success());
}

/// A workspace whose member `front` re-exports the items of the other members: `middle`, which
/// depends on `back` as `renamed`; `a` and `b`, by globs; `other-pkg`, whose lib is `other_lib`
/// and whose bin is `other-pkg`; and std's `collections`, by a glob.
#[cfg(feature = "local")]
fn reexporting_workspace() -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(path, text).expect("written");
    };
    let package = |name: &str, rest: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n{rest}")
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"front\", \"middle\", \"back\", \"a\", \"b\", \"other\"]\n\
         resolver = \"3\"\n",
    );
    write(
        "front/Cargo.toml",
        &package(
            "front",
            "[dependencies]\nmiddle = { path = \"../middle\" }\na = { path = \"../a\" }\n\
             b = { path = \"../b\" }\nother-pkg = { path = \"../other\" }\n",
        ),
    );
    write(
        "front/src/lib.rs",
        "pub use middle::Thing;\npub use middle::*;\npub use a::*;\npub use b::*;\n\
         pub use std::collections::*;\npub use other_lib::Other;\n",
    );
    write("a/Cargo.toml", &package("a", ""));
    write("a/src/lib.rs", "/// In a.\npub fn in_a() {}\n");
    write("b/Cargo.toml", &package("b", ""));
    write("b/src/lib.rs", "/// In b.\npub fn in_b() {}\n");
    // A lib named other than the package, which a bin of the package's name has too.
    write(
        "other/Cargo.toml",
        &package("other-pkg", "[lib]\nname = \"other_lib\"\n"),
    );
    write("other/src/lib.rs", "/// Other.\npub struct Other;\n");
    write("other/src/main.rs", "fn main() {}\n");
    // `back` as `renamed`.
    write(
        "middle/Cargo.toml",
        &package(
            "middle",
            "[dependencies]\nrenamed = { package = \"back\", path = \"../back\" }\n",
        ),
    );
    write(
        "middle/src/lib.rs",
        "pub use renamed::Thing;\n/// In the middle.\npub fn in_middle() {}\n",
    );
    write("back/Cargo.toml", &package("back", ""));
    write("back/src/lib.rs", "/// A thing.\npub struct Thing;\n");
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

/// A member's re-export of another member's item is followed into it, built here, and so is a
/// glob re-export of one, and shown at the path asked for (requirements R-PR-2 and R-PR-4, decided
/// in M2's D6).
#[cfg(feature = "local")]
#[test]
fn a_reexport_of_another_member_is_followed_into_it() {
    let dir = reexporting_workspace();
    let mut server = Server::start_in(Some(dir.path()));
    // Into `back`: rustdoc resolves the re-export of `middle`'s re-export to `back`'s item, which
    // `front` does not depend on directly, and `middle` by another name.
    let found = built_item(
        &mut server,
        1,
        &json!({ "crate": "front", "path": "Thing" }),
    );
    assert_ne!(found["isError"], true, "{found}");
    let found = &found["structuredContent"];
    assert_eq!(found["provenance"]["source"], "local", "{found}");
    assert_eq!(found["provenance"]["crate"], "back", "{found}");
    assert_eq!(found["data"]["path"], "front::Thing", "{found}");
    assert_eq!(found["data"]["defined_at"], "back::Thing", "{found}");
    // Through the glob into `middle`.
    let globbed = built_item(
        &mut server,
        100,
        &json!({ "crate": "front", "path": "in_middle" }),
    );
    assert_ne!(globbed["isError"], true, "{globbed}");
    let globbed = &globbed["structuredContent"];
    assert_eq!(globbed["provenance"]["crate"], "middle", "{globbed}");
    assert_eq!(globbed["data"]["path"], "front::in_middle", "{globbed}");
    // Through each glob in turn, into the member that has it.
    for (id, path, member) in [(300, "in_a", "a"), (400, "in_b", "b")] {
        let found = built_item(&mut server, id, &json!({ "crate": "front", "path": path }));
        assert_eq!(
            found["structuredContent"]["provenance"]["crate"], member,
            "{found}"
        );
    }
    // Not found in `front` when no member has it, and not looked for in std or on crates.io.
    let missing = built_item(
        &mut server,
        500,
        &json!({ "crate": "front", "path": "no_such_thing" }),
    );
    assert_eq!(missing["isError"], true, "{missing}");
    assert!(missing.to_string().contains("item_not_found"), "{missing}");
    // The lib of a member, not its bin of the package's name.
    let other = built_item(
        &mut server,
        600,
        &json!({ "crate": "front", "path": "Other" }),
    );
    assert_eq!(
        other["structuredContent"]["provenance"]["crate"], "other-pkg",
        "{other}"
    );
    assert_eq!(
        other["structuredContent"]["data"]["defined_at"], "other_lib::Other",
        "{other}"
    );
    // Listed as the re-export of the member, by its crate's name, without a version that its
    // JSON does not tell.
    server.send(&call_tool(
        200,
        "list_module",
        json!({ "crate": "front", "path": "front" }),
    ));
    let listed = server.response(200)["result"]["structuredContent"].clone();
    assert!(
        listed
            .to_string()
            .contains(r#""reexport_of":{"crate":"back","path":"back::Thing"}"#),
        "{listed}"
    );
    assert!(!warns(&listed, "is unknown"), "{listed}");
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

/// Over `limits.concurrent_calls`, a call gets `busy`, and a cancelled call gives its place back
/// (requirements R-SEC-7 and R-WS-6). `sleep` is only in debug builds.
#[cfg(debug_assertions)]
#[test]
fn a_call_over_the_limit_is_busy_until_a_cancelled_call_gives_its_place_back() {
    let mut server = Server::start_with(None, &[(
        "CHATORA_RUSTDOC_MCP_LIMITS__CONCURRENT_CALLS",
        "1",
    )]);
    let mut sleep = call_tool(1, "sleep", json!({ "seconds": 30 }));
    sleep["params"]["_meta"] = meta(json!({ "progressToken": "p1" }));
    server.send(&sleep);
    server.notification("notifications/progress");
    let get_item = |id| {
        call_tool(
            id,
            "get_item",
            json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET }),
        )
    };
    server.send(&get_item(2));
    let busy = text_json(&server.response(2)["result"]);
    assert_eq!(busy["code"], "busy", "{busy}");
    assert_eq!(busy["retryable"], true);
    assert_eq!(busy["retry_after_secs"], 1);
    assert!(busy["request_id"].is_string(), "{busy}");

    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 1, "reason": "test" },
    }));
    // The place is given back as soon as the cancelled call has stopped.
    let deadline = Instant::now()
        .checked_add(TIMEOUT)
        .expect("the deadline fits in an Instant");
    let mut id = 3;
    let result = loop {
        server.send(&get_item(id));
        let result = server.response(id)["result"].clone();
        if result["isError"] != true || text_json(&result)["code"] != "busy" {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "the cancelled call kept its place"
        );
        thread::sleep(Duration::from_millis(20));
        id += 1;
    };
    assert_eq!(result["isError"], false, "{result}");

    let log = server.log();
    let error_of = |tool: &str| {
        log.iter()
            .filter(|line| line["kind"] == "call" && line["tool"] == tool)
            .filter_map(|line| line["error"].as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(error_of("sleep"), ["cancelled"], "{log:?}");
    assert!(error_of("get_item").contains(&"busy"), "{log:?}");
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

/// The values of the config file are read, and what is wrong in it is logged after the first line
/// of the log file, without stopping the server (requirements R-DIST-3 and R-OBS-1).
#[test]
fn the_config_file_is_read_and_its_problems_are_logged() {
    let dir = TempDir::new().expect("a temporary directory");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[log]\nretention_days = \"many\"\nretention = 1\n").expect("written");
    let mut server = Server::start_with(None, &[(
        "CHATORA_RUSTDOC_MCP_CONFIG",
        config.to_str().expect("UTF-8"),
    )]);
    server.send(&request(1, "tools/list", json!({})));
    assert!(server.response(1)["result"]["tools"].is_array());
    let log = server.log();
    assert_eq!(log[0]["kind"], "start", "{log:?}");
    let warnings: Vec<&str> = log
        .iter()
        .filter(|line| line["level"] == "WARN")
        .filter_map(|line| line["message"].as_str())
        .collect();
    let path = config.display().to_string();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for expected in [
        format!("log.retention_days in {path} is \"many\", but it must be"),
        format!("log.retention in {path} is not a setting of this version"),
    ] {
        assert!(
            warnings
                .iter()
                .any(|warning| warning.starts_with(&expected)),
            "{expected:?} in {warnings:?}"
        );
    }
    assert!(server.finish().success());
}

/// Offline, what the cache has is looked up, and what it lacks is `offline_miss` with the
/// versions the cache has, without asking any source. The index in the cache is used however
/// old, with when it was read (requirement R-CA-7).
#[test]
fn offline_the_cache_answers_and_what_it_lacks_is_an_offline_miss() {
    let mut server = Server::start_with(None, &[("CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE", "true")]);
    write_old_demo_index(&server);

    let get_item = |id, krate: &str| {
        call_tool(
            id,
            "get_item",
            json!({ "crate": krate, "path": "hello", "target": TARGET }),
        )
    };
    server.send(&get_item(1, "demo"));
    let found = server.response(1)["result"]["structuredContent"].clone();
    assert_eq!(found["provenance"]["version"], "0.1.0", "{found}");
    assert_eq!(
        found["provenance"]["index_fetched_at"], "2023-11-14T22:13:20Z",
        "{found}"
    );
    assert!(
        found["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings.iter().any(|warning| warning
                .as_str()
                .is_some_and(|warning| warning.contains("offline (network.offline)")))),
        "{found}"
    );

    server.send(&get_item(2, "demo@0.2.0"));
    let missed = text_json(&server.response(2)["result"]);
    assert_eq!(missed["code"], "offline_miss", "{missed}");
    assert_eq!(missed["retryable"], false);
    assert!(
        missed["suggestions"][0]
            .as_str()
            .is_some_and(|suggestion| suggestion.contains("demo@0.1.0")),
        "{missed}"
    );
    server.send(&get_item(3, "uncached"));
    let unresolved = text_json(&server.response(3)["result"]);
    assert_eq!(unresolved["code"], "offline_miss", "{unresolved}");

    server.send(&call_tool(4, "resolve_version", json!({ "crate": "demo" })));
    let resolved = server.response(4)["result"]["structuredContent"].clone();
    assert_eq!(
        resolved["provenance"]["index_fetched_at"], "2023-11-14T22:13:20Z",
        "{resolved}"
    );
    assert!(server.finish().success());
}

/// The URL of a proxy on this machine that answers every request, `CONNECT` included, with 503.
fn refusing_proxy() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port is free");
    let url = format!("http://{}", listener.local_addr().expect("bound"));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                line.clear();
            }
            // The client's failure is what the test waits for.
            let _written = reader.into_inner().write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    url
}

/// Puts in the server's cache the crates.io index file of `demo`, which lists only 0.1.0, as read
/// long ago (2023-11-14T22:13:20Z), past its TTL.
fn write_old_demo_index(server: &Server) {
    write_demo_index(server, 1_700_000_000);
}

/// Puts in the server's cache the crates.io index file of `demo`, which lists only 0.1.0, as read
/// at `fetched_at` (seconds since the Unix epoch).
fn write_demo_index(server: &Server, fetched_at: u64) {
    let index = server.cache.path().join("index.crates.io/de/mo/demo");
    std::fs::create_dir_all(index.parent().expect("a parent")).expect("made");
    let line = json!({
        "name": "demo",
        "vers": "0.1.0",
        "deps": [],
        "cksum": sha256_hex(&demo_crate_file()),
        "features": {},
        "yanked": false,
    });
    std::fs::write(&index, format!("{line}\n")).expect("written");
    std::fs::write(
        index.with_file_name("demo.meta.json"),
        json!({ "etag": null, "last_modified": null, "fetched_at": fetched_at }).to_string(),
    )
    .expect("written");
}

/// Online, when the index cannot be reached, the old index file in the cache is used, and a
/// version that it does not list is a transient `network_error`, not `version_not_found`: it may
/// have come after the file was read (requirement R-CA-3).
#[test]
fn a_version_missing_from_an_old_index_that_cannot_be_refreshed_is_a_network_error() {
    // Every request goes to a proxy on this machine that refuses them all at once, so the index
    // cannot be reached on any OS without waiting, and nothing leaves the machine.
    let unreachable = refusing_proxy();
    let unreachable = unreachable.as_str();
    let mut server = Server::start_with(None, &[
        ("HTTPS_PROXY", unreachable),
        ("https_proxy", unreachable),
        ("ALL_PROXY", unreachable),
        ("all_proxy", unreachable),
        ("NO_PROXY", ""),
        ("no_proxy", ""),
    ]);
    write_old_demo_index(&server);
    server.send(&call_tool(
        1,
        "resolve_version",
        json!({ "crate": "demo@0.2.0" }),
    ));
    let missed = text_json(&server.response(1)["result"]);
    assert_eq!(missed["code"], "network_error", "{missed}");
    assert_eq!(missed["retryable"], true, "{missed}");
    assert!(
        missed["message"]
            .as_str()
            .is_some_and(|message| message.contains("could not be reached")),
        "{missed}"
    );
    // A version that the old file lists is answered from it, with when it was read.
    server.send(&call_tool(
        2,
        "resolve_version",
        json!({ "crate": "demo@0.1.0" }),
    ));
    let found = server.response(2)["result"]["structuredContent"].clone();
    assert_eq!(found["data"]["version"], "0.1.0", "{found}");
    assert_eq!(
        found["provenance"]["index_fetched_at"], "2023-11-14T22:13:20Z",
        "{found}"
    );
    assert!(server.finish().success());
}

/// A debug build lists the tool for testing only when the tests ask for it, so that one installed
/// for use (`cargo install --debug`) does not show it.
#[cfg(debug_assertions)]
#[test]
fn the_tool_for_testing_is_only_for_the_tests() {
    let mut server = Server::start_with(None, &[("CHATORA_RUSTDOC_TEST_TOOLS", "")]);
    server.send(&request(1, "tools/list", json!({})));
    let names: Vec<Value> = server.response(1)["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].clone())
        .collect();
    assert!(!names.contains(&json!("sleep")), "{names:?}");
    server.send(&call_tool(2, "sleep", json!({ "seconds": 1 })));
    assert_eq!(server.response(2)["error"]["code"], -32602);
    assert!(server.finish().success());
}

/// Puts in `project` a `.cargo/config.toml` whose `build.rustc` hangs: it starts a grandchild that
/// runs for minutes, writes the process IDs of the grandchild, of its parent (cargo) and of
/// itself to `dir`, and waits.
#[cfg(all(unix, feature = "local"))]
fn set_hanging_rustc(project: &Path, dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let script = project.join("hanging-rustc");
    let dir = dir.display();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nsleep 300 &\necho $! > '{dir}/grandchild'\necho $PPID > '{dir}/cargo'\n\
             echo $$ > '{dir}/rustc'\nwait\n"
        ),
    )
    .expect("written");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("made runnable");
    std::fs::create_dir_all(project.join(".cargo")).expect("made");
    std::fs::write(
        project.join(".cargo/config.toml"),
        format!("[build]\nrustc = '{}'\n", script.display()),
    )
    .expect("written");
}

/// Has the server run cargo on its default project, whose `build.rustc` is [`set_hanging_rustc`]
/// writing to `dir`, and returns the process IDs of cargo, the hanging rustc and its grandchild.
#[cfg(all(unix, feature = "local"))]
fn hang(server: &mut Server, dir: &Path) -> [String; 3] {
    server.send(&call_tool(1, "list_crates", json!({})));
    // The file is there before its contents, which the script writes last.
    assert!(
        eventually(Duration::from_secs(30), || std::fs::read_to_string(
            dir.join("rustc")
        )
        .is_ok_and(|pid| pid.ends_with('\n'))),
        "the hanging rustc did not start"
    );
    ["cargo", "rustc", "grandchild"].map(|name| {
        std::fs::read_to_string(dir.join(name))
            .expect("written")
            .trim()
            .to_owned()
    })
}

/// Whether the process `pid` runs. A zombie, which has stopped and waits for its parent to take
/// its status, does not: the server takes that of a stopped cargo when it gets to it.
#[cfg(all(unix, feature = "local"))]
fn alive(pid: &str) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .expect("ps runs");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

/// Waits up to `limit` for `done`.
#[cfg(all(unix, feature = "local"))]
fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    done()
}

/// When the client closes stdin, the children of the server and what they started stop at once,
/// not after the 5 seconds that rmcp waits for the calls in progress (requirement R-WS-6).
#[cfg(all(unix, feature = "local"))]
#[test]
fn the_end_of_stdin_stops_the_children_and_what_they_started() {
    let pids = TempDir::new().expect("a temporary directory");
    let project = demo_workspace();
    set_hanging_rustc(project.path(), pids.path());
    let mut server = Server::start_in(Some(project.path()));
    let running = hang(&mut server, pids.path());
    assert!(running.iter().all(|pid| alive(pid)), "{running:?}");
    drop(server.stdin.take());
    assert!(
        eventually(Duration::from_secs(2), || running
            .iter()
            .all(|pid| !alive(pid))),
        "still running: {:?}",
        running.iter().filter(|pid| alive(pid)).collect::<Vec<_>>()
    );
    // The call goes on without cargo's answer, but starts no other cargo, whose rustc would
    // hang in its place.
    assert!(server.finish().success());
    let rustc = std::fs::read_to_string(pids.path().join("rustc")).expect("written");
    assert_eq!(rustc.trim(), running[1], "another rustc was started");
}

/// On SIGTERM (or SIGINT), as Claude Code stops the server, the children and what they started
/// stop, and the server ends (requirement R-WS-6).
#[cfg(all(unix, feature = "local"))]
#[test]
fn a_signal_stops_the_children_and_the_server() {
    let pids = TempDir::new().expect("a temporary directory");
    let project = demo_workspace();
    set_hanging_rustc(project.path(), pids.path());
    let mut server = Server::start_in(Some(project.path()));
    let running = hang(&mut server, pids.path());
    let status = Command::new("kill")
        .args(["-TERM", &server.child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success());
    assert!(
        eventually(Duration::from_secs(2), || running
            .iter()
            .all(|pid| !alive(pid))),
        "still running: {:?}",
        running.iter().filter(|pid| alive(pid)).collect::<Vec<_>>()
    );
    assert!(server.finish().success());
}

/// On Linux, cargo dies with the server even when the server is killed outright, which nothing can
/// catch (`PR_SET_PDEATHSIG`, requirement R-WS-6). What cargo started may stay; it is cleaned up
/// at the next start (M2's C4).
#[cfg(all(target_os = "linux", feature = "local"))]
#[test]
fn cargo_dies_with_a_killed_server() {
    let pids = TempDir::new().expect("a temporary directory");
    let project = demo_workspace();
    set_hanging_rustc(project.path(), pids.path());
    let mut server = Server::start_in(Some(project.path()));
    let [cargo, rustc, grandchild] = hang(&mut server, pids.path());
    server.child.kill().expect("killed");
    server.child.wait().expect("waited");
    assert!(
        eventually(Duration::from_secs(2), || !alive(&cargo)),
        "cargo {cargo} outlived the server"
    );
    // What cargo started is left; this test does not leave it.
    for pid in [rustc, grandchild] {
        drop(Command::new("kill").args(["-KILL", &pid]).status());
    }
}

/// What cargo started outlives a server that is killed outright, but the next server that starts
/// with the same cache stops it, at once and in the background, as it is known for sure to be the
/// server's: in the session of a child that the server recorded, started since it (requirement
/// R-WS-6, decided in M2's C4).
#[cfg(all(unix, feature = "local"))]
#[test]
fn the_next_server_stops_what_a_killed_server_left() {
    let pids = TempDir::new().expect("a temporary directory");
    let project = demo_workspace();
    set_hanging_rustc(project.path(), pids.path());
    let mut server = Server::start_in(Some(project.path()));
    let [_, rustc, grandchild] = hang(&mut server, pids.path());
    server.child.kill().expect("killed");
    server.child.wait().expect("waited");
    let cache = std::mem::replace(
        &mut server.cache,
        TempDir::new().expect("a temporary directory"),
    );
    assert!(alive(&grandchild), "the grandchild outlived the server");
    let next = Server::start_with_cache(cache, None, &[]);
    let stopped = eventually(Duration::from_secs(10), || {
        !alive(&rustc) && !alive(&grandchild)
    });
    // Not left behind by this test either way.
    for pid in [&rustc, &grandchild] {
        drop(Command::new("kill").args(["-KILL", pid]).status());
    }
    assert!(
        stopped,
        "rustc {rustc} or the grandchild {grandchild} still runs"
    );
    assert!(next.finish().success());
}

/// Writes `settings` as the `.cargo/config.toml` of `project`.
#[cfg(feature = "local")]
fn set_cargo_settings(project: &Path, settings: &str) {
    std::fs::create_dir_all(project.join(".cargo")).expect("made");
    std::fs::write(project.join(".cargo/config.toml"), settings).expect("written");
}

/// Whether `response` warns about something that contains `part`.
#[cfg(feature = "local")]
fn warns(response: &Value, part: &str) -> bool {
    response["warnings"].as_array().is_some_and(|warnings| {
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .is_some_and(|warning| warning.contains(part))
        })
    })
}

/// Without `target`, a lookup is for the project's `build.target`, the first of several, which
/// the response tells (requirement R-TG-1). The JSON for it is in the cache, on every OS.
#[cfg(feature = "local")]
#[test]
fn the_build_target_of_the_project_is_the_target_of_a_lookup() {
    // Not `demo`, whose member would be built instead (decided in M2's D2).
    let project = workspace_of("user");
    set_cargo_settings(
        project.path(),
        &format!("[build]\ntarget = [\"{TARGET}\", \"wasm32-unknown-unknown\"]\n"),
    );
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(
        1,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "hello" }),
    ));
    let found = server.response(1)["result"]["structuredContent"].clone();
    assert_eq!(found["provenance"]["target"], TARGET, "{found}");
    assert!(warns(&found, "several targets"), "{found}");
    assert!(server.finish().success());
}

/// A workspace whose package `app` depends on `demo` 0.1.0 from crates.io, taken from a vendor
/// directory (source replacement), so that cargo resolves it offline. Cargo.lock records the
/// checksum of the `.crate` in the test cache, as for the release on crates.io.
#[cfg(feature = "local")]
fn workspace_using_demo() -> TempDir {
    let dir = TempDir::new().expect("a temporary directory");
    let write = |path: &str, contents: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("made");
        std::fs::write(path, contents).expect("written");
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\ndemo = \"0.1.0\"\n",
    );
    write("src/lib.rs", "");
    write(
        "vendor/demo/Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write("vendor/demo/src/lib.rs", "");
    write(
        "vendor/demo/.cargo-checksum.json",
        &json!({ "files": {}, "package": sha256_hex(&demo_crate_file()) }).to_string(),
    );
    write(
        ".cargo/config.toml",
        "[source.crates-io]\nreplace-with = \"vendored\"\n\n\
         [source.vendored]\ndirectory = \"vendor\"\n",
    );
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

/// With the version given, the project's build still tells whether an item is in it when the
/// project has that release from crates.io (requirements R-VR-1 and R-FT-2; M2's B8). Without
/// the project, it does not.
#[cfg(feature = "local")]
#[test]
fn a_given_version_that_the_project_has_tells_whether_items_are_in_its_build() {
    let offline = [("CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE", "true")];
    let lookup = call_tool(
        1,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET }),
    );
    let project = workspace_using_demo();
    let mut server = Server::start_with(Some(project.path()), &offline);
    write_old_demo_index(&server);
    server.send(&lookup);
    let found = server.response(1)["result"]["structuredContent"].clone();
    assert_eq!(found["provenance"]["resolved_from"], "explicit", "{found}");
    assert_eq!(
        found["provenance"]["project_from"], "claude_project_dir",
        "{found}"
    );
    assert_eq!(found["data"]["enabled_in_project"], true, "{found}");
    assert!(server.finish().success());

    let mut server = Server::start_with(None, &offline);
    write_old_demo_index(&server);
    server.send(&lookup);
    let found = server.response(1)["result"]["structuredContent"].clone();
    assert_eq!(found["provenance"]["resolved_from"], "explicit", "{found}");
    assert!(found["data"].get("enabled_in_project").is_none(), "{found}");
    assert!(server.finish().success());
}

/// A project that builds for a target of its own is not documented for it yet: the host's target
/// is used, with a warning (decided in M2's B3b, until D7).
#[cfg(feature = "local")]
#[test]
fn a_target_of_its_own_falls_back_to_the_host_with_a_warning() {
    let project = demo_workspace();
    set_cargo_settings(project.path(), "[build]\ntarget = \"my-os.json\"\n");
    let mut server = Server::start_in(Some(project.path()));
    server.send(&call_tool(1, "list_crates", json!({})));
    let listed = server.response(1)["result"]["structuredContent"].clone();
    assert!(warns(&listed, "a target of its own"), "{listed}");
    assert!(server.finish().success());
}

/// The URL of a server on this machine that answers every request with `body` after `delay`, and
/// the number of requests it has had.
#[cfg(debug_assertions)]
fn slow_server(body: Vec<u8>, delay: Duration) -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port is free");
    let url = format!("http://{}", listener.local_addr().expect("bound"));
    let requests = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            counted.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                    line.clear();
                }
                thread::sleep(delay);
                let mut stream = reader.into_inner();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                // What the server makes of it is what the test looks at.
                let _written = stream
                    .write_all(head.as_bytes())
                    .and_then(|()| stream.write_all(&body));
            });
        }
    });
    (url, requests)
}

/// A crate whose JSON takes longer to fetch than a call waits is `status: building`, which the
/// tool's `outputSchema` allows, and the fetch goes on without the call, so that a later call
/// has the item: docs.rs is asked once (requirements R-ARCH-6, R-OUT-9 and R-WS-6).
#[cfg(debug_assertions)]
#[test]
fn a_crate_that_takes_long_to_fetch_is_building_and_then_answered() {
    let json = serde_json::to_vec(&demo_crate()).expect("serializes");
    let (docs_rs, requests) = slow_server(json, Duration::from_secs(2));
    // A call waits two thirds of a second.
    let mut server = server_with_slow_docs_rs(&docs_rs, "1");
    let arguments = json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET });
    server.send(&call_tool(1, "get_item", arguments.clone()));
    let result = server.response(1)["result"].clone();
    assert_eq!(result["isError"], false, "{result}");
    let building = &result["structuredContent"];
    assert_eq!(building["status"], "building", "{building}");
    assert_eq!(building["building_reason"], "fetching");
    assert_eq!(building["retry_after_secs"], 10);
    assert_eq!(building["provenance"]["crate"], "demo");
    assert_eq!(building["provenance"]["version"], "0.1.0");
    assert!(building.get("data").is_none());
    // Called again until the fetch that went on has ended.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(20))
        .expect("in range");
    let mut id = 1;
    let answered = loop {
        thread::sleep(Duration::from_millis(500));
        id += 1;
        server.send(&call_tool(id, "get_item", arguments.clone()));
        let result = server.response(id)["result"].clone();
        if result["structuredContent"].get("status").is_none() {
            break result;
        }
        assert!(Instant::now() < deadline, "still {result}");
    };
    assert_eq!(answered["isError"], false, "{answered}");
    assert_eq!(answered["structuredContent"]["data"]["path"], "demo::hello");
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    // The log tells a call that answered `building` from one that the cache answered.
    let log = server.log();
    let calls: Vec<&Value> = log.iter().filter(|line| line["kind"] == "call").collect();
    let first = calls.first().expect("a record of the first call");
    assert_eq!(first["status"], "building", "{first}");
    assert!(first.get("cache").is_none(), "{first}");
    let last = calls.last().expect("a record of the last call");
    assert!(last.get("status").is_none(), "{last}");
    assert_eq!(last["cache"], "hit", "{last}");
}

/// A server whose docs.rs is the server `docs_rs`, whose calls wait for work two thirds of
/// `timeout_secs`, and whose cache has the index and the `.crate` of `demo` but not its JSON, so
/// that only docs.rs is asked. Only a debug build takes the test server for docs.rs.
#[cfg(debug_assertions)]
fn server_with_slow_docs_rs(docs_rs: &str, timeout_secs: &str) -> Server {
    server_in_with_slow_docs_rs(None, docs_rs, timeout_secs)
}

/// [`server_with_slow_docs_rs`] whose default project is `project`, if any.
#[cfg(debug_assertions)]
fn server_in_with_slow_docs_rs(
    project: Option<&Path>,
    docs_rs: &str,
    timeout_secs: &str,
) -> Server {
    let server = Server::start_with(project, &[
        ("CHATORA_RUSTDOC_TEST_DOCS_RS_URL", docs_rs),
        (
            "CHATORA_RUSTDOC_MCP_LIMITS__REQUEST_TIMEOUT_SECS",
            timeout_secs,
        ),
        ("NO_PROXY", "127.0.0.1"),
        ("no_proxy", "127.0.0.1"),
    ]);
    std::fs::remove_dir_all(server.cache.path().join("docs.rs")).expect("removed");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    write_demo_index(&server, now);
    server
}

/// A call that waits for a slow fetch tells its progress with the token of its request, every
/// 3 seconds, and stops with its response; a call without a token gets none (requirement
/// R-WS-6).
#[cfg(debug_assertions)]
#[test]
fn a_call_that_waits_tells_its_progress_until_it_answers() {
    let json = serde_json::to_vec(&demo_crate()).expect("serializes");
    let (docs_rs, _requests) = slow_server(json, Duration::from_secs(10));
    // A call waits four seconds: one tick, with a margin.
    let mut server = server_with_slow_docs_rs(&docs_rs, "6");
    let arguments = json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET });
    let mut with_token = call_tool(1, "get_item", arguments.clone());
    with_token["params"]["_meta"] = meta(json!({ "progressToken": "p1" }));
    server.send(&with_token);
    server.send(&call_tool(2, "get_item", arguments));
    assert_eq!(
        server.response(1)["result"]["structuredContent"]["status"],
        "building"
    );
    let before = std::mem::take(&mut server.pending);
    let progress: Vec<&Value> = before
        .iter()
        .filter(|message| message["method"] == "notifications/progress")
        .collect();
    assert_eq!(progress.len(), 1, "{before:?}");
    assert_eq!(progress[0]["params"]["progressToken"], "p1");
    assert_eq!(progress[0]["params"]["progress"], 3.0);
    assert_eq!(
        progress[0]["params"]["message"],
        "fetching demo 0.1.0 (3 s)"
    );
    let after = server.messages_during(Duration::from_millis(3500));
    assert!(
        after
            .iter()
            .all(|message| message["method"] != "notifications/progress"),
        "progress after the response: {after:?}"
    );
    assert!(
        before
            .iter()
            .chain(&after)
            .any(|message| message["id"] == 2),
        "{after:?}"
    );
    assert!(server.finish().success());
}

/// A cancelled call tells no more progress (requirement R-WS-6).
#[cfg(debug_assertions)]
#[test]
fn a_cancelled_call_tells_no_more_progress() {
    let json = serde_json::to_vec(&demo_crate()).expect("serializes");
    let (docs_rs, _requests) = slow_server(json, Duration::from_secs(20));
    let mut server = server_with_slow_docs_rs(&docs_rs, "30");
    let mut call = call_tool(
        1,
        "get_item",
        json!({ "crate": "demo@0.1.0", "path": "hello", "target": TARGET }),
    );
    call["params"]["_meta"] = meta(json!({ "progressToken": "p1" }));
    server.send(&call);
    let progress = server.notification("notifications/progress");
    assert_eq!(progress["params"]["progressToken"], "p1");
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 1, "reason": "test" },
    }));
    let later = server.messages_during(Duration::from_secs(4));
    assert!(later.is_empty(), "sent after cancellation: {later:?}");
    assert!(server.finish().success());
}

/// A copy of `demo` 0.1.0 of crates.io in the project, vendored with other contents and the lib
/// `patched_demo`, whose checksum in Cargo.lock is not crates.io's, is built here as a dependency,
/// its public items only: looked up by its package name and by its lib name, and by the re-export
/// of a member (requirement R-VR-2, decided in M2's D7).
#[cfg(feature = "local")]
#[test]
fn a_copy_of_a_crates_io_release_that_differs_is_built_here() {
    let project = workspace_using_demo();
    let write = |path: &str, contents: &str| {
        std::fs::write(project.path().join(path), contents).expect("written");
    };
    write(
        "vendor/demo/src/lib.rs",
        "/// Patched here.\npub struct Patched;\n/// Not public.\nfn inside() {}\n",
    );
    write(
        "vendor/demo/Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [lib]\nname = \"patched_demo\"\n",
    );
    write(
        "vendor/demo/.cargo-checksum.json",
        &json!({ "files": {}, "package": "0".repeat(63) + "1" }).to_string(),
    );
    write("src/lib.rs", "pub use patched_demo::Patched;\n");
    std::fs::remove_file(project.path().join("Cargo.lock")).expect("removed");
    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(project.path())
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("cargo runs");
    assert!(status.success(), "cargo generate-lockfile");
    let offline = [("CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE", "true")];
    let mut server = Server::start_with(Some(project.path()), &offline);
    write_old_demo_index(&server);
    for (first, arguments) in [
        (1, json!({ "crate": "demo", "path": "Patched" })),
        (50, json!({ "crate": "patched_demo", "path": "Patched" })),
        (100, json!({ "crate": "app", "path": "Patched" })),
        // The version given is the project's (requirement R-VR-1).
        (150, json!({ "crate": "demo@0.1.0", "path": "Patched" })),
    ] {
        let found = built_item(&mut server, first, &arguments);
        assert_ne!(found["isError"], true, "{arguments}: {found}");
        let found = &found["structuredContent"];
        assert_eq!(
            found["data"]["path"]
                .as_str()
                .map(|path| path.ends_with("Patched")),
            Some(true),
            "{found}"
        );
        assert_eq!(found["provenance"]["source"], "local", "{found}");
        assert_eq!(
            found["provenance"]["package_source"], "crates.io",
            "{found}"
        );
        assert!(found.to_string().contains("Patched here."), "{found}");
        assert!(
            found["warnings"]
                .to_string()
                .contains("differs from crates.io's"),
            "{found}"
        );
        if arguments["crate"] == "demo@0.1.0" {
            assert!(found.to_string().contains("is the project's"), "{found}");
        }
    }
    let inside = built_item(
        &mut server,
        200,
        &json!({ "crate": "demo", "path": "inside" }),
    );
    assert!(inside.to_string().contains("item_not_found"), "{inside}");
    // The search of the project's dependencies finds it too, built here (decided in M2's D7).
    let searched = searched_in_full(&mut server, 250, "Patched");
    assert!(
        searched["data"]
            .to_string()
            .contains("patched_demo::Patched"),
        "{searched}"
    );
    // By its lib's name too, the index is read by its package's; and with its version given.
    for (id, krate) in [(300, "patched_demo"), (301, "demo@0.1.0")] {
        server.send(&call_tool(id, "resolve_version", json!({ "crate": krate })));
        let resolved = server.response(id)["result"]["structuredContent"].clone();
        assert_eq!(
            resolved["data"]["docs_rs_json"], "not_on_docs_rs",
            "{resolved}"
        );
        assert!(
            resolved.to_string().contains("differs from crates.io's"),
            "{resolved}"
        );
    }
    assert!(server.finish().success());
}

/// The project's `demo` 0.1.0, the release of crates.io, vendored with the lib `patched_demo`,
/// which docs.rs is remembered to have no JSON of (`missing`), or none for the target
/// (`no_target`), is built here as a dependency, before its `.crate` is fetched: with its version
/// from Cargo.lock, by its lib's name, with the version given, and by the re-export of a member,
/// offline (requirements R-DS-3 and R-TG-1, decided in M2's D7). A transient failure is not why.
#[cfg(feature = "local")]
#[test]
fn a_release_that_docs_rs_has_no_json_of_is_built_here() {
    let project = workspace_using_demo();
    std::fs::write(
        project.path().join("vendor/demo/src/lib.rs"),
        "/// Says hello here.\npub fn hello() -> u8 {\n    1\n}\n",
    )
    .expect("written");
    std::fs::write(
        project.path().join("vendor/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [lib]\nname = \"patched_demo\"\n",
    )
    .expect("written");
    std::fs::write(
        project.path().join("src/lib.rs"),
        "pub use patched_demo::hello;\n",
    )
    .expect("written");
    let host = host_triple();
    for (kind, told) in [
        ("missing", "docs.rs has no rustdoc JSON of demo@0.1.0"),
        ("no_target", "only for another target"),
        ("transient", ""),
    ] {
        let offline = [("CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE", "true")];
        let mut server = Server::start_with(Some(project.path()), &offline);
        write_old_demo_index(&server);
        let cache = Cache::at(server.cache.path());
        std::fs::remove_dir_all(server.cache.path().join("docs.rs")).expect("removed");
        let dir = cache.docs_rs_dir(
            &"demo".parse().expect("a crate name"),
            &Version::new(0, 1, 0),
            Some(&host.parse().expect("a target")),
        );
        std::fs::create_dir_all(&dir).expect("made");
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs();
        std::fs::write(
            dir.join("miss.json"),
            json!({ "kind": kind, "at": at, "server": env!("CARGO_PKG_VERSION") }).to_string(),
        )
        .expect("written");
        if kind == "transient" {
            server.send(&call_tool(
                1,
                "get_item",
                json!({ "crate": "demo", "path": "hello" }),
            ));
            let failed = server.response(1)["result"].clone();
            assert_eq!(failed["isError"], true, "{failed}");
            assert!(!failed.to_string().contains("built here"), "{failed}");
            assert!(server.finish().success());
            continue;
        }
        for (first, krate) in [
            (1, "demo"),
            (50, "patched_demo"),
            (100, "demo@0.1.0"),
            (200, "app"),
        ] {
            let arguments = json!({ "crate": krate, "path": "hello" });
            let found = built_item(&mut server, first, &arguments);
            assert_ne!(found["isError"], true, "{kind} {krate}: {found}");
            // Not after its `.crate`, which offline is not fetched.
            assert!(
                !found.to_string().contains("docs.rs built"),
                "{kind} {krate}: {found}"
            );
            let found = &found["structuredContent"];
            assert_eq!(
                found["provenance"]["source"], "local",
                "{kind} {krate}: {found}"
            );
            assert_eq!(
                found["provenance"]["package_source"], "crates.io",
                "{found}"
            );
            assert!(found.to_string().contains("Says hello here."), "{found}");
            assert!(
                found["warnings"].to_string().contains(told),
                "{kind} {krate}: {found}"
            );
        }
        assert!(server.finish().success());
    }
}

/// A project that builds for a target of its own, a target JSON, has its members and its path
/// dependencies not built here, `target_unsupported`, whatever the target; a dependency of
/// crates.io is read on docs.rs for the target given, without the features of the project's
/// build, which `cargo tree` would read (requirement R-TG-1, decided in M2's D7).
#[cfg(feature = "local")]
#[test]
fn a_project_for_a_target_of_its_own_is_not_built() {
    // Added to the settings that the project has, as its vendored sources.
    let own_target = |dir: &Path| {
        let config = dir.join(".cargo/config.toml");
        std::fs::create_dir_all(dir.join(".cargo")).expect("made");
        let before = std::fs::read_to_string(&config).unwrap_or_default();
        std::fs::write(
            &config,
            format!("[build]\ntarget = \"my-target.json\"\n\n{before}"),
        )
        .expect("written");
    };
    let dir = path_dependencies("3");
    own_target(&dir.path().join("ws"));
    let mut server = Server::start_in(Some(&dir.path().join("ws")));
    for (id, arguments) in [
        (1, json!({ "crate": "app", "path": "Shown" })),
        (2, json!({ "crate": "libdep", "path": "Shown" })),
        (
            3,
            json!({ "crate": "app", "path": "Shown", "target": TARGET }),
        ),
    ] {
        server.send(&call_tool(id, "get_item", arguments.clone()));
        let result = server.response(id)["result"].clone();
        assert_eq!(result["isError"], true, "{arguments}: {result}");
        let error = text_json(&result);
        assert_eq!(
            error["reason"], "target_unsupported",
            "{arguments}: {result}"
        );
        assert!(error.to_string().contains("my-target.json"), "{result}");
    }
    assert!(server.finish().success());

    let project = workspace_using_demo();
    own_target(project.path());
    let offline = [("CHATORA_RUSTDOC_MCP_NETWORK__OFFLINE", "true")];
    let mut server = Server::start_with(Some(project.path()), &offline);
    write_old_demo_index(&server);
    let found = built_item(
        &mut server,
        1,
        &json!({ "crate": "demo", "path": "hello", "target": TARGET }),
    );
    assert_ne!(found["isError"], true, "{found}");
    let found = &found["structuredContent"];
    assert_eq!(found["provenance"]["target"], TARGET, "{found}");
    assert!(found["data"].get("enabled_in_project").is_none(), "{found}");
    assert!(
        found["warnings"]
            .to_string()
            .contains("a target of its own"),
        "{found}"
    );
    // Without a target, docs.rs's default target of the crate, which its JSON tells: here only
    // in the cache's place for it, of a target that is not the host's.
    let cache = Cache::at(server.cache.path());
    let (name, version) = ("demo".parse().expect("a crate name"), Version::new(0, 1, 0));
    let mut other = demo_crate();
    other.target.triple = "aarch64-unknown-linux-gnu".to_owned();
    cache
        .write_docs_rs_json(
            &cache.docs_rs_dir(&name, &version, None),
            rustdoc_types::FORMAT_VERSION,
            &serde_json::to_vec(&other).expect("serializes"),
        )
        .expect("the cache is writable");
    let of_target = cache.docs_rs_dir(&name, &version, Some(&TARGET.parse().expect("a target")));
    std::fs::remove_dir_all(of_target).expect("removed");
    let found = built_item(&mut server, 2, &json!({ "crate": "demo", "path": "hello" }));
    assert_ne!(found["isError"], true, "{found}");
    let found = &found["structuredContent"];
    assert_eq!(
        found["provenance"]["target"], "aarch64-unknown-linux-gnu",
        "{found}"
    );
    assert!(!found.to_string().contains("was asked for"), "{found}");
    for told in [
        "its default target there",
        "is for aarch64-unknown-linux-gnu",
    ] {
        assert!(found["warnings"].to_string().contains(told), "{found}");
    }
    // So does the search of the dependencies, whose provenance has each crate's target only.
    server.send(&call_tool(4, "search_items", json!({ "query": "hello" })));
    let searched = server.response(4)["result"]["structuredContent"].clone();
    assert!(searched["provenance"].get("target").is_none(), "{searched}");
    assert_eq!(
        searched["provenance"]["sources"][0]["target"], "aarch64-unknown-linux-gnu",
        "{searched}"
    );
    // Its member, which is not built for a target of its own, is told unavailable.
    let member = searched["provenance"]["sources"]
        .as_array()
        .and_then(|sources| sources.iter().find(|source| source["crate"] == "app"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(member["status"], "unavailable", "{searched}");
    assert_eq!(member["reason"], "target_unsupported", "{searched}");
    server.send(&call_tool(3, "resolve_version", json!({ "crate": "demo" })));
    let resolved = server.response(3)["result"]["structuredContent"].clone();
    assert_eq!(resolved["data"]["docs_rs_json"], "available", "{resolved}");
    assert!(server.finish().success());
}

/// A search of the project's dependencies that finds nothing while they are still being fetched
/// is `status: building`, not an answer that nothing matches; the next search finds the item
/// (requirement R-OUT-9).
#[cfg(all(debug_assertions, feature = "local"))]
#[test]
fn a_search_that_finds_nothing_while_crates_are_fetched_is_building() {
    let json = serde_json::to_vec(&demo_crate()).expect("serializes");
    let (docs_rs, requests) = slow_server(json, Duration::from_secs(2));
    let project = workspace_using_demo();
    let mut server = server_in_with_slow_docs_rs(Some(project.path()), &docs_rs, "1");
    let arguments = json!({ "query": "hello", "target": TARGET });
    server.send(&call_tool(1, "search_items", arguments.clone()));
    let result = server.response(1)["result"].clone();
    let building = &result["structuredContent"];
    assert_eq!(building["status"], "building", "{result}");
    assert_eq!(building["building_reason"], "fetching");
    assert_eq!(building["partial"], true);
    assert_eq!(building["retry_after_secs"], 10);
    assert_eq!(building["provenance"]["sources"][0]["status"], "building");
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(20))
        .expect("in range");
    let mut id = 1;
    let found = loop {
        thread::sleep(Duration::from_millis(500));
        id += 1;
        server.send(&call_tool(id, "search_items", arguments.clone()));
        let result = server.response(id)["result"].clone();
        // Until the workspace's member, built here, is in too (decided in M2's D7).
        if result["structuredContent"].get("partial").is_none() {
            break result;
        }
        assert!(Instant::now() < deadline, "still {result}");
    };
    let found = &found["structuredContent"];
    assert_eq!(found["data"]["items"][0]["path"], "demo::hello", "{found}");
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

/// At startup, the server removes the work directories that a server that has stopped left in
/// the cache, in the background, and when it stops it leaves none of its own, nor its lock
/// (requirements R-WS-6 and R-CA-4, decided in C4).
#[cfg(feature = "local")]
#[test]
fn a_server_removes_what_stopped_servers_left_and_leaves_nothing() {
    let cache = test_cache();
    let left = cache.path().join("work/0123456789abcdef");
    std::fs::create_dir_all(left.join("build")).expect("made");
    std::fs::write(left.join("build/out"), "left").expect("written");
    let mut server = Server::start_with_cache(cache, None, &[]);
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("in range");
    while left.exists() {
        assert!(Instant::now() < deadline, "not removed");
        thread::sleep(Duration::from_millis(50));
    }
    // Kept past `finish`, which drops the server's own.
    let cache = std::mem::replace(
        &mut server.cache,
        TempDir::new().expect("a temporary directory"),
    );
    assert!(server.finish().success());
    let left_locks: Vec<_> = std::fs::read_dir(cache.path().join("locks"))
        .expect("the server made its lock")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(left_locks.is_empty(), "{left_locks:?}");
}
