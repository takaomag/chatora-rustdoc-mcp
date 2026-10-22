//! A fake crates.io on a local port, with a sparse index and the downloads of `.crate` files
//! (`doc/m2-plan.md` §2). cargo uses it in place of crates.io when [`FakeRegistry::cargo_config`]
//! is in its cargo home, and records the packages it takes from it in Cargo.lock as crates.io's.

use std::{
    collections::HashMap,
    io::{BufRead as _, BufReader, Write as _},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use chatora_rustdoc_mcp::data::cache::sha256_hex;
use flate2::{Compression, write::GzEncoder};
use parking_lot::Mutex;
use serde_json::json;

/// The files that the server serves by path, and the requests it has received.
#[derive(Debug, Default)]
struct State {
    files: HashMap<String, Vec<u8>>,
    requests: Vec<String>,
}

/// A local HTTP server that serves a sparse index under `/index/` and `.crate` files under
/// `/dl/`. It stops when dropped.
#[derive(Debug)]
pub(crate) struct FakeRegistry {
    base_url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
}

impl FakeRegistry {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
        let base_url = format!("http://{}", listener.local_addr().expect("bound"));
        let state = Arc::new(Mutex::new(State::default()));
        state.lock().files.insert(
            "/index/config.json".to_owned(),
            json!({ "dl": format!("{base_url}/dl/{{crate}}/{{version}}/download") })
                .to_string()
                .into_bytes(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let (served, stopped) = (Arc::clone(&state), Arc::clone(&stop));
        thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { continue };
                let state = Arc::clone(&served);
                thread::spawn(move || serve(stream, &state));
            }
        });
        Self {
            base_url,
            state,
            stop,
        }
    }

    /// The URL of the registry, without a trailing `/`: the sparse index is under `/index`, and the
    /// `.crate` files are where static.crates.io has them too.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The `config.toml` of a cargo home that replaces crates.io with this registry.
    pub(crate) fn cargo_config(&self) -> String {
        format!(
            "[source.crates-io]\nreplace-with = \"fake\"\n\n[source.fake]\nregistry = \
             \"sparse+{}/index/\"\n",
            self.base_url
        )
    }

    /// Publishes `krate`: adds its line to the index and its `.crate` file to the downloads.
    pub(crate) fn publish(&self, krate: &FakeCrate) {
        let file = krate.crate_file();
        let line = json!({
            "name": krate.name,
            "vers": krate.version,
            "deps": krate.dependencies.iter().map(|(name, req)| json!({
                "name": name,
                "req": req,
                "features": [],
                "optional": false,
                "default_features": true,
                "target": null,
                "kind": "normal",
            })).collect::<Vec<_>>(),
            "cksum": sha256_hex(&file),
            "features": krate.features.iter().cloned().collect::<serde_json::Map<_, _>>(),
            "yanked": false,
            "v": 2,
        });
        let index = format!("/index/{}", index_path(&krate.name));
        let download = format!("/dl/{}/{}/download", krate.name, krate.version);
        // Where static.crates.io has it, for the server's client of it in the tests.
        let on_static = format!(
            "/crates/{name}/{name}-{version}.crate",
            name = krate.name,
            version = krate.version
        );
        let mut state = self.state.lock();
        state
            .files
            .entry(index)
            .or_default()
            .extend_from_slice(format!("{line}\n").as_bytes());
        state.files.insert(on_static, file.clone());
        state.files.insert(download, file);
        drop(state);
    }

    /// The requests received so far, as `GET /index/config.json`.
    pub(crate) fn requests(&self) -> Vec<String> {
        self.state.lock().requests.clone()
    }
}

impl Drop for FakeRegistry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the server up from `accept`, so that it sees `stop`.
        let address = self.base_url.trim_start_matches("http://");
        drop(TcpStream::connect(address));
    }
}

