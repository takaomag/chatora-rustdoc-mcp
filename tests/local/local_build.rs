//! The local build of a member's documentation (requirements R-WS-1, R-WS-6 and R-WS-8;
//! `doc/m2-plan.md`, D2b): the members of `tests/fixtures/local-build` are built with
//! `cargo rustdoc`, checked and published in the cache, and once built, a build runs again only
//! the documented crate's rustdoc. The test runs in an isolated child, as the library starts
//! cargo in the environment of its own process.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chatora_rustdoc_mcp::data::{
    build::{self, Bootstrap, Built, Documented, Plan, TargetDir},
    cache::Cache,
    freshness,
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

/// The fixture's workspace in `dir`, with a Cargo.lock.
fn workspace(dir: &Path) -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/local-build");
    let copy = dir.join("local-build");
    copy_dir(&fixture, &copy);
    let root = copy.join("ws");
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("cargo runs");
    assert!(output.status.success(), "{output:?}");
    root
}

/// The names of the items of the published JSON.
fn items(built: &Built) -> BTreeSet<String> {
    items_at(&built.json)
}

/// The names of the items of the JSON at `path`.
fn items_at(path: &Path) -> BTreeSet<String> {
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("published")).expect("JSON");
    json["index"]
        .as_object()
        .expect("an index")
        .values()
        .filter_map(|item| item["name"].as_str().map(str::to_owned))
        .collect()
}

/// The plan of the build of the member's crate `name`.
fn plan(
    metadata: &Metadata,
    root: &Path,
    cache: &Cache,
    name: &str,
    target: Option<TargetTriple>,
) -> Plan {
    let (package, documented) = build::member_crate(metadata, name).expect(name);
    Plan {
        root: root.to_path_buf(),
        offline: true,
        toolchain: None,
        bootstrap: Bootstrap::of(metadata),
        target_dir: TargetDir::of(cache.dir(), root),
        package: package.id.clone(),
        manifest: package.manifest_path.clone(),
        documented,
        crate_name: name.into(),
        version: package.version.clone(),
        target,
        cfg_docsrs: true,
        cfgs: Box::default(),
        features: None,
    }
}

