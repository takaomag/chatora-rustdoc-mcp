//! The packages of a project, looked up by the names that code and Cargo.toml use.
//!
//! A name is an extern name (a dependency's name in the code, after a rename) or a package name
//! (requirement R-PR-1). Several versions of one package are not chosen between silently
//! (requirement R-VR-2), and docs.rs is used only for a package that is the same as on crates.io.

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

use semver::Version;

use super::{
    cargo_lock::CargoLock,
    metadata::{Metadata, PackageId, Resolution},
    sparse_index::IndexEntry,
};

/// The sources that are crates.io, as Cargo.lock and `cargo metadata` write them.
const CRATES_IO: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// Where a package comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A workspace member or a path dependency.
    Local,
    /// crates.io, even when the source is replaced by a vendor directory or a mirror.
    CratesIo,
    /// Another registry.
    Registry(Box<str>),
    /// A git repository.
    Git(Box<str>),
}

impl Source {
    /// The source of a package from its `source` in Cargo.lock or `cargo metadata`.
    #[must_use]
    pub fn parse(source: Option<&str>) -> Self {
        match source {
            None => Self::Local,
            Some(source) if CRATES_IO.contains(&source) => Self::CratesIo,
            Some(source) if source.starts_with("git+") => Self::Git(source.into()),
            Some(source) => Self::Registry(source.into()),
        }
    }
}

/// A package of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPackage {
    /// The package name.
    pub name: Box<str>,
    /// The version.
    pub version: Version,
    /// Where it comes from.
    pub source: Source,
    /// The name of its lib crate, as code writes it. The package name with `_` for `-` when
    /// only Cargo.lock is known.
    pub lib_name: Box<str>,
    /// The checksum of its `.crate` file in Cargo.lock, for a registry package.
    pub checksum: Option<Box<str>>,
    /// Its `Cargo.toml`, where its source is. Unknown when only Cargo.lock is known.
    pub manifest_path: Option<PathBuf>,
}

/// Whether docs.rs may be used for a package (requirement R-VR-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocsRsCheck {
    /// The package is the one on crates.io.
    Allowed,
    /// The package is not from crates.io, so docs.rs does not have it.
    NotCratesIo,
    /// Cargo.lock has no checksum for it.
    NoChecksum,
    /// The sparse index does not list its version.
    NotInIndex,
    /// The checksum in Cargo.lock is not the one in the sparse index: its contents differ from
    /// crates.io's.
    ChecksumMismatch,
}

impl ProjectPackage {
    /// Whether docs.rs may be used for this package, given its entry in the sparse index: it is
    /// from crates.io, and its checksum in Cargo.lock is the index's. A replaced source (a vendor
    /// directory) is then known to hold what crates.io has (requirement R-VR-2).
    #[must_use]
    pub fn docs_rs_check(&self, index: &IndexEntry) -> DocsRsCheck {
        if self.source != Source::CratesIo {
            return DocsRsCheck::NotCratesIo;
        }
        let Some(checksum) = &self.checksum else {
            return DocsRsCheck::NoChecksum;
        };
        match index
            .versions
            .iter()
            .find(|published| published.version == self.version)
        {
            None => DocsRsCheck::NotInIndex,
            Some(published) if published.checksum.eq_ignore_ascii_case(checksum) => {
                DocsRsCheck::Allowed
            },
            Some(_) => DocsRsCheck::ChecksumMismatch,
        }
    }
}

/// The packages of a project and how they depend on each other.
#[derive(Debug)]
pub struct PackageGraph {
    packages: Box<[ProjectPackage]>,
    /// The workspace members, by index into `packages`. When only Cargo.lock is known, every
    /// package without a source: the members and the path dependencies.
    members: Box<[usize]>,
    /// The dependencies of the members.
    direct: Box<[Direct]>,
    /// For each package, the packages that depend on it.
    dependents: Box<[Box<[usize]>]>,
}

/// A dependency of a member.
#[derive(Debug)]
struct Direct {
    /// The name the member uses for it in code. When only Cargo.lock is known, the lib name,
    /// as renames are not recorded there.
    extern_name: Box<str>,
    /// The package depended on.
    package: usize,
}

/// A package that a name may mean, and the packages that use it.
#[derive(Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    /// The package.
    pub package: &'a ProjectPackage,
    /// The packages that depend on it.
    pub used_by: Vec<&'a ProjectPackage>,
}

