//! The canary (requirement R-QA-4): what changes outside this repository, found before users do.
//! It runs on a schedule in CI (`.github/workflows/canary.yml`), which opens an issue when it
//! fails and closes it when it passes again.
//!
//! - The latest nightly's rustdoc: a new `format_version`, or another way of writing the `cfg` of
//!   items in `Attribute::Other`.
//! - docs.rs, live: the format it serves, how it answers for crates, versions and targets it has
//!   no JSON of (requirements R-DS-6 and §8), and which targets it builds (requirement R-TG-1).
//! - The beta, the next stable: whether it builds the rustdoc JSON of a workspace with
//!   `RUSTC_BOOTSTRAP` for its crates alone, as the server does with a project's toolchain.
//!
//! The tests are ignored by default. Run them with `cargo test --release --test canary --
//! --ignored`; the first needs a nightly toolchain, the second the network, with an empty cache in
//! `CHATORA_RUSTDOC_MCP_CACHE__DIR` to see what docs.rs serves now, and the third the beta
//! toolchain, which CI runs in a job of its own, without a nightly.

#![cfg(test)]
#![expect(
    unused_crate_dependencies,
    reason = "an integration test uses only some of the package's dependencies"
)]

#[path = "real_data/client.rs"]
mod client;
#[path = "common/isolated.rs"]
#[expect(
    dead_code,
    reason = "the canary starts a server in an isolated environment, and no test in a child"
)]
mod isolated;
#[path = "common/schema.rs"]
mod schema;

use std::{
    path::Path,
    process::{Command, Stdio},
};

use chatora_rustdoc_mcp::{
    data::rustdoc_json,
    query::cfg::{Cfg, CfgAttribute},
};
use rustdoc_types::{Attribute, Crate, ItemEnum};
use serde_json::{Value, json};

use crate::{
    client::{Answer, Client},
    isolated::{Isolated, output_within},
};

/// A crate with the `cfg` that the canary checks are still read: a feature, a platform, and
/// `doc(cfg)`, which docs.rs builds turn on with `--cfg docsrs`.
const FIXTURE_MANIFEST: &str = r#"[package]
name = "canary-fixture"
version = "0.1.0"
edition = "2024"
publish = false

[features]
extra = []
"#;

const FIXTURE_LIB: &str = r#"#![cfg_attr(docsrs, feature(doc_cfg))]
//! The canary's crate.

/// Always there.
pub fn always() {}

/// Only with the feature `extra`.
#[cfg(feature = "extra")]
pub fn gated() {}

/// Only on Unix.
#[cfg(unix)]
pub fn unix_only() {}

/// With `extra`, documented as needing it.
#[cfg(feature = "extra")]
#[cfg_attr(docsrs, doc(cfg(feature = "extra")))]
pub struct Documented;
"#;

/// Builds the fixture's rustdoc JSON with the nightly toolchain, as docs.rs builds it.
fn nightly_json(dir: &Path) -> Vec<u8> {
    std::fs::create_dir_all(dir.join("src")).expect("made");
    std::fs::write(dir.join("Cargo.toml"), FIXTURE_MANIFEST).expect("written");
    std::fs::write(dir.join("src/lib.rs"), FIXTURE_LIB).expect("written");
    let output = Command::new("cargo")
        .args(["+nightly", "rustdoc", "--lib", "--all-features", "--"])
        .args([
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
            "--cfg",
            "docsrs",
        ])
        .current_dir(dir)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .stdin(Stdio::null())
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo +nightly rustdoc failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(dir.join("target/doc/canary_fixture.json")).expect("the JSON is there")
}

/// The conditions that the attributes of the item `name` give.
fn conditions(krate: &Crate, name: &str) -> Vec<Cfg> {
    let item = krate
        .index
        .values()
        .find(|item| item.name.as_deref() == Some(name) && !matches!(item.inner, ItemEnum::Use(_)))
        .unwrap_or_else(|| panic!("no item {name}"));
    item.attrs
        .iter()
        .filter_map(|attribute| {
            if let Attribute::Other(text) = attribute {
                Cfg::of_attribute(text)
            } else {
                None
            }
        })
        .map(|attribute| match attribute {
            CfgAttribute::Cfg(_, cfg) => cfg,
            CfgAttribute::Unreadable => {
                panic!(
                    "the cfg of {name} is written in a way this server cannot read: {:?}",
                    item.attrs
                )
            },
        })
        .collect()
}

