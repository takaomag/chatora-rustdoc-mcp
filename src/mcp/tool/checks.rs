//! Checks of what `tools/list` and `server/discover` give the model: names, titles, descriptions,
//! the instructions, schemas and annotations (requirements R-QA-5, R-MCP-4, R-MCP-6, R-MCP-7,
//! R-MCP-10 and §6.2). They run in each build configuration, whose tools differ.

use std::collections::BTreeSet;

use rmcp::model::Tool;
use serde_json::{Map, Value};

use super::definitions;
use crate::mcp::INSTRUCTIONS;

/// The longest description, and the longest instructions (§6.2).
const MAX_DESCRIPTION_CHARS: usize = 2048;

/// The longest full name of a tool that the model is sent (requirement R-MCP-7).
const MAX_FULL_NAME_CHARS: usize = 128;

/// The prefix that Claude Code gives the tools of this server installed as a plugin, the longest
/// of its prefixes (requirement R-MCP-7).
const PLUGIN_PREFIX: &str = "mcp__plugin_chatora-rustdoc-mcp_chatora-rustdoc-mcp__";

/// The tool for testing progress and cancellation, which is not for the model.
const TEST_TOOL: &str = "sleep";

/// The JSON Schema keywords that the schemas may use: a plain subset of 2020-12 that clients
/// handle (requirement R-MCP-10).
const KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "additionalProperties",
    "enum",
    "minimum",
    "maximum",
    "description",
    "anyOf",
    "$defs",
    "$ref",
];

/// The types of JSON Schema, without `null` (requirement R-MCP-10).
const TYPES: &[&str] = &["object", "array", "string", "integer", "number", "boolean"];

fn tools() -> Vec<Tool> {
    definitions()
        .into_iter()
        .filter(|tool| tool.name != TEST_TOOL)
        .collect()
}

fn names() -> BTreeSet<String> {
    definitions()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect()
}

fn description(tool: &Tool) -> &str {
    tool.description.as_deref().unwrap_or_default()
}

#[test]
fn names_are_snake_case_and_short_enough_with_any_prefix() {
    for tool in definitions() {
        let name = &tool.name;
        let words: Vec<&str> = name.split('_').collect();
        assert!(
            words
                .iter()
                .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase())),
            "{name} is not snake_case"
        );
        // A verb and a noun (requirement R-MCP-7).
        assert!(words.len() >= 2 || name == TEST_TOOL, "{name} is one word");
        let full = format!("{PLUGIN_PREFIX}{name}");
        assert!(full.len() <= MAX_FULL_NAME_CHARS, "{full} is too long");
    }
}

#[test]
fn each_tool_has_a_title_of_its_own() {
    for tool in tools() {
        assert!(
            tool.title.as_deref().is_some_and(|title| !title.is_empty()),
            "{} has no title",
            tool.name
        );
        assert!(
            tool.annotations
                .as_ref()
                .is_none_or(|annotations| annotations.title.is_none()),
            "{} has annotations.title",
            tool.name
        );
    }
}

/// The parts that §6.2 asks of a description: the questions it answers, what it returns, and
/// which other tool to use instead, within the length, with Rust in its first sentence.
#[test]
fn descriptions_have_their_parts_within_the_length() {
    let names = names();
    for tool in tools() {
        let name = &tool.name;
        let text = description(&tool);
        let len = text.chars().count();
        assert!(len <= MAX_DESCRIPTION_CHARS, "{name}: {len} characters");
        let first_sentence = text.split(". ").next().unwrap_or_default();
        assert!(
            first_sentence.contains("Rust"),
            "{name}: the first sentence does not say Rust: {first_sentence}"
        );
        assert!(
            text.contains("Answers questions such as"),
            "{name}: no questions"
        );
        assert!(
            text.contains("Returns"),
            "{name}: nothing on what it returns"
        );
        assert!(
            names
                .iter()
                .any(|other| other != name.as_ref() && text.contains(&format!("`{other}`"))),
            "{name}: no other tool to use instead"
        );
    }
}

#[test]
fn the_instructions_are_within_the_length() {
    let len = INSTRUCTIONS.chars().count();
    assert!(len <= MAX_DESCRIPTION_CHARS, "{len} characters");
}

/// The tools that the requirements plan (§6.3), whether this build has them or not.
fn planned_tools() -> BTreeSet<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/doc/requirements.md");
    let requirements = std::fs::read_to_string(path).expect("the requirements are readable");
    let table = requirements
        .split("### 6.3 ")
        .nth(1)
        .and_then(|rest| rest.split("\n### ").next())
        .expect("§6.3 is there");
    let planned: BTreeSet<String> = table
        .lines()
        .filter_map(|line| line.strip_prefix("| `")?.split('`').next())
        .map(str::to_owned)
        .collect();
    assert!(planned.contains("get_item"), "{planned:?}");
    planned
}

