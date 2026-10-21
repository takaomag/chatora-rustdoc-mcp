//! Which packages of the default project the query tools may build locally, and for which target
//! (requirement R-MCP-3).
//!
//! They are those that `cargo check --all-targets` builds for the target, as
//! `cargo tree --workspace --target <t> -e normal,build,dev` shows them. A build-dependency, a
//! proc-macro and what is under them are built for the host, the rest for the target. With the
//! feature resolver 2 and later, `cargo rustdoc -p` makes cargo panic on a package that the
//! members do not reach through normal dependencies built for the target (requirement R-WS-5), so
//! those are told apart before cargo is run (`build_unsupported`).
//!
//! The tree is read with `--prefix indent`, whose headers tell the build and dev edges. Where a
//! line stands tells whether it is built for the host, but for the roots: a member that a
//! proc-macro or a build-dependency uses is a root twice, once for the target and once for the
//! host, in an order that the tree does not tell. The tree inverted from it tells: what depends on
//! the one for the host goes through a proc-macro or a build-dependency (cargo 1.99.1,
//! `doc/m2-plan.md` §6.3, B7).

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{
    cargo,
    cargo_tree::{self, Edge, Line, TreePackage, TreeSource},
    metadata::{CargoError, Metadata, Package, PackageId, ResolveError},
    packages::Source,
    target::TargetTriple,
};

/// The packages that the project's build builds for one target, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScope {
    packages: HashMap<PackageId, Built>,
}

/// How the project's build builds a package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Built {
    /// For the target.
    pub for_target: bool,
    /// For the host.
    pub for_host: bool,
    /// `cargo rustdoc -p` can name it: always with the feature resolver 1.
    pub nameable: bool,
}

/// Whether and how a package may be built locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buildable {
    /// The project's build builds it for the target.
    ForTarget,
    /// The project's build builds it only for the host: a build-dependency, a proc-macro, or a
    /// package that only they use.
    ForHost,
    /// The project's build builds it, but `cargo rustdoc -p` cannot name it: cargo would panic
    /// (requirement R-WS-5, `build_unsupported`).
    Unnameable,
    /// The project's build does not build it for the target (requirement R-MCP-3).
    NotBuilt,
}

impl BuildScope {
    /// Whether and how the package `id` may be built locally.
    #[must_use]
    pub fn buildable(&self, id: &PackageId) -> Buildable {
        match self.packages.get(id) {
            None => Buildable::NotBuilt,
            Some(built) if !built.nameable => Buildable::Unnameable,
            Some(built) if built.for_target => Buildable::ForTarget,
            Some(_) => Buildable::ForHost,
        }
    }

    /// How the project's build builds the package `id`, `None` when it does not.
    #[must_use]
    pub fn get(&self, id: &PackageId) -> Option<Built> {
        self.packages.get(id).copied()
    }
}

/// Why the scope is not known (`scope_unknown`, requirement R-MCP-3).
#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    /// The packages cannot be resolved.
    #[error(transparent)]
    Resolve(#[from] Arc<ResolveError>),
    /// `cargo metadata` failed, so `cargo tree`, which resolves the same way, would fail, too.
    #[error("cargo metadata failed, so the scope is not known")]
    NoMetadata,
    /// `cargo tree` failed.
    #[error(transparent)]
    Cargo(#[from] CargoError),
    /// The output of `cargo tree` does not tell the scope: a line that cannot be read, a package
    /// that matches no single package of `cargo metadata`, or roots whose kinds stay unknown.
    #[error("cargo tree does not tell the scope: {0}")]
    Unknown(Box<str>),
}

/// Where a line of the tree is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Target,
    Host,
}

