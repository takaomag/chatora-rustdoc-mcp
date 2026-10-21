//! Running cargo for a project, and telling its failures apart (requirement R-WS-7).
//!
//! The child never shares the server's stdio or asks the user anything (requirements R-SEC-1 and
//! R-WS-6). cargo tells some failures apart only in its messages, which are matched.

use std::{
    io,
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};

use super::child;

/// Longest run of one cargo command in minutes (`build.timeout_secs`, requirement R-WS-6,
/// default).
const MAX_RUN_MINUTES: u64 = 30;

/// [`MAX_RUN_MINUTES`] as a duration.
const MAX_RUN_TIME: Duration = Duration::from_mins(MAX_RUN_MINUTES);

/// Most of cargo's stderr that is kept for the warnings and the error, from its end.
const MAX_STDERR_CHARS: usize = 8_000;

/// Why cargo did not finish.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// cargo could not be started, as when it is not installed.
    #[error("cargo could not be run")]
    NotStarted(#[source] io::Error),
    /// cargo ran longer than `build.timeout_secs` (requirement R-WS-6) and was stopped.
    #[error("cargo did not finish in {MAX_RUN_MINUTES} minutes")]
    TimedOut,
}

/// How a cargo command failed, as far as its messages tell (requirement R-WS-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Cargo.lock is missing or does not match the manifests, and `--locked` kept cargo from
    /// writing it (`lockfile_outdated`, requirement R-VR-2).
    LockfileOutdated,
    /// Offline, and something cargo needs is not cached (`offline_miss`, requirement R-CA-7).
    OfflineMiss,
    /// A git dependency cannot be fetched, for a reason that trying again does not change
    /// (`fetch_failed`).
    GitFetch(GitFetchFailure),
    /// cargo or one of its children was killed, as by the OOM killer. Trying again may work.
    Killed,
    /// Anything else. Without a message to tell it apart, it is taken as transient.
    Other,
}

/// Why a git dependency cannot be fetched: the `reason` of `fetch_failed` (requirement R-WS-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitFetchFailure {
    /// The credentials were refused or missing. GitHub asks for them for a repository that does
    /// not exist, too.
    Auth,
    /// There is no repository at the URL.
    NoRepository,
    /// The branch, tag or commit does not exist.
    NoRev,
}

impl GitFetchFailure {
    /// The `reason` as the response writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::NoRepository => "no_repository",
            Self::NoRev => "no_rev",
        }
    }
}

/// A cargo command for the project whose workspace root is `root`. It runs there, so that the
/// project's `.cargo/config.toml` and `rust-toolchain.toml` apply as in the user's own builds
/// (requirement R-WS-2), and it never writes Cargo.lock (`--locked`, requirement R-VR-2).
/// Offline, it does not reach the network, and rustup does not install a toolchain
/// (requirements R-CA-7 and R-WS-7).
///
/// `toolchain` is one that the server chose instead of the project's, as `nightly`: it is passed
/// as `+<toolchain>`, and rustup does not install it (requirement R-WS-8, decided in M2's D1).
pub(super) fn project_command(
    root: &Path,
    offline: bool,
    toolchain: Option<&str>,
) -> tokio::process::Command {
    let programs = super::toolchain::Programs::this();
    let mut command = tokio::process::Command::new(programs.cargo());
    // That of the settings, and the project's toolchain: not that of `RUSTUP_TOOLCHAIN`
    // (decided in M2's D1).
    programs.prepare_cargo(&mut command);
    if let Some(toolchain) = toolchain {
        command
            .arg(format!("+{toolchain}"))
            .env("RUSTUP_AUTO_INSTALL", "0");
    }
    command.arg("--locked");
    if offline {
        command.arg("--offline").env("RUSTUP_AUTO_INSTALL", "0");
    }
    command
        .current_dir(root)
        // Never share the server's stdin and stdout (requirement R-SEC-1).
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // git translates its messages, which `classify` matches.
        .env("LC_ALL", "C")
        // The messages are matched, and colors would cut them, as `Blocking` of `Blocking
        // waiting for file lock` with `CARGO_TERM_COLOR=always` in the user's environment.
        .env("CARGO_TERM_COLOR", "never")
        // Nothing may stop to ask for credentials (requirement R-WS-6). Stored credentials are
        // still used.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_ASKPASS");
    // In a group of its own with what it starts, without a controlling terminal (requirement
    // R-WS-6).
    child::prepare(&mut command);
    command
}

/// What a cargo command is doing, as its stderr tells: for `building_reason` (requirements
/// R-WS-2 and R-OUT-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Running, as far as its stderr tells.
    Running,
    /// Waiting for a lock that another cargo holds, as that of the package cache, which
    /// rust-analyzer and the user's own builds share (`waiting_for_lock`).
    WaitingForLock,
    /// Fetching the index or the dependencies, as the first `cargo metadata` of a project does
    /// (`fetching`, requirement R-WS-6).
    Fetching,
}

