//! Running cargo for a project, and telling its failures apart (requirement R-WS-7).
//!
//! The child never shares the server's stdio or asks the user anything (requirements R-SEC-1 and
//! R-WS-6). cargo tells some failures apart only in its messages, which are matched.

use std::{
    io,
    path::Path,
    process::{Output, Stdio},
    sync::{Arc, OnceLock},
    time::Duration,
};

use super::{
    child,
    flight::{Doing, Stage},
};

/// Longest run of one cargo command, unless `build.timeout_secs` sets it (requirement R-WS-6,
/// default; decided in M2's D3): Claude Code's limit of a call over stdio.
const DEFAULT_RUN_TIME: Duration = Duration::from_mins(30);

/// Longest run of one cargo command, as `build.timeout_secs` sets it.
static RUN_TIME: OnceLock<Duration> = OnceLock::new();

/// Sets the longest run of one cargo command, once, at startup: build, `cargo metadata` and `cargo
/// tree` alike (decided in M2's D3). The waits for the server's locks and build slots do not
/// count.
pub fn set_run_time(limit: Duration) {
    if RUN_TIME.set(limit).is_err() {
        tracing::debug!("the longest run of cargo was set before");
    }
}

/// Longest run of one cargo command.
fn run_time() -> Duration {
    RUN_TIME.get().copied().unwrap_or(DEFAULT_RUN_TIME)
}

/// Most of cargo's stderr that is kept for the warnings and the error, from its end.
const MAX_STDERR_CHARS: usize = 8_000;

/// Why cargo did not finish.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// cargo could not be started, as when it is not installed.
    #[error("cargo could not be run")]
    NotStarted(#[source] io::Error),
    /// cargo ran longer than `build.timeout_secs` (requirement R-WS-6), this long, and was
    /// stopped.
    #[error("cargo did not finish in {} seconds", .0.as_secs())]
    TimedOut(Duration),
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
    /// The toolchain is older than what the code needs: a feature that it does not have
    /// (`E0658`), or a package's `rust-version` (`toolchain_too_old`, requirement R-WS-7).
    TooOld,
    /// Anything else: a build that failed so fails again until what it is made from changes,
    /// while `cargo metadata` or `cargo fetch` may pass when tried again (requirement R-WS-7).
    Other,
}

impl Failure {
    /// The `code` of the error, as the response writes it.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::LockfileOutdated => "lockfile_outdated",
            Self::OfflineMiss => "offline_miss",
            Self::GitFetch(_) => "fetch_failed",
            Self::TooOld => "toolchain_too_old",
            Self::Killed | Self::Other => "build_failed",
        }
    }
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
    command_in(root, offline, toolchain, true)
}

/// [`project_command`] without `--locked`, which writes Cargo.lock: for the temporary projects of
/// `build_external_docs`, whose Cargo.lock the server makes (requirement R-WS-5, decided in M2e's
/// E2a), and nowhere else.
pub(super) fn unlocked_command(root: &Path, offline: bool) -> tokio::process::Command {
    command_in(root, offline, None, false)
}

