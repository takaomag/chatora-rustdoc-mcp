//! The tools of the tests themselves: the isolated environment, the fake crates.io and the
//! worktrees (`doc/m2-plan.md`, A1).

use std::{fs, path::Path, process::Command};

use crate::{
    git::{add_worktree, commit_all, git},
    isolated::Isolated,
    registry::{FakeCrate, FakeRegistry},
};

/// Runs cargo with `args` in `dir` in the isolated environment, and returns its stderr. It fails
/// the test if cargo fails.
fn cargo(isolated: &Isolated, dir: &Path, args: &[&str]) -> String {
    let output = isolated
        .apply(&mut Command::new("cargo"))
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("cargo runs");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "cargo {args:?} failed:\n{stderr}");
    stderr
}

/// Writes the package `name` with the dependencies `dependencies` (TOML lines) in `dir`.
fn package(dir: &Path, name: &str, dependencies: &str) {
    fs::create_dir_all(dir.join("src")).expect("made");
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\n{dependencies}"
        ),
    )
    .expect("written");
    fs::write(dir.join("src/lib.rs"), "").expect("written");
}

#[test]
fn cargo_reads_the_config_of_the_isolated_cargo_home() {
    let isolated = Isolated::new();
    isolated.write_cargo_config("[alias]\nisolated-probe = \"version\"\n");
    cargo(&isolated, isolated.path(), &["isolated-probe"]);
}

#[test]
fn git_reads_no_config_of_the_user() {
    let isolated = Isolated::new();
    let output = git(&isolated, isolated.path(), &[
        "config",
        "--list",
        "--show-origin",
        "--show-scope",
    ]);
    // The identity that `git` passes on the command line, and nothing from a file: neither the
    // global nor the system config.
    let from_files: Vec<_> = output
        .lines()
        .filter(|line| !line.starts_with("command\tcommand line:"))
        .collect();
    assert_eq!(from_files, Vec::<&str>::new(), "{output}");
}

#[test]
fn cargo_takes_dependencies_from_the_fake_registry() {
    let isolated = Isolated::new();
    let registry = FakeRegistry::start();
    isolated.write_cargo_config(&registry.cargo_config());
    registry
        .publish(&FakeCrate::new("fixture-leaf", "0.1.0").file("src/lib.rs", "pub fn leaf() {}\n"));
    registry.publish(&FakeCrate::new("fixture-leaf", "0.2.0"));
    registry.publish(
        &FakeCrate::new("fixture-mid", "0.1.0")
            .dependency("fixture-leaf", "0.1")
            .file("src/lib.rs", "pub use fixture_leaf::leaf;\n"),
    );
    let project = isolated.path().join("project");
    package(&project, "project", "fixture-mid = \"0.1\"\n");
    fs::write(
        project.join("src/lib.rs"),
        "pub fn call() { fixture_mid::leaf() }\n",
    )
    .expect("written");

    cargo(&isolated, &project, &["check", "--quiet"]);

    let lock = fs::read_to_string(project.join("Cargo.lock")).expect("Cargo.lock");
    let lock: toml::Table = toml::from_str(&lock).expect("TOML");
    let packages = lock["package"].as_array().expect("packages");
    let leaf = packages
        .iter()
        .find(|package| package["name"].as_str() == Some("fixture-leaf"))
        .expect("fixture-leaf is locked");
    assert_eq!(leaf["version"].as_str(), Some("0.1.0"));
    // Recorded as from crates.io, as a project that depends on crates.io would be.
    assert_eq!(
        leaf["source"].as_str(),
        Some("registry+https://github.com/rust-lang/crates.io-index")
    );
    let requests = registry.requests();
    for expected in [
        "GET /index/config.json",
        "GET /index/fi/xt/fixture-mid",
        "GET /index/fi/xt/fixture-leaf",
        "GET /dl/fixture-mid/0.1.0/download",
        "GET /dl/fixture-leaf/0.1.0/download",
    ] {
        assert!(
            requests.iter().any(|request| request == expected),
            "{requests:?}"
        );
    }
}

#[test]
fn a_worktree_is_made_inside_the_repository() {
    let isolated = Isolated::new();
    let repository = isolated.path().join("repository");
    package(&repository, "project", "");
    commit_all(&isolated, &repository);
    add_worktree(
        &isolated,
        &repository,
        ".claude/worktrees/agent-1",
        "worktree-agent-1",
    );
    let worktree = repository.join(".claude/worktrees/agent-1");
    assert!(worktree.join("Cargo.toml").is_file());
    // A linked worktree has a `.git` file that points at its record in the repository.
    let dot_git = fs::read_to_string(worktree.join(".git")).expect("a file");
    assert!(dot_git.starts_with("gitdir: "), "{dot_git}");
    assert!(
        dot_git.trim_end().ends_with("worktrees/agent-1"),
        "{dot_git}"
    );
}
