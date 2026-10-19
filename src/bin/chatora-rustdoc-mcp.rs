//! The `chatora-rustdoc-mcp` binary: an MCP server that looks up Rust documentation, plus a small CLI to manage its cache.

use std::{error::Error as _, ffi::OsString, process::ExitCode};

use rmcp as _;
use thiserror as _;

const BIN_NAME: &str = env!("CARGO_BIN_NAME");

const USAGE: &str = concat!(
    "Usage: ",
    env!("CARGO_BIN_NAME"),
    " <COMMAND>

Commands:
  serve  Run the MCP server over stdio"
);

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [command] if command == "serve" => serve(),
        _ => {
            eprintln!("{USAGE}");
            // 2 is the conventional exit code for a usage error.
            ExitCode::from(2)
        },
    }
}

fn serve() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{BIN_NAME}: failed to start the async runtime: {error}");
            return ExitCode::FAILURE;
        },
    };
    match runtime.block_on(chatora_rustdoc::mcp::serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Print the whole chain of causes on one line.
            eprint!("{BIN_NAME}: {error}");
            for cause in std::iter::successors(error.source(), |&cause| cause.source()) {
                eprint!(": {cause}");
            }
            eprintln!();
            ExitCode::FAILURE
        },
    }
}
