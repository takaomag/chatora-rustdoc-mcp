# chatora-rustdoc-mcp

> **Status: early, not yet on crates.io.** It looks up the crates of your workspace, which it builds here with their private items, their dependencies, the standard library and crates on docs.rs. Names, arguments and responses may still change.

An MCP server that looks up Rust API documentation from rustdoc JSON, so that coding agents such as Claude Code write Rust against real definitions instead of memory.

- Looks up your workspace's own crates, built here with their private items, and the version of each dependency that your project resolves in `Cargo.lock`, without the agent naming a version, or any version you name.
- Builds a dependency here when docs.rs has no documentation of it for your target, and, when asked, code outside the project: another workspace, a crate of a git repository, a crate or version the project does not use, or the project for another target.
- After you edit the code, builds it again on the next lookup, and meanwhile answers from the build before, marked as stale.
- Shows paths you can `use`, following re-exports, also into other crates (`reqwest::header::HeaderMap` is shown from `http`).
- Tells which features an item needs, whether your project enables them, and which features the documentation was built with.
- Looks up the standard library of a nightly toolchain, with the release each item became stable in.
- Lists the methods of a type (its own and its traits'), the traits a type implements (with the conditions of `Send` and `Sync`), and the implementors of a trait within its crate.

## Targets and platforms

It builds and runs on Linux, macOS and Windows. The documentation is for the project's `build.target` in its cargo settings (the first of several) for the crates the project uses, or else for the host's target; `target` asks for another.

Since 2026-05-01 docs.rs builds most new releases for `x86_64-unknown-linux-gnu` only. A dependency of the project that docs.rs has no documentation of for the target is built here instead, so for other targets, as on macOS and Windows, the first lookup of most releases since then waits for a build. For a crate the project does not use, or for another target, the lookup answers `docs_unavailable`, and `build_external_docs` builds it; asking with `target: "x86_64-unknown-linux-gnu"` gets the Linux documentation from docs.rs instead, which may lack items for your platform.

## Installation

```sh
cargo install --locked --git https://github.com/takaomag/chatora-rustdoc-mcp
```

It is not on crates.io yet. It needs Rust 1.99 or later to build, and Claude Code v2.1.292 or later. Prebuilt binaries are planned.

Register it with Claude Code for all your projects:

```sh
claude mcp add --scope user chatora-rustdoc-mcp -- chatora-rustdoc-mcp serve
```

To build documentation, the server uses the project's toolchain (through rustup if it is installed), whose rustdoc it lets write rustdoc JSON, an unstable output, by setting `RUSTC_BOOTSTRAP` for the builds it runs. When that toolchain writes a format older than this server reads, it builds with rustup's `nightly` toolchain instead (or the one of the date that `build.nightly_date` names), which you install: the server installs no nightly toolchain and no target for builds. Building for another target needs the standard library of that target (`rustup target add`). For the standard library itself, it uses a nightly toolchain with the `rust-docs-json` component if one is installed and is not older than the project's toolchain, and otherwise downloads that component of the latest nightly from static.rust-lang.org (about 4 MB), or of the newest one of the last 30 days whose format this server reads.

Built with `cargo install --no-default-features`, the server builds nothing: it looks up crates on docs.rs and the standard library only, runs no program at all, and does not read the project, so a version not given is the latest release. It always downloads the standard library, so `std.fetch_from_dist` must stay `true` for it.

The server speaks only MCP 2026-07-28. When Claude Code first connects to a server, it asks whether the server supports that version; if the answer comes too late, Claude Code uses the older protocol with that server for seven days, and during that time it cannot connect to this server. Clients that speak only older versions cannot connect either.

Subagents that list their `tools` get no MCP tools unless you add `mcp__chatora-rustdoc-mcp` to the list.

## Projects

The server reads the project Claude Code is opened in (`CLAUDE_PROJECT_DIR`, or its working directory): its crates and their dependencies at the versions in `Cargo.lock`. Opened in a directory with `Cargo.toml`, the project is that workspace, and a workspace of its own inside it (such as `fuzz/`) is not. Opened in a directory without one, each workspace directly inside it is the project, as found when the server starts (a repository cloned there later is not); with none there either, such as in `src`, the server takes the nearest `Cargo.toml` above it in the same git working tree. A workspace that is not the project is looked up after `build_external_docs` builds it, or with Claude Code opened in it.

The tools take a `project` argument to choose a workspace of the project, or one outside it that `build_external_docs` has built. The server does not see the git worktree that a subagent works in: an agent working there has to pass the worktree's directory as `project`, which the tools' descriptions tell it to do, and the server reads it as the project itself.

## Tools

| Tool | Answers |
| --- | --- |
| `list_crates` | What the project depends on, at which versions, and why a crate is in the graph |
| `resolve_version` | The latest release, the version the project uses, whether one exists or is yanked, and whether docs.rs has its JSON |
| `crate_overview` | What a crate is and how it is organized: its modules, items and features |
| `search_items` | Items by name, alias, documentation or path, in one crate or across the workspace's crates (with their private items) and the project's direct dependencies |
| `get_item` | One item: its signature, documentation, the features it needs, its stability, and where it is defined |
| `list_module` | What a module has and re-exports, or a crate's whole public API |
| `list_methods` | The methods that can be called on a type, and which need a `use` |
| `list_impls` | The traits a type implements, and the conditions of its auto traits |
| `list_implementors` | The types that implement a trait in its crate, and what an implementation must write |
| `build_external_docs` | Builds code that the other tools do not build themselves, for them to look up: another workspace, a crate of a git repository, a crate or version the project does not use, or the project's crates for another target or with the features of their `[package.metadata.docs.rs]`. See [Security](#security) |

`list_crates` and `build_external_docs` are not in a server built with `--no-default-features`.

Every response says where its data came from (`provenance`): the crate, the version and how it was chosen, the target, whether docs.rs or a build here made it, and the features it was built with.

## When an answer is not ready

The tools answer within 30 seconds (`limits.request_timeout_secs`); work that takes longer goes on in the background. A build of the project's documentation is waited for 15 seconds (`build.wait_secs`). Building this repository's own documentation (274 packages) took 51 to 114 seconds from nothing on a Linux machine of 12 CPUs, depending on the jobs, and about 7 seconds after an edit; on the runners of GitHub Actions (3 or 4 CPUs) it took 48 to 97 seconds on Linux, 102 to 181 on macOS and 114 to 231 on Windows from nothing, 8 to 15 seconds after an edit on macOS and Windows, and a dependency such as `axum` 14 to 17 seconds.

| The answer | What it means |
| --- | --- |
| `status: building`, with `building_reason` and `retry_after_secs` | The documentation is being built (`building`), fetched (`fetching`) or indexed (`indexing`), or the build waits for cargo's lock (`waiting_for_lock`) or for another build to end (`waiting_for_slot`). Call again after `retry_after_secs` |
| `partial` with `retry_after_secs` | A search across crates answered with some of them while others are still being built |
| `stale`, with `stale_reason` | An answer from an earlier build: the code was edited and is being built again (`rebuilding`), what `build_external_docs` built has changed since (`inputs_changed`), the branch of a git crate was resolved long ago (`ref_ttl_expired`), or the workspace it built is gone (`project_missing`). The hints say what to do; an item may have changed, and its lines moved |
| `build_failed`, with the compiler's first errors | The build failed. The failure is kept while the sources and settings are the same: after you fix its cause elsewhere, as a missing system library or credentials, call again with `refresh: true`. For `build_external_docs`, calling it again with the same arguments tries again |
| `toolchain_missing` | The toolchain, or the standard library of the target, that the build needs is not installed: install it |
| `toolchain_too_old` | The code needs a newer toolchain than the one it is built with, as its `rust-version` or an unstable feature says: update it, or set `build.nightly_date` when the server chose the nightly |
| `unsupported_format` | The toolchain writes a rustdoc JSON format that this server does not read: update the server or the toolchain, or install rustup's `nightly`, which the server builds with when the project's toolchain is too old |
| `lockfile_outdated` | `Cargo.lock` does not match `Cargo.toml`: builds run with `--locked`, so update it yourself with `cargo fetch`, which changes only what it must (`cargo update` raises every dependency) |
| `fetch_failed`, `offline_miss` | cargo could not fetch a git dependency, as `reason` tells, or the server is offline and the cache lacks what the build needs |
| `docs_unavailable` | No documentation of it can be had here: `reason` says why, such as `build_unsupported` (`build_external_docs` builds it, or will in a later version) or `target_unsupported` (a target of the project's own, which the server does not build) |

Builds use target directories of their own in the cache, so they do not wait for the project's `target/` or for rust-analyzer's builds. They do share cargo's package cache (`~/.cargo`) and its lock with rust-analyzer and with your own cargo, which is when `waiting_for_lock` comes, and they share the CPUs: `build.jobs` and `limits.concurrent_builds` limit what the server takes.

## Security

The server needs no confirmation for anything it does. Builds run `cargo`, and with it build scripts and procedural macros, outside Claude Code's sandbox.

- The lookup tools build only the crates of the project Claude Code is opened in, and the dependencies that `cargo check --all-targets` builds for the project's build targets (`build.target`, or the host's): the same set rust-analyzer builds when it opens the project. Given another `target`, they build nothing but the project's procedural macros, which are built for the host: they use docs.rs, or what `build_external_docs` built for that target.
- `build_external_docs` builds code from anywhere else the agent points it at: another workspace, a git repository, a crate that is not a dependency, or a version that is not in `Cargo.lock`, as well as the dependencies of this project from crates.io that the lookup tools cannot build. It can also build the project's crates for another target, or with the features of their `[package.metadata.docs.rs]`.
- Even when nothing is built, lookups in the project run `cargo metadata`, `cargo tree`, `rustc -vV`, `rustdoc -vV` and `git ls-files` there, and for the standard library `rustup run nightly rustc` in the cache directory, where the server also runs `rustc -vV` when it starts, to know the host's target. `[toolchain] path` in the project's `rust-toolchain.toml` can make them run any executable, and `build.rustc` and `build.rustc-wrapper` in its `.cargo/config.toml` can do the same for cargo; `cargo metadata` also fetches the git dependencies in `Cargo.toml` from whatever hosts they name, and when `rust-toolchain.toml` names a toolchain that is not installed, rustup may install it (hundreds of MB), as it would for your own `cargo`. rust-analyzer does the same when it opens the project. Do not open untrusted repositories in Claude Code with this server enabled.

To allow the lookup tools but still be asked before code from elsewhere is built, use these Claude Code permission rules (for a plugin install, the tool names start with `mcp__plugin_<plugin>_<server>__` instead):

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

- Do not point `build_external_docs` at code you do not trust.
- Do not use this server in CI jobs that handle untrusted code, such as pull requests from forks: their build scripts would run with the job's secrets.
- Documentation built here holds the contents of files that the code pulls into it with `include_str!`, even files outside the project (for example `~/.ssh/id_rsa`), which the lookup tools then answer with, whatever Claude Code's `Read` deny rules say.
- Builds never prompt you for git or SSH credentials themselves (terminal prompts and askpass dialogs are turned off), but prompts raised by your SSH agent or credential helper still appear: for example, 1Password's or Secretive's SSH agent asking you to approve a key, macOS asking for Keychain access, or gnome-keyring asking to unlock.
- The server connects to docs.rs, index.crates.io, static.crates.io and static.rust-lang.org over HTTPS, and to nothing else: it follows a redirect only within the same host. The cargo it runs fetches the project's dependencies from where `Cargo.toml` and `Cargo.lock` name, and `build_external_docs` clones a git repository from its URL; `git` URLs with `http://` or `git://` are accepted, with a warning in the result, as what comes over them can be changed on the way. `network.offline` runs cargo with `--offline` too. A `.crate` that cargo has already fetched into `$CARGO_HOME/registry/cache` is read from there, after its checksum is checked against the crates.io index, instead of being fetched again. The server sends a User-Agent of its own, which crates.io does not count as a download. It uses a proxy that the usual environment variables (`HTTPS_PROXY`) name.
- Answers lose what third parties wrote that could hide text from you while the model reads it: control characters other than newlines and tabs, ANSI escape sequences, bidirectional controls and marks, and invisible characters, in documentation, names and build output alike. The log counts those removed from each answer as it is sent.
- A repository you trust in Claude Code can also change this server's settings through the `env` block of its `.claude/settings.json`, just as its hooks can run commands.
- Keep the cache, state and log directories out of places other users can write to, such as `/tmp`: cargo reads `.cargo/config.toml` from every parent directory, and rustup reads `rust-toolchain.toml` from the directory a command runs in and every parent of it, so files another user puts there would apply to what the server runs in the cache (the builds of `build_external_docs`, and `rustc` and `rustup` for the host and the standard library), and another user could change the cached documentation or the builds in the cache to run code as you. The server creates its directories with your default permissions, so if other users can read your home directory, they can also read the documentation of your private code in the cache, and the logs, which contain project paths.
- A server built with `--no-default-features` runs no cargo, build script or procedural macro at all.

## Cache

rustdoc JSON, the builds and what the server makes of them are kept in `$XDG_CACHE_HOME/chatora-rustdoc-mcp/` (`~/.cache/chatora-rustdoc-mcp/` when unset) on Linux and macOS, and in `%LOCALAPPDATA%\chatora-rustdoc-mcp\cache\` on Windows; the setting `cache.dir` (an absolute path; see [Settings](#settings)) moves it. The directory has a `chatora-rustdoc-mcp.cache` file and a `CACHEDIR.TAG`, and the server removes nothing from a directory without that file.

What is fetched from docs.rs, crates.io and static.rust-lang.org may take 5 GB (`cache.docs_max_size`), and what builds make 20 GB (`cache.target_max_size`), so the cache may take 25 GB in all. A build takes hundreds of MB (this repository's, 535 MB), the standard library about 175 MB for each nightly, and a large crate from docs.rs tens of MB. When the server starts, and after its builds at most once an hour, it removes what has gone unused longest while either part is over its limit, builds unused for 30 days (`cache.target_max_age_days`), and the standard library of a nightly unused for 7 days; it keeps what was used in the last hour and what a running server uses.

The binary manages the cache too:

```sh
chatora-rustdoc-mcp cache ls                    # what the cache has, and how much of its limits it takes
chatora-rustdoc-mcp cache gc [--dry-run]        # remove what the server would remove
chatora-rustdoc-mcp cache clear [--dry-run]     # remove all that the cache keeps
chatora-rustdoc-mcp cache clear tokio@1.48.0    # or what it keeps of a crate, of one version or of all
```

They are safe to run while servers run: what a server uses is left, and listed (on Windows, a file in use cannot be removed, and is left for the next time). They exit with 1 when they cannot run, as on a directory that is not this server's cache or while another removes from it, and `cache clear` also when something was left; with 2 for arguments they do not take, such as a crate of the standard library, which `cache clear` without a crate removes.

Servers that share the cache, as the sessions of Claude Code open at once do, take turns on what they fetch, build, make and remove, with OS file locks on the empty files in `locks/` in the cache directory. Such locks may not work on a network file system (NFS), where two servers could then fetch the same thing at once, and a server could take another for stopped; what they write stays whole either way. Each running server holds a lock of its own there, by which the others tell that it runs: what a server that has stopped left in `work/` is removed by the next server that starts.

## Logs

The server writes a log file of JSON Lines, a new one for each run, in `$XDG_STATE_HOME/chatora-rustdoc-mcp/logs/` (`~/.local/state/chatora-rustdoc-mcp/logs/` when unset) on Linux and macOS, and in `%LOCALAPPDATA%\chatora-rustdoc-mcp\state\logs\` on Windows. Each tool call has a line with:

- its `request_id`, which an error result also carries, so that a reported error leads to its line
- the tool and its arguments: the crate, paths (including the project's path, which may contain your user name), and the cursor in full, and other strings cut to 200 characters
- how long it took, the size of the response and its estimated tokens, and the error code if it failed
- where the data came from, whether the cache had it, and the URLs fetched (without query strings); `status: building` instead of whether the cache had it, when the data was still being built, fetched or indexed
- how many characters that could hide text were removed from the response (`hidden_chars`), when there were any

Warnings and errors go there too, with the `request_id` of the call they happened in. The contents of responses are not logged. A file larger than 20MB is followed by another; files older than 14 days, and the oldest while all of them are over 50MB, are removed, except those a running server is writing. Nothing is removed from a directory the server did not create (one with no `.chatora-rustdoc-mcp-logs` file).

The server also logs to stderr. The `log` settings (see [Settings](#settings)) change the logs.

## Settings

The server reads its settings when it starts, from `chatora-rustdoc-mcp/config.toml` in `$XDG_CONFIG_HOME` (`~/.config` when unset) on Linux and macOS and in `%APPDATA%` on Windows, or from the file that `CHATORA_RUSTDOC_MCP_CONFIG` names (an absolute path), and from environment variables, which win over the file. A key such as `log.dir` is written in the file as

```toml
[log]
dir = "/var/log/chatora-rustdoc-mcp"
```

and in the environment as `CHATORA_RUSTDOC_MCP_LOG__DIR`: the key in capitals, with `__` between its parts. An empty variable counts as unset. In the environment, a number is digits, and a size or a path is the text itself.

| Key | Default | |
| --- | --- | --- |
| `build.cargo_path`, `build.rustc_path`, `build.rustdoc_path` | none | The cargo, rustc and rustdoc to build the documentation of the project with (absolute paths), instead of those of rustup or `PATH`. Those not set are looked for next to one that is, with its version suffix (`rustdoc-1.80` next to `rustc-1.80`) or else without one; rustc and rustdoc must be of one toolchain |
| `build.cfg_docsrs` | `true` | Whether a build of a member's documentation passes `--cfg docsrs`, as docs.rs does; a member whose build fails with it is built without it, with a warning |
| `build.jobs` | half the CPUs, two at least where there are two | The jobs (`-j`) of a build of the project's documentation |
| `build.max_diagnostics` | `2` | How many errors of the compiler the error of a failed build of the project's documentation shows, each cut to 3,500 characters and all to 8,000 |
| `build.nightly_date` | none | The date of the nightly (`2026-07-10`) to build with when the project's toolchain writes a rustdoc JSON format older than this server reads; the latest nightly when unset. It must be installed: the server does not install it |
| `build.timeout_secs` | `1800` | How long one run of cargo may take, a build, `cargo metadata` or `cargo tree`, before it is stopped |
| `build.wait_secs` | `15` | How long a tool call waits for a build of the project's documentation, telling `status: building` after it; no longer than a call waits for other work |
| `cache.dir` | see [Cache](#cache) | The directory of the cache (an absolute path) |
| `cache.docs_max_size` | `"5GB"` | How much what is fetched may take, before what has gone unused longest is removed |
| `cache.target_max_age_days` | `30` | How many days a build is kept unused |
| `cache.target_max_size` | `"20GB"` | How much what builds make may take, before what has gone unused longest is removed |
| `limits.concurrent_builds` | `2` | How many builds of the project's documentation run at once; more wait for one to end, telling `waiting_for_slot` |
| `limits.concurrent_calls` | `16` | How many tool calls are handled at once; more get `busy` |
| `limits.extract_max_size` | `"4GiB"` | The most bytes unpacked from one archive of the standard library |
| `limits.request_timeout_secs` | `30` | How long a tool call may take. A call waits two thirds of it for a crate to be fetched and indexed, telling its progress every 3 seconds when the client asks for it; the work then goes on in the background, and the call answers `status: building` with `retry_after_secs` |
| `log.dir` | see [Logs](#logs) | The directory of the log files (an absolute path) |
| `log.file_max_size` | `"20MB"` | The size of a file before the next one (`KB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`) |
| `log.retention_days` | `14` | How many days files are kept |
| `log.total_max_size` | `"50MB"` | The size of all files together |
| `log.filter` | `"info"` | Which lines stderr gets, in [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) syntax; if any part is not valid, the default is used. The log file is not filtered |
| `network.connect_timeout_secs` | `10` | How long a connection to docs.rs and the other sources may take |
| `network.offline` | `false` | Whether the server is offline: it sends no request, uses only what the cache has (the crates.io index however old, with a warning), runs cargo with `--offline`, and answers `offline_miss` for what the cache lacks |
| `network.read_timeout_secs` | `120` | How long a response may stay silent while it is received (not a limit on the whole transfer) |
| `std.fetch_from_dist` | `true` | Whether the standard library may be downloaded from static.rust-lang.org; with `false`, only a nightly toolchain with `rust-docs-json` installed here gives it, even one older than the project's toolchain |

The server always starts: a file that cannot be read or is not TOML is ignored, a value that is not valid is replaced by its default, and a key that this version does not read, in the file or as a `CHATORA_RUSTDOC_MCP_` variable, is ignored. The log says each of them, so that a misspelled setting does not go unnoticed.

## Planned

- Feature inspection, examples, changelogs, and the source of items.
- The methods of the type a type dereferences to and of primitive types, the implementors of a trait across your dependency graph, and API diffs between versions.
- Crate metadata and advisories.
- A Claude Code plugin with a skill, and prebuilt binaries.

## Known limitations

- Some code is not built yet, and is answered as coming in a later version: a path or git dependency that `cargo rustdoc -p` cannot single out in its workspace (one from crates.io is looked up on docs.rs, or built by `build_external_docs`), such a crate of a workspace outside the project or for another target, and a project that builds for no target.
- An item whose name has a character that answers remove, such as a zero-width joiner or a variation selector, cannot be looked up by the path an answer shows.
- On Windows, the builds of a server end with it through a job object, but a process that a build starts in the moment before it joins the job may outlive the server, and the next server does not stop it, as it does on Linux and macOS for what a server that was killed left running.
- A workspace inside the project's workspace, or two or more levels below the directory Claude Code is opened in, is not the project.

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
