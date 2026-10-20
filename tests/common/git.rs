//! git repositories and worktrees for the tests, made with the git CLI in an [`Isolated`]
//! environment (`doc/m2-plan.md` §2).

use std::{path::Path, process::Command};

use crate::isolated::Isolated;

/// Runs git with `args` in `dir`, and returns its stdout. It fails the test if git fails.
pub(crate) fn git(isolated: &Isolated, dir: &Path, args: &[&str]) -> String {
    let output = isolated
        .apply(&mut Command::new("git"))
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

/// Makes `dir` a git repository on the branch `main`, with one commit of all its files.
pub(crate) fn commit_all(isolated: &Isolated, dir: &Path) {
    git(isolated, dir, &["init", "--quiet", "--initial-branch=main"]);
    git(isolated, dir, &["add", "--all"]);
    git(isolated, dir, &["commit", "--quiet", "--message=fixture"]);
}

/// Adds to the repository `repository` a linked worktree at `path` (relative to it) on the new
/// branch `branch`, as `claude --worktree` and the subagents' worktrees do.
pub(crate) fn add_worktree(isolated: &Isolated, repository: &Path, path: &str, branch: &str) {
    git(isolated, repository, &[
        "worktree", "add", "--quiet", "-b", branch, path,
    ]);
}
