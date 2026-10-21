//! The `.crate` files of crates.io releases from static.crates.io, for what their `Cargo.toml`
//! says: the features and `--cfg` that docs.rs built the documentation with (requirements R-FT-1
//! and R-SC-2).

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use flate2::read::GzDecoder;
use reqwest::StatusCode;
use semver::Version;

use super::{
    cache::{Cache, Verified},
    crate_name::CrateName,
    http::{self, BodyError, Network},
};

/// static.crates.io itself.
pub const DEFAULT_BASE_URL: &str = "https://static.crates.io";

/// Largest `.crate` held in memory (requirement R-NW-3, initial value). crates.io takes crates
/// of up to 10 MB unless it allows a crate more.
const MAX_CRATE_BYTES: u64 = 128 * 1024 * 1024;

/// Most bytes decompressed while looking for `Cargo.toml`, against a `.crate` that inflates
/// without end. Cargo writes `Cargo.toml` among the first entries.
const MAX_SCANNED_BYTES: u64 = 256 * 1024 * 1024;

/// Largest `Cargo.toml` read.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// A client for static.crates.io.
#[derive(Debug, Clone)]
pub struct StaticCrates {
    client: reqwest::Client,
    base_url: Arc<str>,
    /// Whether no request may be sent (`network.offline`, requirement R-CA-7).
    offline: bool,
    /// `$CARGO_HOME`, whose `registry/cache` holds the `.crate` files that cargo has fetched.
    cargo_home: Option<Arc<Path>>,
}

