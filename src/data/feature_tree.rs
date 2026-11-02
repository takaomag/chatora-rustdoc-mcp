//! The features of a package and what each enables, as Cargo reads them.
//!
//! They are those of `[features]`, and one for each optional dependency that no feature names as
//! `dep:<name>` (requirement §5.9's features, decided in M3d, decisions 40 and 41).
//!
//! A feature's values enable another feature of the package (`fs`), an optional dependency
//! (`dep:mio`), or a feature of a dependency (`hyper/http1`, which enables `hyper` too, or
//! `hyper?/http1`, which does not). Names that Cargo does not take are left out (requirement
//! R-SEC-4).

use std::collections::{BTreeMap, BTreeSet};

/// What one value of a feature enables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enables<'a> {
    /// A feature of the package, or the one that an optional dependency makes (`fs`).
    Feature(&'a str),
    /// An optional dependency, without making its feature (`dep:mio`).
    Dependency(&'a str),
    /// A feature of a dependency (`hyper/http1`), which enables the dependency too unless `weak`
    /// (`hyper?/http1`).
    DependencyFeature {
        /// The dependency, by the name the package gives it.
        dependency: &'a str,
        /// Its feature.
        feature: &'a str,
        /// Whether it is enabled only if the dependency is by another feature.
        weak: bool,
    },
}

impl<'a> Enables<'a> {
    /// The value `value` of a feature, or `None` if Cargo does not take it.
    #[must_use]
    pub fn parse(value: &'a str) -> Option<Self> {
        if let Some(dependency) = value.strip_prefix("dep:") {
            return is_name(dependency).then_some(Self::Dependency(dependency));
        }
        match value.split_once('/') {
            Some((dependency, feature)) => {
                let (dependency, weak) = dependency
                    .strip_suffix('?')
                    .map_or((dependency, false), |dependency| (dependency, true));
                (is_name(dependency) && is_name(feature)).then_some(Self::DependencyFeature {
                    dependency,
                    feature,
                    weak,
                })
            },
            None => is_name(value).then_some(Self::Feature(value)),
        }
    }
}

/// Whether `name` is a feature or a dependency as Cargo takes it: a letter, a digit or `_`, then
/// those, `-`, `+` and `.`.
#[must_use]
pub fn is_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
}

/// The features of a package, each with its values as written, in the order of their names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureTree {
    /// Each feature and its values.
    features: BTreeMap<Box<str>, Box<[Box<str>]>>,
    /// The features that optional dependencies make, which no one wrote.
    implicit: BTreeSet<Box<str>>,
    /// The optional dependencies, whose feature, if they make one or one has their name, `x/f`
    /// enables.
    optional: BTreeSet<Box<str>>,
}

