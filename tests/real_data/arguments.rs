//! Each argument that a tool declares is used: changing its value changes the result (requirement
//! R-QA-5). That every declared argument has a case is checked without the data.

use std::path::Path;

use serde_json::{Value, json};

use crate::{
    client::{Answer, Client},
    schema::tools,
};

/// A release in the test data.
const TOKIO: &str = "tokio@1.53.3";
const AXUM: &str = "axum@0.8.9";

/// Another target than that of the test data (the client's `DATA_TARGET`), whose JSON is not in
/// it.
const OTHER_TARGET: &str = "x86_64-pc-windows-msvc";

/// A workspace inside the default project (this package) that is not part of it.
const OTHER_PROJECT: &str = "poc/m0a-a10";

/// How a case changes the arguments of its base call.
enum Change {
    /// Sets the argument to the value.
    Set(Value),
    /// Passes the `next_cursor` of the base call's result.
    NextCursor,
    /// Changes what the server does but not a result that worked, as another test checks.
    CheckedElsewhere,
}

struct Case {
    tool: &'static str,
    argument: &'static str,
    base: Value,
    change: Change,
}

fn case(tool: &'static str, argument: &'static str, base: Value, change: Change) -> Case {
    Case {
        tool,
        argument,
        base,
        change,
    }
}

/// `base` with the fields of `fields`.
fn with(base: &Value, fields: &Value) -> Value {
    let mut arguments = base.clone();
    for (key, value) in fields.as_object().into_iter().flatten() {
        arguments[key] = value.clone();
    }
    arguments
}

/// The cases of the arguments that the tools on docs.rs share: `crate`, `version`, `target` and
/// `project`. `base` has the other arguments, for a tokio item.
fn shared(tool: &'static str, base: &Value) -> Vec<Case> {
    let mut cases = vec![
        case(
            tool,
            "crate",
            with(base, &json!({ "crate": TOKIO })),
            Change::Set(json!(AXUM)),
        ),
        // Both releases are in the test data.
        case(
            tool,
            "version",
            with(base, &json!({ "crate": "tokio", "version": "1.53.3" })),
            Change::Set(json!("1.45.1")),
        ),
        case(
            tool,
            "target",
            with(base, &json!({ "crate": TOKIO })),
            Change::Set(json!(OTHER_TARGET)),
        ),
    ];
    if cfg!(feature = "local") {
        // Without a version, the project decides it.
        cases.push(case(
            tool,
            "project",
            with(base, &json!({ "crate": "tokio" })),
            Change::Set(json!(OTHER_PROJECT)),
        ));
    }
    cases
}

/// Cases of the arguments of `tool` that `base` has, each set to another value.
fn each(tool: &'static str, base: &Value, changes: Vec<(&'static str, Change)>) -> Vec<Case> {
    changes
        .into_iter()
        .map(|(argument, change)| case(tool, argument, base.clone(), change))
        .collect()
}

fn item_cases() -> Vec<Case> {
    let mutex = json!({ "path": "sync::Mutex" });
    let item = with(&mutex, &json!({ "crate": TOKIO }));
    let mut cases = shared("get_item", &mutex);
    cases.extend(each("get_item", &item, vec![
        ("path", Change::Set(json!("sync::RwLock"))),
        ("detail", Change::Set(json!("brief"))),
    ]));
    cases.extend(shared("crate_overview", &json!({})));
    cases.extend(each("crate_overview", &json!({ "crate": TOKIO }), vec![(
        "detail",
        Change::Set(json!("full")),
    )]));
    cases
}

fn list_cases() -> Vec<Case> {
    let module = json!({ "path": "tokio::sync" });
    let sync = with(&module, &json!({ "crate": TOKIO }));
    let mut cases = shared("list_module", &module);
    cases.extend(each("list_module", &sync, vec![
        ("path", Change::Set(json!("tokio::time"))),
        ("recursive", Change::Set(json!(true))),
        ("kind", Change::Set(json!("struct"))),
        ("detail", Change::Set(json!("normal"))),
        ("limit", Change::Set(json!(3))),
    ]));
    cases.extend(each(
        "list_module",
        &with(&sync, &json!({ "recursive": true })),
        vec![("depth", Change::Set(json!(1)))],
    ));
    cases.extend(each(
        "list_module",
        &with(&sync, &json!({ "limit": 3 })),
        vec![("cursor", Change::NextCursor)],
    ));

    let query = json!({ "query": "mutex" });
    let search = with(&query, &json!({ "crate": TOKIO }));
    cases.extend(shared("search_items", &query));
    cases.extend(each("search_items", &search, vec![
        ("query", Change::Set(json!("interval"))),
        ("fields", Change::Set(json!("docs"))),
        ("kind", Change::Set(json!("struct"))),
        ("detail", Change::Set(json!("normal"))),
        ("limit", Change::Set(json!(2))),
    ]));
    cases.extend(each(
        "search_items",
        &with(&search, &json!({ "limit": 2 })),
        vec![("cursor", Change::NextCursor)],
    ));
    cases
}

