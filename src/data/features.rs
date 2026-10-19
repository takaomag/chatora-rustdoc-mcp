//! The features that a project's normal build enables on each package, from `cargo tree`
//! (requirements R-FT-2 and R-FT-3), for `enabled_in_project` and `enabled_in_dev_only`.
//!
//! `cargo metadata` is not used for this: its `resolve.nodes[].features` merges the features of
//! dev, build and per-target dependencies, more than the normal build enables (requirement
//! R-FT-3).

use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::Path,
};

use semver::Version;

use super::{cargo, metadata::CargoError, target::TargetTriple};

/// The features enabled on each package, in the build for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureSets {
    /// The normal build: `cargo tree -e normal`.
    normal: HashMap<PackageKey, BTreeSet<Box<str>>>,
    /// The build with the dev-dependencies of the members: `cargo tree -e normal,dev`.
    with_dev: HashMap<PackageKey, BTreeSet<Box<str>>>,
}

/// A package as `cargo tree` names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PackageKey {
    name: Box<str>,
    version: Version,
}

impl FeatureSets {
    /// The features of the package `name` at `version` in the normal build. `None` when the
    /// normal build does not build it for the target.
    #[must_use]
    pub fn normal(&self, name: &str, version: &Version) -> Option<&BTreeSet<Box<str>>> {
        self.normal.get(&PackageKey {
            name: name.into(),
            version: version.clone(),
        })
    }

    /// The features of the package in the build with the members' dev-dependencies, as in the
    /// tests. With resolver 1, the normal build has these, too.
    #[must_use]
    pub fn with_dev(&self, name: &str, version: &Version) -> Option<&BTreeSet<Box<str>>> {
        self.with_dev.get(&PackageKey {
            name: name.into(),
            version: version.clone(),
        })
    }
}

/// The feature sets of the workspace at `root` for `target`, the actual build target
/// (requirement R-FT-2).
///
/// # Errors
///
/// Returns an error when `cargo tree` fails or prints what cannot be read.
pub(super) async fn read(
    root: &Path,
    offline: bool,
    target: &TargetTriple,
) -> Result<FeatureSets, CargoError> {
    // One after the other: both take cargo's locks.
    let normal = tree(root, offline, target, "normal").await?;
    let with_dev = tree(root, offline, target, "normal,dev").await?;
    Ok(FeatureSets { normal, with_dev })
}

/// Runs `cargo tree` over the workspace with the dependency kinds `edges`, and reads the feature
/// set of each package.
async fn tree(
    root: &Path,
    offline: bool,
    target: &TargetTriple,
    edges: &str,
) -> Result<HashMap<PackageKey, BTreeSet<Box<str>>>, CargoError> {
    let mut command = cargo::project_command(root, offline);
    command.args([
        "tree",
        "--workspace",
        "--target",
        target.as_str(),
        "-e",
        edges,
    ]);
    command.args([
        "--prefix",
        "depth",
        "--format",
        "{p}\t{f}",
        "--charset",
        "ascii",
    ]);
    let output = cargo::run(command).await?;
    if !output.status.success() {
        return Err(CargoError::Failed {
            command: "tree",
            failure: cargo::classify(&output, offline),
            stderr: cargo::stderr_tail(&output),
        });
    }
    let text = String::from_utf8(output.stdout).map_err(|error| {
        CargoError::Unreadable(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    parse(&text).map_err(CargoError::Unreadable)
}

/// Where a line of the tree stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Under a member, built for the target.
    Target,
    /// A root of the tree: a member, built for the target, or, when a proc-macro also uses it,
    /// for the host. The line does not tell which.
    Root,
    /// A proc-macro, or under one: built for the host.
    Host,
}

/// The features a package has at each place of the tree, `None` where it is not.
#[derive(Debug, Default)]
struct Seen {
    target: Option<BTreeSet<Box<str>>>,
    root: Option<BTreeSet<Box<str>>>,
    host: Option<BTreeSet<Box<str>>>,
}

/// Reads the output of `cargo tree --prefix depth --format '{p}\t{f}'`. With resolver 2 and
/// later, a package built for both the target and the host has a set for each (requirement
/// R-FT-3). It gets the features it has under a member, built for the target. Failing that, those
/// it has as a root; failing that, those it has for the host, as a package that only proc-macros
/// use.
fn parse(text: &str) -> io::Result<HashMap<PackageKey, BTreeSet<Box<str>>>> {
    let mut seen: HashMap<PackageKey, Seen> = HashMap::new();
    // The places of the lines above, by depth.
    let mut path: Vec<Place> = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (key, features, proc_macro, depth) = parse_line(line)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("{line:?}")))?;
        path.truncate(depth);
        let place = match path.last() {
            _ if proc_macro => Place::Host,
            None => Place::Root,
            Some(Place::Host) => Place::Host,
            Some(Place::Target | Place::Root) => Place::Target,
        };
        path.push(place);
        let seen = seen.entry(key).or_default();
        let set = match place {
            Place::Target => &mut seen.target,
            Place::Root => &mut seen.root,
            Place::Host => &mut seen.host,
        };
        set.get_or_insert_default().extend(features);
    }
    Ok(seen
        .into_iter()
        .map(|(key, seen)| {
            let set = seen.target.or(seen.root).or(seen.host).unwrap_or_default();
            (key, set)
        })
        .collect())
}