/// Why the `.crate` or its `Cargo.toml` could not be read.
#[derive(Debug, thiserror::Error)]
pub enum CrateError {
    /// static.crates.io has no such file.
    #[error("static.crates.io has no .crate for this version")]
    NotFound,
    /// static.crates.io is overloaded or failing; trying again later may work.
    #[error("static.crates.io answered {status}; try again later")]
    Transient {
        /// The HTTP status.
        status: StatusCode,
    },
    /// static.crates.io answered with a status this client does not expect.
    #[error("static.crates.io answered with an unexpected status {status}")]
    UnexpectedStatus {
        /// The HTTP status.
        status: StatusCode,
    },
    /// The request or the transfer of the body failed.
    #[error("the request to static.crates.io failed")]
    Network(#[source] reqwest::Error),
    /// Fetching the same `.crate` failed transiently a moment ago (requirement R-ERR-3).
    #[error("fetching this .crate failed a moment ago; it is tried again in {} seconds", retry_after.as_secs())]
    RecentFailure {
        /// How long until it is fetched again.
        retry_after: std::time::Duration,
    },
    /// The `.crate` is larger than the limit.
    #[error("the .crate is larger than {limit} bytes")]
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// The `.crate` does not have the checksum of the crates.io index (requirement R-SC-2).
    #[error("the .crate does not match the checksum in the crates.io index")]
    ChecksumMismatch,
    /// The `.crate` is not a gzipped tar with a `Cargo.toml` that can be read.
    #[error("the .crate could not be read")]
    Corrupt(#[source] io::Error),
    /// The `Cargo.toml` is not TOML.
    #[error("the Cargo.toml in the .crate is not valid TOML")]
    Manifest(#[source] toml::de::Error),
    /// The cache could not be read or written.
    #[error("the cache could not be used")]
    Cache(#[source] io::Error),
    /// No request may be sent (`network.offline`), and the cache does not have it (requirement
    /// R-CA-7).
    #[error("the server is offline (network.offline), and the cache does not have it")]
    Offline,
}

impl CrateError {
    /// Whether trying again later may help.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Transient { .. } | Self::Network(_) | Self::ChecksumMismatch
        )
    }
}

impl StaticCrates {
    /// A client for static.crates.io itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn new(network: &Network) -> Result<Self, reqwest::Error> {
        Self::with_base_url(DEFAULT_BASE_URL, network)
    }

    /// A client for a server at `base_url` that serves the URLs of static.crates.io, such as a
    /// mirror or a test server. `base_url` has no trailing `/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be set up (for example, TLS).
    pub fn with_base_url(base_url: &str, network: &Network) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: http::client_builder(network).build()?,
            base_url: base_url.into(),
            offline: network.offline,
            cargo_home: None,
        })
    }

    /// The client that looks for a `.crate` among those that cargo has fetched into
    /// `cargo_home` before fetching it (doc/m2-plan.md, B6).
    #[must_use]
    pub fn with_cargo_home(mut self, cargo_home: Option<PathBuf>) -> Self {
        self.cargo_home = cargo_home.map(Into::into);
        self
    }

    /// The `.crate` of `name` at `version` that cargo has fetched, if one with `checksum` is in
    /// `$CARGO_HOME/registry/cache/<registry>/`. The registries are those of crates.io: its
    /// sparse index (`index.crates.io-…`) and its git index (`github.com-…`), whose directory names
    /// end in a hash that cargo versions may change.
    fn fetched_by_cargo(
        cargo_home: Option<&Path>,
        name: &CrateName,
        version: &Version,
        checksum: &str,
    ) -> Option<Box<[u8]>> {
        let registries = fs::read_dir(cargo_home?.join("registry/cache")).ok()?;
        let file = format!("{name}-{version}.crate");
        registries
            .filter_map(Result::ok)
            .filter(|registry| {
                let name = registry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("index.crates.io-") || name.starts_with("github.com-")
            })
            .find_map(|registry| {
                let path = registry.path().join(&file);
                let size = fs::metadata(&path)
                    .ok()
                    .filter(fs::Metadata::is_file)?
                    .len();
                if size > MAX_CRATE_BYTES {
                    return None;
                }
                let contents = fs::read(&path).ok()?;
                super::cache::sha256_hex(&contents)
                    .eq_ignore_ascii_case(checksum)
                    .then(|| contents.into_boxed_slice())
            })
    }

    /// The `.crate` of `name` at `version`, from the cache, or else fetched, checked against
    /// `checksum` (the sha256 in the crates.io index, in hex) and cached. Without `checksum`, only
    /// the cache is read, and `None` means it is not there.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be fetched, does not match `checksum`, or the cache
    /// cannot be used.
    pub async fn crate_file(
        &self,
        cache: &Cache,
        name: &CrateName,
        version: &Version,
        checksum: Option<&str>,
    ) -> Result<Option<Box<[u8]>>, CrateError> {
        let path = cache.crate_file_path(name, version);
        // Another call that wants the same `.crate` goes first (requirement R-NW-2).
        let held = cache.lock(&path).await;
        // Megabytes, read and digested off the async threads, as below.
        let (reading, read) = (cache.clone(), path.clone());
        match super::blocking(move || reading.read_verified(&read))
            .await
            .map_err(CrateError::Cache)?
        {
            Verified::Found(contents) => return Ok(Some(contents)),
            Verified::Mismatch | Verified::None => {},
        }
        let Some(checksum) = checksum else {
            return Ok(None);
        };
        let (home, wanted, sum) = (
            self.cargo_home.clone(),
            (name.clone(), version.clone()),
            Box::<str>::from(checksum),
        );
        let fetched = super::blocking(move || {
            Ok(Self::fetched_by_cargo(
                home.as_deref(),
                &wanted.0,
                &wanted.1,
                &sum,
            ))
        })
        .await
        .ok()
        .flatten();
        let contents = match fetched {
            Some(contents) => contents,
            None => self.fetch_remembering(cache, &path, name, version).await?,
        };
        let (writing, sum) = (cache.clone(), Box::<str>::from(checksum));
        // With the lock, which goes when the writing is done, even when this call is dropped.
        super::blocking(move || {
            let checked = if super::cache::sha256_hex(&contents).eq_ignore_ascii_case(&sum) {
                if let Err(error) = writing.write_verified(&path, &contents) {
                    tracing::warn!(path = %path.display(), %error, "a .crate could not be cached");
                }
                Ok(Some(contents))
            } else {
                Err(CrateError::ChecksumMismatch)
            };
            drop(held);
            Ok(checked)
        })
        .await
        .map_err(CrateError::Cache)?
    }

    /// [`Self::fetch`], unless it failed transiently a moment ago, which is remembered next to
    /// `path` in `cache`: every call, and every process that shares the cache, would wait for the
    /// same failure in turn (requirement R-ERR-3).
    async fn fetch_remembering(
        &self,
        cache: &Cache,
        path: &Path,
        name: &CrateName,
        version: &Version,
    ) -> Result<Box<[u8]>, CrateError> {
        if let Some(retry_after) = super::recent_failure(cache, path).await {
            return Err(CrateError::RecentFailure { retry_after });
        }
        let fetched = self.fetch(name, version).await;
        if let Err(CrateError::Network(_) | CrateError::Transient { .. }) = &fetched {
            super::remember_failure(path).await;
        }
        fetched
    }

    async fn fetch(&self, name: &CrateName, version: &Version) -> Result<Box<[u8]>, CrateError> {
        // The parts are validated, so they cannot change the shape of the URL. static.crates.io
        // takes the `+` of build metadata as itself.
        if self.offline {
            return Err(CrateError::Offline);
        }
        let url = format!("{}/crates/{name}/{name}-{version}.crate", self.base_url);
        let mut response = http::send(self.client.get(&url))
            .await
            .map_err(CrateError::Network)?;
        match response.status() {
            StatusCode::OK => {},
            // A missing file is 403 from the storage behind static.crates.io.
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => return Err(CrateError::NotFound),
            status if http::is_transient(status) => return Err(CrateError::Transient { status }),
            status => return Err(CrateError::UnexpectedStatus { status }),
        }
        http::read_body(&mut response, MAX_CRATE_BYTES)
            .await
            .map(Vec::into_boxed_slice)
            .map_err(|error| match error {
                BodyError::Network(error) => CrateError::Network(error),
                BodyError::TooLarge { limit } => CrateError::TooLarge { limit },
            })
    }
}

