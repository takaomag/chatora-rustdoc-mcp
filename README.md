# chatora-rustdoc-mcp

> **Status: early development.** Nothing works yet. Everything below describes what is planned, and it will change.

An MCP server that looks up Rust API documentation from rustdoc JSON, so that coding agents such as Claude Code write Rust against real definitions instead of memory.

## Planned features

- Looks up the versions your project actually resolves (from `Cargo.lock`), without the agent having to name a version.
- Covers your dependencies, your own workspace (including private items), and the standard library in one server.
- Shows paths you can actually `use`, resolving re-exports instead of exposing private definition paths.
- Fetches rustdoc JSON for crates.io dependencies from docs.rs (building it locally when docs.rs has none), and builds it locally for your workspace and for path, git, and private-registry dependencies.
- Comes with a small CLI to manage the cache (and possibly to install the skill).
- Ships as a Claude Code plugin (MCP server and skill).
- Supports Linux, macOS, and Windows.

## Installation (planned)

This project is not published on crates.io yet. Until it is, do not install a crate by this name. Once published, it will be installed with:

```sh
cargo install --locked chatora-rustdoc-mcp
```

Prebuilt binaries will also be published on GitHub Releases; whether to support `cargo binstall` or installer scripts is not decided yet. Claude Code v2.1.292 or later is required.

The server speaks only MCP 2026-07-28. When Claude Code first connects to a server, it asks whether the server supports that version; if the answer comes too late, Claude Code uses the older protocol with that server for seven days, and during that time it cannot connect to this server. How to reset this is not confirmed yet.

## Usage (planned)

```sh
# MCP server over stdio
chatora-rustdoc-mcp serve

# CLI for humans (syntax not final)
chatora-rustdoc-mcp cache ls
chatora-rustdoc-mcp cache gc
chatora-rustdoc-mcp cache clear serde@1.0.228
chatora-rustdoc-mcp install-skill   # optional, may come later
```

Subagents that list their `tools` get no MCP tools unless you add `mcp__chatora-rustdoc-mcp` (or, for a plugin install, the `mcp__plugin_<plugin>_<server>` name) to the list.

## Logs

