//! The size of `get_item` at `detail=normal` for representative items stays within a budget
//! (requirement R-QA-1), so that a response that grows by mistake (26 KB for `Vec`) fails CI.
//!
//! The budgets are about 1.5 times the sizes measured in M1 (doc/m1-plan.md, E2), with room for
//! the documentation to change between releases of std. ferritin's hosted server returns about
//! 5,000 characters for `tokio::sync::Mutex` and 21,000 for `std::vec::Vec` (docs, methods and
//! impls at once), where `list_methods` and `list_impls` give the rest here.

use std::path::Path;

use serde_json::json;

use crate::client::Client;

/// Items and their budgets in bytes of the structured content.
const BUDGETS: &[(&str, &str, usize)] = &[
    ("tokio@1.53.3", "sync::Mutex", 1_000),
    ("tokio@1.53.3", "sync::Mutex::lock", 1_300),
    ("std", "vec::Vec", 1_700),
    ("std", "collections::HashMap", 1_700),
    ("core", "option::Option", 1_500),
    ("axum@0.8.9", "Router", 900),
    ("serde@1.0.228", "Serialize", 1_300),
    ("serde_json@1.0.154", "Value", 1_400),
];

#[test]
#[ignore = "needs the JSON of testdata/docs-rs-json.txt in the cache, and std"]
fn representative_items_stay_within_their_budgets() {
    let logs = tempfile::tempdir().expect("a temporary directory");
    let mut client = Client::start(None::<&Path>, logs.path());
    let mut over = Vec::new();
    for &(release, path, budget) in BUDGETS {
        let answer = client.call("get_item", &json!({ "crate": release, "path": path }));
        let size = answer.ok().to_string().len();
        eprintln!("{release} {path}: {size} bytes (budget {budget})");
        if size > budget {
            over.push(format!("{release} {path}: {size} bytes, over {budget}"));
        }
    }
    client.finish();
    assert!(over.is_empty(), "over budget: {over:#?}");
}
