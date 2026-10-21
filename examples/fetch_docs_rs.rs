//! Fetches rustdoc JSON from docs.rs into the cache and prints what it got (`doc/m0-plan.md`,
//! A4-3).
//!
//! ```sh
//! cargo run --example fetch_docs_rs -- tokio@1.45.1 serde@1.0.228
//! ```
//!
//! For each `crate@version`, it prints one tab-separated line: the crate and version, the
//! `format_version`, the size, the sha256, where the JSON came from (`cache` or `docs.rs`), and the
//! cache path. With `CHATORA_RUSTDOC_MCP_LOG__FILTER=debug`, the log also shows how many zstd
//! layers a download had.

#![expect(
    unused_crate_dependencies,
    reason = "an example uses only some of the package's dependencies"
)]

use std::{
    error::Error,
    fmt::Write as _,
    io::{self, Write as _},
    process::ExitCode,
};

use chatora_rustdoc_mcp::data::{
    self,
    cache::{Cache, CachedJson},
    crate_name::CrateName,
    docs_rs::DocsRs,
    format_version,
};
use semver::Version;
use sha2::{Digest as _, Sha256};

#[tokio::main]
async fn main() -> ExitCode {
    let settings = chatora_rustdoc_mcp::config::Settings::load();
    let _flushing = match chatora_rustdoc_mcp::logging::init(&settings) {
        Ok(flushing) => flushing,
        Err(error) => {
            eprintln!("failed to set up logging: {error}");
            return ExitCode::FAILURE;
        },
    };
    let opened = Cache::open(&settings);
    settings.report();
    let network = data::Network::from_settings(&settings);
    let (cache, docs_rs) = match (opened, DocsRs::new(&network)) {
        (Ok(cache), Ok(docs_rs)) => (cache, docs_rs),
        (Err(error), _) => {
            eprintln!("failed to open the cache: {error}");
            return ExitCode::FAILURE;
        },
        (_, Err(error)) => {
            eprintln!("failed to set up the HTTP client: {error}");
            return ExitCode::FAILURE;
        },
    };
    let mut status = ExitCode::SUCCESS;
    for arg in std::env::args().skip(1) {
        match fetch(&cache, &docs_rs, &arg).await {
            Ok(line) => {
                if let Err(error) = writeln!(io::stdout().lock(), "{line}") {
                    eprintln!("failed to write to stdout: {error}");
                    return ExitCode::FAILURE;
                }
            },
            Err(error) => {
                eprintln!("{arg}: {error}");
                status = ExitCode::FAILURE;
            },
        }
    }
    status
}

/// Fetches one `crate@version` and describes it in a line.
async fn fetch(cache: &Cache, docs_rs: &DocsRs, arg: &str) -> Result<String, Box<dyn Error>> {
    let (name, version) = arg.split_once('@').ok_or("expected crate@version")?;
    let name: CrateName = name.parse()?;
    let version = Version::parse(version)?;
    let dir = cache.docs_rs_dir(&name, &version, None);
    let cached = matches!(cache.read_docs_rs_json(&dir)?, CachedJson::Found(_));
    let source = if cached { "cache" } else { "docs.rs" };
    let json = data::docs_rs_json(cache, docs_rs, &name, &version, None)
        .await?
        .json;
    let format_version = format_version::read(&json)?;
    let path = dir.join(format!("{format_version}.json"));
    let mut sha256 = String::new();
    for byte in Sha256::digest(&json) {
        write!(sha256, "{byte:02x}")?;
    }
    Ok(format!(
        "{name}@{version}\tformat={}\tsize={}\tsha256={sha256}\tsource={source}\t{}",
        format_version,
        json.len(),
        path.display()
    ))
}
