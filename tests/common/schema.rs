//! The check of tool results against the tools' `outputSchema` (requirements R-OUT-1 and
//! R-QA-5), shared by the integration tests.

use std::{
    collections::BTreeMap,
    io::{BufRead as _, BufReader, Write as _},
    process::{Command, Stdio},
    sync::OnceLock,
};

use serde_json::{Map, Value, json};

/// The tools of this build by name, as `tools/list` gives them.
pub(crate) fn tools() -> &'static BTreeMap<String, Value> {
    static TOOLS: OnceLock<BTreeMap<String, Value>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let logs = tempfile::tempdir().expect("a temporary directory");
        let mut child = Command::new(env!("CARGO_BIN_EXE_chatora-rustdoc-mcp"))
            .arg("serve")
            .env("CHATORA_RUSTDOC_MCP_LOG__DIR", logs.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server starts");
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
            } },
        });
        writeln!(stdin, "{request}").expect("the server reads stdin");
        drop(stdin);
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout is piped"))
            .read_line(&mut line)
            .expect("a response");
        assert!(child.wait().expect("the server exits").success());
        let response: Value = serde_json::from_str(&line).expect("JSON");
        response["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| {
                (
                    tool["name"].as_str().expect("a name").to_owned(),
                    tool.clone(),
                )
            })
            .collect()
    })
}

/// Checks the structured content of a result of `tool` against its `outputSchema`
/// (requirement R-OUT-1), and that it has no field that the schema does not declare.
#[track_caller]
pub(crate) fn check_output(tool: &str, structured: &Value) {
    let schema = &tools()
        .get(tool)
        .unwrap_or_else(|| panic!("no tool {tool}"))["outputSchema"];
    let mut problems = Vec::new();
    check(schema, structured, tool, &mut problems);
    assert!(
        problems.is_empty(),
        "{tool}: the result does not match its outputSchema:\n{}\n{structured}",
        problems.join("\n")
    );
}

/// Checks `value` at `at` against `schema`, the subset of JSON Schema that the tools use, and
/// adds what does not match to `problems`.
fn check(schema: &Value, value: &Value, at: &str, problems: &mut Vec<String>) {
    if let Some(Value::Array(alternatives)) = schema.get("anyOf") {
        let matches = alternatives.iter().any(|alternative| {
            let mut found = Vec::new();
            check(alternative, value, at, &mut found);
            found.is_empty()
        });
        if !matches {
            problems.push(format!("{at}: none of anyOf matches {value}"));
        }
    }
    if let Some(Value::String(kind)) = schema.get("type") {
        let fits = match kind.as_str() {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            _ => false,
        };
        if !fits {
            problems.push(format!("{at}: {value} is not {kind}"));
            return;
        }
    }
    if let Some(Value::Array(values)) = schema.get("enum")
        && !values.contains(value)
    {
        problems.push(format!("{at}: {value} is not one of {values:?}"));
    }
    if let (Some(minimum), Some(number)) = (
        schema.get("minimum").and_then(Value::as_f64),
        value.as_f64(),
    ) && number < minimum
    {
        problems.push(format!("{at}: {number} is below {minimum}"));
    }
    if let (Some(maximum), Some(number)) = (
        schema.get("maximum").and_then(Value::as_f64),
        value.as_f64(),
    ) && number > maximum
    {
        problems.push(format!("{at}: {number} is above {maximum}"));
    }
    match value {
        Value::Object(fields) => check_object(schema, fields, at, problems),
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    check(item_schema, item, &format!("{at}[{index}]"), problems);
                }
            }
        },
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {},
    }
}

fn check_object(schema: &Value, fields: &Map<String, Value>, at: &str, problems: &mut Vec<String>) {
    for required in schema["required"].as_array().into_iter().flatten() {
        let name = required.as_str().unwrap_or_default();
        if !fields.contains_key(name) {
            problems.push(format!("{at}: no {name}"));
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    let additional = schema.get("additionalProperties");
    // A schema of alternatives leaves the fields to them.
    if properties.is_none() && additional.is_none() {
        return;
    }
    for (name, field) in fields {
        let at = format!("{at}.{name}");
        match (
            properties.and_then(|properties| properties.get(name)),
            additional,
        ) {
            (Some(field_schema), _) | (None, Some(field_schema)) => {
                check(field_schema, field, &at, problems);
            },
            (None, None) => problems.push(format!("{at}: not in the schema")),
        }
    }
}