/// [`project_command`], with `--locked` if `locked`.
fn command_in(
    root: &Path,
    offline: bool,
    toolchain: Option<&str>,
    locked: bool,
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
    if locked {
        command.arg("--locked");
    }
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

impl Activity {
    /// What a build is doing when cargo does this (decided in M2's D2): it builds unless it
    /// waits or fetches.
    #[must_use]
    pub const fn of_build(self) -> Doing {
        match self {
            Self::Running => Doing::Building,
            Self::WaitingForLock => Doing::WaitingForLock,
            Self::Fetching => Doing::Fetching,
        }
    }
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

tokio::task_local! {
    /// What the shared work that runs cargo is doing, which [`run`] tells, set by [`staged`].
    static STAGE: Arc<Stage>;
}

/// `work`, in which [`run`] tells `stage` whether cargo waits for a lock or fetches (requirement
/// R-OUT-9): for the shared work that runs `cargo metadata` and `cargo tree`, which a call may
/// stop waiting for.
pub(super) async fn staged<T>(stage: Arc<Stage>, work: impl Future<Output = T>) -> T {
    STAGE.scope(stage, work).await
}

/// The stage of the [`staged`] work that this task runs, if any.
pub(super) fn current_stage() -> Option<Arc<Stage>> {
    STAGE.try_with(Arc::clone).ok()
}

/// What `activity` of `cargo metadata` or `cargo tree` is, for a call that waits for it: most of
/// what takes long in them is fetching.
pub(super) const fn doing_of(activity: Activity) -> Doing {
    match activity {
        Activity::WaitingForLock => Doing::WaitingForLock,
        Activity::Fetching | Activity::Running => Doing::Fetching,
    }
}

/// Runs `command` to the end, and stops it after `build.timeout_secs`. In [`staged`] work, it
/// tells its stage what cargo does.
///
/// # Errors
///
/// Returns an error when cargo cannot be started or runs too long.
pub(super) async fn run(command: tokio::process::Command) -> Result<Output, RunError> {
    let stage = STAGE.try_with(Arc::clone).ok();
    run_reporting(command, |activity| {
        if let Some(stage) = &stage {
            stage.set(doing_of(activity));
        }
    })
    .await
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
    limited(run_time(), output).await
}

/// `output` of a cargo command, which is stopped after `limit`: dropped then, as when the call is
/// cancelled, which stops the whole group.
async fn limited(
    limit: Duration,
    output: impl Future<Output = io::Result<Output>>,
) -> Result<Output, RunError> {
    tokio::time::timeout(limit, output)
        .await
        .map_err(|_elapsed| RunError::TimedOut(limit))?
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
    limited(run_time(), output).await
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
        || (offline && no_such_package(&stderr))
    {
        Failure::OfflineMiss
    } else if let Some(reason) = git_fetch_failure(&stderr) {
        Failure::GitFetch(reason)
    } else if has("(signal: 9, SIGKILL") || output.status.code().is_none() {
        Failure::Killed
    } else if has("requires rustc") {
        // `package `x` cannot be built because it requires rustc 1.99 or newer`.
        Failure::TooOld
    } else {
        Failure::Other
    }
}

/// Whether cargo's `stderr` says that no package of the name depended on was found: offline, as
/// it was not fetched; else, as the registry or the git repository has none (decided in M2e's
/// E3).
#[must_use]
pub fn no_such_package(stderr: &str) -> bool {
    stderr.contains("no matching package named")
}

/// Whether cargo's `stderr` says that no package named `name` was found: not another, as a
/// dependency of the package depended on (decided in M2e's E3).
#[must_use]
pub fn no_package_named(stderr: &str, name: &str) -> bool {
    stderr.contains(&format!("no matching package named `{name}` found"))
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

/// The end of cargo's `stderr`, at most [`MAX_STDERR_CHARS`] characters, where its error is,
/// without what would hide text: removed before it is cut, so that a cut leaves no part of an
/// escape sequence of a build script's colors (requirement R-SEC-7).
pub(super) fn stderr_tail(output: &Output) -> Box<str> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let (stderr, _) = super::hidden::strip(&stderr);
    let stderr = stderr.trim_end();
    let skip = stderr.chars().count().saturating_sub(MAX_STDERR_CHARS);
    stderr.chars().skip(skip).collect()
}

// The tests make exit statuses as Unix has them; what they check does not depend on the OS.
#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt as _;

    use super::*;

    #[tokio::test]
    async fn a_command_that_runs_too_long_is_stopped_with_its_limit() {
        let limit = Duration::from_millis(20);
        let never = std::future::pending::<io::Result<Output>>();
        let error = limited(limit, never).await.expect_err("timed out");
        assert!(
            matches!(error, RunError::TimedOut(at) if at == limit),
            "{error:?}"
        );
        assert_eq!(
            RunError::TimedOut(Duration::from_mins(30)).to_string(),
            "cargo did not finish in 1800 seconds"
        );
    }

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
        // The package depended on, not a dependency of it.
        let missing_dependency = "error: no matching package named `bar` found\nlocation searched: \
                                  crates.io index\nrequired by package `foo v0.1.0 (https://x/r)`";
        assert!(no_package_named(missing_dependency, "bar"));
        assert!(!no_package_named(missing_dependency, "foo"));
        assert!(!no_package_named(missing_dependency, "ba"));
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
        // cargo 1.99.1, of a package's `rust-version`, and the wording of older cargo.
        assert_eq!(
            classify(
                "error: rustc 1.99.1 is not supported by the following package:\n  a@0.1.0 requires rustc 1.999",
                false
            ),
            Failure::TooOld
        );
        assert_eq!(
            classify(
                "error: package `a v0.1.0` cannot be built because it requires rustc 1.999 or newer, while the currently active rustc version is 1.70.0",
                false
            ),
            Failure::TooOld
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
        // Colors that a cut would leave a part of.
        let colored = format!("{}error", "\u{1B}[31m".repeat(MAX_STDERR_CHARS));
        assert_eq!(&*stderr_tail(&failed(&colored)), "error");
    }
}

/// What cargo itself does while another holds a lock, on every OS: cargo locks with `flock` on
/// Unix and `LockFileEx` on Windows, as `File::lock` does.
#[cfg(test)]
mod lock_tests {
    use std::fs;

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