/// How docs.rs built the documentation of a release, from `[package.metadata.docs.rs]` in its
/// `Cargo.toml` (requirement R-FT-1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocsRsBuild {
    /// Every feature was enabled (`all-features`).
    pub all_features: bool,
    /// The features enabled by name: those of `features`, `default` unless
    /// `no-default-features`, or with `all-features` all of `[features]`.
    pub features: Box<[Box<str>]>,
    /// The names of the `--cfg` that `rustdoc-args` and `rustc-args` pass (`tokio_unstable`), but
    /// `docsrs`, which docs.rs always sets, and `feature`.
    pub cfg: Box<[Box<str>]>,
    /// The name of the package as published, from `[package]`.
    pub package_name: Option<Box<str>>,
    /// The features of the crate that were not enabled, even through others: those of
    /// `[features]` and those that optional dependencies make. Items that need them are not in
    /// the documentation.
    pub missing: Box<[Box<str>]>,
    /// Every feature of the crate: those of `[features]` and those that optional dependencies
    /// make, sorted.
    pub declared: Box<[Box<str>]>,
    /// The features that `default` enables, through others too, sorted.
    pub default: Box<[Box<str>]>,
    /// Whether the manifest has `[package.metadata.docs.rs]`.
    pub asked: bool,
    /// Whether it asks for features: `features`, `all-features` or `no-default-features`
    /// (requirement R-FT-3).
    pub asks_features: bool,
    /// The names of the `--cfg` that `rustdoc-args` passes without a value (`--cfg
    /// tokio_unstable`, `--cfg=tokio_unstable`), but `docsrs`: what a local build passes to the
    /// documented crate's rustdoc (decided in M2's D3). One with a value is not passed.
    pub rustdoc_cfg: Box<[Box<str>]>,
}

/// How the workspace member whose `Cargo.toml` is at `manifest` asks docs.rs to build it, from
/// its `[package.metadata.docs.rs]` (requirement R-FT-3, decided in M2's D3).
///
/// # Errors
///
/// Returns an error when the manifest cannot be read or is not TOML.
pub fn member_docs_rs(manifest: &Path) -> io::Result<DocsRsBuild> {
    let mut text = String::new();
    fs::File::open(manifest)?
        .take(MAX_MANIFEST_BYTES)
        .read_to_string(&mut text)?;
    let table: toml::Table =
        toml::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(build_of(&table))
}

/// How docs.rs built `name` at `version`, from the `Cargo.toml` in its `.crate` `contents`.
///
/// # Errors
///
/// Returns an error when the `.crate` has no `Cargo.toml` that can be read.
pub fn docs_rs_build(
    contents: &[u8],
    name: &CrateName,
    version: &Version,
) -> Result<DocsRsBuild, CrateError> {
    let manifest = manifest(contents, &format!("{name}-{version}/Cargo.toml"))?;
    let table: toml::Table = toml::from_str(&manifest).map_err(CrateError::Manifest)?;
    Ok(build_of(&table))
}