#[tokio::test]
async fn the_members_are_built_and_published() {
    let Some(dir) = in_isolated_child("local_build::the_members_are_built_and_published") else {
        return;
    };
    let root = workspace(&dir);
    let workspace = project::workspace_of(&root).expect("a workspace");
    let resolver = Resolver::new(true);
    let resolution = resolver.resolve(&workspace).await.expect("resolved");
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        panic!("cargo metadata works offline here: {resolution:?}");
    };
    let host = host_target(&dir).await.triple;
    let cache = Cache::at(&dir.join("cache"));
    let work = dir.join("cache/work/test");
    let plan = |name: &str, target: Option<TargetTriple>| {
        plan(metadata, &workspace.root, &cache, name, target)
    };
    let run = async |plan: &Plan| {
        build::build(&cache, plan, &work, None, None, |_| {})
            .await
            .unwrap_or_else(|error| panic!("{}: {error:?}", plan.crate_name))
    };

    // A lib, with its private items and the features that cargo built it with.
    let one = run(&plan("one", None)).await;
    assert!(
        items(&one).is_superset(&["answer".to_owned(), "private".to_owned()].into()),
        "{:?}",
        items(&one)
    );
    assert_eq!(one.record.crate_name.as_ref(), "one");
    assert_eq!(one.record.crate_version.as_ref(), "0.1.0");
    assert_eq!(one.record.target.as_deref(), Some(host.as_str()));
    assert_eq!(
        one.record.features_effective,
        Some(vec!["default".into(), "extra".into()])
    );
    assert!(!one.record.bootstrap_all);
    let dir_of = |built: &Built| built.json.parent().expect("a directory").to_path_buf();
    assert!(dir_of(&one).join("record.json").is_file());
    assert!(
        dir_of(&one)
            .join(format!("{}.json.sha256", one.record.format_version))
            .is_file()
    );

    // The cfg and the variable of a build script. The dependency that it shares with `one`,
    // whose build script watches `RUSTC_BOOTSTRAP`, is not built again: the value is that of
    // the workspace, not of the member (requirement R-WS-8).
    let cfgenv = run(&plan("cfgenv", None)).await;
    assert!(
        items(&cfgenv).is_superset(&["configured".to_owned(), "FROM_BUILD".to_owned()].into()),
        "{:?}",
        items(&cfgenv)
    );
    assert!(
        cfgenv.rebuilt.iter().all(|unit| &**unit != "watcher"),
        "{:?}",
        cfgenv.rebuilt
    );

    // A proc-macro is built for the host, even with `--target`.
    let macros = run(&plan("macros", Some(host.clone()))).await;
    assert!(items(&macros).contains("same_tokens"));
    assert_eq!(macros.record.target.as_deref(), Some(host.as_str()));

    // A lib that is only a cdylib.
    let cdy = run(&plan("cdy", None)).await;
    assert!(items(&cdy).contains("exported"));

    // A lib and a bin of the same name write the same file, and are published apart.
    let lib = run(&plan("same", None)).await;
    let mut bin_plan = plan("same", None);
    bin_plan.documented = Documented::Bin {
        name: "same".into(),
    };
    let bin = run(&bin_plan).await;
    assert!(items(&lib).contains("from_lib") && !items(&lib).contains("from_bin"));
    assert!(items(&bin).contains("from_bin"));
    assert_ne!(lib.json, bin.json);
    assert_eq!(lib.record.crate_version.as_ref(), "0.2.0");

    // After the bin of a package, which documents its lib as HTML too, its lib is built and
    // built again: cargo runs rustdoc every time (cargo 1.99.1).
    let (package, documented) = build::member_crate(metadata, "tool").expect("tool");
    assert_eq!(&*package.name, "tool");
    assert!(matches!(documented, Documented::Bin { .. }));
    let tool = run(&plan("tool", None)).await;
    assert!(items(&tool).contains("main"));
    for _ in 0..2 {
        let core = run(&plan("tool_core", None)).await;
        assert!(items(&core).contains("core_fn"));
    }

    // With the value fixed, only the documented crate is built again.
    let again = run(&plan("one", None)).await;
    assert_eq!(again.rebuilt, ["one".into()]);
    assert_eq!(again.json, one.json);

    // Nothing is left in the work directory.
    let left: Vec<_> = fs::read_dir(&work)
        .expect("the work directory")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn a_member_is_built_and_archived_for_the_calls_that_want_it() {
    use std::sync::Arc;

    use chatora_rustdoc_mcp::{
        data::{
            flight::{Flights, Joined},
            owner::Owner,
        },
        query::local::{LocalWanted, local_archive},
    };

    let Some(dir) =
        in_isolated_child("local_build::a_member_is_built_and_archived_for_the_calls_that_want_it")
    else {
        return;
    };
    let root = workspace(&dir);
    let workspace = project::workspace_of(&root).expect("a workspace");
    let resolver = Resolver::new(true);
    let resolution = resolver.resolve(&workspace).await.expect("resolved");
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        panic!("cargo metadata works offline here: {resolution:?}");
    };
    let host = host_target(&dir).await.triple;
    let cache = Cache::at(&dir.join("cache"));
    // The work directories are this process's.
    Owner::start_this(&cache);
    let wanted = LocalWanted {
        plan: plan(metadata, &workspace.root, &cache, "one", None),
        target: host.clone(),
        project_sets_target: false,
        cargo_home: resolver.cargo_home().map(Path::to_path_buf),
        docs_rs_features: None,
        warnings: Vec::new(),
        input_files: Vec::new(),
        input_packages: Vec::new(),
    };
    let flights = Flights::default();
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    // Two calls at once share one build.
    let (first, second) = tokio::join!(
        local_archive(&flights, &cache, wanted.clone(), until),
        local_archive(&flights, &cache, wanted, until)
    );
    let (Joined::Done(Ok(first)), Joined::Done(Ok(second))) = (first, second) else {
        panic!("built and archived");
    };
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.record.crate_name.as_ref(), "one");
    assert_eq!(first.record.target.as_deref(), Some(host.as_str()));
    assert!(
        first
            .record
            .toolchain
            .as_deref()
            .is_some_and(|toolchain| toolchain.starts_with("rustc 1.")),
        "{:?}",
        first.record.toolchain
    );
    assert!(
        first.archived.warnings.is_empty(),
        "{:?}",
        first.archived.warnings
    );
}

