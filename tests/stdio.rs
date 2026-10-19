//! Runs the server binary over stdio and checks its JSON-RPC behavior (`doc/m0-plan.md`, A2-6).
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
    io::{BufRead as _, BufReader, Write as _},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// Longest wait for a message the test expects.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The only protocol version the server supports.
const PROTOCOL_VERSION: &str = "2026-07-28";

/// The `_meta` that every 2026-07-28 request carries, plus `extra` fields.
fn meta(extra: &Value) -> Value {
    let mut meta = json!({
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    if let (Some(meta), Some(extra)) = (meta.as_object_mut(), extra.as_object()) {
        meta.extend(extra.clone());
    }
    meta
}

/// A 2026-07-28 request: `params` plus the required `_meta`.
fn request(id: u64, method: &str, params: &Value) -> Value {
    let mut params = params.clone();
    params["_meta"] = meta(&json!({}));
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A `tools/call` request.
fn call_tool(id: u64, name: &str, arguments: &Value) -> Value {
    request(
        id,
        "tools/call",
        &json!({ "name": name, "arguments": arguments }),
    )
}

/// Parses the JSON in `content[0].text` of a tool result.
fn text_json(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .expect("content[0] is text");
    serde_json::from_str(text).expect("the text is JSON")
}

/// A running server, talking JSON-RPC over its stdin and stdout.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Lines from stdout: a parsed message, or a line that is not a JSON-RPC message.
    lines: Receiver<Result<Value, String>>,
    /// Messages received while waiting for another one.
    pending: Vec<Value>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_chatora-rustdoc-mcp"))
            .arg("serve")
            .env("CHATORA_RUSTDOC_LOG__FILTER", "off")
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
    server.send(&request(1, "server/discover", &json!({})));
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
    server.send(&request(1, "tools/list", &json!({})));
    server.send(&request(2, "tools/list", &json!({ "cursor": "x" })));
    let response = server.response(1);
    let result = &response["result"];
    let names: Vec<&Value> = result["tools"]
        .as_array()
        .expect("tools is an array")
        .iter()
        .map(|tool| &tool["name"])
        .collect();
    assert_eq!(names, [
        &json!("echo"),
        &json!("probe_result"),
        &json!("sleep")
    ]);
    assert_eq!(result["ttlMs"], 3_600_000);
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(server.response(2)["error"]["code"], -32602);
    assert!(server.finish().success());
}

#[test]
fn echo_returns_structured_content_and_tool_errors() {
    let mut server = Server::start();
    server.send(&call_tool(1, "echo", &json!({ "text": "hi" })));
    server.send(&call_tool(2, "echo", &json!({})));
    server.send(&call_tool(3, "nope", &json!({})));

    let response = server.response(1);
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"], json!({ "text": "hi" }));
    assert_eq!(text_json(result), result["structuredContent"]);

    // Bad arguments are a tool error whose text is self-contained JSON, with no structured
    // content (requirements R-ARG-1, R-ERR-2 and R-OUT-1).
    let response = server.response(2);
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert!(result.get("structuredContent").is_none());
    let error = text_json(result);
    assert_eq!(error["code"], "invalid_argument");
    assert_eq!(error["retryable"], false);

    // Only an unknown tool is a JSON-RPC error.
    assert_eq!(server.response(3)["error"]["code"], -32602);
    assert!(server.finish().success());
}

#[test]
fn protocol_errors_follow_the_2026_07_28_rules() {
    let mut server = Server::start();
    // The first request with `_meta` starts the session.
    server.send(&request(1, "tools/list", &json!({})));
    server.response(1);
    server.send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    server.send(&request(3, "ping", &json!({})));
    server.send(&request(4, "logging/setLevel", &json!({ "level": "info" })));
    assert_eq!(server.response(2)["error"]["code"], -32602);
    assert_eq!(server.response(3)["error"]["code"], -32601);
    assert_eq!(server.response(4)["error"]["code"], -32601);
    assert!(server.finish().success());
}

/// rmcp behaviors that differ from the requirements and cannot be changed from the handler. If
/// one of these assertions fails, rmcp has changed: update the requirements and `doc/m0-plan.md`.
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
        &json!({ "name": "echo", "arguments": "hi" }),
    ));
    assert_eq!(server.response(2)["error"]["code"], -32601);
    assert!(server.finish().success());
}

#[test]
fn progress_is_reported_and_a_cancelled_request_gets_no_response() {
    let mut server = Server::start();
    let mut sleep = call_tool(1, "sleep", &json!({ "seconds": 5 }));
    sleep["params"]["_meta"] = meta(&json!({ "progressToken": "p1" }));
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
    server.send(&call_tool(2, "echo", &json!({ "text": "after" })));
    assert_eq!(
        server.response(2)["result"]["structuredContent"]["text"],
        "after"
    );
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
        &json!({ "notifications": { "toolsListChanged": true } }),
    ));
    let acknowledged = server.notification("notifications/subscriptions/acknowledged");
    assert_eq!(
        acknowledged["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        5
    );
    assert_eq!(acknowledged["params"]["notifications"], json!({}));
    // The subscription does not block other requests.
    server.send(&call_tool(6, "echo", &json!({ "text": "during listen" })));
    assert_eq!(server.response(6)["result"]["isError"], false);
    // A client cancels the subscription before it leaves. Otherwise rmcp waits 5 seconds for it
    // to end after stdin closes, as it does for every request still in progress.
    server.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 5 },
    }));
    assert!(server.finish().success());
}