/// The text of the entry at `wanted` in the gzipped tar `contents`.
fn manifest(contents: &[u8], wanted: &str) -> Result<String, CrateError> {
    let corrupt = CrateError::Corrupt;
    let decoder = GzDecoder::new(contents).take(MAX_SCANNED_BYTES);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().map_err(corrupt)? {
        let entry = entry.map_err(corrupt)?;
        let is_wanted = entry.path_bytes().as_ref().eq(wanted.as_bytes());
        if !is_wanted {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            break;
        }
        let mut text = String::new();
        entry
            .take(MAX_MANIFEST_BYTES)
            .read_to_string(&mut text)
            .map_err(corrupt)?;
        return Ok(text);
    }
    Err(CrateError::Corrupt(io::Error::new(
        io::ErrorKind::NotFound,
        format!("the .crate has no {wanted}"),
    )))
}

/// The build that `[package.metadata.docs.rs]` in the manifest `table` asks for. What is not of
/// the expected type counts as absent, as names that are not valid are left out
/// (requirement R-SEC-4).
fn build_of(table: &toml::Table) -> DocsRsBuild {
    let metadata = table
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("docs"))
        .and_then(|docs| docs.get("rs"));
    let flag = |key: &str| {
        metadata
            .and_then(|metadata| metadata.get(key))
            .and_then(toml::Value::as_bool)
            .unwrap_or(false)
    };
    let strings = |key: &str| -> Vec<&str> {
        metadata
            .and_then(|metadata| metadata.get(key))
            .and_then(toml::Value::as_array)
            .map(|values| values.iter().filter_map(toml::Value::as_str).collect())
            .unwrap_or_default()
    };
    let features_table = table.get("features").and_then(toml::Value::as_table);
    let declared = declared_features(table);
    let all_features = flag("all-features");
    let features: BTreeSet<&str> = if all_features {
        declared.iter().map(String::as_str).collect()
    } else {
        let mut features: BTreeSet<&str> = strings("features")
            .into_iter()
            .flat_map(|list| list.split([',', ' ']))
            .filter(|feature| is_feature_name(feature))
            .collect();
        if !flag("no-default-features") && declared.contains("default") {
            features.insert("default");
        }
        features
    };
    let enabled = closure(&features, features_table);
    let default: Box<[Box<str>]> = if declared.contains("default") {
        closure(&BTreeSet::from(["default"]), features_table)
            .into_iter()
            .filter(|&feature| feature != "default")
            .map(Into::into)
            .collect()
    } else {
        Box::default()
    };
    let missing = declared
        .iter()
        .filter(|feature| !enabled.contains(feature.as_str()))
        .map(|feature| feature.as_str().into())
        .collect();
    let mut cfg = BTreeSet::new();
    let mut rustdoc_cfg = BTreeSet::new();
    for key in ["rustdoc-args", "rustc-args"] {
        let args = strings(key);
        let mut args = args.iter();
        while let Some(&arg) = args.next() {
            let value = match arg.strip_prefix("--cfg") {
                Some("") => args.next().copied(),
                Some(rest) => rest.strip_prefix('='),
                None => None,
            };
            let Some(value) = value else {
                continue;
            };
            let name = value.split('=').next().unwrap_or_default().trim();
            if key == "rustdoc-args" && name == value && name != "docsrs" && is_cfg_name(name) {
                rustdoc_cfg.insert(name);
            }
            // `--cfg 'feature="x"'` enables a feature, which `features` tells better.
            if !matches!(name, "docsrs" | "feature") && is_cfg_name(name) {
                cfg.insert(name);
            }
        }
    }
    DocsRsBuild {
        package_name: table
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .map(Into::into),
        all_features,
        features: features.into_iter().map(Into::into).collect(),
        cfg: cfg.into_iter().map(Into::into).collect(),
        missing,
        declared: declared
            .iter()
            .map(|feature| feature.as_str().into())
            .collect(),
        default,
        asked: metadata.is_some(),
        asks_features: ["features", "all-features", "no-default-features"]
            .into_iter()
            .any(|key| metadata.is_some_and(|metadata| metadata.get(key).is_some())),
        rustdoc_cfg: rustdoc_cfg.into_iter().map(Into::into).collect(),
    }
}

