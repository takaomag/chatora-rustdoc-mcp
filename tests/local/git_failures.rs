//! The failures to fetch a git dependency, as cargo tells them through libgit2 and through the
//! git CLI (`net.git-fetch-with-cli`), and how the server tells them apart (`fetch_failed` with
//! `auth`, `no_repository` and `no_rev`, requirement R-WS-7; doc/m2-plan.md, B5). The repository
//! is a bare one made here and reached by `file://`, and the one that asks for credentials is a
//! local HTTP server that answers 401: nothing leaves the machine.

use std::{
    fs,
    io::{BufRead as _, BufReader, Write as _},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
    thread,
};

use chatora_rustdoc_mcp::data::{
    cargo::{Failure, GitFetchFailure, classify},
    metadata::{Resolution, Resolver},
    project,
};

use crate::{
    git::{commit_all, git},
    isolated::{Isolated, in_isolated_child, output_within},
};

/// A bare repository with the package `dep`, a branch `main` and a tag `v1`.
fn bare_repository(isolated: &Isolated) -> PathBuf {
    let work = isolated.path().join("dep");
    fs::create_dir_all(work.join("src")).expect("made");
    fs::write(
        work.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("written");
    fs::write(work.join("src/lib.rs"), "").expect("written");
    commit_all(isolated, &work);
    git(isolated, &work, &["tag", "v1"]);
    let bare = isolated.path().join("dep.git");
    git(isolated, isolated.path(), &[
        "clone", "--quiet", "--bare", "dep", "dep.git",
    ]);
    bare
}

/// The `file://` URL of `path`: `file:///tmp/x`, and `file:///C:/x` on Windows.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

/// The address of a local HTTP server that answers every request with 401, as a server that asks
/// for credentials (GitHub does so for a repository that does not exist, too).
fn asking_for_credentials() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
    let address = format!("http://{}", listener.local_addr().expect("bound"));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                line.clear();
            }
            // The client's failure is what the test looks at.
            let _written = reader.into_inner().write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    address
}

/// Runs cargo `command` (`metadata`, `fetch` or `tree`) on a new package in `dir` that depends on
/// `dependency` (a TOML inline table), as the server runs it but without `--locked`, as there is
/// no Cargo.lock.
fn run(isolated: &Isolated, dir: &Path, dependency: &str, command: &str) -> Output {
    fs::create_dir_all(dir.join("src")).expect("made");
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\ndep = {dependency}\n"
        ),
    )
    .expect("written");
    fs::write(dir.join("src/lib.rs"), "").expect("written");
    let args: &[&str] = match command {
        "metadata" => &["metadata", "--format-version", "1"],
        other => &[other],
    };
    output_within(
        isolated
            .apply(&mut Command::new("cargo"))
            .args(args)
            .current_dir(dir)
            // As the server runs cargo (`data::cargo::project_command`).
            .env("LC_ALL", "C")
            .env("CARGO_TERM_COLOR", "never"),
        std::time::Duration::from_secs(300),
    )
}

#[test]
fn each_failure_to_fetch_a_git_dependency_is_told_apart() {
    for cli in [false, true] {
        let isolated = Isolated::new();
        // Without retries, which only wait before the same failure (cargo retries the git CLI
        // and a request that may be spurious `net.retry` times, 3 by default).
        isolated.write_cargo_config(&format!("[net]\nretry = 0\ngit-fetch-with-cli = {cli}\n"));
        let bare = bare_repository(&isolated);
        let url = file_url(&bare);
        let missing = file_url(&isolated.path().join("missing.git"));
        let credentials = format!("{}/dep.git", asking_for_credentials());
        let cases = [
            (
                "branch",
                format!("{{ git = \"{url}\", branch = \"nope\" }}"),
                GitFetchFailure::NoRev,
            ),
            (
                "tag",
                format!("{{ git = \"{url}\", tag = \"nope\" }}"),
                GitFetchFailure::NoRev,
            ),
            (
                "rev",
                format!(
                    "{{ git = \"{url}\", rev = \"0123456789abcdef0123456789abcdef01234567\" }}"
                ),
                GitFetchFailure::NoRev,
            ),
            (
                "repository",
                format!("{{ git = \"{missing}\" }}"),
                GitFetchFailure::NoRepository,
            ),
            (
                "credentials",
                format!("{{ git = \"{credentials}\" }}"),
                GitFetchFailure::Auth,
            ),
        ];
        for (name, dependency, expected) in cases {
            for command in ["metadata", "fetch", "tree"] {
                let dir = isolated.path().join(format!("{name}-{command}"));
                let output = run(&isolated, &dir, &dependency, command);
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(!output.status.success(), "{name} {command}: {stderr}");
                assert_eq!(
                    classify(&output, false),
                    Failure::GitFetch(expected),
                    "{name} {command} (cli: {cli}):\n{stderr}"
                );
            }
        }
        // The control: the repository itself is fetched.
        let fetched = run(
            &isolated,
            &isolated.path().join("control"),
            &format!("{{ git = \"{url}\", tag = \"v1\" }}"),
            "fetch",
        );
        assert!(
            fetched.status.success(),
            "{}",
            String::from_utf8_lossy(&fetched.stderr)
        );
    }
}

/// A failure of `cargo metadata` to fetch a git dependency that cannot be had is kept while the
/// workspace's files are the same, a member's manifest among them, until `refresh` (requirement
/// R-WS-7, decided in M3a). The repository does not exist; Cargo.lock names its commit.
#[tokio::test]
async fn a_failure_of_cargo_metadata_to_fetch_a_git_dependency_is_kept() {
    let Some(dir) = in_isolated_child(
        "git_failures::a_failure_of_cargo_metadata_to_fetch_a_git_dependency_is_kept",
    ) else {
        return;
    };
    let root = dir.join("ws");
    let member = root.join("crates/a");
    fs::create_dir_all(member.join("src")).expect("made");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n",
    )
    .expect("written");
    let url = file_url(&dir.join("missing.git"));
    let manifest = format!(
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\ndep = {{ git = \"{url}\" }}\n"
    );
    fs::write(member.join("Cargo.toml"), &manifest).expect("written");
    fs::write(member.join("src/lib.rs"), "").expect("written");
    fs::write(
        root.join("Cargo.lock"),
        format!(
            "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\ndependencies = [\n \
             \"dep\",\n]\n\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \
             \"git+{url}#0123456789abcdef0123456789abcdef01234567\"\n"
        ),
    )
    .expect("written");
    let workspace = project::workspace_of(&root).expect("a workspace");
    let resolver = Resolver::new(false);
    let first = resolver.resolve(&workspace).await.expect("resolved");
    let Resolution::CargoLockRaw { because, .. } = &*first else {
        panic!("cargo metadata cannot fetch the repository: {first:?}");
    };
    assert!(because.lasts(), "{because:?}");
    let again = resolver.resolve(&workspace).await.expect("resolved");
    assert!(Arc::ptr_eq(&first, &again), "kept");
    // A member's manifest is an input of the failure too.
    fs::write(
        member.join("Cargo.toml"),
        format!("{manifest}\n# changed\n"),
    )
    .expect("written");
    let changed = resolver.resolve(&workspace).await.expect("resolved");
    assert!(
        !Arc::ptr_eq(&first, &changed),
        "a member's manifest is an input"
    );
    // `refresh` forgets it.
    resolver.forget_failures(&workspace.root).await;
    let refreshed = resolver.resolve(&workspace).await.expect("resolved");
    assert!(!Arc::ptr_eq(&changed, &refreshed), "forgotten");
}
