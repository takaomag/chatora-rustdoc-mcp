//! Tests with real data: the rustdoc JSON of `testdata/docs-rs-json.txt`, which must be in the
//! cache that `CHATORA_RUSTDOC_MCP_CACHE__DIR` names, std, and the network for what the cache
//! lacks. They are ignored by default, and run in the `testdata` job of CI with
//! `cargo test --release --test real_data -- --ignored`. The checks that each argument and each
//! promised question has a case are not ignored.

#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

mod arguments;
mod budgets;
mod client;
mod determinism;
mod questions;
#[path = "../common/schema.rs"]
mod schema;

/// The check against `outputSchema` finds what does not match.
#[test]
fn the_check_of_results_finds_what_does_not_match() {
    use serde_json::json;
    let valid = json!({
        "provenance": {
            "source": "docs.rs",
            "crate": "demo",
            "version": "0.1.0",
            "target": "x86_64-unknown-linux-gnu",
        },
        "data": { "path": "demo::hello", "kind": "function" },
    });
    schema::check_output("get_item", &valid);
    let mut undeclared = valid.clone();
    undeclared["data"]["surprise"] = json!(1);
    let mut wrong_type = valid.clone();
    wrong_type["data"]["path"] = json!(1);
    let mut missing = valid.clone();
    if let Some(provenance) = missing["provenance"].as_object_mut() {
        // Required even while the version is not known (requirement R-OUT-9), as the target is
        // not before docs.rs's JSON of its default target is read (decided in M2's D7).
        provenance.remove("crate");
    }
    let mut not_in_enum = valid;
    not_in_enum["provenance"]["source"] = json!("elsewhere");
    for invalid in [undeclared, wrong_type, missing, not_in_enum] {
        let checked = std::panic::catch_unwind(|| schema::check_output("get_item", &invalid));
        assert!(checked.is_err(), "{invalid} passed");
    }
}