#[tokio::test]
async fn a_member_that_breaks_with_docsrs_is_built_without_it() {
    use chatora_rustdoc_mcp::{
        data::{
            flight::{Flights, Joined},
            owner::Owner,
        },
        query::local::{LocalBuilt, LocalWanted, local_archive},
    };

    let Some(dir) =
        in_isolated_child("local_build::a_member_that_breaks_with_docsrs_is_built_without_it")
    else {
        return;
    };
    let root = workspace(&dir);
    let workspace = project::workspace_of(&root).expect("a workspace");
    let resolver = Resolver::new(true);
    let resolution = resolver.resolve(&workspace).await.expect("resolved");
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        panic!("cargo metadata works offline here: {resolution:?}");
    };
    let host = host_target(&dir).await.triple;
    let cache = Cache::at(&dir.join("cache"));
    // The work directories are this process's.
    Owner::start_this(&cache);
    let flights = Flights::default();
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    // A member whose build `--cfg docsrs` breaks is built without it, with a warning (decided in
    // M2's D3).
    let wanted = |name: &str| LocalWanted {
        plan: plan(metadata, &workspace.root, &cache, name, None),
        target: host.clone(),
        project_sets_target: false,
        cargo_home: resolver.cargo_home().map(Path::to_path_buf),
        docs_rs_features: None,
        warnings: Vec::new(),
        input_files: Vec::new(),
        input_packages: Vec::new(),
    };
    let without = |built: &LocalBuilt| {
        !built.record.cfg_docsrs
            && built
                .archived
                .warnings
                .iter()
                .any(|warning| warning.contains("without --cfg docsrs"))
    };
    let Joined::Done(Ok(broken)) = local_archive(&flights, &cache, wanted("broken"), until).await
    else {
        panic!("built without --cfg docsrs");
    };
    assert!(without(&broken), "{:?}", broken.archived.warnings);
    // And so from then on: with sources that build with it, what is only for docs.rs is not there.
    fs::write(
        workspace.root.join("broken/src/lib.rs"),
        "/// A function.\npub fn works() {}\n\n/// Only for docs.rs.\n#[cfg(docsrs)]\npub fn docsrs_only() {}\n",
    )
    .expect("written");
    let Joined::Done(Ok(again)) = local_archive(&flights, &cache, wanted("broken"), until).await
    else {
        panic!("built");
    };
    assert!(without(&again), "{:?}", again.archived.warnings);
    assert!(!items_at(&again.json).contains("docsrs_only"));
    // An error of the crate's own is not taken for one of `--cfg docsrs`: once it is fixed, the
    // build has it again.
    let source = workspace.root.join("typo/src/lib.rs");
    let fine = fs::read_to_string(&source).expect("readable");
    fs::write(&source, "pub fn broken() -> Missing {}\n").expect("written");
    let failed = local_archive(&flights, &cache, wanted("typo"), until).await;
    assert!(matches!(failed, Joined::Done(Err(_))), "{failed:?}");
    fs::write(&source, fine).expect("written");
    let Joined::Done(Ok(fixed)) = local_archive(&flights, &cache, wanted("typo"), until).await
    else {
        panic!("built once fixed");
    };
    assert!(fixed.record.cfg_docsrs);
    assert!(
        fixed.archived.warnings.is_empty(),
        "{:?}",
        fixed.archived.warnings
    );
    assert!(items_at(&fixed.json).contains("docsrs_only"));
}

/// Builds the members of the fixture's workspace as the server does, planned again for each call:
/// an edit to a manifest changes the plan.
struct Members {
    workspace: project::Workspace,
    resolver: Resolver,
    host: TargetTriple,
    cache: Cache,
    flights:
        chatora_rustdoc_mcp::data::flight::Flights<chatora_rustdoc_mcp::query::local::SharedLocal>,
}

impl Members {
    /// The members of the fixture's workspace, copied into `dir`.
    async fn in_dir(dir: &Path) -> Self {
        Self::with_cache(dir, "cache").await
    }