/// The scope of the workspace at `root` for `target`, an actual build target, whose packages
/// `metadata` gives.
///
/// # Errors
///
/// Returns an error when `cargo tree` fails, or its output does not tell the scope.
pub(super) async fn read(
    root: &Path,
    offline: bool,
    target: &TargetTriple,
    metadata: &Metadata,
) -> Result<BuildScope, ScopeError> {
    let manifest = metadata.workspace_root.join("Cargo.toml");
    let resolver_2 = uses_resolver_2(manifest.clone())
        .await
        .map_err(|error| ScopeError::Unknown(format!("{}: {error}", manifest.display()).into()))?;
    let ids = Ids::new(metadata);
    let all = tree(root, offline, target, "normal,build,dev", &[]).await?;
    let roots = root_places(root, offline, target, &all, &ids).await?;
    let mut packages: HashMap<PackageId, Built> = HashMap::new();
    for (line, place) in all.iter().zip(places(&all, &roots)) {
        let built = packages
            .entry(ids.of(&line.package)?.clone())
            .or_insert_with(|| Built {
                nameable: !resolver_2,
                ..Built::default()
            });
        match place {
            Place::Target => built.for_target = true,
            Place::Host => built.for_host = true,
        }
    }
    if resolver_2 {
        // Without the dev-dependencies, as `cargo rustdoc --lib` resolves the features: cargo
        // finds the features of the package for the target, or, for a proc-macro, for the host.
        // A member that is a root twice is taken as built for the target both times, which only
        // names more: cargo then panics before it builds anything.
        let normal = tree(root, offline, target, "normal,build", &[]).await?;
        for (line, place) in normal.iter().zip(places(&normal, &[])) {
            if place == Place::Target || line.package.proc_macro {
                // A package that the dev-dependencies leave out is not in the scope.
                if let Some(built) = packages.get_mut(ids.of(&line.package)?) {
                    built.nameable = true;
                }
            }
        }
    }
    Ok(BuildScope { packages })
}

/// Runs `cargo tree` over the workspace for `target` with the dependency kinds `edges` and the
/// arguments `extra`, and reads it.
async fn tree(
    root: &Path,
    offline: bool,
    target: &TargetTriple,
    edges: &str,
    extra: &[&str],
) -> Result<Vec<Line>, ScopeError> {
    let mut command = cargo::project_command(root, offline, None);
    command.args([
        "tree",
        "--workspace",
        "--target",
        target.as_str(),
        "-e",
        edges,
        "--prefix",
        "indent",
        "--format",
        "{p}\t{f}",
        "--charset",
        "ascii",
    ]);
    command.args(extra);
    let output = cargo::run(command).await.map_err(CargoError::from)?;
    if !output.status.success() {
        return Err(CargoError::Failed {
            command: "tree",
            failure: cargo::classify(&output, offline),
            stderr: cargo::stderr_tail(&output),
        }
        .into());
    }
    let text = String::from_utf8(output.stdout).map_err(|error| {
        CargoError::Unreadable(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    cargo_tree::parse(&text).map_err(|error| ScopeError::Unknown(error.to_string().into()))
}

/// Where each line of `lines` is built, given where each root is built, in the order of the roots
/// (built for the target when not given).
fn places(lines: &[Line], roots: &[Place]) -> Vec<Place> {
    let mut places = Vec::with_capacity(lines.len());
    // Where the lines above are built, by depth.
    let mut above: Vec<Place> = Vec::new();
    let mut roots = roots.iter();
    for line in lines {
        above.truncate(line.depth);
        let place = if line.depth == 0 {
            let given = roots.next().copied();
            given.unwrap_or(if line.package.proc_macro {
                Place::Host
            } else {
                Place::Target
            })
        } else if line.package.proc_macro
            || line.edge == Edge::Build
            || above.last() == Some(&Place::Host)
        {
            Place::Host
        } else {
            Place::Target
        };
        above.push(place);
        places.push(place);
    }
    places
}

/// Where each root of `lines` is built, in their order. A package that is a root more than once
/// is built for the target and for the host: the tree inverted from it tells which is which.
async fn root_places(
    root: &Path,
    offline: bool,
    target: &TargetTriple,
    lines: &[Line],
    ids: &Ids<'_>,
) -> Result<Vec<Place>, ScopeError> {
    let roots: Vec<&Line> = lines.iter().filter(|line| line.depth == 0).collect();
    let mut counts: BTreeMap<&TreePackage, usize> = BTreeMap::new();
    for line in &roots {
        let count = counts.entry(&line.package).or_default();
        *count = count.saturating_add(1);
    }
    counts.retain(|package, count| !package.proc_macro && *count > 1);
    let inverted_lines = if counts.is_empty() {
        Vec::new()
    } else {
        // Without dedupe, so that each root shows all that depends on it.
        let mut extra = vec!["--no-dedupe"];
        for package in counts.keys() {
            extra.extend(["-i", &*ids.of(package)?.0]);
        }
        tree(root, offline, target, "normal,build,dev", &extra).await?
    };
    let mut inverted: BTreeMap<&TreePackage, VecDeque<InvertedRoot<'_>>> = BTreeMap::new();
    for found in inverted_roots(&inverted_lines) {
        inverted
            .entry(&found.line.package)
            .or_default()
            .push_back(found);
    }
    let unknown = |package: &TreePackage| {
        ScopeError::Unknown(
            format!(
                "the roots of {} {} in the inverted tree",
                package.name, package.version
            )
            .into(),
        )
    };
    // The same nodes, one for the target: those of the package, in the order cargo added them.
    for (package, count) in &counts {
        let same = inverted.get(package).is_some_and(|found| {
            found.len() == *count
                && found
                    .iter()
                    .filter(|found| found.place == Place::Target)
                    .count()
                    == 1
        });
        if !same {
            return Err(unknown(package));
        }
    }
    roots
        .iter()
        .map(|line| {
            if line.package.proc_macro {
                return Ok(Place::Host);
            }
            let Some(found) = inverted.get_mut(&line.package) else {
                return Ok(Place::Target);
            };
            match found.pop_front() {
                Some(found) if found.line.features == line.features => Ok(found.place),
                _ => Err(unknown(&line.package)),
            }
        })
        .collect()
}

/// A root of an inverted tree, and where it is built.
struct InvertedRoot<'l> {
    line: &'l Line,
    place: Place,
}

/// The roots of an inverted tree and where each is built: for the host when what depends on it
/// goes through a proc-macro or a build-dependency.
fn inverted_roots(lines: &[Line]) -> Vec<InvertedRoot<'_>> {
    let mut roots: Vec<InvertedRoot<'_>> = Vec::new();
    for line in lines {
        if line.depth == 0 {
            roots.push(InvertedRoot {
                line,
                place: Place::Target,
            });
        } else if (line.package.proc_macro || line.edge == Edge::Build)
            && let Some(root) = roots.last_mut()
        {
            root.place = Place::Host;
        }
    }
    roots
}

/// The packages of `cargo metadata`, to find the one that a line of `cargo tree` shows, which has
/// no package id: by name and version, and by source when several have both.
struct Ids<'m> {
    by_name: HashMap<&'m str, Vec<&'m Package>>,
}

