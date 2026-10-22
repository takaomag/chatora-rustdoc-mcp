//! The temporary projects of `build_external_docs` (requirement R-WS-5, decided in M2e's E2a).
//!
//! A temporary project depends on one release of crates.io and on nothing else, so that cargo
//! documents the release in a workspace of its own: not in the user's project, whose Cargo.lock
//! does not have it. The server writes its Cargo.toml and makes its Cargo.lock once; the builds
//! then run as those of the default project do, with `--locked`.

use std::{
    collections::BTreeSet,
    fs,
    io,
    path::{Path, PathBuf},
};

use semver::Version;

use super::{
    build::{self, Bootstrap, BuildError, Plan, TargetDir},
    cargo,
    crate_name::CrateName,
    external::EXT_DIR,
    flight::Doing,
    metadata::Metadata,
    packages::Source,
    target::TargetTriple,
};

/// The name of the package of a temporary project.
const PACKAGE: &str = "chatora-rustdoc-temporary";

/// The name of the package of a temporary project for a release of [`PACKAGE`] itself.
const OTHER_PACKAGE: &str = "chatora-rustdoc-temporary-project";

/// A release of crates.io to document in a temporary project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The name of its package, as the crates.io index spells it: cargo takes no other.
    pub name: CrateName,
    /// Its exact version.
    pub version: Version,
    /// The features to build it with, as docs.rs does, by the names that its
    /// `[package.metadata.docs.rs]` asks for ([`super::crate_file::DocsRsBuild::features`]):
    /// `default` among them unless it says otherwise (requirement R-FT-1).
    pub features: Vec<Box<str>>,
    /// The names of the `--cfg` of its `rustdoc-args`, which its rustdoc gets with `--cfg docsrs`
    /// (decided in M2's D3).
    pub cfgs: Box<[Box<str>]>,
}

/// The `Cargo.toml` of a temporary project that documents `release`: a lib of its own, which
/// depends on the release at its exact version with its features and no others, in a workspace of
/// its own.
///
/// The same release gives the same text, whatever the order of its features, so that the
/// Cargo.lock made for it is kept ([`write()`]). It is of edition 2021, whose resolver takes the
/// newest versions, as docs.rs does, rather than those that the rustc that makes the Cargo.lock
/// builds; and which an older cargo reads too.
#[must_use]
pub fn manifest(release: &Release) -> String {
    let package_name = if release.name.cache_key() == PACKAGE {
        OTHER_PACKAGE
    } else {
        PACKAGE
    };
    let mut package = toml::Table::new();
    package.insert("name".to_owned(), package_name.into());
    package.insert("version".to_owned(), "0.0.0".into());
    package.insert("edition".to_owned(), "2021".into());
    package.insert("publish".to_owned(), false.into());
    // `default` as `default-features`, which a crate without such a feature takes too.
    let features: BTreeSet<&str> = release.features.iter().map(|feature| &**feature).collect();
    let mut dependency = toml::Table::new();
    dependency.insert("version".to_owned(), format!("={}", release.version).into());
    dependency.insert(
        "default-features".to_owned(),
        features.contains("default").into(),
    );
    dependency.insert(
        "features".to_owned(),
        toml::Value::Array(
            features
                .into_iter()
                .filter(|&feature| feature != "default")
                .map(toml::Value::from)
                .collect(),
        ),
    );
    let mut dependencies = toml::Table::new();
    dependencies.insert(release.name.as_str().to_owned(), dependency.into());
    let mut lib = toml::Table::new();
    lib.insert("path".to_owned(), "lib.rs".into());
    let mut manifest = toml::Table::new();
    manifest.insert("package".to_owned(), package.into());
    manifest.insert("lib".to_owned(), lib.into());
    manifest.insert("dependencies".to_owned(), dependencies.into());
    // A workspace of its own, not one above it.
    manifest.insert("workspace".to_owned(), toml::Table::new().into());
    manifest.to_string()
}