/// What a line of cargo's stderr tells that cargo does: its status word, as cargo 1.99.1 writes
/// it (`    Updating crates.io index`, `  Downloaded serde v1.0.228`).
fn activity_of(line: &str) -> Activity {
    if line.contains(WAITING_FOR_LOCK) {
        return Activity::WaitingForLock;
    }
    match line.split_whitespace().next() {
        Some("Updating" | "Downloading" | "Downloaded") => Activity::Fetching,
        _ => Activity::Running,
    }
}

/// The line that cargo writes to stderr while it waits for a lock: `Blocking waiting for file
/// lock on package cache`, or `on build directory` (cargo 1.99.1).
const WAITING_FOR_LOCK: &str = "Blocking waiting for file lock";

/// Runs `command` to the end, and stops it after [`MAX_RUN_TIME`].
///
/// # Errors
///
/// Returns an error when cargo cannot be started or runs too long.
pub(super) async fn run(command: tokio::process::Command) -> Result<Output, RunError> {
    run_reporting(command, |_| {}).await
}

/// [`run`], telling `on_activity` when what cargo does changes, as its stderr tells: it waits for
/// a lock, fetches, or goes on. A line after the one about the lock means that it has the lock.
///
/// # Errors
///
/// Returns an error when cargo cannot be started or runs too long.
pub(super) async fn run_reporting(
    command: tokio::process::Command,
    mut on_activity: impl FnMut(Activity) + Send,
) -> Result<Output, RunError> {
    let child = child::spawn(command).await.map_err(RunError::NotStarted)?;
    let mut activity = Activity::Running;
    let output = child.output_watching(|line| {
        let now = activity_of(line);
        if now != activity {
            activity = now;
            on_activity(now);
        }
    });
    // Dropped on the timeout, or when the call is cancelled, which stops the whole group.
    tokio::time::timeout(MAX_RUN_TIME, output)
        .await
        .map_err(|_elapsed| RunError::TimedOut)?
        .map_err(RunError::NotStarted)
}

/// [`run_reporting`] for a build in `target_dir`, a target directory in the cache, which the
/// child's record names (decided in M2's D2), handing each line of stdout, as cargo's JSON
/// messages, to `on_message`. The stdout of the output is empty.
///
/// # Errors
///
/// Returns an error when cargo cannot be started or runs too long.
pub(super) async fn run_building(
    command: tokio::process::Command,
    target_dir: &Path,
    mut on_activity: impl FnMut(Activity) + Send,
    on_message: impl FnMut(&str) + Send,
) -> Result<Output, RunError> {
    let child = child::spawn_building(command, target_dir)
        .await
        .map_err(RunError::NotStarted)?;
    let mut activity = Activity::Running;
    let output = child.output_streaming(on_message, |line| {
        let now = activity_of(line);
        if now != activity {
            activity = now;
            on_activity(now);
        }
    });
    // Dropped on the timeout, or when the call is cancelled, which stops the whole group.
    tokio::time::timeout(MAX_RUN_TIME, output)
        .await
        .map_err(|_elapsed| RunError::TimedOut)?
        .map_err(RunError::NotStarted)
}

/// What a failed cargo command's `stderr` says about the failure.
///
/// `offline` is whether cargo was offline: the server passed `--offline`, or the user's settings
/// make it so. The exit code does not tell these apart, so the messages are matched (requirement
/// R-WS-7, which lists the only matches allowed). The messages of git, through libgit2 and
/// through the git CLI, are checked in `tests/local/git_failures.rs`.
#[must_use]
pub fn classify(output: &Output, offline: bool) -> Failure {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let has = |text: &str| stderr.contains(text);
    if has("because --locked was passed") {
        Failure::LockfileOutdated
    } else if has("attempting to make an HTTP request, but --offline was specified")
        || has("you are in the offline mode (--offline)")
        // The same words as a misspelt crate name when online.
        || (offline && has("no matching package named"))
    {
        Failure::OfflineMiss
    } else if let Some(reason) = git_fetch_failure(&stderr) {
        Failure::GitFetch(reason)
    } else if has("(signal: 9, SIGKILL") || output.status.code().is_none() {
        Failure::Killed
    } else {
        Failure::Other
    }
}

/// The git fetch failure that `stderr` reports, in the words that libgit2 and the git CLI use
/// (checked with cargo 1.99.1 and git 2.56.0, requirement R-WS-7).
fn git_fetch_failure(stderr: &str) -> Option<GitFetchFailure> {
    let has = |text: &str| stderr.contains(text);
    let missing_revision = stderr.lines().any(|line| {
        line.contains("revision ") && line.contains(" not found")
            || line.contains("revspec '") && line.contains("' not found")
    });
    // libgit2 tells a missing tag only as a missing reference, after cargo's own line.
    if missing_revision
        || has("not our ref")
        || has("cannot locate remote-tracking branch")
        || has("couldn't find remote ref")
        || has("failed to find tag `")
        || has("failed to find branch `")
    {
        Some(GitFetchFailure::NoRev)
    } else if has("failed to resolve path") || has("does not appear to be a git repository") {
        Some(GitFetchFailure::NoRepository)
    } else if has("failed to authenticate when downloading repository")
        || has("could not read Username")
        || has("Authentication failed for")
    {
        Some(GitFetchFailure::Auth)
    } else {
        None
    }
}