/// Answers one request on `stream` with the file at its path, or with 404, and closes it.
fn serve(stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => {},
        }
    }
    let mut words = request_line.split_whitespace();
    let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let body = {
        let mut state = state.lock();
        state.requests.push(format!("{method} {path}"));
        state.files.get(path).cloned()
    };
    let head = |status: &str, length: usize| {
        format!("HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
    };
    let mut stream = reader.into_inner();
    // A client that hangs up early is not the test's concern.
    let _written = match body {
        Some(body) => stream
            .write_all(head("200 OK", body.len()).as_bytes())
            .and_then(|()| stream.write_all(&body)),
        None => stream.write_all(head("404 Not Found", 0).as_bytes()),
    };
}

/// Where the sparse index keeps the lines of the crate `name`.
fn index_path(name: &str) -> String {
    let name = name.to_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", name.get(..1).expect("ASCII")),
        _ => format!(
            "{}/{}/{name}",
            name.get(..2).expect("ASCII"),
            name.get(2..4).expect("ASCII")
        ),
    }
}

/// A crate to publish on a [`FakeRegistry`]: a lib with normal dependencies on the registry.
#[derive(Debug)]
pub(crate) struct FakeCrate {
    name: String,
    version: String,
    /// The names and version requirements of its dependencies.
    dependencies: Vec<(String, String)>,
    /// Its features, by name, with those that each enables.
    features: Vec<(String, serde_json::Value)>,
    /// What its Cargo.toml has after its features.
    manifest_tail: String,
    /// The files besides Cargo.toml, by path in the package.
    files: Vec<(String, String)>,
}

impl FakeCrate {
    /// The crate `name` at `version` with an empty `src/lib.rs`.
    pub(crate) fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: version.to_owned(),
            dependencies: Vec::new(),
            features: Vec::new(),
            manifest_tail: String::new(),
            files: vec![("src/lib.rs".to_owned(), String::new())],
        }
    }

    /// Adds a dependency on the crate `name` of the registry.
    #[must_use]
    pub(crate) fn dependency(mut self, name: &str, req: &str) -> Self {
        self.dependencies.push((name.to_owned(), req.to_owned()));
        self
    }

    /// Adds the feature `name`, which enables the features `enables`.
    #[must_use]
    #[cfg_attr(
        not(feature = "local"),
        expect(
            dead_code,
            reason = "only the tests of local features build with features"
        )
    )]
    pub(crate) fn feature(mut self, name: &str, enables: &[&str]) -> Self {
        self.features.push((name.to_owned(), json!(enables)));
        self
    }

    /// Adds `text` to its Cargo.toml, after its features: any table, as `[package.metadata.docs.rs]`
    /// or `[lib]`.
    #[must_use]
    #[cfg_attr(
        not(feature = "local"),
        expect(dead_code, reason = "only the tests of local features build with it")
    )]
    pub(crate) fn manifest_tail(mut self, text: &str) -> Self {
        self.manifest_tail.push_str(text);
        self
    }

    /// Adds the file `path` to the package, or replaces it.
    #[must_use]
    pub(crate) fn file(mut self, path: &str, contents: &str) -> Self {
        self.files.retain(|(existing, _)| existing != path);
        self.files.push((path.to_owned(), contents.to_owned()));
        self
    }

    fn manifest(&self) -> String {
        let mut manifest = format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\nedition = \"2024\"\n\n[dependencies]\n",
            self.name, self.version
        );
        manifest.extend(
            self.dependencies
                .iter()
                .map(|(name, req)| format!("{name} = \"{req}\"\n")),
        );
        manifest.push_str("\n[features]\n");
        manifest.extend(
            self.features
                .iter()
                .map(|(name, enables)| format!("{name} = {enables}\n")),
        );
        manifest.push_str(&self.manifest_tail);
        manifest
    }

    /// The `.crate` file: a gzipped tar of the package under `<name>-<version>/`.
    fn crate_file(&self) -> Vec<u8> {
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        let manifest = ("Cargo.toml".to_owned(), self.manifest());
        for (path, contents) in std::iter::once(&manifest).chain(&self.files) {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(
                &mut header,
                format!("{}-{}/{path}", self.name, self.version),
                contents.as_bytes(),
            )
            .expect("appended");
        }
        tar.into_inner()
            .expect("written")
            .finish()
            .expect("compressed")
    }
}