fn type_cases() -> Vec<Case> {
    let mutex = json!({ "path": "sync::Mutex" });
    let item = with(&mutex, &json!({ "crate": TOKIO }));
    let mut cases = shared("list_impls", &mutex);
    cases.extend(each("list_impls", &item, vec![
        ("path", Change::Set(json!("sync::RwLock"))),
        ("limit", Change::Set(json!(2))),
    ]));
    cases.extend(each(
        "list_impls",
        &with(&item, &json!({ "limit": 2 })),
        vec![("cursor", Change::NextCursor)],
    ));

    cases.extend(shared("list_methods", &mutex));
    cases.extend(each("list_methods", &item, vec![
        ("path", Change::Set(json!("sync::RwLock"))),
        ("name", Change::Set(json!("lock"))),
        ("from", Change::Set(json!("trait"))),
        ("expand_blanket", Change::Set(json!(true))),
        ("detail", Change::Set(json!("normal"))),
        ("limit", Change::Set(json!(3))),
    ]));
    cases.extend(each(
        "list_methods",
        &with(&item, &json!({ "limit": 3 })),
        vec![("cursor", Change::NextCursor)],
    ));

    let trait_path = json!({ "path": "io::AsyncRead" });
    let read = with(&trait_path, &json!({ "crate": TOKIO }));
    cases.extend(shared("list_implementors", &trait_path));
    cases.extend(each("list_implementors", &read, vec![
        ("path", Change::Set(json!("io::AsyncWrite"))),
        // `crate` is the only scope until M4, so another value is refused.
        ("scope", Change::Set(json!("dependency_graph"))),
        ("limit", Change::Set(json!(2))),
    ]));
    cases.extend(each(
        "list_implementors",
        &with(&read, &json!({ "limit": 2 })),
        vec![("cursor", Change::NextCursor)],
    ));
    cases
}

fn project_cases() -> Vec<Case> {
    let mut cases = each(
        "resolve_version",
        &json!({ "crate": "tokio", "version": "1.53.3" }),
        vec![
            ("crate", Change::Set(json!("axum"))),
            ("version", Change::Set(json!("1.45.1"))),
            ("target", Change::Set(json!(OTHER_TARGET))),
        ],
    );
    if cfg!(feature = "local") {
        cases.extend(each("resolve_version", &json!({ "crate": "tokio" }), vec![
            ("project", Change::Set(json!(OTHER_PROJECT))),
        ]));
        cases.extend(each("list_crates", &json!({}), vec![
            ("project", Change::Set(json!(OTHER_PROJECT))),
            ("target", Change::Set(json!(OTHER_TARGET))),
            ("scope", Change::Set(json!("all"))),
            ("crate", Change::Set(json!("tokio"))),
            ("limit", Change::Set(json!(2))),
        ]));
        cases.extend(each("list_crates", &json!({ "limit": 2 }), vec![(
            "cursor",
            Change::NextCursor,
        )]));
    }
    cases
}

/// `refresh` tries again what failed, and redoes nothing that worked (§6.4, decided in M2's D5).
fn refresh_cases() -> Vec<Case> {
    if !cfg!(feature = "local") {
        return Vec::new();
    }
    // As the local test a_member_that_failed_is_not_built_again_until_what_it_is_made_from_changes
    // and the stdio test a_member_whose_build_fails_again_is_told_failed check.
    [
        "crate_overview",
        "get_item",
        "list_crates",
        "list_implementors",
        "list_impls",
        "list_methods",
        "list_module",
        "resolve_version",
        "search_items",
    ]
    .into_iter()
    .map(|tool| case(tool, "refresh", json!({}), Change::CheckedElsewhere))
    .collect()
}

fn cases() -> Vec<Case> {
    let mut cases = item_cases();
    cases.extend(list_cases());
    cases.extend(type_cases());
    cases.extend(project_cases());
    cases.extend(refresh_cases());
    cases
}

/// Every argument that a tool of this build declares has a case.
#[test]
fn every_argument_has_a_case() {
    let cases = cases();
    let mut missing = Vec::new();
    for (name, tool) in tools() {
        if name == "sleep" {
            continue;
        }
        for argument in tool["inputSchema"]["properties"]
            .as_object()
            .expect("properties")
            .keys()
        {
            if !cases
                .iter()
                .any(|case| case.tool == name && case.argument == argument)
            {
                missing.push(format!("{name} {argument}"));
            }
        }
    }
    assert!(missing.is_empty(), "no case for: {missing:?}");
    // And no case is for an argument that is not declared.
    for case in &cases {
        assert!(
            tools()[case.tool]["inputSchema"]["properties"]
                .get(case.argument)
                .is_some(),
            "{} does not declare {}",
            case.tool,
            case.argument
        );
    }
}

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache, and the network"]
fn changing_an_argument_changes_the_result() {
    let logs = tempfile::tempdir().expect("a temporary directory");
    let mut client = Client::start(Some(Path::new(env!("CARGO_MANIFEST_DIR"))), logs.path());
    let mut unchanged = Vec::new();
    for case in cases() {
        if matches!(case.change, Change::CheckedElsewhere) {
            continue;
        }
        let base = client.call(case.tool, &case.base);
        // The base call works, so that a change is not just another error.
        assert!(
            matches!(base, Answer::Ok(_)),
            "{} {}: {base:?}",
            case.tool,
            case.base
        );
        let mut changed = case.base.clone();
        changed[case.argument] = match case.change {
            Change::Set(value) => value,
            Change::NextCursor => base.ok()["page"]["next_cursor"].clone(),
            Change::CheckedElsewhere => continue,
        };
        let after = client.call(case.tool, &changed);
        if after.text() == base.text() {
            unchanged.push(format!("{} {} -> {changed}", case.tool, case.base));
        }
    }
    client.finish();
    assert!(
        unchanged.is_empty(),
        "the result did not change: {unchanged:#?}"
    );
}
