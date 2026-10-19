//! The `chatora-rustdoc-mcp` binary: an MCP server that looks up Rust documentation, plus a small CLI to manage its cache.

use std::{error::Error as _, ffi::OsString};

use const_format as _;
use derive_more as _;
use etcetera as _;
use reqwest as _;
use rmcp as _;
use semver as _;
use serde_json as _;
// A dev-dependency of the library; the binary's test build sees it too.
#[cfg(test)]
use tempfile as _;
use thiserror as _;
use tracing as _;
use tracing_subscriber as _;
use zstd as _;

const BIN_NAME: &str = env!("CARGO_BIN_NAME");

const USAGE: &str = concat!(
    "Usage: ",
    env!("CARGO_BIN_NAME"),
    " <COMMAND>

Commands:
  serve  Run the MCP server over stdio"
);

fn main() -> std::process::ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [command] if command == "serve" => serve(),
        _ => {
            eprintln!("{USAGE}");
            // 2 is the conventional exit code for a usage error.
            std::process::ExitCode::from(2)
        },
    }
}

fn serve() -> std::process::ExitCode {
    if let Err(error) = chatora_rustdoc_mcp::logging::init() {
        eprintln!("{BIN_NAME}: failed to set up logging: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{BIN_NAME}: failed to start the async runtime: {error}");
            return std::process::ExitCode::FAILURE;
        },
    };
    match runtime.block_on(chatora_rustdoc_mcp::mcp::serve()) {
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
