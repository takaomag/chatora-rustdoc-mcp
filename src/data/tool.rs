//! Running a short command of the toolchain, as `rustc -vV`: in a directory without project
//! settings, or in a project, whose toolchain then applies.

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
    time::{Duration, Instant},
};

use parking_lot::Mutex;

/// Longest wait for a command that takes milliseconds when it works.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`output_kept`] keeps what a command printed. Toolchains are seldom installed or
/// switched while a session runs, and running rustc through rustup takes about 25 milliseconds,
/// twice for each lookup in std, against a target of 50 for a whole lookup (§9.1, decided in M1).
const KEPT_FOR: Duration = Duration::from_secs(60);

/// Where a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Place {
    /// A directory without project settings, as the cache.
    Neutral,
    /// A project, whose `rust-toolchain.toml` rustup is to follow: `RUSTUP_TOOLCHAIN`, which
    /// would win over it, is taken out (decided in M2's D1).
    Project,
}

/// What commands printed, or why they failed, by directory, place, program and arguments, and
/// when.
type Kept =
    HashMap<(PathBuf, Place, PathBuf, Vec<String>), (Instant, Result<String, Cow<'static, str>>)>;

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);

/// [`output`], kept for [`KEPT_FOR`]: for the commands that only tell about a toolchain.
///
/// # Errors
///
/// As [`output`].
pub(super) async fn output_kept(
    dir: &Path,
    place: Place,
    program: &Path,
    args: &[&str],
) -> Result<String, Cow<'static, str>> {
    let key = (
        dir.to_path_buf(),
        place,
        program.to_path_buf(),
        args.iter().map(|&arg| arg.to_owned()).collect::<Vec<_>>(),
    );
    if let Some((at, kept)) = KEPT.lock().get(&key)
        && at.elapsed() < KEPT_FOR
    {
        return kept.clone();
    }
    let printed = output(dir, place, program, args).await;
    let mut kept = KEPT.lock();
    kept.retain(|_, (at, _)| at.elapsed() < KEPT_FOR);
    kept.insert(key, (Instant::now(), printed.clone()));
    printed
}

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
    place: Place,
    program: &Path,
    args: &[&str],
) -> Result<String, Cow<'static, str>> {
    let program_path = program;
    let program = program.display();
    std::fs::create_dir_all(dir)
        .map_err(|error| Cow::Owned(format!("the directory to run {program} in: {error}")))?;
    let mut command = tokio::process::Command::new(program_path);
    command
        .args(args)
        .current_dir(dir)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if place == Place::Project {
        command.env_remove("RUSTUP_TOOLCHAIN");
    }
    // In a group of its own with what it starts (requirement R-WS-6).
    super::child::prepare(&mut command);
    let not_run = |error| Cow::Owned(format!("{program} could not be run: {error}"));
    let child = super::child::spawn(command).await.map_err(not_run)?;
    let command = || format!("{program} {}", args.join(" "));
    let output = tokio::time::timeout(TIMEOUT, child.output())
        .await
        .map_err(|_elapsed| Cow::Owned(format!("{} did not finish in 10 seconds", command())))?
        .map_err(not_run)?;
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
