//! Measures how long loading a cached rustdoc JSON takes, step by step (`doc/m1-plan.md`, B9).
//!
//! ```sh
//! cargo run --release --example measure_load -- <path of a JSON> ...
//! ```
//!
//! Each file is loaded five times, on a thread with the stack the server gives its blocking
//! threads. For each step, it prints the median: reading the file, checking its SHA-256 (as the
//! cache does, requirement R-CA-4), parsing it into the latest types, building the public path
//! index, and freeing both. Then the crate and its index are archived once (requirement R-PF-1),
//! and the archive is opened five times: mapped, its header compared and its bytes checked.

#![expect(
    unused_crate_dependencies,
    reason = "an example uses only some of the package's dependencies"
)]

use std::{
    env,
    fs,
    hint::black_box,
    io::{self, Write as _},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use chatora_rustdoc_mcp::{
    data::{
        archive::{self, Header, Mapped},
        cache::{self, Cache},
        rustdoc_json,
    },
    query::{
        across::{self, Indexed},
        path_index::PathIndex,
    },
    task,
};
use sha2::{Digest as _, Sha256};

/// How many times each file is loaded.
const RUNS: usize = 5;

/// The place of the median among [`RUNS`] sorted times.
const MEDIAN: usize = 2;

fn main() -> ExitCode {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: measure_load <path of a JSON> ...");
        return ExitCode::FAILURE;
    }
    let measured = thread::Builder::new()
        .stack_size(task::THREAD_STACK_SIZE)
        .spawn(move || paths.iter().map(|path| measure(path)).collect::<Vec<_>>());
    let Ok(Ok(lines)) = measured.map(thread::JoinHandle::join) else {
        eprintln!("the measuring thread failed");
        return ExitCode::FAILURE;
    };
    let mut status = ExitCode::SUCCESS;
    let mut stdout = io::stdout().lock();
    for line in lines {
        let written = match line {
            Ok(line) => writeln!(stdout, "{line}"),
            Err(error) => {
                status = ExitCode::FAILURE;
                writeln!(stdout, "{error}")
            },
        };
        if written.is_err() {
            return ExitCode::FAILURE;
        }
    }
    status
}

/// Loads the JSON at `path` [`RUNS`] times and describes the median of each step in a line.
fn measure(path: &str) -> Result<String, String> {
    let mut steps: [Vec<Duration>; 5] = Default::default();
    let mut description = String::new();
    for _ in 0..RUNS {
        let started = Instant::now();
        let json = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        let read = started.elapsed();

        let started = Instant::now();
        black_box(Sha256::digest(&json));
        let digest = started.elapsed();

        let started = Instant::now();
        let parsed = rustdoc_json::parse(&json).map_err(|error| format!("{path}: {error}"))?;
        let parse = started.elapsed();

        let started = Instant::now();
        let index = PathIndex::build(&parsed.krate);
        let build = started.elapsed();

        description = format!(
            "{path}: format {}, {} bytes, {} items",
            parsed.krate.format_version,
            json.len(),
            parsed.krate.index.len()
        );
        let started = Instant::now();
        drop((parsed, index, json));
        let free = started.elapsed();

        for (times, time) in steps.iter_mut().zip([read, digest, parse, build, free]) {
            times.push(time);
        }
    }
    let [read, digest, parse, build, free] = steps.map(median);
    let archive = measure_archive(path)?;
    let total = [digest, parse, build, free]
        .into_iter()
        .fold(read, Duration::saturating_add);
    Ok(format!(
        "{description}\n  read {read:?}, sha256 {digest:?}, parse {parse:?}, index {build:?}, free \
         {free:?}, total {total:?} (medians of {RUNS})\n  {archive}"
    ))
}

/// The median of `times`.
fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times.get(MEDIAN).copied().unwrap_or_default()
}

/// Archives the crate at `path` with its index, then opens the archive [`RUNS`] times.
fn measure_archive(path: &str) -> Result<String, String> {
    let error = |error: &dyn std::fmt::Display| format!("{path}: {error}");
    let json = fs::read(path).map_err(|e| error(&e))?;
    let parsed = rustdoc_json::parse(&json).map_err(|e| error(&e))?;
    let header = Header {
        schema: across::ARCHIVE_SCHEMA,
        format_version: parsed.krate.format_version,
        types_version: rustdoc_types::FORMAT_VERSION,
        source_sha256: cache::sha256_hex(&json).into(),
    };
    drop(json);
    let dir = tempfile::tempdir().map_err(|e| error(&e))?;
    let cache = Cache::at(dir.path());
    let archive_path = dir.path().join("crate.rkyv");
    let started = Instant::now();
    let indexed = Indexed::new(parsed);
    archive::write(&cache, &archive_path, &header, &indexed).map_err(|e| error(&e))?;
    let write = started.elapsed();
    drop(indexed);
    let size = fs::metadata(&archive_path).map_err(|e| error(&e))?.len();
    let mut opens = Vec::new();
    let mut items = 0;
    for _ in 0..RUNS {
        let started = Instant::now();
        let mapped = Mapped::<Indexed>::open(&cache, &archive_path, &header)
            .map_err(|e| error(&e))?
            .ok_or_else(|| format!("{path}: the archive does not check out"))?;
        items = mapped.get().parsed.krate.index.len();
        opens.push(started.elapsed());
    }
    Ok(format!(
        "archive: {size} bytes, written in {write:?} (with the index); opened and checked in {:?} \
         ({items} items)",
        median(opens)
    ))
}