/// What a name means in a project.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// One package.
    Found {
        /// The package.
        package: &'a ProjectPackage,
        /// Other versions of the same package in the project, which a member does not depend
        /// on directly. They go into the warnings.
        other_versions: Vec<&'a ProjectPackage>,
    },
    /// Several versions of one package, and none or more than one that a member depends on
    /// directly (`ambiguous_version`).
    AmbiguousVersion(Vec<Candidate<'a>>),
    /// Different packages: the name is the extern name of one and the package name of another.
    AmbiguousPackage(Vec<Candidate<'a>>),
    /// No package of the project.
    NotFound,
}

impl PackageGraph {
    /// The packages of a resolution.
    #[must_use]
    pub fn new(resolution: &Resolution) -> Self {
        match resolution {
            Resolution::Metadata { metadata, lock } => Self::from_metadata(metadata, lock),
            Resolution::CargoLockRaw { lock, .. } => Self::from_lock(lock),
        }
    }

    /// The packages that `cargo metadata` reports, with the checksums from Cargo.lock.
    #[must_use]
    pub fn from_metadata(metadata: &Metadata, lock: &CargoLock) -> Self {
        let packages: Box<[ProjectPackage]> = metadata
            .packages
            .iter()
            .map(|package| {
                let lib_name = package
                    .targets
                    .iter()
                    .find(|target| target.kind.iter().any(|kind| is_lib_kind(kind)))
                    .map_or_else(
                        || package.name.replace('-', "_").into(),
                        |target| target.name.clone(),
                    );
                let checksum = lock
                    .packages
                    .iter()
                    .find(|locked| {
                        locked.name == package.name
                            && locked.version == package.version
                            && locked.source == package.source
                    })
                    .and_then(|locked| locked.checksum.clone());
                ProjectPackage {
                    name: package.name.clone(),
                    version: package.version.clone(),
                    source: Source::parse(package.source.as_deref()),
                    lib_name,
                    checksum,
                    manifest_path: Some(package.manifest_path.clone()),
                }
            })
            .collect();
        let indices: HashMap<&PackageId, usize> = metadata
            .packages
            .iter()
            .enumerate()
            .map(|(index, package)| (&package.id, index))
            .collect();
        let index_of = |id: &PackageId| indices.get(id).copied();
        let members: Box<[usize]> = metadata
            .workspace_members
            .iter()
            .filter_map(index_of)
            .collect();
        let mut direct = Vec::new();
        let mut dependents = vec![BTreeSet::new(); packages.len()];
        for node in metadata.resolve.iter().flat_map(|resolve| &resolve.nodes) {
            let Some(from) = index_of(&node.id) else {
                continue;
            };
            for dep in &node.deps {
                let Some(to) = index_of(&dep.pkg) else {
                    continue;
                };
                if let Some(users) = dependents.get_mut(to) {
                    users.insert(from);
                }
                if members.contains(&from) {
                    direct.push(Direct {
                        extern_name: dep.name.clone(),
                        package: to,
                    });
                }
            }
        }
        Self::with(packages, members, direct, dependents)
    }

    /// The packages that Cargo.lock lists, when `cargo metadata` failed. Renames and lib names
    /// are not known, and the members cannot be told from path dependencies.
    #[must_use]
    pub fn from_lock(lock: &CargoLock) -> Self {
        let packages: Box<[ProjectPackage]> = lock
            .packages
            .iter()
            .map(|locked| ProjectPackage {
                name: locked.name.clone(),
                version: locked.version.clone(),
                source: Source::parse(locked.source.as_deref()),
                lib_name: locked.name.replace('-', "_").into(),
                checksum: locked.checksum.clone(),
                manifest_path: None,
            })
            .collect();
        let members: Box<[usize]> = packages
            .iter()
            .enumerate()
            .filter(|(_, package)| package.source == Source::Local)
            .map(|(index, _)| index)
            .collect();
        let mut direct = Vec::new();
        let mut dependents = vec![BTreeSet::new(); packages.len()];
        for (from, locked) in lock.packages.iter().enumerate() {
            for dependency in &locked.dependencies {
                let Some(to) = find_locked(lock, dependency) else {
                    continue;
                };
                if let Some(users) = dependents.get_mut(to) {
                    users.insert(from);
                }
                if let Some(package) = packages.get(to)
                    && members.contains(&from)
                {
                    direct.push(Direct {
                        extern_name: package.lib_name.clone(),
                        package: to,
                    });
                }
            }
        }
        Self::with(packages, members, direct, dependents)
    }

    fn with(
        packages: Box<[ProjectPackage]>,
        members: Box<[usize]>,
        direct: Vec<Direct>,
        dependents: Vec<BTreeSet<usize>>,
    ) -> Self {
        Self {
            packages,
            members,
            direct: direct.into(),
            dependents: dependents
                .into_iter()
                .map(|users| users.into_iter().collect())
                .collect(),
        }
    }

    /// The packages that the members depend on directly, but the members themselves, each once,
    /// sorted by name and version. Whether each is built for a target, and in which builds, is for
    /// the caller to tell (`cargo tree`).
    #[must_use]
    pub fn direct_packages(&self) -> Vec<&ProjectPackage> {
        let mut indices: Vec<usize> = self
            .direct
            .iter()
            .map(|direct| direct.package)
            .filter(|package| !self.members.contains(package))
            .collect();
        indices.sort_unstable();
        indices.dedup();
        let mut packages: Vec<&ProjectPackage> = indices
            .into_iter()
            .filter_map(|index| self.packages.get(index))
            .collect();
        packages.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
        packages
    }

    /// The packages.
    #[must_use]
    pub fn packages(&self) -> &[ProjectPackage] {
        &self.packages
    }

    /// The package that `name` means: the extern name of a member's dependency, or the package
    /// or lib name of a package, with `-` and `_` and the case not told apart (requirement
    /// R-PR-1). Of several versions of one package, the one a member depends on directly is
    /// taken if there is just one (requirement R-VR-2).
    #[must_use]
    pub fn look_up(&self, name: &str) -> Lookup<'_> {
        let key = normalize(name);
        let by_extern = self
            .direct
            .iter()
            .filter(|direct| normalize(&direct.extern_name) == key)
            .map(|direct| direct.package);
        let by_name = self
            .packages
            .iter()
            .enumerate()
            .filter(|(_, package)| {
                normalize(&package.name) == key || normalize(&package.lib_name) == key
            })
            .map(|(index, _)| index);
        let found: BTreeSet<usize> = by_extern.chain(by_name).collect();
        let names: BTreeSet<String> = found
            .iter()
            .filter_map(|&index| self.packages.get(index))
            .map(|package| normalize(&package.name))
            .collect();
        if names.len() > 1 {
            return Lookup::AmbiguousPackage(self.candidates(&found));
        }
        let direct: Vec<usize> = found
            .iter()
            .copied()
            .filter(|index| {
                self.members.contains(index)
                    || self.direct.iter().any(|direct| direct.package == *index)
            })
            .collect();
        let chosen = match (found.len(), direct.as_slice()) {
            (0, _) => return Lookup::NotFound,
            (1, _) => found.first().copied(),
            (_, &[only]) => Some(only),
            (..) => None,
        };
        let Some(package) = chosen.and_then(|index| self.packages.get(index)) else {
            return Lookup::AmbiguousVersion(self.candidates(&found));
        };
        Lookup::Found {
            package,
            other_versions: found
                .iter()
                .filter(|&&index| Some(index) != chosen)
                .filter_map(|&index| self.packages.get(index))
                .collect(),
        }
    }

    /// `found` as candidates, oldest version first, with their dependents.
    fn candidates(&self, found: &BTreeSet<usize>) -> Vec<Candidate<'_>> {
        let mut candidates: Vec<Candidate<'_>> = found
            .iter()
            .filter_map(|&index| {
                let package = self.packages.get(index)?;
                let used_by = self
                    .dependents
                    .get(index)
                    .into_iter()
                    .flatten()
                    .filter_map(|&user| self.packages.get(user))
                    .collect();
                Some(Candidate { package, used_by })
            })
            .collect();
        candidates.sort_by(|a, b| {
            (&a.package.name, &a.package.version).cmp(&(&b.package.name, &b.package.version))
        });
        candidates
    }
}

