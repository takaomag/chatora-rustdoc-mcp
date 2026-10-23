//! The `chatora-rustdoc-mcp` binary. `serve` runs the MCP server that looks up Rust
//! documentation; `cache` manages its cache (requirement R-DIST-2).

use std::{error::Error as _, ffi::OsString};

// Dependencies that only the library uses. Those under `#[cfg(test)]` are dev-dependencies of the
// tests and examples, which the binary's test build sees too.
use derive_more as _;
#[cfg(feature = "local")]
use dunce as _;
use elsa as _;
use etcetera as _;
use flate2 as _;
#[cfg(test)]
use insta as _;
#[cfg(unix)]
use libc as _;
use lzma_rust2 as _;
use memmap2 as _;
use parking_lot as _;
use reqwest as _;
use rkyv as _;
use rmcp as _;
use rustdoc_types as _;
use semver as _;
use serde as _;
use serde_json as _;
use serde_stacker as _;
use sha2 as _;
use tar as _;
#[cfg(test)]
use tempfile as _;
use thiserror as _;
use toml as _;
use tracing as _;
use tracing_subscriber as _;
#[cfg(all(windows, feature = "local"))]
use windows_sys as _;
use zstd as _;

const BIN_NAME: &str = env!("CARGO_BIN_NAME");

const USAGE: &str = const_format::concatcp!(
    "Usage: ",
    env!("CARGO_BIN_NAME"),
    " <COMMAND>

Commands:
  serve     Run the MCP server over stdio
",
    chatora_rustdoc_mcp::cli::USAGE
);

fn main() -> std::process::ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [command] if command == "serve" => serve(),
        [command, rest @ ..] if command == "cache" => {
            let ran = chatora_rustdoc_mcp::cli::cache(rest, USAGE);
            print_out(&ran.stdout);
            eprint!("{}", ran.stderr);
            ran.code
        },
        [command] if command == "help" || command == "-h" || command == "--help" => {
            print_out(&format!("{USAGE}\n"));
            std::process::ExitCode::SUCCESS
        },
        _ => {
            eprintln!("{USAGE}");
            // 2 is the conventional exit code for a usage error.
            std::process::ExitCode::from(2)
        },
    }
}

/// Prints `text` to stdout, which only the commands other than `serve` write to: `serve` keeps it
/// for the messages of MCP (requirement R-SEC-1).
fn print_out(text: &str) {
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    if let Err(error) = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        eprintln!("{BIN_NAME}: failed to write to stdout: {error}");
    }
}

fn serve() -> std::process::ExitCode {
    // Read once, at startup (requirement R-DIST-3).
    let settings = chatora_rustdoc_mcp::config::Settings::load();
    // Written to its end when `serve` returns or a panic unwinds out of it.
    let _flushing = match chatora_rustdoc_mcp::logging::init(&settings) {
        Ok(flushing) => flushing,
        Err(error) => {
            eprintln!("{BIN_NAME}: failed to set up logging: {error}");
            return std::process::ExitCode::FAILURE;
        },
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(chatora_rustdoc_mcp::task::THREAD_STACK_SIZE)
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{BIN_NAME}: failed to start the async runtime: {error}");
            return std::process::ExitCode::FAILURE;
        },
    };
    let served = runtime.block_on(chatora_rustdoc_mcp::mcp::serve(&settings));
    // Not the drop of the runtime, which waits for the work on blocking threads however long it
    // takes: an archive being made, or a wait for a lock that another process holds.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    match served {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // Print the whole chain of causes on one line.
            eprint!("{BIN_NAME}: {error}");
            for cause in std::iter::successors(error.source(), |&cause| cause.source()) {
                eprint!(": {cause}");
            }
            eprintln!();
            std::process::ExitCode::FAILURE
        },
    }
}
