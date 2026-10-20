# chatora-rustdoc-mcp

> **Status: early (0.1).** It looks up crates on docs.rs and the standard library. Building documentation locally (for your own workspace, and for crates docs.rs has none of) is not there yet. Names, arguments and responses may still change.

An MCP server that looks up Rust API documentation from rustdoc JSON, so that coding agents such as Claude Code write Rust against real definitions instead of memory.

- Looks up the version your project resolves in `Cargo.lock`, without the agent naming a version, or any version you name.
- Shows paths you can `use`, following re-exports, also into other crates (`reqwest::header::HeaderMap` is shown from `http`).
- Tells which features an item needs, whether your project enables them, and whether docs.rs built the documentation with them.
- Looks up the standard library of a nightly toolchain, with the release each item became stable in.
- Lists the methods of a type (its own and its traits'), the traits a type implements (with the conditions of `Send` and `Sync`), and the implementors of a trait within its crate.

## Platforms: Linux for now

It builds and runs on Linux, macOS and Windows, but since 2026-05-01 docs.rs builds most new releases for `x86_64-unknown-linux-gnu` only. The server looks up the documentation for the host's target, so on macOS and Windows it answers `docs_unavailable` (`no_target_json`) for most releases since then; asking with `target: "x86_64-unknown-linux-gnu"` gets the Linux documentation, which may lack items for your platform. Building documentation locally, which will cover other targets, comes in a later release.

## Installation

```sh
cargo install --locked chatora-rustdoc-mcp
```

It needs Rust 1.99 or later to build, and Claude Code v2.1.292 or later. Prebuilt binaries are planned.

Register it with Claude Code for all your projects:

```sh
claude mcp add --scope user chatora-rustdoc-mcp -- chatora-rustdoc-mcp serve
```

The server reads the project Claude Code is opened in (`CLAUDE_PROJECT_DIR`, or its working directory) for the versions in `Cargo.lock`; tools also take a `project` argument inside it. Opened in a directory without `Cargo.toml` and without one directly inside it, such as `src`, the server takes the nearest `Cargo.toml` above it in the same git working tree. The server does not see the git worktree that a subagent works in: an agent working there has to pass the worktree's directory as `project`, which the tools' descriptions tell it to do, and the server reads the same project there. For the standard library, it uses a nightly toolchain with the `rust-docs-json` component if one is installed and is not older than the project's toolchain, and otherwise downloads that component of the latest nightly from static.rust-lang.org (about 4 MB).

The server speaks only MCP 2026-07-28. When Claude Code first connects to a server, it asks whether the server supports that version; if the answer comes too late, Claude Code uses the older protocol with that server for seven days, and during that time it cannot connect to this server.

Subagents that list their `tools` get no MCP tools unless you add `mcp__chatora-rustdoc-mcp` to the list.

## Tools

| Tool | Answers |
| --- | --- |
| `list_crates` | What the project depends on, at which versions, and why a crate is in the graph |
| `resolve_version` | The latest release, the version the project uses, whether one exists or is yanked, and whether docs.rs has its JSON |
| `crate_overview` | What a crate is and how it is organized: its modules, items and features |
| `search_items` | Items by name, alias, documentation or path, in one crate or across the project's direct dependencies |
| `get_item` | One item: its signature, documentation, the features it needs, its stability, and where it is defined |
| `list_module` | What a module has and re-exports, or a crate's whole public API |
| `list_methods` | The methods that can be called on a type, and which need a `use` |
| `list_impls` | The traits a type implements, and the conditions of its auto traits |
| `list_implementors` | The types that implement a trait in its crate, and what an implementation must write |

Every response says where its data came from (`provenance`): the crate, the version and how it was chosen, the target, and the features docs.rs built it with.

## Cache

rustdoc JSON and what the server makes of it are cached in `$XDG_CACHE_HOME/chatora-rustdoc-mcp/` (`~/.cache/chatora-rustdoc-mcp/` when unset) on Linux and macOS, and in `%LOCALAPPDATA%\chatora-rustdoc-mcp\cache\` on Windows; `CHATORA_RUSTDOC_MCP_CACHE__DIR` (an absolute path; a relative one is replaced by the default, which the log says) moves it.

The standard library takes about 175 MB for each nightly. The server uses the latest nightly (or, when it writes a rustdoc JSON format that this version of the server does not read, the newest one of the last 30 days that it reads), so a new one comes each day it is used; the standard library of a nightly unused for 7 days is removed. A large crate takes tens of MB, and nothing else is removed from the cache yet; deleting the directory is safe while no server runs. A command to manage the cache is planned.

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

## Settings

Settings are environment variables for now; a settings file is planned. Besides those of the cache and the logs above:

| Variable | Default | |
| --- | --- | --- |
| `CHATORA_RUSTDOC_MCP_LIMITS__CONCURRENT_CALLS` | `16` | How many tool calls are handled at once; more get `busy` |

A setting with a value that is not valid is replaced by its default, which the log says.

## Security

The server needs no confirmation for anything it does, and builds nothing yet. When it starts, it runs `rustc -vV` in its cache directory to know the host's target. For a lookup in the project Claude Code is opened in, it runs `cargo metadata`, `cargo tree` and `rustc -vV` there; for the standard library, `rustup run nightly rustc` in its cache directory. It does not run anything for a `project` outside the project Claude Code is opened in, except for the same place in another working tree of its git repository (`git worktree`, as `claude --worktree` and subagents make), which it reads as the project itself.

- `[toolchain] path` in the project's `rust-toolchain.toml` can make these commands run any executable, and `build.rustc` and `build.rustc-wrapper` in its `.cargo/config.toml` can do the same for `cargo metadata` and `cargo tree`; `cargo metadata` also fetches the git dependencies in `Cargo.toml` from whatever hosts they name, and when `rust-toolchain.toml` names a toolchain that is not installed, rustup may install it (hundreds of MB) to run them, as it would for your own `cargo`. rust-analyzer does the same when it opens the project. Do not open untrusted repositories in Claude Code with this server enabled.
- The server connects to docs.rs, index.crates.io, static.crates.io and static.rust-lang.org over HTTPS, and to nothing else: it follows a redirect only within the same host. It uses a proxy that the usual environment variables (`HTTPS_PROXY`) name.
- A repository you trust in Claude Code can also change this server's settings through the `env` block of its `.claude/settings.json`, just as its hooks can run commands.
- Keep the cache and log directories out of places other users can write to, such as `/tmp`: another user could change the cached documentation, or, as rustup reads `rust-toolchain.toml` from the directory a command runs in and every parent of it, make `rustc` and `rustup` in the cache directory run their programs. The server creates its directories with your default permissions, so if other users can read your home directory, they can also read the logs, which contain project paths.

## Planned

- Local builds of rustdoc JSON for your workspace (with private items), for path, git and private-registry dependencies, for crates docs.rs has no JSON of, and for other targets. Local builds will run build scripts and procedural macros outside Claude Code's sandbox; a separate tool, which you can make ask for permission, will build code from outside the project.
- The implementors of a trait across your dependency graph, the methods of the type a type dereferences to, and the source of items.
- Feature inspection, examples, changelogs, API diffs between versions, crate metadata, and advisories.
- A CLI to manage the cache, a settings file, and a Claude Code plugin with a skill.

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
