//! The default project and git worktrees made by git itself (`doc/m2-plan.md`, A2): the records
//! that git writes are read as the hand-written ones of the unit tests are.

use std::{fs, path::Path};

use chatora_rustdoc_mcp::data::project::{DefaultProject, workspace_of};

use crate::{
    git::{add_worktree, commit_all},
    isolated::Isolated,
};

/// A repository whose root is a workspace with the member `crates/a`, and a linked worktree of it
/// in `.claude/worktrees/agent-1`, where `claude --worktree` and the subagents put theirs.
fn repository(isolated: &Isolated) -> std::path::PathBuf {
    let repository = isolated.path().join("repository");
    let write = |path: &str, contents: &str| {
        let path = repository.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("made");
        fs::write(path, contents).expect("written");
    };
    write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    write(
        "crates/a/Cargo.toml",
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write("crates/a/src/lib.rs", "");
    commit_all(isolated, &repository);
    add_worktree(
        isolated,
        &repository,
        ".claude/worktrees/agent-1",
        "worktree-agent-1",
    );
    repository
}

fn default_project(dir: &Path) -> DefaultProject {
    DefaultProject::find(Some(dir.as_os_str().to_owned()), None).expect("a directory")
}

#[test]
fn a_worktree_of_the_default_projects_repository_is_the_default_project() {
    let isolated = Isolated::new();
    let repository = repository(&isolated);
    let worktree = repository.join(".claude/worktrees/agent-1");

    // The worktree is a workspace of its own, not a part of the one around it.
    let in_worktree = workspace_of(&worktree.join("crates/a")).expect("a workspace");
    assert_eq!(
        in_worktree.root,
        dunce::canonicalize(&worktree).expect("there")
    );
    assert!(default_project(&repository).contains(&in_worktree));
    // And the other way round, for a session opened with `claude --worktree`.
    let main = workspace_of(&repository).expect("a workspace");
    assert!(default_project(&worktree).contains(&main));
    // `.claude` has no Cargo.toml, so the default project of the repository is its root only.
    assert_eq!(&*default_project(&repository).workspaces, [main]);
}

#[test]
fn another_repository_is_not_the_default_project() {
    let isolated = Isolated::new();
    let repository = repository(&isolated);
    let other = isolated.path().join("other");
    fs::create_dir_all(other.join("src")).expect("made");
    fs::write(
        other.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .expect("written");
    commit_all(&isolated, &other);
    let other = workspace_of(&other).expect("a workspace");
    assert!(!default_project(&repository).contains(&other));
}
