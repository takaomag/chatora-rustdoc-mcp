//! Each question that a description says its tool answers is answered (requirement R-QA-5): a
//! call that answers it, and a check of the answer. That every question has one is checked
//! without the data.

use std::path::Path;

use serde_json::{Value, json};

use crate::{client::Client, schema::tools};

struct Question {
    tool: &'static str,
    /// As the description has it.
    text: &'static str,
    arguments: Value,
    /// Whether the result answers the question.
    answers: fn(&Value) -> bool,
    /// The question is about the project, which only a build with local features reads.
    local: bool,
}

const fn question(
    tool: &'static str,
    text: &'static str,
    arguments: Value,
    answers: fn(&Value) -> bool,
) -> Question {
    Question {
        tool,
        text,
        arguments,
        answers,
        local: false,
    }
}

/// The text of every string in `value`, to look for what an answer must have.
fn has(value: &Value, text: &str) -> bool {
    value.to_string().contains(text)
}

/// The questions about crates' items and traits.
fn item_questions() -> Vec<Question> {
    vec![
        question(
            "crate_overview",
            "what is this crate, and how is it organized?",
            json!({ "crate": "tokio@1.53.3" }),
            |result| {
                result["data"]["docs"]
                    .as_str()
                    .is_some_and(|docs| !docs.is_empty())
                    && has(&result["data"]["modules"], "tokio::sync")
            },
        ),
        question(
            "get_item",
            "what is the signature of `tokio::sync::Mutex::lock` in tokio 1.47.1?",
            json!({ "crate": "tokio@1.47.1", "path": "tokio::sync::Mutex::lock" }),
            |result| {
                result["provenance"]["version"] == "1.47.1"
                    && has(&result["data"]["signature"], "async fn lock")
            },
        ),
        question(
            "get_item",
            "what fields does this struct have?",
            json!({ "crate": "std", "path": "ops::Range" }),
            |result| has(&result["data"], "start") && has(&result["data"], "end"),
        ),
        question(
            "list_implementors",
            "which types in tokio implement `AsyncRead`?",
            json!({ "crate": "tokio@1.53.3", "path": "io::AsyncRead" }),
            |result| has(&result["data"]["implementors"], "TcpStream"),
        ),
        question(
            "list_implementors",
            "what must I write to implement `Iterator`?",
            json!({ "crate": "std", "path": "iter::Iterator" }),
            |result| {
                result["data"]["items"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item["name"] == "next" && item["provided"] == false)
                })
            },
        ),
        question(
            "list_implementors",
            "can I implement this trait outside its crate?",
            json!({ "crate": "serde_json@1.0.154", "path": "value::Index" }),
            |result| result["data"].get("sealed").is_some(),
        ),
        question(
            "list_impls",
            "is `tokio::sync::Mutex` Send?",
            json!({ "crate": "tokio@1.53.3", "path": "sync::Mutex" }),
            |result| result["data"]["auto_traits"]["Send"].is_string(),
        ),
        question(
            "list_impls",
            "under which conditions is `Arc<T>` Sync?",
            json!({ "crate": "std", "path": "sync::Arc" }),
            |result| {
                result["data"]["auto_traits"]["Sync"]
                    .as_str()
                    .is_some_and(|sync| sync.contains("Sync"))
            },
        ),
        question(
            "list_impls",
            "which traits does `MyType` implement?",
            json!({ "crate": "axum@0.8.9", "path": "Router" }),
            |result| has(&result["data"]["impls"], "Clone"),
        ),
    ]
}

/// The questions about methods, modules, versions and search.
fn other_questions() -> Vec<Question> {
    vec![
        question(
            "list_methods",
            "what can I call on `tokio::sync::Mutex`?",
            json!({ "crate": "tokio@1.53.3", "path": "sync::Mutex" }),
            |result| has(&result["data"]["methods"], "Mutex::lock"),
        ),
        question(
            "list_methods",
            "which trait gives `Duration` its `checked_add`?",
            json!({ "crate": "std", "path": "time::Duration", "name": "checked_add" }),
            |result| {
                result["data"]["methods"].as_array().is_some_and(|methods| {
                    methods.iter().any(|method| {
                        has(&method["path"], "checked_add") && method["from"].is_string()
                    })
                })
            },
        ),
        question(
            "list_methods",
            "does this need a `use`?",
            json!({ "crate": "tokio@1.53.3", "path": "net::TcpStream", "from": "trait" }),
            |result| {
                result["data"]["methods"]
                    .as_array()
                    .is_some_and(|methods| methods.iter().any(|method| method["needs_use"] == true))
            },
        ),
        question(
            "list_module",
            "what is in `axum::routing`?",
            json!({ "crate": "axum@0.8.9", "path": "axum::routing" }),
            |result| has(&result["data"]["items"], "axum::routing::get"),
        ),
        question(
            "list_module",
            "what does this module re-export?",
            json!({ "crate": "reqwest@0.13.5" }),
            |result| has(&result["data"]["items"], "reexport_of"),
        ),
        question(
            "resolve_version",
            "what is the latest axum?",
            json!({ "crate": "axum" }),
            |result| result["data"]["latest"].is_string(),
        ),
        question(
            "resolve_version",
            "does serde 1.0.200 exist, and is it yanked?",
            json!({ "crate": "serde@1.0.200" }),
            // `yanked` is there only when it is.
            |result| {
                result["data"]["version"] == "1.0.200"
                    && result["data"].get("yanked").is_none_or(Value::is_boolean)
            },
        ),
        question(
            "search_items",
            "which function in tokio does X?",
            json!({ "crate": "tokio@1.53.3", "query": "spawn a task", "kind": "function" }),
            |result| has(&result["data"]["items"], "tokio::spawn"),
        ),
        question(
            "search_items",
            "what is the type for Y called?",
            json!({ "crate": "tokio@1.53.3", "query": "interval" }),
            |result| has(&result["data"]["items"], "tokio::time::Interval"),
        ),
    ]
}