/// The end of cargo's `stderr`, at most [`MAX_STDERR_CHARS`] characters, where its error is.
pub(super) fn stderr_tail(output: &Output) -> Box<str> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim_end();
    let skip = stderr.chars().count().saturating_sub(MAX_STDERR_CHARS);
    stderr.chars().skip(skip).collect()
}

// The tests make exit statuses as Unix has them; what they check does not depend on the OS.
#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt as _;

    use super::*;

    fn failed(stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(101 << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn failures_are_told_apart_by_their_messages() {
        let classify = |stderr: &str, offline| classify(&failed(stderr), offline);
        assert_eq!(
            classify(
                "error: cannot update the lock file /w/Cargo.lock because --locked was passed to prevent this",
                false
            ),
            Failure::LockfileOutdated
        );
        assert_eq!(
            classify("error: no matching package named `serde` found", true),
            Failure::OfflineMiss
        );
        assert_eq!(
            classify("error: no matching package named `serde` found", false),
            Failure::Other
        );
        assert_eq!(
            classify(
                "error: failed to get `x` as a dependency\n  revision 0123abc not found",
                false
            ),
            Failure::GitFetch(GitFetchFailure::NoRev)
        );
        assert_eq!(
            classify("fatal: couldn't find remote ref refs/heads/nope", false),
            Failure::GitFetch(GitFetchFailure::NoRev)
        );
        assert_eq!(
            classify("fatal: '/x' does not appear to be a git repository", false),
            Failure::GitFetch(GitFetchFailure::NoRepository)
        );
        assert_eq!(
            classify(
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
                false
            ),
            Failure::GitFetch(GitFetchFailure::Auth)
        );
        assert_eq!(
            classify(
                "error: process didn't exit successfully: `rustc` (signal: 9, SIGKILL: kill)",
                false
            ),
            Failure::Killed
        );
        assert_eq!(
            classify("error: failed to parse manifest", false),
            Failure::Other
        );
    }

    #[test]
    fn a_signal_is_a_kill() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(9),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(classify(&output, false), Failure::Killed);
    }

    #[test]
    fn the_status_word_tells_what_cargo_does() {
        assert_eq!(
            activity_of("    Blocking waiting for file lock on package cache\n"),
            Activity::WaitingForLock
        );
        for line in [
            "    Updating crates.io index\n",
            " Downloading crates ...\n",
            "  Downloaded serde v1.0.228\n",
        ] {
            assert_eq!(activity_of(line), Activity::Fetching, "{line}");
        }
        assert_eq!(activity_of("     Locking 2 packages\n"), Activity::Running);
        assert_eq!(
            activity_of("warning: unused manifest key\n"),
            Activity::Running
        );
    }

    #[test]
    fn the_stderr_tail_keeps_the_end() {
        let long = format!("{}end", "あ".repeat(MAX_STDERR_CHARS));
        let tail = stderr_tail(&failed(&long));
        assert_eq!(tail.chars().count(), MAX_STDERR_CHARS);
        assert!(tail.ends_with("end"));
    }
}

/// What cargo itself does while another holds a lock, on every OS: cargo locks with `flock` on
/// Unix and `LockFileEx` on Windows, as `File::lock` does.
#[cfg(test)]
mod lock_tests {
    use std::{fs, sync::Arc};

    use super::*;

    #[tokio::test]
    async fn waiting_for_the_lock_of_the_package_cache_is_told() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let (home, project) = (dir.path().join("cargo-home"), dir.path().join("project"));
        fs::create_dir_all(project.join("src")).expect("made");
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("written");
        fs::write(project.join("src/lib.rs"), "").expect("written");
        // Without dependencies, as `generate-lockfile` would write it, which `--locked` refuses.
        fs::write(
            project.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"p\"\nversion = \"0.1.0\"\n",
        )
        .expect("written");
        fs::create_dir_all(&home).expect("made");
        let cargo = |args: &[&str]| {
            let mut command = project_command(&project, true, None);
            command.args(args).env("CARGO_HOME", &home);
            command
        };

        // Another cargo, such as rust-analyzer's, holds the package cache.
        let lock = fs::File::create(home.join(".package-cache")).expect("made");
        lock.lock().expect("locked");
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let metadata = {
            let seen = Arc::clone(&seen);
            tokio::spawn(run_reporting(
                cargo(&["metadata", "--format-version", "1"]),
                move |activity| seen.lock().push(activity),
            ))
        };
        let waited = async {
            while !seen.lock().contains(&Activity::WaitingForLock) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(30), waited)
            .await
            .expect("cargo told that it waits for the lock");
        lock.unlock().expect("unlocked");
        let output = metadata.await.expect("the task ends").expect("cargo runs");
        assert!(output.status.success(), "{}", stderr_tail(&output));
        assert_eq!(seen.lock().first(), Some(&Activity::WaitingForLock));
    }
}
