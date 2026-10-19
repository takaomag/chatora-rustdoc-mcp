//! Running a short command of the toolchain, as `rustc -vV`, in a directory without project
//! settings.

use std::{borrow::Cow, path::Path, process::Stdio, time::Duration};

/// Longest wait for a command that takes milliseconds when it works.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `program` with `args` in `dir` and returns what it printed. rustup may not install a
/// toolchain on the way (requirement R-WS-7), and the server's stdin and stdout are never shared
/// (requirement R-SEC-1).
///
/// # Errors
///
/// Returns why, in words for the warnings, when the command cannot be run, does not finish in 10
/// seconds, or fails.
pub(super) async fn output(
    dir: &Path,
    program: &str,
    args: &[&str],
) -> Result<String, Cow<'static, str>> {
    std::fs::create_dir_all(dir)
        .map_err(|error| Cow::Owned(format!("the directory to run {program} in: {error}")))?;
    let output = tokio::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let command = || format!("{program} {}", args.join(" "));
    let output = tokio::time::timeout(TIMEOUT, output)
        .await
        .map_err(|_elapsed| Cow::Owned(format!("{} did not finish in 10 seconds", command())))?
        .map_err(|error| Cow::Owned(format!("{program} could not be run: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first_line = stderr.lines().next().unwrap_or_default();
        return Err(Cow::Owned(format!(
            "{} failed ({}): {first_line}",
            command(),
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