#[test]
#[ignore = "needs a nightly toolchain"]
fn the_latest_nightly_writes_rustdoc_json_this_server_reads() {
    let temp = tempfile::tempdir().expect("a temporary directory");
    let json = nightly_json(temp.path());
    let parsed = rustdoc_json::parse(&json).unwrap_or_else(|error| {
        panic!("the nightly's rustdoc JSON cannot be read: {error}");
    });
    let format = parsed.krate.format_version;
    assert!(
        !parsed.newer_than_supported && rustdoc_json::SUPPORTED.contains(&format),
        "the nightly writes format {format}, newer than {:?}: update rustdoc-types",
        rustdoc_json::SUPPORTED
    );
    let feature = Cfg::NameValue("feature".into(), "extra".into());
    assert!(
        conditions(&parsed.krate, "gated").contains(&feature),
        "the cfg of a feature is no longer found"
    );
    assert!(
        conditions(&parsed.krate, "unix_only").contains(&Cfg::Name("unix".into())),
        "the cfg of a platform is no longer found"
    );
    assert!(
        conditions(&parsed.krate, "Documented").contains(&feature),
        "the doc(cfg) of a feature is no longer found"
    );
    assert_eq!(conditions(&parsed.krate, "always"), Vec::<Cfg>::new());
}

/// What docs.rs is expected to answer, and what the server makes of it.
struct Expectation {
    arguments: Value,
    /// The error code, or `None` for a result.
    code: Option<&'static str>,
    /// The `reason` of the error, if it has one.
    reason: Option<&'static str>,
}

const fn expect(
    arguments: Value,
    code: Option<&'static str>,
    reason: Option<&'static str>,
) -> Expectation {
    Expectation {
        arguments,
        code,
        reason,
    }
}

#[test]
#[ignore = "needs the network; asks docs.rs and crates.io"]
fn docs_rs_serves_what_this_server_expects() {
    let expectations = [
        // The latest releases of much-used crates, for the host's target (requirement R-TG-1).
        expect(json!({ "crate": "serde", "path": "Serialize" }), None, None),
        expect(
            json!({ "crate": "tokio", "path": "sync::Mutex" }),
            None,
            None,
        ),
        expect(json!({ "crate": "anyhow", "path": "Error" }), None, None),
        // What docs.rs has no JSON of (requirement R-DS-6 and §8).
        expect(
            json!({ "crate": "no-such-crate-chatora-canary", "path": "x" }),
            Some("crate_not_found"),
            None,
        ),
        expect(
            json!({ "crate": "serde@1.0.999", "path": "Serialize" }),
            Some("version_not_found"),
            None,
        ),
        // Built before docs.rs made rustdoc JSON (2025-05-23).
        expect(
            json!({ "crate": "serde@1.0.100", "path": "Serialize" }),
            Some("docs_unavailable"),
            Some("no_json"),
        ),
        // Since 2026-05-01, docs.rs builds most new releases for one target. When it builds
        // more again, this fails, and the server's advice should change.
        expect(
            json!({ "crate": "tokio", "path": "sync::Mutex", "target": "aarch64-apple-darwin" }),
            Some("docs_unavailable"),
            Some("no_target_json"),
        ),
    ];
    // Outside any project, so that what is asked of is the latest release on docs.rs, not the
    // dependency of this repository that the server would find from where the tests run.
    let logs = tempfile::tempdir().expect("a temporary directory");
    let mut client = Client::start(None, logs.path());
    let mut unexpected = Vec::new();
    for expectation in expectations {
        let answer = client.call("get_item", &expectation.arguments);
        let fits = match (&answer, expectation.code) {
            (Answer::Ok(_), None) => {
                let result = answer.ok();
                let format = result["provenance"]["format_version"]
                    .as_u64()
                    .unwrap_or_default();
                u32::try_from(format).is_ok_and(|format| rustdoc_json::SUPPORTED.contains(&format))
                    && result["provenance"]["target"].is_string()
            },
            (Answer::Error(error), Some(code)) => {
                error["code"] == code
                    && expectation
                        .reason
                        .is_none_or(|reason| error["reason"] == reason)
            },
            (Answer::Ok(_), Some(_)) | (Answer::Error(_), None) => false,
        };
        if !fits {
            unexpected.push(format!(
                "{} (expected {:?} {:?}):\n{}",
                expectation.arguments,
                expectation.code,
                expectation.reason,
                answer.text().chars().take(800).collect::<String>()
            ));
        }
    }
    client.finish();
    assert!(
        unexpected.is_empty(),
        "docs.rs answered otherwise:\n{}",
        unexpected.join("\n\n")
    );
}