impl<'m> Ids<'m> {
    fn new(metadata: &'m Metadata) -> Self {
        let mut by_name: HashMap<&str, Vec<&Package>> = HashMap::new();
        for package in &metadata.packages {
            by_name.entry(&package.name).or_default().push(package);
        }
        Self { by_name }
    }

    fn of(&self, shown: &TreePackage) -> Result<&'m PackageId, ScopeError> {
        let candidates: Vec<&Package> = self
            .by_name
            .get(&*shown.name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|package| package.version == shown.version)
            .collect();
        let found: Vec<&Package> = if candidates.len() > 1 {
            candidates
                .into_iter()
                .filter(|package| same_source(&shown.source, package))
                .collect()
        } else {
            candidates
        };
        match found.as_slice() {
            [package] => Ok(&package.id),
            _ => Err(ScopeError::Unknown(
                format!(
                    "{} {} matches {} packages of cargo metadata",
                    shown.name,
                    shown.version,
                    found.len()
                )
                .into(),
            )),
        }
    }
}

/// Whether `package` comes from the source that `cargo tree` shows as `shown`.
fn same_source(shown: &TreeSource, package: &Package) -> bool {
    match (shown, Source::parse(package.source.as_deref())) {
        (TreeSource::CratesIo, Source::CratesIo)
        | (TreeSource::Registry(_), Source::Registry(_)) => true,
        (TreeSource::Other(path), Source::Local) => {
            package.manifest_path.parent() == Some(Path::new(&**path))
        },
        // `git+<url>#<commit>` against `<url>#<the first digits of the commit>`.
        (TreeSource::Other(shown), Source::Git(source)) => {
            let (url, commit) = source
                .strip_prefix("git+")
                .and_then(|source| source.rsplit_once('#'))
                .unwrap_or_default();
            shown.rsplit_once('#').is_some_and(|(shown_url, short)| {
                shown_url == url && !short.is_empty() && commit.starts_with(short)
            })
        },
        _ => false,
    }
}

/// Whether the workspace whose root manifest is at `path` uses the feature resolver 2 or later:
/// `resolver` in `[workspace]` or `[package]`, else the edition of the root package (2021 and
/// later), else, for a virtual workspace, 1.
async fn uses_resolver_2(path: PathBuf) -> io::Result<bool> {
    super::blocking(move || {
        let text = std::fs::read_to_string(&path)?;
        let manifest: toml::Table = toml::from_str(&text).map_err(io::Error::other)?;
        Ok(resolver_2(&manifest))
    })
    .await
}