    /// [`Self::in_dir`], with the cache in `dir` under `name`.
    async fn with_cache(dir: &Path, name: &str) -> Self {
        let root = workspace(dir);
        let cache = Cache::at(&dir.join(name));
        chatora_rustdoc_mcp::data::owner::Owner::start_this(&cache);
        Self {
            workspace: project::workspace_of(&root).expect("a workspace"),
            resolver: Resolver::new(true),
            host: host_target(dir).await.triple,
            cache,
            flights: chatora_rustdoc_mcp::data::flight::Flights::default(),
        }
    }

    /// The build of the member `name`.
    async fn built(
        &self,
        name: &str,
    ) -> std::sync::Arc<chatora_rustdoc_mcp::query::local::LocalBuilt> {
        use chatora_rustdoc_mcp::{
            data::flight::Joined,
            query::local::{LocalWanted, local_archive},
        };

        let resolution = self
            .resolver
            .resolve(&self.workspace)
            .await
            .expect("resolved");
        let Resolution::Metadata { metadata, .. } = &*resolution else {
            panic!("cargo metadata works offline here: {resolution:?}");
        };
        let (package, _) = build::member_crate(metadata, name).expect("a member");
        let wanted = LocalWanted {
            plan: plan(metadata, &self.workspace.root, &self.cache, name, None),
            target: self.host.clone(),
            project_sets_target: false,
            cargo_home: self.resolver.cargo_home().map(Path::to_path_buf),
            docs_rs_features: None,
            warnings: Vec::new(),
            input_files: self
                .resolver
                .input_files(&self.workspace.root, metadata)
                .await,
            input_packages: build::source_packages(metadata, package),
        };
        let until = tokio::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(300))
            .expect("a time");
        let Joined::Done(Ok(built)) =
            local_archive(&self.flights, &self.cache, wanted, until).await
        else {
            panic!("built");
        };
        built
    }

    /// The build of the member `name`, once its dep-info is recorded (decided in M2's D4).
    async fn told(
        &self,
        name: &str,
    ) -> std::sync::Arc<chatora_rustdoc_mcp::query::local::LocalBuilt> {
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(120))
            .expect("a time");
        loop {
            let now = self.built(name).await;
            if now.record.dep_info {
                return now;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the dep-info is not recorded"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}

/// When the inputs of `built` were taken: the same for the same build.
fn taken(built: &chatora_rustdoc_mcp::query::local::LocalBuilt) -> u128 {
    built
        .record
        .inputs
        .as_ref()
        .expect("the inputs are recorded")
        .taken_at
}

/// Writes `text` to `path`, and waits until it is settled: a file outside the packages that
/// changed close to a build, in its time or its status (ctime), makes the next call build again
/// (decided in M2's D4).
async fn write_settled(path: &Path, text: &str) {
    fs::write(path, text).expect("written");
    tokio::time::sleep(std::time::Duration::from_millis(3_500)).await;
}

#[tokio::test]
async fn a_member_is_built_again_only_when_what_it_is_made_from_changes() {
    let Some(dir) = in_isolated_child(
        "local_build::a_member_is_built_again_only_when_what_it_is_made_from_changes",
    ) else {
        return;
    };
    let members = Members::in_dir(&dir).await;
    let first = members.built("one").await;
    assert!(first.record.key.is_some());
    // Nothing changed: not built again.
    let again = members.built("one").await;
    assert_eq!(taken(&again), taken(&first));
    // A source of the member, a file of its package that build.rs may read, and the source of a
    // path dependency outside the workspace: built again.
    for (path, text) in [
        ("ws/one/src/lib.rs", "/// Added.\npub fn added() {}\n"),
        ("ws/one/proto/a.proto", "message A {}\n"),
        ("deps/watcher/src/lib.rs", "/// Added.\npub fn more() {}\n"),
    ] {
        let before = members.built("one").await;
        let path = dir.join("local-build").join(path);
        fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        let mut contents = fs::read_to_string(&path).unwrap_or_default();
        contents.push_str(text);
        fs::write(&path, contents).expect("written");
        let after = members.built("one").await;
        assert_ne!(taken(&after), taken(&before), "{}", path.display());
        assert_ne!(after.record.key, before.record.key, "{}", path.display());
    }
    assert!(items_at(&members.built("one").await.json).contains("added"));
}

#[tokio::test]
async fn a_file_that_the_dep_info_of_a_member_tells_is_in_its_key() {
    let Some(dir) =
        in_isolated_child("local_build::a_file_that_the_dep_info_of_a_member_tells_is_in_its_key")
    else {
        return;
    };
    let members = Members::in_dir(&dir).await;
    // A file outside the package that the crate reads, which its dep-info tells after the build:
    // built again when it changes.
    let notes = dir.join("local-build/ws/notes.md");
    write_settled(&notes, "Notes.\n").await;
    let lib = dir.join("local-build/ws/one/src/lib.rs");
    let mut contents = fs::read_to_string(&lib).expect("read");
    contents.push_str("#[doc = include_str!(\"../../notes.md\")]\npub fn noted() {}\n");
    fs::write(&lib, contents).expect("written");
    let first = members.built("one").await;
    let told = members.told("one").await;
    assert!(
        told.record
            .deps
            .files
            .iter()
            .any(|file| file.ends_with("ws/notes.md")),
        "{:?}",
        told.record.deps
    );
    // The record with the dep-info is of the same build, which is fresh.
    assert_eq!(taken(&told), taken(&first));
    assert_ne!(told.record.key, first.record.key);
    assert_eq!(taken(&*members.built("one").await), taken(&first));
    // As when a server ended before the dep-info: the next call finds the build fresh, and has
    // the dep-info then.
    fs::write(
        first.json.with_file_name("record.json"),
        serde_json::to_vec(&first.record).expect("JSON"),
    )
    .expect("written");
    let again = members.told("one").await;
    assert_eq!(taken(&again), taken(&first));
    assert_eq!(again.record.key, told.record.key);
    fs::write(&notes, "Other notes.\n").expect("written");
    let after = members.built("one").await;
    assert_ne!(taken(&after), taken(&first));
    assert!(
        fs::read_to_string(&after.json)
            .expect("published")
            .contains("Other notes.")
    );
}

/// Whether the record of the build published at `json` on disk notes that its dep-info was had,
/// or tried.
fn dep_info_on_disk(json: &Path) -> bool {
    fs::read(json.with_file_name("record.json"))
        .ok()
        .and_then(|record| serde_json::from_slice::<serde_json::Value>(&record).ok())
        .is_some_and(|record| record["dep_info"] == true)
}

#[tokio::test]
async fn the_dep_info_of_a_member_changes_with_its_includes() {
    let Some(dir) =
        in_isolated_child("local_build::the_dep_info_of_a_member_changes_with_its_includes")
    else {
        return;
    };
    let members = Members::in_dir(&dir).await;
    let notes = dir.join("local-build/ws/notes.md");
    write_settled(&notes, "Notes.\n").await;
    let lib = dir.join("local-build/ws/one/src/lib.rs");
    let original = fs::read_to_string(&lib).expect("read");
    let with =
        format!("{original}#[doc = include_str!(\"../../notes.md\")]\npub fn noted() {{}}\n");
    fs::write(&lib, &with).expect("written");
    members.built("one").await;
    let told = members.told("one").await;
    // Built again for an edit, with the dep-info before: fresh once its own is had.
    fs::write(&notes, "Other.\n").expect("written");
    let rebuilt = members.built("one").await;
    assert_ne!(taken(&rebuilt), taken(&told));
    assert_eq!(taken(&*members.told("one").await), taken(&rebuilt));
    assert_eq!(taken(&*members.built("one").await), taken(&rebuilt));
    // Without the include, `notes.md` leaves the key.
    fs::write(&lib, &original).expect("written");
    let without = members.built("one").await;
    let told = members.told("one").await;
    assert!(
        !told
            .record
            .deps
            .files
            .iter()
            .any(|file| file.ends_with("ws/notes.md")),
        "{:?}",
        told.record.deps
    );
    assert_eq!(taken(&*members.built("one").await), taken(&without));
    fs::write(&notes, "More.\n").expect("written");
    assert_eq!(taken(&*members.built("one").await), taken(&without));
}

#[tokio::test]
async fn an_edit_while_the_dep_info_is_had_makes_the_member_built_again() {
    let Some(dir) = in_isolated_child(
        "local_build::an_edit_while_the_dep_info_is_had_makes_the_member_built_again",
    ) else {
        return;
    };
    let members = Members::in_dir(&dir).await;
    let notes = dir.join("local-build/ws/notes.md");
    write_settled(&notes, "Notes.\n").await;
    let lib = dir.join("local-build/ws/one/src/lib.rs");
    let original = fs::read_to_string(&lib).expect("read");
    let with =
        format!("{original}#[doc = include_str!(\"../../notes.md\")]\npub fn noted() {{}}\n");
    // Right after the build, an edit that the run for the dep-info may read instead: without
    // the include, or broken. Undone after it.
    for edited in [original.clone(), format!("{with}pub fn broken( {{\n")] {
        fs::write(&lib, &with).expect("written");
        let first = members.built("one").await;
        fs::write(&lib, &edited).expect("written");
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_secs(120))
            .expect("a time");
        while !dep_info_on_disk(&first.json) {
            assert!(
                std::time::Instant::now() < deadline,
                "the dep-info is not tried"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        fs::write(&lib, &with).expect("written");
        // Not taken for the build: built again, with the dep-info of what it reads.
        let again = members.built("one").await;
        assert_ne!(taken(&again), taken(&first));
        let told = members.told("one").await;
        assert!(
            told.record
                .deps
                .files
                .iter()
                .any(|file| file.ends_with("ws/notes.md")),
            "{:?}",
            told.record.deps
        );
        fs::write(&notes, format!("Notes for {}.\n", edited.len())).expect("written");
        let after = members.built("one").await;
        assert_ne!(taken(&after), taken(&again));
    }
}

#[tokio::test]
async fn a_build_script_that_writes_in_its_package_does_not_make_it_built_again() {
    let Some(dir) = in_isolated_child(
        "local_build::a_build_script_that_writes_in_its_package_does_not_make_it_built_again",
    ) else {
        return;
    };
    let members = Members::in_dir(&dir).await;
    // The same contents each time it runs, which, without `rerun-if-changed`, cargo runs again
    // for the run for the dep-info too: the file is written after the build. It was written
    // before too: a new file in the package makes the next call build again.
    fs::write(dir.join("local-build/ws/one/src/gen.rs"), "// Generated.\n").expect("written");
    fs::write(
        dir.join("local-build/ws/one/build.rs"),
        "fn main() {\n\
             std::fs::write(\"src/gen.rs\", \"// Generated.\\n\").unwrap();\n\
         }\n",
    )
    .expect("written");
    let first = members.built("one").await;
    let told = members.told("one").await;
    assert_eq!(taken(&told), taken(&first));
    for _ in 0..3 {
        assert_eq!(taken(&*members.built("one").await), taken(&first));
    }
}

#[tokio::test]
async fn a_cache_with_a_comma_in_its_path_has_no_dep_info() {
    let Some(dir) =
        in_isolated_child("local_build::a_cache_with_a_comma_in_its_path_has_no_dep_info")
    else {
        return;
    };
    // rustdoc splits `--emit` at commas.
    let members = Members::with_cache(&dir, "ca,che").await;
    let first = members.built("one").await;
    let told = members.told("one").await;
    assert_eq!(taken(&told), taken(&first));
    assert_eq!(told.record.key, first.record.key);
    assert_eq!(told.record.deps, freshness::Deps::default());
    assert!(
        told.record
            .warnings
            .iter()
            .any(|warning| warning.contains("comma")),
        "{:?}",
        told.record.warnings
    );
}

#[tokio::test]
async fn what_the_build_script_of_a_member_watches_is_in_its_key() {
    let Some(dir) =
        in_isolated_child("local_build::what_the_build_script_of_a_member_watches_is_in_its_key")
    else {
        return;
    };
    let members = Members::in_dir(&dir).await;
    // A file outside the package that the build script reads, and a variable that it watches.
    let flags = dir.join("local-build/ws/flags.txt");
    fs::write(
        dir.join("local-build/ws/cfgenv/build.rs"),
        "fn main() {\n\
             println!(\"cargo::rustc-check-cfg=cfg(from_build)\");\n\
             println!(\"cargo::rustc-cfg=from_build\");\n\
             let flags = std::fs::read_to_string(\"../flags.txt\").unwrap();\n\
             println!(\"cargo::rustc-env=FROM_BUILD={}\", flags.trim());\n\
             println!(\"cargo::rerun-if-changed=../flags.txt\");\n\
             println!(\"cargo::rerun-if-env-changed=CHATORA_RUSTDOC_MCP_TEST_FLAG\");\n\
         }\n",
    )
    .expect("written");
    write_settled(&flags, "one\n").await;
    let first = members.built("cfgenv").await;
    let told = members.told("cfgenv").await;
    let deps = &told.record.deps;
    assert!(
        deps.files.iter().any(|file| file.ends_with("ws/flags.txt")),
        "{deps:?}"
    );
    // And what the build script of its path dependency watches, as proc-macro2's does; what the
    // build script sets is not of the environment.
    assert_eq!(deps.env, [
        Box::from("CHATORA_RUSTDOC_MCP_TEST_FLAG"),
        Box::from("RUSTC_BOOTSTRAP")
    ]);
    // Nor what it writes in the target directory.
    assert!(
        deps.files
            .iter()
            .all(|file| !file.starts_with(dir.join("cache"))),
        "{deps:?}"
    );
    assert_eq!(taken(&*members.built("cfgenv").await), taken(&first));
    fs::write(&flags, "two\n").expect("written");
    let after = members.built("cfgenv").await;
    assert_ne!(taken(&after), taken(&first));
    assert!(
        fs::read_to_string(&after.json)
            .expect("published")
            .contains("two")
    );
}

#[tokio::test]
async fn the_sources_of_a_package_in_git_are_what_git_does_not_ignore() {
    let Some(dir) = in_isolated_child(
        "local_build::the_sources_of_a_package_in_git_are_what_git_does_not_ignore",
    ) else {
        return;
    };
    let package = dir.join("p");
    let write = |path: &str| {
        let path = package.join(path);
        fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        fs::write(path, "x").expect("written");
    };
    for path in [
        "Cargo.toml",
        ".gitignore",
        "src/lib.rs",
        "work/build/out",
        "target/debug/out",
        "inner/Cargo.toml",
        "inner/src/lib.rs",
    ] {
        write(path);
    }
    fs::write(package.join(".gitignore"), "work/\n").expect("written");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&package)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "Cargo.toml", "src/lib.rs"]);
    // Not tracked, and not ignored: a source too.
    write("src/new.rs");
    let (listed, walked) = freshness::sources(std::slice::from_ref(&package)).await;
    let mut listed: Vec<_> = listed
        .iter()
        .map(|path| path.strip_prefix(&package).expect("inside").to_owned())
        .collect();
    listed.sort();
    assert_eq!(listed, [
        PathBuf::from(".gitignore"),
        PathBuf::from("Cargo.toml"),
        PathBuf::from("src/lib.rs"),
        PathBuf::from("src/new.rs"),
    ]);
    assert_eq!(walked, Vec::<PathBuf>::new());
    // A repository inside, which git lists as a directory, is walked.
    write("c/inner.c");
    let nested = Command::new("git")
        .args(["init", "-q"])
        .current_dir(package.join("c"))
        .status()
        .expect("git runs");
    assert!(nested.success());
    let (_, walked) = freshness::sources(std::slice::from_ref(&package)).await;
    assert_eq!(walked, [package.join("c/")]);
    // So is a package that git ignores.
    fs::write(package.join(".gitignore"), "*\n").expect("written");
    git(&["rm", "-q", "--cached", "Cargo.toml", "src/lib.rs"]);
    let (listed, walked) = freshness::sources(std::slice::from_ref(&package)).await;
    assert_eq!(listed, Vec::<PathBuf>::new());
    assert_eq!(walked, std::slice::from_ref(&package));
    // Outside git, the directory is walked.
    let outside = dir.join("outside");
    fs::create_dir_all(&outside).expect("made");
    let (listed, walked) = freshness::sources(std::slice::from_ref(&outside)).await;
    assert_eq!(listed, Vec::<PathBuf>::new());
    assert_eq!(walked, [outside]);
}
