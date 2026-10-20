//! Runs a test of this test binary in a process of its own, for the tests that need another
//! process: one that holds a lock across processes, or fetches at the same time as another.

use std::{
    ffi::OsStr,
    process::{Child, Command, Stdio},
};

/// Starts the test `name` (its path, as `data::cache::tests::x`), which is `#[ignore]`d so that it
/// runs only so, with the environment variables `env`. Its stdout is piped.
pub(crate) fn start(name: &str, env: &[(&str, &OsStr)]) -> Child {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        // Not `--test-threads=1`, with which the line that names the test goes before what the
        // test prints, on the same line.
        .args([name, "--exact", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    command.spawn().expect("the test binary starts")
}

/// Waits for the test that [`start`] started, fails unless it passed, and returns the lines it
/// printed.
pub(crate) fn finish(child: Child) -> Vec<String> {
    let output = child.wait_with_output().expect("waited");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // A name that matches no test passes, too.
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}"
    );
    stdout.lines().map(str::to_owned).collect()
}