/// Writes in `dir` the temporary project that documents `release`: its Cargo.toml and its lib.
///
/// A Cargo.toml that is already so is left, with the Cargo.lock made for it; another is written
/// again, without its Cargo.lock.
///
/// It writes files, so it is for a blocking thread.
///
/// # Errors
///
/// Returns an error when the files cannot be read or written.
pub fn write(dir: &Path, release: &Release) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let manifest = manifest(release);
    let path = dir.join("Cargo.toml");
    if fs::read_to_string(&path).ok().as_deref() != Some(manifest.as_str()) {
        match fs::remove_file(dir.join("Cargo.lock")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            Ok(()) | Err(_) => {},
        }
        fs::write(&path, manifest)?;
    }
    let lib = dir.join("lib.rs");
    if !lib.is_file() {
        fs::write(lib, "")?;
    }
    Ok(())
}

/// Makes the Cargo.lock of the temporary project in `dir`, unless it has one.
///
/// It is made once, with the versions that crates.io has then, so that the builds after it build
/// the same (decided in M2e's E2a). Offline, the index that cargo has is used. `on_doing` is told
/// when cargo waits for a lock or fetches.
///
/// # Errors
///
/// Returns why cargo failed, as `BuildError::Fetch`, or could not run.
pub async fn lock(
    dir: &Path,
    offline: bool,
    on_doing: impl FnMut(Doing) + Send,
) -> Result<(), BuildError> {
    let lock = dir.join("Cargo.lock");
    if super::blocking(move || Ok(lock.is_file()))
        .await
        .unwrap_or(false)
    {
        return Ok(());
    }
    let mut command = cargo::unlocked_command(dir, offline);
    command.arg("generate-lockfile");
    build::run_fetching(command, offline, on_doing).await
}

/// The target directory of every temporary project in the cache at `cache` (decided in M2e, 2):
/// what one builds, another that depends on the same releases does not build again.
#[must_use]
pub fn target_dir(cache: &Path) -> TargetDir {
    TargetDir::of(cache, &cache.join(EXT_DIR))
}

/// Why a temporary project has no plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// The release is not among its packages as one of crates.io: the cargo settings may patch
    /// crates.io (`[patch.crates-io]`) with another source.
    #[error("cargo did not resolve the release from crates.io in the temporary project")]
    NotFound,
    /// The release has no lib to document.
    #[error("the release has no lib to document")]
    NoLib,
}

/// The plan of the build of `release` in the temporary project at `dir`, whose packages `metadata`
/// resolves.
///
/// It is for `target` if given, in the cache at `cache`, as a dependency's build, with its public
/// items only. `RUSTC_BOOTSTRAP` is `1` for every temporary project, which share a target
/// directory: a value that changed from one to the next would build again what reads it (decided
/// in M2e's E2a).
///
/// # Errors
///
/// Returns why there is none.
pub fn plan(
    (dir, metadata): (&Path, &Metadata),
    release: &Release,
    target: Option<TargetTriple>,
    (cache, offline): (&Path, bool),
) -> Result<Plan, PlanError> {
    let found = metadata
        .packages
        .iter()
        .find(|package| {
            package
                .name
                .parse::<CrateName>()
                .is_ok_and(|name| name.cache_key() == release.name.cache_key())
                && package.version == release.version
                && Source::parse(package.source.as_deref()) == Source::CratesIo
                && !metadata.workspace_members.contains(&package.id)
        })
        .ok_or(PlanError::NotFound)?;
    let (crate_name, documented) = build::lib_of(found).ok_or(PlanError::NoLib)?;
    Ok(Plan {
        root: dir.to_path_buf(),
        offline,
        toolchain: None,
        bootstrap: Bootstrap::All,
        target_dir: target_dir(cache),
        package: found.id.clone(),
        manifest: dir.join("Cargo.toml"),
        crate_name,
        documented,
        version: found.version.clone(),
        target,
        cfg_docsrs: build::cfg_docsrs(),
        cfgs: release.cfgs.clone(),
        features: None,
        document_private: false,
    })
}

