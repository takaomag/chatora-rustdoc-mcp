//! A server asked one request at a time over stdio, whose results are checked against their
//! `outputSchema`.

use std::{
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};

use crate::schema::check_output;

/// The `_meta` that every request of MCP 2026-07-28 has.
fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// A server asked one request at a time.
pub(crate) struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// The result of a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The structured content of a result.
    Ok(Value),
    /// The JSON of an error's text, without its `request_id`, which differs on purpose
    /// (requirement R-ERR-2).
    Error(Value),
}

impl Answer {
    /// The structured content, or a panic that shows the error.
    #[track_caller]
    pub(crate) fn ok(&self) -> &Value {
        match self {
            Self::Ok(value) => value,
            Self::Error(error) => panic!("an error: {error}"),
        }
    }

    /// As the client reads it. Maps that differ only in order are equal as values, but not as
    /// text.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Ok(value) => value.to_string(),
            Self::Error(error) => json!({ "error": error }).to_string(),
        }
    }
}

impl Client {
    /// Starts a server with the default project `project`, logging to `logs`. The cache is that
    /// of the environment (`CHATORA_RUSTDOC_MCP_CACHE__DIR`).
    pub(crate) fn start(project: Option<&Path>, logs: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_chatora-rustdoc-mcp"));
        // Not the config file of the user who runs the tests (requirement R-DIST-3). The cache
        // is still the one of the environment.
        let config = logs.join("config.toml");
        std::fs::write(&config, "").expect("written");
        command.env("CHATORA_RUSTDOC_MCP_CONFIG", config);
        match project {
            Some(project) => command.env("CLAUDE_PROJECT_DIR", project),
            None => command.env_remove("CLAUDE_PROJECT_DIR"),
        };
        let mut child = command
            .arg("serve")
            .env("CHATORA_RUSTDOC_MCP_LOG__DIR", logs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    /// Sends a request and returns its result.
    pub(crate) fn request(&mut self, method: &str, mut params: Value) -> Value {
        let id = self.next_id;
        self.next_id = id.saturating_add(1);
        params["_meta"] = meta();
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{request}").expect("the server reads stdin");
        stdin.flush().expect("the server reads stdin");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("a response");
        let mut response: Value = serde_json::from_str(&line).expect("JSON");
        assert_eq!(response["id"], id, "{response}");
        response["result"].take()
    }

    /// Calls `tool` with `arguments`, with the target of the data if they name none (but for
    /// std). A result is checked against the tool's `outputSchema`.
    pub(crate) fn call(&mut self, tool: &str, arguments: &Value) -> Answer {
        let arguments = with_data_target(arguments);
        let call = json!({ "name": tool, "arguments": arguments });
        let mut result = self.request("tools/call", call.clone());
        // While the work it needs goes on, as cargo resolving the project on a slow runner, the
        // answer comes again (requirement R-OUT-9): within a few minutes.
        for _ in 0..30 {
            if result["structuredContent"]["status"] != "building" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
            result = self.request("tools/call", call.clone());
        }
        if result["isError"] == true {
            let mut error: Value = result["content"][0]["text"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or_else(|| panic!("an error with JSON in its text: {result}"));
            if let Some(error) = error.as_object_mut() {
                error.remove("request_id");
            }
            return Answer::Error(error);
        }
        let structured = result["structuredContent"].clone();
        check_output(tool, &structured);
        Answer::Ok(structured)
    }

    /// Closes stdin and waits for the server to exit.
    pub(crate) fn finish(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().expect("the server exits").success());
    }
}

/// The target of the data, which docs.rs builds most releases for alone since 2026-05-01: the
/// same on every OS, so that the answers do not depend on the host's (decided in M2's D7). Which
/// target a lookup without one takes is tested in `tests/stdio.rs`.
const DATA_TARGET: &str = "x86_64-unknown-linux-gnu";

/// `arguments` with `target` set to [`DATA_TARGET`] if they have none, but for a crate of std,
/// which this server reads for the host from the Rust distribution on every OS; or without it,
/// if they set it to `null`.
fn with_data_target(arguments: &Value) -> Value {
    let mut arguments = arguments.clone();
    let of_std = arguments
        .get("crate")
        .and_then(Value::as_str)
        .map(|krate| krate.split('@').next().unwrap_or(krate))
        .map(|name| name.to_ascii_lowercase().replace('-', "_"))
        .is_some_and(|name| {
            matches!(
                name.as_str(),
                "std" | "core" | "alloc" | "proc_macro" | "test"
            )
        });
    // `null` leaves it out: for what is built here, for the host, as a workspace outside the
    // project that `build_external_docs` documents.
    if let Some(fields) = arguments.as_object_mut()
        && fields.get("target") == Some(&Value::Null)
    {
        fields.remove("target");
        return arguments;
    }
    if let Some(fields) = arguments.as_object_mut()
        && !of_std
        && !fields.contains_key("target")
    {
        fields.insert("target".to_owned(), json!(DATA_TARGET));
    }
    arguments
}