/// Copies the directory `from` into `to`.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("made");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("readable");
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).expect("copied");
        }
    }
}

/// The beta, as a project pins it with its `rust-toolchain.toml` (requirement R-WS-8), builds the
/// rustdoc JSON of the workspace's crates with `RUSTC_BOOTSTRAP` for them alone, which the next
/// stable will (requirement R-QA-4, decided in M2f's F3b): a lib whose dependency's build script
/// watches `RUSTC_BOOTSTRAP`, what a build script sets (a cfg and a variable), a proc macro, and a
/// bin. Each is the beta's, of a format that this server reads, without warnings: no fall-back to a
/// nightly, which the job does not install, nor a build without `--cfg docsrs`.
#[test]
#[ignore = "needs the beta toolchain"]
fn the_beta_builds_the_rustdoc_json_of_a_workspace() {
    let isolated = Isolated::new();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/local-build");
    let copy = isolated.path().join("local-build");
    copy_dir(&fixture, &copy);
    let workspace = copy.join("ws");
    std::fs::write(
        workspace.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"beta\"\n",
    )
    .expect("written");
    let locked = output_within(
        isolated
            .apply(&mut Command::new("cargo"))
            .args(["generate-lockfile", "--offline"])
            .current_dir(&workspace),
        std::time::Duration::from_secs(120),
    );
    assert!(locked.status.success(), "{locked:?}");
    let logs = tempfile::tempdir().expect("a temporary directory");
    // Not in this repository either, whose `rust-toolchain.toml` would pin what runs there.
    let mut client = Client::start_in(Some(&workspace), logs.path(), |command| {
        isolated.apply(command).current_dir(isolated.path());
    });
    let mut unexpected = Vec::new();
    for (krate, path) in [
        ("one", "answer"),
        ("cfgenv", "configured"),
        ("cfgenv", "FROM_BUILD"),
        ("macros", "same_tokens"),
        ("tool", "main"),
    ] {
        // For the host, which the job runs on.
        let answer = client.call(
            "get_item",
            &json!({ "crate": krate, "path": path, "target": null }),
        );
        let fits = match &answer {
            Answer::Ok(result) => {
                let provenance = &result["provenance"];
                eprintln!(
                    "{krate} {path}: {} {} {}",
                    provenance["source"], provenance["toolchain"], result["warnings"]
                );
                let format = provenance["format_version"].as_u64().unwrap_or_default();
                provenance["source"] == "local"
                    && provenance["toolchain"]
                        .as_str()
                        .is_some_and(|toolchain| toolchain.contains("-beta."))
                    && u32::try_from(format)
                        .is_ok_and(|format| rustdoc_json::SUPPORTED.contains(&format))
                    && result["warnings"].as_array().is_none_or(Vec::is_empty)
            },
            Answer::Error(_) => false,
        };
        if !fits {
            unexpected.push(format!(
                "{krate} {path}:\n{}",
                answer.text().chars().take(1_500).collect::<String>()
            ));
        }
    }
    client.finish();
    assert!(
        unexpected.is_empty(),
        "the beta built otherwise:\n{}",
        unexpected.join("\n\n")
    );
}
