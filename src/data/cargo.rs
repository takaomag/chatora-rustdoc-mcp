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
pub(super) fn project_command(root: &Path, offline: bool) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("cargo");
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
        .kill_on_drop(true)
        // git translates its messages, which `classify` matches.
        .env("LC_ALL", "C")
        // Nothing may stop to ask for credentials (requirement R-WS-6). Stored credentials are
        // still used.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_ASKPASS");
    #[cfg(unix)]
    start_session(&mut command);
    command
}

/// Runs the child in a session of its own, without a controlling terminal, so that git and ssh
/// cannot open `/dev/tty` to ask for a password or a host key (requirement R-WS-6).
#[cfg(unix)]
#[expect(
    unsafe_code,
    reason = "setsid has to be called in the child, between fork and exec"
)]
fn start_session(command: &mut tokio::process::Command) {
    let setsid = || {
        // SAFETY: setsid takes no arguments and only changes the calling process.
        if unsafe { libc::setsid() } == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    // SAFETY: the closure runs in the forked child. It calls only setsid, which is
    // async-signal-safe, and reads errno; it neither allocates nor takes a lock.
    unsafe { command.pre_exec(setsid) };
}

/// Runs `command` to the end, and stops it after [`MAX_RUN_TIME`].
///
/// # Errors
///
/// Returns an error when cargo cannot be started or runs too long.
pub(super) async fn run(mut command: tokio::process::Command) -> Result<Output, RunError> {
    tokio::time::timeout(MAX_RUN_TIME, command.output())
        .await
        .map_err(|_elapsed| RunError::TimedOut)?
        .map_err(RunError::NotStarted)
}

/// What a failed cargo command's `stderr` says about the failure. `offline` is whether the server
/// passed `--offline`. The exit code does not tell these apart, so the messages are matched
/// (requirement R-WS-7, which lists the only matches allowed).
pub(super) fn classify(output: &Output, offline: bool) -> Failure {
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
    if missing_revision
        || has("not our ref")
        || has("cannot locate remote-tracking branch")
        || has("couldn't find remote ref")
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_child_leads_a_session_of_its_own() {
        let mut command = tokio::process::Command::new("sh");
        // The first and sixth fields of the stat are the pid and the session id.
        command
            .args(["-c", "cut -d ' ' -f 1,6 /proc/$$/stat"])
            .stdout(Stdio::piped());
        start_session(&mut command);
        let output = command.output().await.expect("sh runs");
        let stat = String::from_utf8(output.stdout).expect("text");
        let (pid, session) = stat.trim().split_once(' ').expect("two fields");
        assert_eq!(pid, session, "{stat}");
    }

    #[test]
    fn the_stderr_tail_keeps_the_end() {
        let long = format!("{}end", "あ".repeat(MAX_STDERR_CHARS));
        let tail = stderr_tail(&failed(&long));
        assert_eq!(tail.chars().count(), MAX_STDERR_CHARS);
        assert!(tail.ends_with("end"));
    }
}