/// The directory of the temporary project in the directory of a record, `record_dir`.
#[must_use]
pub fn dir_in(record_dir: &Path) -> PathBuf {
    record_dir.join("project")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(name: &str, features: &[&str]) -> Release {
        Release {
            name: name.parse().expect("a crate name"),
            version: Version::new(1, 2, 3),
            features: features.iter().map(|&feature| feature.into()).collect(),
            cfgs: Box::default(),
        }
    }

    #[test]
    fn the_manifest_depends_on_the_release_alone() {
        let manifest = manifest(&release("tokio", &["full", "default", "full"]));
        let table: toml::Table = toml::from_str(&manifest).expect("TOML");
        assert_eq!(table["package"]["name"].as_str(), Some(PACKAGE));
        assert_eq!(table["lib"]["path"].as_str(), Some("lib.rs"));
        assert!(
            table["workspace"]
                .as_table()
                .is_some_and(toml::Table::is_empty)
        );
        let dependencies = table["dependencies"].as_table().expect("a table");
        assert_eq!(dependencies.len(), 1);
        let tokio = &dependencies["tokio"];
        assert_eq!(tokio["version"].as_str(), Some("=1.2.3"));
        assert_eq!(tokio["default-features"].as_bool(), Some(true));
        assert_eq!(
            tokio["features"].as_array().map(Vec::as_slice),
            Some([toml::Value::from("full")].as_slice()),
            "{manifest}"
        );
        // The same, whatever the order of the features.
        assert_eq!(
            super::manifest(&release("tokio", &["default", "full"])),
            manifest
        );
        let none = super::manifest(&release("tokio", &[]));
        let table: toml::Table = toml::from_str(&none).expect("TOML");
        assert_eq!(
            table["dependencies"]["tokio"]["default-features"].as_bool(),
            Some(false)
        );
        // Not a package of the name of the release, however it is spelled.
        for name in [PACKAGE, "chatora_rustdoc_temporary"] {
            let other = super::manifest(&release(name, &[]));
            let table: toml::Table = toml::from_str(&other).expect("TOML");
            assert_eq!(table["package"]["name"].as_str(), Some(OTHER_PACKAGE));
        }
    }

    #[test]
    fn its_cargo_lock_is_kept_while_its_manifest_is_the_same() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let project = dir_in(dir.path());
        write(&project, &release("tokio", &["default"])).expect("written");
        fs::write(project.join("Cargo.lock"), "locked").expect("written");
        write(&project, &release("tokio", &["default"])).expect("written");
        assert!(project.join("Cargo.lock").is_file());
        write(&project, &release("tokio", &["full"])).expect("written");
        assert!(!project.join("Cargo.lock").exists());
        assert!(project.join("lib.rs").is_file());
    }

    /// The metadata of a temporary project with the package `name` 1.2.3 from `source`, whose
    /// target is of `kinds`.
    fn metadata(name: &str, source: &str, kinds: &[&str]) -> Metadata {
        serde_json::from_value(serde_json::json!({
            "workspace_root": "/p",
            "workspace_members": ["path+file:///p#chatora-rustdoc-temporary@0.0.0"],
            "packages": [{
                "id": format!("{source}#{name}@1.2.3"),
                "name": name,
                "version": "1.2.3",
                "source": source,
                "manifest_path": "/r/Cargo.toml",
                "targets": [{ "name": name.replace('-', "_"), "kind": kinds }],
            }],
            "resolve": null,
            "version": 1,
        }))
        .expect("metadata")
    }

    #[test]
    fn the_plan_is_of_the_release_from_crates_io() {
        const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
        let cache = Path::new("/c");
        let plan_of = |metadata: &Metadata, name: &str| {
            plan(
                (Path::new("/p"), metadata),
                &release(name, &[]),
                None,
                (cache, false),
            )
        };
        let found = metadata("tokio-util", CRATES_IO, &["lib"]);
        let plan = plan_of(&found, "Tokio_Util").expect("a plan");
        assert_eq!(plan.bootstrap, Bootstrap::All);
        assert_eq!(&*plan.crate_name, "tokio_util");
        assert!(!plan.document_private);
        assert_eq!(plan.target_dir, target_dir(cache));
        assert_eq!(plan.manifest, Path::new("/p/Cargo.toml"));
        // Patched with another source.
        let patched = metadata("tokio-util", "git+https://example.invalid/t", &["lib"]);
        assert_eq!(
            plan_of(&patched, "tokio-util").expect_err("not found"),
            PlanError::NotFound
        );
        let bin = metadata("tokio-util", CRATES_IO, &["bin"]);
        assert_eq!(
            plan_of(&bin, "tokio-util").expect_err("no lib"),
            PlanError::NoLib
        );
    }

    #[test]
    fn every_temporary_project_shares_one_target_directory() {
        let cache = Path::new("/c");
        assert_eq!(
            target_dir(cache),
            TargetDir::of(cache, &cache.join(EXT_DIR))
        );
        assert!(target_dir(cache).path().starts_with("/c/t"));
    }
}