/// The features of the manifest `table`: those of `[features]`, and one for each optional
/// dependency that no feature names as `dep:<name>`, as Cargo makes them.
fn declared_features(table: &toml::Table) -> BTreeSet<String> {
    let features_table = table.get("features").and_then(toml::Value::as_table);
    let mut declared: BTreeSet<String> = features_table
        .map(|features| features.keys().cloned().collect())
        .unwrap_or_default();
    let named_with_dep: BTreeSet<&str> = features_table
        .into_iter()
        .flat_map(|features| features.values())
        .filter_map(toml::Value::as_array)
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter_map(|entry| entry.strip_prefix("dep:"))
        .collect();
    let targets = table
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|targets| targets.values());
    let tables = std::iter::once(table).chain(targets.filter_map(toml::Value::as_table));
    for owner in tables {
        for kind in ["dependencies", "build-dependencies"] {
            let Some(dependencies) = owner.get(kind).and_then(toml::Value::as_table) else {
                continue;
            };
            for (name, dependency) in dependencies {
                let optional = dependency
                    .get("optional")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                if optional && !named_with_dep.contains(name.as_str()) {
                    declared.insert(name.clone());
                }
            }
        }
    }
    declared
}

/// The features that `features` enable, with those they enable in turn through `[features]`
/// (`features_table`). `dep/feature` enables the feature of the optional dependency `dep`;
/// `dep?/feature` does not enable `dep`.
fn closure<'a>(
    features: &BTreeSet<&'a str>,
    features_table: Option<&'a toml::Table>,
) -> BTreeSet<&'a str> {
    let mut enabled = features.clone();
    let mut pending: Vec<&str> = features.iter().copied().collect();
    while let Some(feature) = pending.pop() {
        let entries = features_table
            .and_then(|table| table.get(feature))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str);
        for entry in entries {
            let next = match entry.split_once('/') {
                Some((dependency, _)) if !dependency.ends_with('?') => dependency,
                Some(_) => continue,
                None => entry,
            };
            if !next.starts_with("dep:") && enabled.insert(next) {
                pending.push(next);
            }
        }
    }
    enabled
}

/// Whether `name` is a feature as Cargo takes it, or a feature of a dependency (`dep/feature`).
pub(super) fn is_feature_name(name: &str) -> bool {
    let part = |part: &str| {
        part.chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
            && part
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
    };
    match name.split_once('/') {
        Some((dependency, feature)) => part(dependency) && part(feature),
        None => part(name),
    }
}

/// Whether `name` is the name of a `cfg`: an identifier.
fn is_cfg_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
pub(crate) mod tests {
    use flate2::{Compression, write::GzEncoder};

    use super::*;

    /// A transient failure to fetch a `.crate` is remembered for a while in the cache, so that
    /// the next call, in this process or another, does not ask again (requirement R-ERR-3).
    #[tokio::test]
    async fn a_transient_failure_is_not_asked_again_for_a_while() {
        let unavailable = || {
            (
                "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n".to_owned(),
                Vec::new(),
            )
        };
        // Each fetch is tried 3 times.
        let (base_url, requests) = super::super::test_http::serve_sequence(vec![
            unavailable(),
            unavailable(),
            unavailable(),
        ]);
        let dir = tempfile::tempdir().expect("a temporary directory");
        let cache = Cache::at(dir.path());
        let crates = StaticCrates::with_base_url(&base_url, &Network::default())
            .expect("a client")
            .with_cargo_home(None);
        let (name, version) = (
            "demo".parse::<CrateName>().expect("a name"),
            Version::new(0, 1, 0),
        );
        assert!(matches!(
            crates.crate_file(&cache, &name, &version, Some("00")).await,
            Err(CrateError::Transient { .. })
        ));
        assert!(matches!(
            crates.crate_file(&cache, &name, &version, Some("00")).await,
            Err(CrateError::RecentFailure { .. })
        ));
        assert_eq!(requests.try_iter().count(), 3, "not asked again");
    }