fn questions() -> Vec<Question> {
    let mut questions = item_questions();
    questions.extend(other_questions());
    questions.extend(
        [
            question(
                "list_crates",
                "what does this project depend on, and at which versions?",
                json!({}),
                |result| has(&result["data"]["crates"], "\"tokio\""),
            ),
            question(
                "list_crates",
                "why is `windows-sys` 0.52 in my dependencies?",
                json!({ "crate": "windows-sys" }),
                |result| {
                    result["data"]["crates"].as_array().is_some_and(|crates| {
                        !crates.is_empty()
                            && crates.iter().all(|krate| {
                                krate["used_by"].as_array().is_some_and(|by| !by.is_empty())
                            })
                    })
                },
            ),
            question(
                "resolve_version",
                "which tokio does this project use?",
                json!({ "crate": "tokio" }),
                |result| result["provenance"]["resolved_from"] == "cargo_lock",
            ),
        ]
        .map(|question| Question {
            local: true,
            ..question
        }),
    );
    questions
}

/// The questions in `description`: the quoted ones after "Answers questions such as".
fn promised(description: &str) -> Vec<String> {
    let Some((_, rest)) = description.split_once("Answers questions such as") else {
        return Vec::new();
    };
    let mut questions = Vec::new();
    let mut rest = rest.trim_start();
    while let Some(quoted) = rest.strip_prefix('"') {
        let Some((question, after)) = quoted.split_once('"') else {
            break;
        };
        questions.push(question.to_owned());
        rest = after
            .trim_start_matches([',', ' '])
            .trim_start_matches("or ")
            .trim_start();
    }
    questions
}

/// Every question that a description of this build promises has a case, and every case is for a
/// question that is promised. Questions about the project are left out without local features,
/// where the descriptions still ask them (the remote build of M6 will have its own).
#[test]
fn every_question_promised_has_a_case() {
    let questions = questions();
    for (name, tool) in tools() {
        let description = tool["description"].as_str().unwrap_or_default();
        for promised in promised(description) {
            let case = questions
                .iter()
                .find(|question| question.tool == name && question.text == promised);
            assert!(case.is_some(), "{name}: no case for {promised:?}");
        }
    }
    for question in &questions {
        let Some(tool) = tools().get(question.tool) else {
            assert!(question.local, "no tool {}", question.tool);
            continue;
        };
        let description = tool["description"].as_str().unwrap_or_default();
        assert!(
            promised(description)
                .iter()
                .any(|promised| promised == question.text),
            "{} does not promise {:?}",
            question.tool,
            question.text
        );
    }
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache, and the network"]
fn every_question_promised_is_answered() {
    let logs = tempfile::tempdir().expect("a temporary directory");
    // The project is this package.
    let mut client = Client::start(Some(Path::new(env!("CARGO_MANIFEST_DIR"))), logs.path());
    let mut unanswered = Vec::new();
    for question in questions() {
        if question.local && !cfg!(feature = "local") {
            continue;
        }
        let answer = client.call(question.tool, &question.arguments);
        let answered = match &answer {
            crate::client::Answer::Ok(result) => (question.answers)(result),
            crate::client::Answer::Error(_) => false,
        };
        if !answered {
            unanswered.push(format!(
                "{}: {:?} with {}:\n{}",
                question.tool,
                question.text,
                question.arguments,
                answer.text().chars().take(1500).collect::<String>()
            ));
        }
    }
    client.finish();
    assert!(
        unanswered.is_empty(),
        "not answered:\n{}",
        unanswered.join("\n\n")
    );
}
