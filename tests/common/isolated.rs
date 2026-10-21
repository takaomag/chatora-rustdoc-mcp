//! An environment for the cargo, rustup and git that a test starts, which keeps the user's
//! settings and credentials out and asks nothing (`doc/m2-plan.md` §2, as `isolate_home` in
//! `poc/m0b/env.sh`). In M0b, git started without it showed an askpass dialog on the desktop.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// A temporary directory with a home, a cargo home and an empty git config of its own, and room
/// for the test's projects.
#[derive(Debug)]
pub(crate) struct Isolated {
    dir: tempfile::TempDir,
}

impl Isolated {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let isolated = Self { dir };
        fs::create_dir_all(isolated.home().join(".config")).expect("made");
        fs::create_dir_all(isolated.cargo_home()).expect("made");
        fs::write(isolated.git_config(), "").expect("written");
        isolated
    }

    /// The directory for the test's own files, such as its projects.
    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn home(&self) -> PathBuf {
        self.path().join("home")
    }

    pub(crate) fn cargo_home(&self) -> PathBuf {
        self.path().join("cargo-home")
    }

    /// The global git config, which is empty.
    fn git_config(&self) -> PathBuf {
        self.home().join(".gitconfig")
    }

    /// Writes the `config.toml` of the cargo home, which every cargo started with [`Self::apply`]
    /// reads.
    pub(crate) fn write_cargo_config(&self, text: &str) {
        fs::write(self.cargo_home().join("config.toml"), text).expect("written");
    }

    /// Makes `command` run in this environment. Of the test's own environment, it keeps what
    /// rustup needs to find its toolchains (`RUSTUP_HOME`, `RUSTUP_TOOLCHAIN` and `PATH`), and
    /// removes what would change cargo, rustc or git: the variables that `cargo test` and the
    /// user set for them. The server does not pass `RUSTUP_TOOLCHAIN` on to what it runs in a
    /// project (M2's D1), which then uses the project's toolchain, or else rustup's default.
    pub(crate) fn apply<'c>(&self, command: &'c mut Command) -> &'c mut Command {
        for (name, _) in std::env::vars_os() {
            if is_removed(&name) {
                command.env_remove(name);
            }
        }
        command
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("CARGO_HOME", self.cargo_home())
            .env("CARGO_TERM_COLOR", "never")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.git_config())
            // Never ask for credentials: not on the terminal, and not through a GUI askpass
            // program on the desktop.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("SSH_ASKPASS_REQUIRE", "never")
    }
}

/// The environment variable that tells a test run by [`in_isolated_child`] where its isolated
/// directory is.
const ISOLATED_DIR: &str = "CHATORA_RUSTDOC_TEST_ISOLATED_DIR";

/// Runs the test `name` (its path in the test binary, as `module::test`) again, alone, in a child
/// process in a new [`Isolated`] environment: for a test that calls the library, which starts
/// cargo in the environment of its own process. In that child, it returns the directory of the
/// environment, for the test's projects, and the test goes on. In the test itself, it waits for
/// the child, fails if the child fails, and returns `None`.
#[cfg_attr(
    not(feature = "local"),
    expect(
        dead_code,
        reason = "only the tests of local features call the library's cargo"
    )
)]
pub(crate) fn in_isolated_child(name: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ISOLATED_DIR) {
        return Some(PathBuf::from(dir));
    }
    let isolated = Isolated::new();
    let binary = std::env::current_exe().expect("the test binary");
    let output = output_within(
        isolated
            .apply(&mut Command::new(binary))
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env(ISOLATED_DIR, isolated.path()),
        Duration::from_secs(600),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // A name that matches no test passes, too.
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{name} in an isolated child:\n{stdout}\n{stderr}"
    );
    None
}

/// The output of `command`, run with no stdin, which must end within `limit`: else it is
/// stopped, with what it started on Unix (its process group), and the test fails with what it
/// wrote. What it leaves running may hold its output open after it ends, which is read a few
/// seconds more, and then not waited for.
pub(crate) fn output_within(command: &mut Command, limit: Duration) -> Output {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut *command, 0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the command runs");
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("the command is waited for") {
            break status;
        }
        if started.elapsed() > limit {
            #[cfg(unix)]
            drop(
                Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .status(),
            );
            drop(child.kill());
            drop(child.wait());
            panic!(
                "{command:?} did not end in {limit:?}:\n{}\n{}",
                String::from_utf8_lossy(&rest(&stdout)),
                String::from_utf8_lossy(&rest(&stderr))
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Output {
        status,
        stdout: rest(&stdout),
        stderr: rest(&stderr),
    }
}

/// What `pipe` gives, read on a thread of its own and sent as it comes, so that what was read is
/// there even when the pipe does not end.
fn read_all(pipe: Option<impl std::io::Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut piece = vec![0; 8 * 1024];
            while let Ok(read) = pipe.read(&mut piece) {
                if read == 0 || sender.send(piece[..read].to_vec()).is_err() {
                    break;
                }
            }
        });
    }
    receiver
}

/// What `receiver` has from [`read_all`]: until its pipe ends, or for five seconds.
fn rest(receiver: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let started = Instant::now();
    let mut read = Vec::new();
    while let Ok(piece) =
        receiver.recv_timeout(Duration::from_secs(5).saturating_sub(started.elapsed()))
    {
        read.extend(piece);
    }
    read
}

/// Whether the variable `name` of the test's environment is kept from the children.
fn is_removed(name: &OsString) -> bool {
    let name = name.to_string_lossy();
    [
        "CARGO",
        "__CARGO",
        "RUSTC",
        "RUSTDOC",
        "RUSTFLAGS",
        "GIT_",
        "SSH_ASKPASS",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}
