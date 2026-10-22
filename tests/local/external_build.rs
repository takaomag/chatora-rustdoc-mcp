//! The builds of `build_external_docs` in temporary projects (requirement R-WS-5, decided in M2e's
//! E2a): a release of the fake crates.io that no project uses is documented in a project of its
//! own, with the features that docs.rs builds it with, and the temporary projects share one target
//! directory, which what reads `RUSTC_BOOTSTRAP` is not built again in. The test runs in an
//! isolated child, as the library starts cargo in the environment of its own process.

use std::{collections::BTreeSet, fs};

use chatora_rustdoc_mcp::data::{
    build::{self, Bootstrap, Built},
    cache::Cache,
    external::Key,
    metadata::{Resolution, Resolver},
    project,
    temp_project::{self, Release},
};
use semver::Version;

use crate::{
    isolated::in_isolated_child,
    registry::{FakeCrate, FakeRegistry},
};

/// The names of the items of the published JSON.
fn items(built: &Built) -> BTreeSet<String> {
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(&built.json).expect("published")).expect("JSON");
    json["index"]
        .as_object()
        .expect("an index")
        .values()
        .filter_map(|item| item["name"].as_str().map(str::to_owned))
        .collect()
}

/// Publishes the releases of the test: `fixture-far` and `fixture-near`, which both depend on
/// `fixture-watcher`, whose build script watches `RUSTC_BOOTSTRAP`.
fn publish(registry: &FakeRegistry) {
    registry.publish(
        &FakeCrate::new("fixture-watcher", "0.1.0")
            .file(
                "build.rs",
                "fn main() { println!(\"cargo:rerun-if-env-changed=RUSTC_BOOTSTRAP\"); }\n",
            )
            .file("src/lib.rs", "pub fn watched() {}\n"),
    );
    registry.publish(
        &FakeCrate::new("fixture-far", "0.2.0")
            .dependency("fixture-watcher", "0.1")
            .feature("default", &[])
            .feature("extra", &[])
            .file(
                "src/lib.rs",
                "pub fn far() {}\n#[cfg(feature = \"extra\")]\npub fn extra() {}\n\
                 #[cfg(docsrs)]\npub fn on_docs_rs() {}\n#[cfg(far_cfg)]\npub fn with_cfg() {}\n",
            ),
    );
    registry.publish(
        &FakeCrate::new("fixture-near", "0.1.0")
            .dependency("fixture-watcher", "0.1")
            .file("src/lib.rs", "pub fn near() {}\n"),
    );
}

/// Documents `release` in its temporary project in `cache`, as `build_external_docs` does.
async fn build_release(cache: &Cache, release: &Release) -> (Built, build::Plan) {
    let key = Key::Release {
        name: release.name.cache_key().into(),
        version: release.version.to_string().into(),
        target: "host".into(),
        hidden: false,
    };
    let dir = temp_project::dir_in(&key.dir(cache.dir()));
    let written = dir.clone();
    let release_copy = release.clone();
    tokio::task::spawn_blocking(move || temp_project::write(&written, &release_copy))
        .await
        .expect("joined")
        .expect("written");
    temp_project::lock(&dir, false, |_| {})
        .await
        .expect("locked");
    let workspace = project::workspace_of(&dir).expect("a workspace");
    let resolution = Resolver::new(false)
        .resolve(&workspace)
        .await
        .expect("resolved");
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        panic!("cargo metadata works here: {resolution:?}");
    };
    let plan = temp_project::plan(
        (&workspace.root, metadata),
        release,
        None,
        (cache.dir(), false),
    )
    .expect("a plan");
    let work = cache.dir().join("work/test");
    let built = build::build(cache, &plan, &work, None, None, |_| {})
        .await
        .unwrap_or_else(|error| panic!("{}: {error:?}", release.name));
    (built, plan)
}

#[tokio::test]
async fn a_release_that_no_project_uses_is_built_in_a_temporary_project() {
    let Some(dir) = in_isolated_child(
        "external_build::a_release_that_no_project_uses_is_built_in_a_temporary_project",
    ) else {
        return;
    };
    let registry = FakeRegistry::start();
    fs::write(dir.join("cargo-home/config.toml"), registry.cargo_config()).expect("written");
    publish(&registry);
    let cache = Cache::at(&dir.join("cache"));

    // With the features and the `--cfg` that docs.rs builds it with.
    let far = Release {
        name: "fixture-far".parse().expect("a crate name"),
        version: Version::new(0, 2, 0),
        features: vec!["default".into(), "extra".into()],
        cfgs: Box::from(["far_cfg".into()]),
    };
    let (built, plan) = build_release(&cache, &far).await;
    assert!(
        items(&built).is_superset(
            &["far", "extra", "on_docs_rs", "with_cfg"]
                .map(str::to_owned)
                .into()
        ),
        "{:?}",
        items(&built)
    );
    assert_eq!(plan.bootstrap, Bootstrap::All);
    assert!(built.record.bootstrap_all);
    assert!(!built.record.private);
    assert_eq!(built.record.crate_version.as_ref(), "0.2.0");
    assert!(built.json.starts_with(cache.dir().join("local")));

    // Another release, in a temporary project of its own, in the same target directory: the
    // dependency that watches `RUSTC_BOOTSTRAP` is not built again.
    let near = Release {
        name: "fixture-near".parse().expect("a crate name"),
        version: Version::new(0, 1, 0),
        features: Vec::new(),
        cfgs: Box::default(),
    };
    let (near_built, near_plan) = build_release(&cache, &near).await;
    assert!(items(&near_built).contains("near"));
    assert_eq!(near_plan.target_dir, plan.target_dir);
    assert_ne!(near_plan.root, plan.root);
    assert!(
        near_built
            .rebuilt
            .iter()
            .all(|unit| &**unit != "fixture_watcher"),
        "{:?}",
        near_built.rebuilt
    );

    // Its Cargo.lock is kept: built again, nothing is resolved again.
    let requests = registry.requests().len();
    let (again, _) = build_release(&cache, &far).await;
    assert_eq!(again.rebuilt, ["fixture_far".into()]);
    assert_eq!(registry.requests().len(), requests);
}