The server writes a log file of JSON Lines, a new one for each run, in `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs/` (`~/.local/state/chatora-rustdoc-mcp/logs/` when unset) on Linux and macOS, and in `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs\` on Windows. Each tool call has a line with:

- its `request_id`, which an error result also carries, so that a reported error leads to its line
- the tool and its arguments: the crate, paths (including the project's path, which may contain your user name), and the cursor in full, and other strings cut to 200 characters
- how long it took, the size of the response and its estimated tokens, and the error code if it failed
- where the data came from, whether the cache had it, and the URLs fetched (without query strings)

Warnings and errors go there too, with the `request_id` of the call they happened in. GitHub tokens and the contents of responses are not logged. A file larger than 20MB is followed by another; files older than 14 days, and the oldest while all of them are over 50MB, are removed, except those a running server is writing. Nothing is removed from a directory the server did not create (one with no `.chatora-rustdoc-mcp-logs` file).

The server also logs to stderr. These environment variables change the logs:

| Variable | Default | |
| --- | --- | --- |
| `CHATORA_RUSTDOC_MCP_LOG__DIR` | as above | The directory of the log files (an absolute path) |
| `CHATORA_RUSTDOC_MCP_LOG__FILE_MAX_SIZE` | `20MB` | The size of a file before the next one (`KB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`) |
| `CHATORA_RUSTDOC_MCP_LOG__RETENTION_DAYS` | `14` | How many days files are kept |
| `CHATORA_RUSTDOC_MCP_LOG__TOTAL_MAX_SIZE` | `50MB` | The size of all files together |
| `CHATORA_RUSTDOC_MCP_LOG__FILTER` | `info` | Which lines stderr gets, in [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) syntax; if any part is not valid, the default is used. The log file is not filtered |

## Security

Local builds run `cargo`, and with it build scripts and procedural macros, outside Claude Code's sandbox. The server itself never asks for confirmation.

- The lookup tools build only the project Claude Code is opened in, plus the dependencies that `cargo check --all-targets` builds for the project's target platforms (`build.target` in `.cargo/config.toml`, or the host if unset; if several are listed, lookups without a `target` argument use the first), the same set rust-analyzer builds. If that directory has no `Cargo.toml`, each workspace directly inside it (as of server startup) counts as a project; a repository cloned there later is not included, and neither is a workspace nested deeper (open Claude Code in it, or point `build_external_docs` at it). Given a different `target`, the lookup tools build nothing: they use docs.rs, or what `build_external_docs` built for that target.
- A separate tool, `build_external_docs`, builds code from anywhere else the agent points it at: another project directory, a git repository, a crate that is not a dependency, or a version that is not in `Cargo.lock`, as well as the dependencies of this project that the lookup tools cannot build. It can also build any of these, as well as this project's own code and dependencies, for another target.

Lookups in a project run `cargo metadata`, `cargo tree`, and `rustc -vV` even when nothing is built. `[toolchain] path` in the project's `rust-toolchain.toml` can make all of them run any executable; `build.rustc` and `build.rustc-wrapper` in its `.cargo/config.toml` can do the same for `cargo metadata` and `cargo tree`; and `cargo metadata` fetches the git dependencies in `Cargo.toml` from whatever hosts they name.

To allow the lookup tools but still be asked before code from elsewhere is built, use these Claude Code permission rules (for a plugin install, the tool names start with `mcp__plugin_<plugin>_<server>__`):

```json
{
  "permissions": {
    "allow": ["mcp__chatora-rustdoc-mcp__*"],
    "ask": ["mcp__chatora-rustdoc-mcp__build_external_docs"]
  }
}
```

Without the `ask` rule, the wildcard also allows `build_external_docs` without a prompt. With it, what happens depends on where and how Claude Code runs (checked with Claude Code 2.1.301):

| Where | Permission mode | `build_external_docs` |
| --- | --- | --- |
| Terminal | Manual, `acceptEdits`, auto, plan | Asks first |
| Terminal | `bypassPermissions` | Runs without asking (contradicts the Claude Code docs, which say ask rules apply in every mode; see [anthropics/claude-code#95741](https://github.com/anthropics/claude-code/issues/95741)) |
| VS Code | Manual | Asks first |
| VS Code | auto | Runs without asking (also contradicts the docs, and the terminal asks in auto mode; see [anthropics/claude-code#83766](https://github.com/anthropics/claude-code/issues/83766)) |
| Headless (`claude -p`) | Any, even with `--allowedTools`, `bypassPermissions`, `dontAsk`, or a `PreToolUse` hook that returns `allow` | Refused |

If you want to be asked before code from elsewhere is built, do not use `bypassPermissions`, and in VS Code use Manual rather than auto. To use `build_external_docs` in headless mode, run with settings that have no `ask` rule for it and add it to `--allowedTools`.

- Do not point `build_external_docs` at code you do not trust, and do not open untrusted repositories in Claude Code with this server enabled.
- A repository you trust in Claude Code can also change this server's settings through the `env` block of its `.claude/settings.json` (for example the cargo path, or, if mirror settings are added, the mirror URLs, including where your GitHub token is sent), just as its hooks can run commands or read the token.
- `git` URLs with `http://` or `git://` are accepted, with a warning in the result; downloads over them can be tampered with in transit.
- Do not use this server in CI jobs that handle untrusted code, such as pull requests from forks: their build scripts would run with the job's secrets.
- Builds never prompt you for git or SSH credentials themselves (terminal prompts and askpass dialogs are turned off), but prompts raised by your SSH agent or credential helper still appear: for example, 1Password's or Secretive's SSH agent asking you to approve a key, macOS asking for Keychain access, or gnome-keyring asking to unlock.
- Keep the cache, state, and log directories out of places other users can write to, such as `/tmp`: cargo reads `.cargo/config.toml` from every parent directory, so a file another user puts in `/tmp/.cargo/` would apply to this server's builds, and another user could tamper with the build directories in the cache to run code as you. Like cargo, the server creates its cache, state, and log directories with your default permissions, so if other users can read your home directory, they can also read the cached documentation of your private code and the logs.
- The source lookup tools read only package contents (sources and the files their builds read, `Cargo.toml`, the readme and examples a manifest names, and for registry and git dependencies the contents of the `.crate` or checkout; never dotfiles such as `.env`), but they still bypass Claude Code's `Read` deny rules for those files, and the `project` argument lets them read those files in any workspace. Local builds also put the contents of files that code pulls into its documentation with `include_str!` into lookup results, even files outside the project (for example `~/.ssh/id_rsa`).

## Development

The project builds on the stable channel (selected in `rust-toolchain.toml`; the MSRV is `rust-version` in `Cargo.toml`), but formatting requires nightly rustfmt, because `rustfmt.toml` uses unstable options.

```sh
rustup toolchain install nightly --component rustfmt
cargo +nightly fmt
```

Plain `cargo fmt` uses the stable toolchain, which `rust-toolchain.toml` installs with the minimal profile, without rustfmt. If your stable toolchain already has rustfmt, rustfmt ignores the unstable options with warnings and produces different output. rust-analyzer is already configured to format with nightly (in `rust-analyzer.toml`, and in `.vscode/settings.json` for VS Code).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