/// See [`uses_resolver_2`].
fn resolver_2(manifest: &toml::Table) -> bool {
    fn table<'t>(table: &'t toml::Table, key: &str) -> Option<&'t toml::Table> {
        table.get(key).and_then(toml::Value::as_table)
    }
    let workspace = table(manifest, "workspace");
    let package = table(manifest, "package");
    let resolver = workspace
        .into_iter()
        .chain(package)
        .find_map(|table| table.get("resolver").and_then(toml::Value::as_str));
    if let Some(resolver) = resolver {
        return resolver != "1";
    }
    let Some(package) = package else {
        return false;
    };
    let edition = match package.get("edition") {
        Some(toml::Value::String(edition)) => Some(edition.as_str()),
        // `edition.workspace = true`
        Some(toml::Value::Table(_)) => workspace
            .and_then(|workspace| table(workspace, "package"))
            .and_then(|package| package.get("edition"))
            .and_then(toml::Value::as_str),
        _ => None,
    };
    edition.is_some_and(|edition| !matches!(edition, "2015" | "2018"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<Line> {
        cargo_tree::parse(text).expect("a tree")
    }

    #[test]
    fn build_edges_and_proc_macros_are_built_for_the_host() {
        let tree = lines(
            "app v0.1.0 (/w/app)\t\n\
             |-- pm v0.1.0 (proc-macro) (/d/pm)\t\n\
             |   `-- pmdep v0.1.0 (/d/pmdep)\t\n\
             `-- shared v0.1.0 (/d/shared)\t\n\
             [build-dependencies]\n\
             `-- cc v1.0.0\t\n    \
                 `-- libc v1.0.0\t\n\
             [dev-dependencies]\n\
             `-- devonly v0.1.0 (/d/devonly)\t\n",
        );
        let places = places(&tree, &[]);
        let names: Vec<(&str, Place)> = tree
            .iter()
            .map(|line| &*line.package.name)
            .zip(places)
            .collect();
        assert_eq!(names, [
            ("app", Place::Target),
            ("pm", Place::Host),
            ("pmdep", Place::Host),
            ("shared", Place::Target),
            ("cc", Place::Host),
            ("libc", Place::Host),
            ("devonly", Place::Target),
        ]);
    }

    #[test]
    fn a_root_is_where_it_is_given_to_be_built() {
        let tree = lines(
            "helper v0.1.0 (/w/helper)\t\n`-- hdep v0.1.0 (/w/hdep)\t\n\n\
             helper v0.1.0 (/w/helper)\tx\n`-- hdep v0.1.0 (/w/hdep)\t\n",
        );
        assert_eq!(places(&tree, &[Place::Host, Place::Target]), [
            Place::Host,
            Place::Host,
            Place::Target,
            Place::Target,
        ]);
    }

    #[test]
    fn the_inverted_tree_tells_the_root_for_the_host() {
        // `cargo tree -i helper -i bhelper --no-dedupe` (cargo 1.99.1).
        let tree = lines(
            "bhelper v0.1.0 (/w/bhelper)\t\n[build-dependencies]\n`-- z v0.1.0 (/w/z)\t\n\n\
             bhelper v0.1.0 (/w/bhelper)\t\n\n\
             helper v0.1.0 (/w/helper)\t\n`-- pm v0.1.0 (proc-macro) (/w/pm)\t\n    \
             `-- z v0.1.0 (/w/z)\t\n\n\
             helper v0.1.0 (/w/helper)\tx\n`-- z v0.1.0 (/w/z)\t\n",
        );
        let places: Vec<(&str, Place)> = inverted_roots(&tree)
            .into_iter()
            .map(|root| (&*root.line.package.name, root.place))
            .collect();
        assert_eq!(places, [
            ("bhelper", Place::Host),
            ("bhelper", Place::Target),
            ("helper", Place::Host),
            ("helper", Place::Target),
        ]);
    }

    #[test]
    fn the_resolver_follows_cargo() {
        let resolver = |text: &str| resolver_2(&toml::from_str(text).expect("TOML"));
        assert!(!resolver("[workspace]\nmembers = []\n"), "virtual: 1");
        assert!(resolver("[workspace]\nresolver = \"2\"\n"));
        assert!(resolver("[workspace]\nresolver = \"3\"\n"));
        assert!(!resolver("[package]\nname = \"a\"\n"), "edition 2015");
        assert!(!resolver("[package]\nname = \"a\"\nedition = \"2018\"\n"));
        assert!(resolver("[package]\nname = \"a\"\nedition = \"2021\"\n"));
        assert!(!resolver(
            "[package]\nname = \"a\"\nedition = \"2024\"\nresolver = \"1\"\n"
        ));
        assert!(resolver(
            "[package]\nname = \"a\"\nedition.workspace = true\n\
             [workspace.package]\nedition = \"2024\"\n"
        ));
        assert!(!resolver(
            "[package]\nname = \"a\"\nedition = \"2024\"\n[workspace]\nresolver = \"1\"\n"
        ));
    }
}
