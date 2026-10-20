//! An environment for the cargo, rustup and git that a test starts, which keeps the user's
//! settings and credentials out and asks nothing (`doc/m2-plan.md` §2, as `isolate_home` in
//! `poc/m0b/env.sh`). In M0b, git started without it showed an askpass dialog on the desktop.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
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
    /// user set for them.
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