    /// A `.crate` of `name`-`version` with `manifest` as its `Cargo.toml`.
    pub(crate) fn crate_file(name: &str, version: &str, manifest: &str) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, contents) in [
            (format!("{name}-{version}/src/lib.rs"), ""),
            (format!("{name}-{version}/Cargo.toml"), manifest),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len().try_into().expect("small"));
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_bytes())
                .expect("appended");
        }
        builder
            .into_inner()
            .and_then(GzEncoder::finish)
            .expect("finished")
    }

    /// A `.crate` that cargo has fetched is read from its cache and checked, and static.crates.io
    /// is not asked; one of another checksum is not taken (doc/m2-plan.md, B6).
    #[tokio::test]
    async fn a_crate_that_cargo_has_fetched_is_read_from_its_cache() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let home = dir.path().join("cargo-home");
        let registry = home.join("registry/cache/index.crates.io-1949cf8c6b5b557f");
        fs::create_dir_all(&registry).expect("made");
        let contents = crate_file(
            "demo",
            "0.1.0",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        );
        fs::write(registry.join("demo-0.1.0.crate"), &contents).expect("written");
        let checksum = super::super::cache::sha256_hex(&contents);
        // Offline, a request to static.crates.io would be an error.
        let network = Network {
            offline: true,
            ..Network::default()
        };
        let crates = StaticCrates::with_base_url("http://127.0.0.1:9", &network)
            .expect("a client")
            .with_cargo_home(Some(home));
        let (name, version) = (
            "demo".parse::<CrateName>().expect("a name"),
            Version::new(0, 1, 0),
        );
        let cache = Cache::at(&dir.path().join("cache"));
        let read = crates
            .crate_file(&cache, &name, &version, Some(&checksum))
            .await
            .expect("from cargo's cache");
        assert_eq!(read.as_deref(), Some(&*contents));
        let other = Cache::at(&dir.path().join("other"));
        assert!(matches!(
            crates.crate_file(&other, &name, &version, Some("00")).await,
            Err(CrateError::Offline)
        ));
    }

    #[test]
    fn the_cfg_of_rustdoc_args_without_a_value_is_passed_to_a_local_build() {
        let docs = build(
            "[package]\nname = \"m\"\n\n[package.metadata.docs.rs]\nrustdoc-args = [\"--cfg\", \
             \"tokio_unstable\", \"--cfg=loom\", \"--cfg\", \"docsrs\", \"--cfg\", \
             'kind=\"x\"', \"--cfg\", \"not a name\"]\nrustc-args = [\"--cfg\", \"from_rustc\"]\n",
        );
        assert!(docs.asked && !docs.asks_features);
        assert!(
            build("[package]\nname = \"m\"\n\n[package.metadata.docs.rs]\nall-features = true\n")
                .asks_features
        );
        assert_eq!(&*docs.rustdoc_cfg, [
            Box::from("loom"),
            "tokio_unstable".into()
        ]);
        assert!(!build("[package]\nname = \"m\"\n").asked);
    }

    fn build(manifest: &str) -> DocsRsBuild {
        let name: CrateName = "demo".parse().expect("a name");
        let version = Version::new(0, 1, 0);
        docs_rs_build(&crate_file("demo", "0.1.0", manifest), &name, &version).expect("read")
    }

    #[test]
    fn all_features_lists_those_of_the_manifest() {
        let build = build(
            r#"[package]
name = "demo"
[features]
default = ["rt"]
rt = []
net = []
[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs", "--cfg", "tokio_unstable"]
rustc-args = ["--cfg=tokio_taskdump", "--cfg", "feature=\"x\""]
"#,
        );
        assert!(build.all_features);
        assert!(build.missing.is_empty(), "{build:?}");
        assert_eq!(build.package_name.as_deref(), Some("demo"));
        assert_eq!(&*build.features, [
            Box::from("default"),
            "net".into(),
            "rt".into()
        ]);
        assert_eq!(&*build.cfg, [
            Box::from("tokio_taskdump"),
            "tokio_unstable".into()
        ]);
    }

    #[test]
    fn named_features_come_with_the_default_ones_unless_turned_off() {
        let manifest = |extra: &str| {
            format!(
                "[package]\nname = \"demo\"\n[features]\ndefault = []\nfull = []\n[package.metadata.docs.rs]\nfeatures = [\"full\", \"serde/derive\", \"../x\"]\n{extra}"
            )
        };
        assert_eq!(&*build(&manifest("")).features, [
            Box::from("default"),
            "full".into(),
            "serde/derive".into()
        ]);
        assert_eq!(&*build(&manifest("no-default-features = true")).features, [
            Box::from("full"),
            "serde/derive".into()
        ]);
        // Without the table, docs.rs builds with the default features.
        assert_eq!(
            &*build("[package]\nname = \"demo\"\n[features]\ndefault = []\n").features,
            [Box::from("default")]
        );
    }

    #[test]
    fn features_left_out_are_found_through_what_enables_them() {
        let build = build(
            r#"[package]
name = "demo"
[features]
default = ["rt"]
full = ["rt", "net", "serde?/derive"]
rt = ["dep:mio"]
net = ["bytes/std"]
macros = []
[dependencies]
mio = { version = "1", optional = true }
bytes = { version = "1", optional = true }
serde = { version = "1", optional = true }
[target.'cfg(unix)'.dependencies]
libc = { version = "0.2", optional = true }
[package.metadata.docs.rs]
features = ["full"]
"#,
        );
        assert_eq!(&*build.default, [Box::from("rt")]);
        // `full` enables `rt` and `net`, and `net` the optional `bytes`; `serde?/derive` does not
        // enable `serde`, and `mio` has no feature of its own as `dep:mio` names it.
        assert_eq!(&*build.missing, [
            Box::from("libc"),
            "macros".into(),
            "serde".into()
        ]);
    }

    #[test]
    fn a_crate_without_its_manifest_is_corrupt() {
        let name: CrateName = "demo".parse().expect("a name");
        let other = crate_file("other", "0.1.0", "");
        assert!(matches!(
            docs_rs_build(&other, &name, &Version::new(0, 1, 0)),
            Err(CrateError::Corrupt(_))
        ));
        assert!(matches!(
            docs_rs_build(b"not gzip", &name, &Version::new(0, 1, 0)),
            Err(CrateError::Corrupt(_))
        ));
    }

    /// The environment variables that tell [`fetch_a_crate_file_for_another_test`] what to fetch
    /// and where to cache it.
    const ROOT: &str = "CHATORA_RUSTDOC_TEST_CACHE_ROOT";
    const BASE_URL: &str = "CHATORA_RUSTDOC_TEST_BASE_URL";
    const CHECKSUM: &str = "CHATORA_RUSTDOC_TEST_CHECKSUM";

    /// Not a test by itself: a process that [`two_processes_fetch_a_crate_file_once`] starts,
    /// which fetches `demo` 0.1.0 into the cache it is given.
    #[tokio::test]
    #[ignore = "started by two_processes_fetch_a_crate_file_once"]
    async fn fetch_a_crate_file_for_another_test() {
        let (Some(root), Some(base_url), Some(checksum)) = (
            std::env::var_os(ROOT),
            std::env::var(BASE_URL).ok(),
            std::env::var(CHECKSUM).ok(),
        ) else {
            return;
        };
        let crates = StaticCrates::with_base_url(&base_url, &Network::default())
            .expect("a client")
            .with_cargo_home(None);
        let fetched = crates
            .crate_file(
                &Cache::at(Path::new(&root)),
                &"demo".parse().expect("a name"),
                &Version::new(0, 1, 0),
                Some(&checksum),
            )
            .await
            .expect("fetched");
        assert!(fetched.is_some());
    }

    /// Two processes that want the same `.crate` at once take turns, and the second finds what
    /// the first fetched: static.crates.io (a local server that answers slowly) is asked once
    /// (requirement R-CA-4).
    #[test]
    fn two_processes_fetch_a_crate_file_once() {
        use std::{
            io::{BufRead as _, BufReader, Write as _},
            net::TcpListener,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            thread,
            time::Duration,
        };

        let contents = crate_file("demo", "0.1.0", "[package]\nname = \"demo\"\n");
        let checksum = super::super::cache::sha256_hex(&contents);
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port is free");
        let base_url = format!("http://{}", listener.local_addr().expect("bound"));
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                counted.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 0) && line != "\r\n" {
                    line.clear();
                }
                // Slowly, so that the other process comes while this one is fetching.
                thread::sleep(Duration::from_millis(500));
                let mut stream = reader.into_inner();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    contents.len()
                );
                // The client's view is what the test looks at.
                let _written = stream
                    .write_all(head.as_bytes())
                    .and_then(|()| stream.write_all(&contents));
            }
        });
        let dir = tempfile::tempdir().expect("a temporary directory");
        let env = [
            (ROOT, dir.path().as_os_str()),
            (BASE_URL, std::ffi::OsStr::new(&base_url)),
            (CHECKSUM, std::ffi::OsStr::new(&checksum)),
        ];
        let name = "data::crate_file::tests::fetch_a_crate_file_for_another_test";
        let both = [
            crate::test_process::start(name, &env),
            crate::test_process::start(name, &env),
        ];
        for child in both {
            crate::test_process::finish(child);
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }
}