impl FeatureTree {
    /// The features `declared`, with their values, and one for each of the optional dependencies
    /// `optional` that no value names as `dep:<name>` and no feature has the name of, whose value
    /// is `dep:<name>`. Names and values that Cargo does not take are left out.
    pub fn new<'a>(
        declared: impl IntoIterator<Item = (&'a str, Vec<&'a str>)>,
        optional: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut features: BTreeMap<Box<str>, Box<[Box<str>]>> = declared
            .into_iter()
            .filter(|(name, _)| is_name(name))
            .map(|(name, values)| {
                let values = values
                    .into_iter()
                    .filter(|value| Enables::parse(value).is_some())
                    .map(Box::from)
                    .collect();
                (Box::from(name), values)
            })
            .collect();
        let named: BTreeSet<&str> = features
            .values()
            .flatten()
            .filter_map(|value| value.strip_prefix("dep:"))
            .collect();
        let optional: BTreeSet<Box<str>> = optional
            .into_iter()
            .filter(|dependency| is_name(dependency))
            .map(Box::from)
            .collect();
        let implicit: BTreeSet<Box<str>> = optional
            .iter()
            .filter(|dependency| {
                !named.contains(dependency.as_ref()) && !features.contains_key(dependency.as_ref())
            })
            .cloned()
            .collect();
        for dependency in &implicit {
            features.insert(
                dependency.clone(),
                Box::from([format!("dep:{dependency}").into_boxed_str()]),
            );
        }
        Self {
            features,
            implicit,
            optional,
        }
    }

    /// The features of the manifest `table`: its `[features]`, and its optional dependencies,
    /// normal and build, of any target too.
    #[must_use]
    pub fn from_manifest(table: &toml::Table) -> Self {
        let declared = table
            .get("features")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flatten()
            .map(|(name, values)| {
                let values = values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .collect();
                (name.as_str(), values)
            });
        let targets = table
            .get("target")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flat_map(|targets| targets.values())
            .filter_map(toml::Value::as_table);
        let optional = std::iter::once(table)
            .chain(targets)
            .flat_map(|owner| {
                // `build_dependencies`, as older manifests spell it, too.
                ["dependencies", "build-dependencies", "build_dependencies"]
                    .into_iter()
                    .filter_map(|kind| owner.get(kind).and_then(toml::Value::as_table))
            })
            .flatten()
            .filter(|(_, dependency)| {
                dependency
                    .get("optional")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|(name, _)| name.as_str());
        Self::new(declared, optional)
    }

    /// The names of the features, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.features.keys().map(AsRef::as_ref)
    }

    /// How many features there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.features.len()
    }

    /// Whether there is none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Whether the package has `feature`.
    #[must_use]
    pub fn contains(&self, feature: &str) -> bool {
        self.features.contains_key(feature)
    }

    /// The values of `feature`, as written, if the package has it.
    #[must_use]
    pub fn values(&self, feature: &str) -> Option<&[Box<str>]> {
        self.features.get(feature).map(AsRef::as_ref)
    }

    /// Whether `feature` is one that an optional dependency makes, which no one wrote; read from
    /// `cargo metadata`, which lists those too, one written as Cargo makes them, `x = ["dep:x"]`,
    /// which means the same.
    #[must_use]
    pub fn is_implicit(&self, feature: &str) -> bool {
        self.implicit.contains(feature)
    }

    /// The features of the package that `enabled`, values of a feature or names of features
    /// (`--features`), enable, with those that they enable in turn: `x/f` enables the feature
    /// named `x` if `x` is an optional dependency, the one that it makes or one written with its
    /// name, and `x?/f` and `dep:x` none.
    pub fn closure<'s>(&'s self, enabled: impl IntoIterator<Item = &'s str>) -> BTreeSet<&'s str> {
        let mut closure = BTreeSet::new();
        let mut pending: Vec<&str> = enabled.into_iter().collect();
        while let Some(value) = pending.pop() {
            let feature = match Enables::parse(value) {
                Some(Enables::Feature(feature)) => feature,
                Some(Enables::DependencyFeature {
                    dependency,
                    weak: false,
                    ..
                }) if self.optional.contains(dependency) => dependency,
                Some(Enables::Dependency(_) | Enables::DependencyFeature { .. }) | None => {
                    continue;
                },
            };
            if let Some((name, values)) = self.features.get_key_value(feature)
                && closure.insert(name.as_ref())
            {
                pending.extend(values.iter().map(AsRef::as_ref));
            }
        }
        closure
    }

    /// For each feature, the others that enable it, through others too: each closure once, as
    /// a package may have thousands of features.
    #[must_use]
    pub fn enabled_by(&self) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut enabled_by: BTreeMap<&str, BTreeSet<&str>> = self
            .names()
            .map(|feature| (feature, BTreeSet::new()))
            .collect();
        for other in self.names() {
            for feature in self.closure([other]) {
                if feature != other
                    && let Some(by) = enabled_by.get_mut(feature)
                {
                    by.insert(other);
                }
            }
        }
        enabled_by
    }

    /// The values of the features `enabled` that enable a dependency or its feature, as written
    /// (`dep:mio`, `hyper/http1`, `hyper?/http1`), with `dep:x` of a feature that an optional
    /// dependency `x` makes.
    #[must_use]
    pub fn dependencies<'s>(&'s self, enabled: &BTreeSet<&'s str>) -> BTreeSet<&'s str> {
        enabled
            .iter()
            .filter_map(|feature| self.values(feature))
            .flatten()
            .map(AsRef::as_ref)
            .filter(|value| {
                matches!(
                    Enables::parse(value),
                    Some(Enables::Dependency(_) | Enables::DependencyFeature { .. })
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The features of tokio as its index line has them: some in `features`, `io-uring` in
    /// `features2`, as it names `dep:`, and the optional dependencies, of a target too.
    fn tokio() -> FeatureTree {
        FeatureTree::new(
            [
                ("default", vec![]),
                ("fs", vec![]),
                ("full", vec!["fs", "net", "rt-multi-thread", "macros"]),
                ("macros", vec!["tokio-macros"]),
                ("net", vec!["libc", "mio/os-poll", "mio/net", "socket2"]),
                ("rt", vec![]),
                ("rt-multi-thread", vec!["rt"]),
                ("io-uring", vec![
                    "dep:io-uring",
                    "libc",
                    "mio/os-poll",
                    "dep:slab",
                    "rt",
                ]),
            ],
            [
                "tokio-macros",
                "libc",
                "libc",
                "mio",
                "socket2",
                "tracing",
                "io-uring",
                "slab",
            ],
        )
    }

    /// An optional dependency makes a feature unless a value names it as `dep:`; a name that
    /// Cargo does not take is left out.
    #[test]
    fn optional_dependencies_make_features() {
        let tree = tokio();
        for implicit in ["libc", "mio", "socket2", "tokio-macros", "tracing"] {
            assert!(tree.is_implicit(implicit), "{implicit}");
            assert_eq!(
                tree.values(implicit),
                Some([format!("dep:{implicit}").into_boxed_str()].as_slice())
            );
        }
        assert!(!tree.contains("slab"));
        // `io-uring` is a feature that names its dependency with `dep:`.
        assert!(tree.contains("io-uring") && !tree.is_implicit("io-uring"));
        assert_eq!(tree.len(), 13);
        let odd = FeatureTree::new(
            [
                ("ok", vec!["a b", "dep:", "x/", "fine"]),
                ("bad name", vec![]),
            ],
            ["-x"],
        );
        assert_eq!(odd.names().collect::<Vec<_>>(), ["ok"]);
        assert_eq!(odd.values("ok"), Some([Box::from("fine")].as_slice()));
    }

    /// The closure follows the features of the package, through `x/f` to the feature that `x`
    /// makes, but not `x?/f` nor `dep:x` (decided in M3d, decision 40).
    #[test]
    fn a_feature_enables_others_in_turn() {
        let tree = tokio();
        assert_eq!(tree.closure(["full"]).into_iter().collect::<Vec<_>>(), [
            "fs",
            "full",
            "libc",
            "macros",
            "mio",
            "net",
            "rt",
            "rt-multi-thread",
            "socket2",
            "tokio-macros"
        ]);
        // `dep:io-uring` makes no feature; `slab` has none.
        assert_eq!(
            tree.closure(["io-uring"]).into_iter().collect::<Vec<_>>(),
            ["io-uring", "libc", "mio", "rt"]
        );
        let weak = FeatureTree::new([("a", vec!["hyper?/http1"]), ("b", vec!["hyper/http1"])], [
            "hyper",
        ]);
        assert_eq!(weak.closure(["a"]).into_iter().collect::<Vec<_>>(), ["a"]);
        assert_eq!(weak.closure(["b"]).into_iter().collect::<Vec<_>>(), [
            "b", "hyper"
        ]);
        // As `--features` takes them.
        assert_eq!(
            weak.closure(["hyper/http1"])
                .into_iter()
                .collect::<Vec<_>>(),
            ["hyper"]
        );
        // A dependency named with `dep:` makes no feature for `x/f` to enable, but one written with
        // its name; one that is not optional none.
        let named = FeatureTree::new([("a", vec!["dep:x", "x/f"])], ["x"]);
        assert_eq!(named.closure(["a"]).into_iter().collect::<Vec<_>>(), ["a"]);
        let written = FeatureTree::new(
            [
                ("a", vec!["serde/derive"]),
                ("serde", vec!["dep:serde", "chrono/serde"]),
            ],
            ["serde", "chrono"],
        );
        assert_eq!(written.closure(["a"]).into_iter().collect::<Vec<_>>(), [
            "a", "chrono", "serde"
        ]);
        let required = FeatureTree::new([("a", vec!["x/f"]), ("x", vec![])], []);
        assert_eq!(required.closure(["a"]).into_iter().collect::<Vec<_>>(), [
            "a"
        ]);
        // A cycle ends.
        let cycle = FeatureTree::new([("a", vec!["b"]), ("b", vec!["a"])], []);
        assert_eq!(cycle.closure(["a"]).into_iter().collect::<Vec<_>>(), [
            "a", "b"
        ]);
    }

    /// What enables a feature, through others too, and what the features enable of their
    /// dependencies, as written.
    #[test]
    fn what_enables_a_feature_and_what_it_enables_of_dependencies() {
        let tree = tokio();
        let enabled_by = tree.enabled_by();
        assert_eq!(enabled_by.len(), tree.len());
        assert_eq!(enabled_by["rt"].iter().copied().collect::<Vec<_>>(), [
            "full",
            "io-uring",
            "rt-multi-thread"
        ]);
        assert!(enabled_by["full"].is_empty());
        let full = tree.closure(["net"]);
        assert_eq!(tree.dependencies(&full).into_iter().collect::<Vec<_>>(), [
            "dep:libc",
            "dep:mio",
            "dep:socket2",
            "mio/net",
            "mio/os-poll"
        ]);
        assert_eq!(
            tree.dependencies(&tree.closure(["io-uring"]))
                .into_iter()
                .collect::<Vec<_>>(),
            [
                "dep:io-uring",
                "dep:libc",
                "dep:mio",
                "dep:slab",
                "mio/os-poll"
            ]
        );
    }

    /// A manifest's features and its optional dependencies, of a target and of the build too,
    /// renamed by their key.
    #[test]
    fn a_manifest_tells_its_features() {
        let manifest: toml::Table = toml::from_str(
            r#"
            [features]
            default = ["tls"]
            tls = ["dep:native-tls-crate"]

            [dependencies.native-tls-crate]
            version = "0.2"
            optional = true
            package = "native-tls"

            [dependencies.serde]
            version = "1"
            optional = true

            [build-dependencies.cc]
            version = "1"
            optional = true

            [build_dependencies.old]
            version = "1"
            optional = true

            [target.'cfg(unix)'.dependencies.libc]
            version = "0.2"
            optional = true

            [dev-dependencies.tokio]
            version = "1"
            "#,
        )
        .expect("TOML");
        let tree = FeatureTree::from_manifest(&manifest);
        assert_eq!(tree.names().collect::<Vec<_>>(), [
            "cc", "default", "libc", "old", "serde", "tls"
        ]);
        assert_eq!(tree.closure(["default"]).into_iter().collect::<Vec<_>>(), [
            "default", "tls"
        ]);
    }
}