/// Whether a target kind is one of a lib crate.
fn is_lib_kind(kind: &str) -> bool {
    matches!(
        kind,
        "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
    )
}

/// A crate name with the case and `-` and `_` made the same.
fn normalize(name: &str) -> String {
    name.to_lowercase().replace('-', "_")
}

/// The package of Cargo.lock that a `dependencies` entry names: `name`, `name version` or
/// `name version (source)`, which Cargo writes only as fully as it needs to tell them apart.
fn find_locked(lock: &CargoLock, dependency: &str) -> Option<usize> {
    let mut parts = dependency.splitn(3, ' ');
    let name = parts.next()?;
    let version = parts.next();
    let source = parts
        .next()
        .and_then(|source| source.strip_prefix('('))
        .and_then(|source| source.strip_suffix(')'));
    lock.packages.iter().position(|locked| {
        &*locked.name == name
            && version.is_none_or(|version| locked.version.to_string() == version)
            && source.is_none_or(|source| locked.source.as_deref() == Some(source))
    })
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use serde_json::json;

    use super::*;
    use crate::data::sparse_index::IndexVersion;

    const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

    /// A package of `cargo metadata` with a lib target named after it.
    fn package(name: &str, version: &str, source: Option<&str>) -> serde_json::Value {
        let id = format!("{}#{name}@{version}", source.unwrap_or("path+file:///w"));
        json!({
            "id": id,
            "name": name,
            "version": version,
            "source": source,
            "manifest_path": format!("/src/{name}-{version}/Cargo.toml"),
            "targets": [{ "name": name.replace('-', "_"), "kind": ["lib"] }],
        })
    }

    fn id(package: &serde_json::Value) -> serde_json::Value {
        package["id"].clone()
    }

    /// A member `app` that depends on serde 1, on `old` (which depends on serde 0.9), on `bar`
    /// as `foo`, and on `left` and `right`, which depend on rand 0.8 and rand 0.9. A package
    /// named `foo` comes through `left`.
    fn metadata() -> (Metadata, CargoLock) {
        let app = package("app", "0.1.0", None);
        let serde1 = package("serde", "1.0.229", Some(REGISTRY));
        let serde09 = package("serde", "0.9.15", Some(REGISTRY));
        let old = package("old", "1.0.0", Some(REGISTRY));
        let bar = package("bar", "2.0.0", Some(REGISTRY));
        let foo = package("foo", "3.0.0", Some(REGISTRY));
        let left = package("left", "1.0.0", Some(REGISTRY));
        let right = package("right", "1.0.0", Some(REGISTRY));
        let rand08 = package("rand", "0.8.5", Some(REGISTRY));
        let rand09 = package("rand", "0.9.2", Some(REGISTRY));
        let dep = |name: &str, package: &serde_json::Value| json!({ "name": name, "pkg": id(package), "dep_kinds": [{ "kind": null, "target": null }] });
        let node = |package: &serde_json::Value, deps: Vec<serde_json::Value>| json!({ "id": id(package), "deps": deps, "features": [] });
        let nodes = vec![
            node(&app, vec![
                dep("serde", &serde1),
                dep("old", &old),
                dep("foo", &bar),
                dep("left", &left),
                dep("right", &right),
            ]),
            node(&old, vec![dep("serde", &serde09)]),
            node(&left, vec![dep("rand", &rand08), dep("foo", &foo)]),
            node(&right, vec![dep("rand", &rand09)]),
        ];
        let metadata = json!({
            "packages": [&app, &serde1, &serde09, &old, &bar, &foo, &left, &right, &rand08, &rand09],
            "workspace_members": [id(&app)],
            "resolve": { "nodes": nodes },
            "workspace_root": "/w",
            "version": 1,
        });
        let metadata = serde_json::from_value(metadata).expect("metadata");
        let lock = CargoLock::parse(&format!(
            "[[package]]\nname = \"serde\"\nversion = \"1.0.229\"\nsource = \"{REGISTRY}\"\nchecksum = \"ab\"\n"
        ))
        .expect("a Cargo.lock");
        (metadata, lock)
    }

    fn version(lookup: &Lookup<'_>) -> Option<String> {
        match lookup {
            Lookup::Found { package, .. } => Some(format!("{}@{}", package.name, package.version)),
            Lookup::AmbiguousVersion(_) | Lookup::AmbiguousPackage(_) | Lookup::NotFound => None,
        }
    }

    #[test]
    fn the_version_a_member_depends_on_is_taken() {
        let (metadata, lock) = metadata();
        let packages = PackageGraph::from_metadata(&metadata, &lock);
        let Lookup::Found {
            package,
            other_versions,
        } = packages.look_up("Serde")
        else {
            panic!("serde 1 is direct");
        };
        assert_eq!(package.version, Version::new(1, 0, 229));
        assert_eq!(package.checksum.as_deref(), Some("ab"));
        assert_eq!(package.source, Source::CratesIo);
        assert_eq!(other_versions.len(), 1);
        assert_eq!(
            version(&packages.look_up("app")).as_deref(),
            Some("app@0.1.0")
        );
        assert_eq!(
            version(&packages.look_up("bar")).as_deref(),
            Some("bar@2.0.0")
        );
        assert_eq!(packages.look_up("nothing"), Lookup::NotFound);
    }

    #[test]
    fn versions_that_no_member_depends_on_are_ambiguous() {
        let (metadata, lock) = metadata();
        let packages = PackageGraph::from_metadata(&metadata, &lock);
        let Lookup::AmbiguousVersion(candidates) = packages.look_up("rand") else {
            panic!("neither rand is direct");
        };
        let used_by: Vec<_> = candidates
            .iter()
            .map(|candidate| {
                let users: Vec<_> = candidate.used_by.iter().map(|user| &*user.name).collect();
                (candidate.package.version.to_string(), users)
            })
            .collect();
        assert_eq!(used_by, [
            ("0.8.5".to_owned(), vec!["left"]),
            ("0.9.2".to_owned(), vec!["right"]),
        ]);
    }

    #[test]
    fn an_extern_name_that_is_another_package_name_is_ambiguous() {
        let (metadata, lock) = metadata();
        let packages = PackageGraph::from_metadata(&metadata, &lock);
        let Lookup::AmbiguousPackage(candidates) = packages.look_up("foo") else {
            panic!("foo is bar in app, and a package of its own");
        };
        let names: Vec<_> = candidates
            .iter()
            .map(|candidate| &*candidate.package.name)
            .collect();
        assert_eq!(names, ["bar", "foo"]);
    }

    #[test]
    fn cargo_lock_alone_resolves_by_package_name() {
        let lock = CargoLock::parse(&format!(
            r#"
[[package]]
name = "app"
version = "0.1.0"
dependencies = ["serde 1.0.229", "old"]

[[package]]
name = "old"
version = "1.0.0"
source = "{REGISTRY}"
dependencies = ["serde 0.9.15"]

[[package]]
name = "serde"
version = "0.9.15"
source = "{REGISTRY}"

[[package]]
name = "serde"
version = "1.0.229"
source = "{REGISTRY}"
checksum = "ab"
"#
        ))
        .expect("a Cargo.lock");
        let packages = PackageGraph::from_lock(&lock);
        assert_eq!(
            version(&packages.look_up("serde")).as_deref(),
            Some("serde@1.0.229")
        );
        let Lookup::Found { package, .. } = packages.look_up("old") else {
            panic!("old is direct");
        };
        assert_eq!(package.manifest_path, None);
    }

    #[test]
    fn docs_rs_is_used_only_for_the_crate_on_crates_io() {
        let (metadata, lock) = metadata();
        let packages = PackageGraph::from_metadata(&metadata, &lock);
        let entry = |checksum: &str| IndexEntry {
            name: "serde".into(),
            versions: [IndexVersion {
                version: Version::new(1, 0, 229),
                checksum: checksum.into(),
                yanked: false,
                rust_version: None,
            }]
            .into(),
            fetched_at: SystemTime::UNIX_EPOCH,
        };
        let check = |name: &str, checksum: &str| {
            let package = packages
                .packages()
                .iter()
                .find(|package| &*package.name == name)
                .expect("a package");
            package.docs_rs_check(&entry(checksum))
        };
        assert_eq!(check("serde", "AB"), DocsRsCheck::Allowed);
        assert_eq!(check("serde", "cd"), DocsRsCheck::ChecksumMismatch);
        assert_eq!(check("app", "ab"), DocsRsCheck::NotCratesIo);
        assert_eq!(check("old", "ab"), DocsRsCheck::NoChecksum);
        assert_eq!(
            Source::parse(Some("git+https://github.com/a/b?rev=1#0123")),
            Source::Git("git+https://github.com/a/b?rev=1#0123".into())
        );
        assert_eq!(
            Source::parse(Some("sparse+https://index.crates.io/")),
            Source::CratesIo
        );
    }
}