/// A line of the tree: the package, its features, whether it is a proc-macro, and its depth.
fn parse_line(line: &str) -> Option<(PackageKey, Vec<Box<str>>, bool, usize)> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let (depth, rest) = line.split_at_checked(digits)?;
    let depth = depth.parse().ok()?;
    let (package, features) = rest.split_once('\t')?;
    // A package shown before has ` (*)` after its features.
    let features = features.trim_end_matches(" (*)");
    // `name v1.2.3`, then ` (proc-macro)` and the source, if any.
    let (name, after) = package.split_once(" v")?;
    let version = after.split(' ').next()?.parse().ok()?;
    let proc_macro = after.contains(" (proc-macro)");
    let features = features
        .split(',')
        .filter(|feature| !feature.is_empty())
        .map(Box::from)
        .collect();
    let key = PackageKey {
        name: name.into(),
        version,
    };
    Some((key, features, proc_macro, depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree of a workspace with members `a` and `b`, where `a` uses `dep` with `y`, as a
    /// dev-dependency with `z`, and through the proc-macro `pm`, which uses `dep` with `x`; `b`
    /// uses `dep` with `w`. `dep` and `pm` are path dependencies, so members, too (cargo 1.99.1,
    /// resolver 3).
    const NORMAL: &str = "\
0a v0.1.0 (/w/a)\t
1dep v0.1.0 (/w/dep)\tw,x,y
1pm v0.1.0 (proc-macro) (/w/pm)\t
2dep v0.1.0 (/w/dep)\tx

0b v0.1.0 (/w/b)\t
1dep v0.1.0 (/w/dep)\tw,x,y

0dep v0.1.0 (/w/dep)\tw,x,y

0dep v0.1.0 (/w/dep)\tx

0pm v0.1.0 (proc-macro) (/w/pm)\t (*)
";

    fn features(sets: &HashMap<PackageKey, BTreeSet<Box<str>>>, name: &str) -> Vec<String> {
        let key = PackageKey {
            name: name.into(),
            version: Version::new(0, 1, 0),
        };
        sets.get(&key)
            .expect("in the tree")
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn a_package_gets_its_features_for_the_target() {
        let sets = parse(NORMAL).expect("a tree");
        assert_eq!(features(&sets, "dep"), ["w", "x", "y"]);
        assert_eq!(features(&sets, "a"), Vec::<String>::new());
        assert_eq!(features(&sets, "pm"), Vec::<String>::new());
    }

    #[test]
    fn a_package_only_proc_macros_use_gets_its_host_features() {
        let sets = parse(
            "0a v0.1.0 (/w/a)\t\n1pm v0.1.0 (proc-macro) (/w/pm)\t\n2syn v0.1.0\tfull,parsing\n",
        )
        .expect("a tree");
        assert_eq!(features(&sets, "syn"), ["full", "parsing"]);
    }

    #[test]
    fn a_line_that_is_not_of_the_tree_is_an_error() {
        parse("0a 0.1.0\t\n").expect_err("no version");
        parse("a v0.1.0\t\n").expect_err("no depth");
    }
}