/// A tool that a description or the instructions name must be one of this build: not one planned
/// for later, nor one left out of this configuration (requirement R-QA-5).
#[test]
fn every_tool_named_is_one_of_this_build() {
    let names = names();
    let unbuilt: BTreeSet<String> = planned_tools().difference(&names).cloned().collect();
    let texts = tools()
        .into_iter()
        .map(|tool| (tool.name.to_string(), description(&tool).to_owned()))
        .chain([("instructions".to_owned(), INSTRUCTIONS.to_owned())]);
    for (whose, text) in texts {
        for name in &unbuilt {
            assert!(
                !text.contains(&format!("`{name}`")),
                "{whose} names {name}, which this build does not have"
            );
        }
    }
}

/// Walks a schema, checking each subschema: only the keywords of [`KEYWORDS`], no `null` type,
/// `$ref` only within the schema, and in an input schema a `minimum` on each integer.
fn check_schema(whose: &str, schema: &Map<String, Value>, input: bool) {
    for (keyword, value) in schema {
        assert!(
            KEYWORDS.contains(&keyword.as_str()),
            "{whose}: keyword {keyword}"
        );
        match (keyword.as_str(), value) {
            ("type", Value::String(kind)) => {
                assert!(TYPES.contains(&kind.as_str()), "{whose}: type {kind}");
            },
            ("type", other) => panic!("{whose}: type {other}"),
            ("enum", Value::Array(values)) => {
                assert!(!values.contains(&Value::Null), "{whose}: null in an enum");
            },
            ("$ref", Value::String(target)) => {
                assert!(target.starts_with("#/$defs/"), "{whose}: $ref {target}");
            },
            ("properties" | "$defs", Value::Object(subschemas)) => {
                for (name, subschema) in subschemas {
                    let Value::Object(subschema) = subschema else {
                        panic!("{whose}.{name} is not a schema");
                    };
                    check_schema(&format!("{whose}.{name}"), subschema, input);
                }
            },
            ("items" | "additionalProperties", Value::Object(subschema)) => {
                check_schema(&format!("{whose}[]"), subschema, input);
            },
            ("anyOf", Value::Array(subschemas)) => {
                for subschema in subschemas {
                    let Value::Object(subschema) = subschema else {
                        panic!("{whose}: anyOf with something other than a schema");
                    };
                    check_schema(&format!("{whose}|"), subschema, input);
                }
            },
            _ => {},
        }
    }
    if input && schema.get("type") == Some(&Value::from("integer")) {
        assert!(
            schema.contains_key("minimum"),
            "{whose}: an integer without a minimum"
        );
    }
}

/// The schemas are plain JSON Schema 2020-12 that strict clients take (requirement R-MCP-10).
#[test]
fn schemas_are_plain_and_their_roots_are_objects() {
    for tool in definitions() {
        let name = &tool.name;
        let mut schemas = vec![("input", tool.input_schema.as_ref())];
        if let Some(output) = &tool.output_schema {
            schemas.push(("output", output.as_ref()));
        }
        for (which, schema) in schemas {
            let whose = format!("{name} {which}");
            assert_eq!(
                schema.get("type"),
                Some(&Value::from("object")),
                "{whose}: the root is not an object"
            );
            for combinator in ["anyOf", "oneOf", "allOf"] {
                assert!(
                    !schema.contains_key(combinator),
                    "{whose}: {combinator} at the root"
                );
            }
            check_schema(&whose, schema, which == "input");
        }
        // The names of the arguments that the Claude API takes (requirement R-MCP-7).
        if let Some(Value::Object(properties)) = tool.input_schema.get("properties") {
            for property in properties.keys() {
                assert!(
                    (1..=64).contains(&property.len())
                        && property
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)),
                    "{name}: argument {property}"
                );
            }
        }
    }
}

/// With local features, every tool may run cargo or rustc, so none is read-only; without them,
/// none runs anything (requirement R-MCP-4). Each may reach docs.rs or crates.io.
#[test]
fn annotations_say_what_the_tools_may_do() {
    for tool in tools() {
        let name = &tool.name;
        let annotations = tool
            .annotations
            .unwrap_or_else(|| panic!("{name} has no annotations"));
        assert_eq!(
            annotations.read_only_hint,
            Some(!cfg!(feature = "local")),
            "{name}"
        );
        assert_eq!(annotations.destructive_hint, Some(false), "{name}");
        assert_eq!(annotations.idempotent_hint, Some(true), "{name}");
        assert_eq!(annotations.open_world_hint, Some(true), "{name}");
    }
}

/// Without local features, no tool takes `project`, which would do nothing (requirement R-QA-5).
#[test]
fn only_a_build_that_reads_projects_takes_project() {
    for tool in tools() {
        let takes_project = tool
            .input_schema
            .get("properties")
            .and_then(|properties| properties.get("project"))
            .is_some();
        assert!(
            !takes_project || cfg!(feature = "local"),
            "{} takes project",
            tool.name
        );
    }
}
