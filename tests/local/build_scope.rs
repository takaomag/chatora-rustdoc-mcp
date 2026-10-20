//! The scope of the local builds (requirements R-MCP-3 and R-WS-5; `doc/m2-plan.md`, B7): the
//! packages, and whether for the target or the host, are those that `cargo check --workspace
//! --all-targets` builds, and those that `cargo rustdoc -p` cannot name are told apart, with the
//! feature resolver 1 and 2. The test runs in an isolated child, as the library starts cargo in
//! the environment of its own process.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use chatora_rustdoc_mcp::data::{
    build_scope::{BuildScope, Buildable},
    metadata::{Metadata, Resolution, Resolver},
    project,
    target::{TargetTriple, host_target},
};

use crate::isolated::in_isolated_child;

/// Copies the directory `from` into `to`.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("made");
    for entry in fs::read_dir(from).expect("readable") {
        let entry = entry.expect("readable");
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            fs::copy(&path, to.join(entry.file_name())).expect("copied");
        }
    }
}

/// Runs cargo with `args` in `dir`, offline.
fn cargo(dir: &Path, args: &[&str]) -> Output {
    Command::new("cargo")
        .args(args)
        .arg("--offline")
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("cargo runs")
}

/// The fixture's workspace in `dir`, with the feature resolver `resolver` and a Cargo.lock.
fn workspace(dir: &Path, resolver: &str) -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/build-scope");
    let copy = dir.join(format!("resolver-{resolver}"));
    copy_dir(&fixture, &copy);
    let root = copy.join("ws");
    let manifest = root.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).expect("readable");
    fs::write(
        &manifest,
        text.replace("resolver = \"2\"", &format!("resolver = \"{resolver}\"")),
    )
    .expect("written");
    let output = cargo(&root, &["generate-lockfile"]);
    assert!(output.status.success(), "{output:?}");
    root
}

/// The scope that the server reads, and the packages of `cargo metadata`.
async fn read_scope(root: &Path, target: &TargetTriple) -> (BuildScope, BTreeMap<String, String>) {
    let workspace = project::workspace_of(root).expect("a workspace");
    let resolver = Resolver::new(true);
    let scope = resolver.scope(&workspace, target).await.expect("a scope");
    let resolution = resolver.resolve(&workspace).await.expect("resolved");
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        panic!("cargo metadata works offline here: {resolution:?}");
    };
    ((*scope).clone(), ids_by_name(metadata))
}

/// The package ids by package name (the names in the fixture are unique).
fn ids_by_name(metadata: &Metadata) -> BTreeMap<String, String> {
    metadata
        .packages
        .iter()
        .map(|package| (package.name.to_string(), package.id.0.to_string()))
        .collect()
}

/// How the scope places each package: `(name, for the target, for the host)`.
fn placed(scope: &BuildScope, ids: &BTreeMap<String, String>) -> BTreeSet<(String, bool)> {
    let mut placed = BTreeSet::new();
    for (name, id) in ids {
        let id = chatora_rustdoc_mcp::data::metadata::PackageId(id.as_str().into());
        if let Some(built) = scope.get(&id) {
            if built.for_target {
                placed.insert((name.clone(), true));
            }
            if built.for_host {
                placed.insert((name.clone(), false));
            }
        }
    }
    placed
}

/// What `cargo check --workspace --all-targets --target <host>` builds: `(name, for the
/// target)`, without build scripts. A unit for the target has the target's directory in its
/// path.
fn checked(root: &Path, host: &TargetTriple) -> BTreeSet<(String, bool)> {
    let output = cargo(root, &[
        "check",
        "--workspace",
        "--all-targets",
        "--target",
        host.as_str(),
        "--message-format=json",
    ]);
    assert!(output.status.success(), "{output:?}");
    let mut checked = BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let message: serde_json::Value = serde_json::from_str(line).expect("JSON");
        if message["reason"] != "compiler-artifact"
            || message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "custom-build"))
        {
            continue;
        }
        let for_target = message["filenames"]
            .as_array()
            .expect("filenames")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .any(|file| {
                Path::new(file)
                    .components()
                    .any(|part| part.as_os_str() == host.as_str())
            });
        let name = message["target"]["name"].as_str().expect("a name");
        checked.insert((name.to_owned(), for_target));
    }
    checked
}

/// Whether `cargo rustdoc -p <name> --lib` gets past the resolution of the features: cargo
/// panics on a package it cannot name (requirement R-WS-5).
fn names(root: &Path, host: &TargetTriple, name: &str) -> bool {
    let output = cargo(root, &[
        "rustdoc",
        "-p",
        name,
        "--lib",
        "--target",
        host.as_str(),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() || stderr.contains("panicked"),
        "{name}: {stderr}"
    );
    output.status.success()
}

#[tokio::test]
async fn the_scope_is_what_cargo_builds() {
    let Some(dir) = in_isolated_child("build_scope::the_scope_is_what_cargo_builds") else {
        return;
    };
    let host = host_target(&dir).await.triple;
    let wasm: TargetTriple = "wasm32-unknown-unknown".parse().expect("a triple");
    for resolver in ["1", "2"] {
        let root = workspace(&dir, resolver);
        let (scope, ids) = read_scope(&root, &host).await;
        assert_eq!(
            placed(&scope, &ids),
            checked(&root, &host),
            "resolver {resolver}"
        );
        let buildable = |scope: &BuildScope, name: &str| {
            let id = chatora_rustdoc_mcp::data::metadata::PackageId(ids[name].as_str().into());
            scope.buildable(&id)
        };
        // Built for the host only: a proc-macro member.
        assert_eq!(buildable(&scope, "macros"), Buildable::ForHost);
        // Both: as a member, and under `macros` or as a build-dependency.
        for name in ["helper", "bhelper", "hdep", "bdep", "shared"] {
            assert_eq!(buildable(&scope, name), Buildable::ForTarget, "{name}");
        }
        assert_eq!(buildable(&scope, "wasmonly"), Buildable::NotBuilt);
        // What cargo names, for the dependencies outside the workspace.
        for name in ["shared", "hdep", "bdep", "pmdep", "buildonly", "devonly"] {
            let nameable = buildable(&scope, name) != Buildable::Unnameable;
            let named = names(&root, &host, name);
            if resolver == "2" && name == "pmdep" {
                // cargo names a package under a proc-macro member, as it resolves the features of
                // the member's dependencies for the target, too. It builds it only for the host,
                // so the scope leaves it to `build_external_docs` (cargo 1.99.1).
                assert!(named && !nameable, "{name}");
            } else {
                assert_eq!(nameable, named, "resolver {resolver}: {name}");
            }
        }
        // Another target: the scope follows the target's dependencies.
        let (other, _) = read_scope(&root, &wasm).await;
        assert_eq!(buildable(&other, "wasmonly"), Buildable::ForTarget);
        let expected = if resolver == "1" {
            Buildable::ForHost
        } else {
            Buildable::Unnameable
        };
        assert_eq!(buildable(&other, "buildonly"), expected);
    }
}
