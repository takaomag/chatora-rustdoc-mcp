//! The examples of a package, with the features that each needs.
//!
//! They are the files of its `examples/`, as Cargo finds them, and those that its manifest's
//! `[[example]]` names (requirement §5.9, decided in M3e, decisions 69 and 70).
//!
//! As Cargo does, `examples/<name>.rs` and `examples/<name>/main.rs` are examples unless
//! `autoexamples = false`; an `[[example]]` names one by its `name`, at its `path` or else at one of
//! those, and tells its `required-features`. Only a file of the package that may be read is one:
//! under its root, a `.rs` file, not under a hidden directory (requirement R-SC-4).

use std::collections::{BTreeMap, HashSet};

use super::readme;

/// An example of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Example {
    /// Its name, as `cargo run --example` takes it.
    pub name: Box<str>,
    /// Its file, relative to the package's root with `/`.
    pub file: Box<str>,
    /// The features that it needs.
    pub required_features: Box<[Box<str>]>,
}

/// The examples of the package whose manifest is `table` and whose files are `files`, relative to
/// its root with `/`, sorted by name.
#[must_use]
pub fn of_package<'f>(
    table: &toml::Table,
    files: impl IntoIterator<Item = &'f str>,
) -> Vec<Example> {
    // A set, as a `.crate` may have 100,000 files and its manifest as many examples.
    let files: HashSet<&str> = files.into_iter().collect();
    let has = |file: &str| files.contains(file);
    let package = table.get("package").and_then(toml::Value::as_table);
    let declared = table
        .get("example")
        .and_then(toml::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    // As Cargo does: by default, except in edition 2015 when examples are declared.
    let edition_2015 = package
        .and_then(|package| package.get("edition"))
        .is_none_or(|edition| edition.as_str() == Some("2015"));
    let automatic = package
        .and_then(|package| package.get("autoexamples"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(!edition_2015 || declared.is_empty());
    let mut found: BTreeMap<Box<str>, Example> = BTreeMap::new();
    // The names and the files that `[[example]]` takes, though one is not read: Cargo finds no
    // other example of them.
    let mut taken_names: HashSet<&str> = HashSet::new();
    let mut taken_files: HashSet<String> = HashSet::new();
    for entry in declared {
        let Some(name) = entry.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        taken_names.insert(name);
        let file = entry.get("path").and_then(toml::Value::as_str).map_or_else(
            || {
                [
                    format!("examples/{name}.rs"),
                    format!("examples/{name}/main.rs"),
                ]
                .into_iter()
                .find(|file| has(file))
            },
            readme::relative,
        );
        let Some(file) = file else {
            continue;
        };
        taken_files.insert(file.clone());
        if !(has(&file) && readable(&file)) {
            continue;
        }
        let required_features = entry
            .get("required-features")
            .and_then(toml::Value::as_array)
            .map(|features| {
                features
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(Box::from)
                    .collect()
            })
            .unwrap_or_default();
        found.insert(name.into(), Example {
            name: name.into(),
            file: file.into(),
            required_features,
        });
    }
    if automatic {
        let mut discovered_files: Vec<&str> = files.iter().copied().collect();
        discovered_files.sort_unstable();
        for file in discovered_files {
            if let Some(name) = discovered(file)
                && !taken_names.contains(name)
                && !taken_files.contains(file)
                && !found.contains_key(name)
            {
                found.insert(name.into(), Example {
                    name: name.into(),
                    file: file.into(),
                    required_features: Box::default(),
                });
            }
        }
    }
    found.into_values().collect()
}

/// The examples of a package built here, as `cargo metadata` tells its `targets`, of its directory
/// `dir`, sorted by name.
///
/// They are those of the kind `example` whose file is in it, a `.rs` file not under a hidden
/// directory (requirement R-SC-4).
#[cfg(feature = "local")]
#[must_use]
pub fn of_targets(targets: &[super::metadata::Target], dir: &std::path::Path) -> Vec<Example> {
    let mut examples: Vec<Example> = targets
        .iter()
        .filter(|target| target.kind.iter().any(|kind| &**kind == "example"))
        .filter_map(|target| {
            let (path, dir) = (
                super::source_root::lexically(&target.src_path),
                super::source_root::lexically(dir),
            );
            let relative = path.strip_prefix(&dir).ok()?;
            let file = relative
                .components()
                .map(|part| part.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()?
                .join("/");
            readable(&file).then(|| Example {
                name: target.name.clone(),
                file: file.into(),
                required_features: target.required_features.clone(),
            })
        })
        .collect();
    examples.sort_by(|a, b| a.name.cmp(&b.name));
    examples
}

/// The most entries of `examples/` looked at (decided in M3e, decision 83).
#[cfg(feature = "local")]
const MAX_SCANNED: usize = 10_000;

/// The files of the package in `dir`, whose manifest is `table`, that may be its examples.
///
/// They are relative to `dir` with `/`, for [`of_package`] when `cargo metadata` does not tell
/// them (decided in M3e, decisions 83 and 86): `examples/<name>.rs` and `examples/<name>/main.rs`
/// as Cargo finds them, and the files that `[[example]]` names. What starts with a dot, a name that
/// is not UTF-8 and a directory under `examples/` through a symlink are not looked into, but
/// `examples/` itself is followed, as Cargo does; a symlink to a file is listed, for the reading to
/// judge where it leads. At most 10,000 entries of `examples/` and 10,000 `[[example]]` are looked
/// at.
#[cfg(feature = "local")]
#[must_use]
pub fn files_in(dir: &std::path::Path, table: &toml::Table) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut looked_at = 0_usize;
    let is_file = |path: &std::path::Path| {
        std::fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_file() || metadata.file_type().is_symlink())
    };
    if let Ok(entries) = std::fs::read_dir(dir.join("examples")) {
        for entry in entries.flatten() {
            looked_at = looked_at.saturating_add(1);
            if looked_at > MAX_SCANNED {
                break;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if is_file(&entry.path().join("main.rs")) {
                    files.push(format!("examples/{name}/main.rs"));
                }
            } else if std::path::Path::new(&name)
                .extension()
                .is_some_and(|extension| extension == "rs")
            {
                files.push(format!("examples/{name}"));
            }
        }
    }
    let declared = table
        .get("example")
        .and_then(toml::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for path in declared
        .iter()
        .take(MAX_SCANNED)
        .filter_map(|entry| entry.get("path").and_then(toml::Value::as_str))
        .filter_map(readme::relative)
    {
        if is_file(&dir.join(&path)) {
            files.push(path);
        }
    }
    files.sort_unstable();
    files.dedup();
    files
}

/// The name of the example that Cargo finds at `file`: `examples/<name>.rs` or
/// `examples/<name>/main.rs`.
fn discovered(file: &str) -> Option<&str> {
    let rest = file.strip_prefix("examples/")?;
    let name = match rest.split_once('/') {
        None => rest.strip_suffix(".rs")?,
        Some((name, "main.rs")) => name,
        Some(_) => return None,
    };
    (!name.is_empty() && !name.starts_with('.')).then_some(name)
}

/// Whether the example at `file` may be read: a `.rs` file not under a hidden directory
/// (requirement R-SC-4).
fn readable(file: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .is_some_and(|extension| extension == "rs")
        && !file.split('/').any(|part| part.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).expect("a manifest")
    }

    fn told(examples: &[Example]) -> Vec<(&str, &str, Vec<&str>)> {
        examples
            .iter()
            .map(|example| {
                (
                    &*example.name,
                    &*example.file,
                    example
                        .required_features
                        .iter()
                        .map(AsRef::as_ref)
                        .collect(),
                )
            })
            .collect()
    }

    const FILES: [&str; 7] = [
        "examples/chat.rs",
        "examples/echo/main.rs",
        "examples/echo/util.rs",
        "examples/common/mod.rs",
        "examples/.hidden.rs",
        "demos/tls.rs",
        "src/lib.rs",
    ];

    /// Cargo finds `examples/<name>.rs` and `examples/<name>/main.rs`, not the modules they use;
    /// `[[example]]` adds its `required-features`, or one at another path.
    #[test]
    fn the_examples_are_found_as_cargo_finds_them() {
        let manifest = table(
            "[package]\nname = \"demo\"\nedition = \"2021\"\n\n[[example]]\nname = \"chat\"\nrequired-features = \
             [\"full\"]\n\n[[example]]\nname = \"tls\"\npath = \"./demos/tls.rs\"\n\n\
             [[example]]\nname = \"gone\"\n",
        );
        assert_eq!(told(&of_package(&manifest, FILES)), [
            ("chat", "examples/chat.rs", vec!["full"]),
            ("echo", "examples/echo/main.rs", vec![]),
            ("tls", "demos/tls.rs", vec![]),
        ]);
    }

    /// As Cargo does: an `[[example]]` takes the name and the file, though it is not read, from
    /// what would be found; edition 2015 finds none when examples are declared; of a name found
    /// twice, the `.rs` file; an odd path is none.
    #[test]
    fn declared_examples_take_their_names_and_files() {
        let files = [
            "examples/chat.rs",
            "examples/a.rs",
            "examples/a/main.rs",
            "notes.md",
        ];
        let of = |manifest: &str| {
            told(&of_package(&table(manifest), files))
                .into_iter()
                .map(|(name, file, _)| (name.to_owned(), file.to_owned()))
                .collect::<Vec<_>>()
        };
        let pair = |name: &str, file: &str| (name.to_owned(), file.to_owned());
        // The same file by another name: that name only.
        assert_eq!(
            of(
                "[package]\nedition = \"2021\"\n[[example]]\nname = \"talk\"\npath = \"examples/chat.rs\"\n"
            ),
            [pair("a", "examples/a.rs"), pair("talk", "examples/chat.rs")]
        );
        // The name, at a file outside: not the file of that name found.
        assert_eq!(
            of(
                "[package]\nedition = \"2021\"\n[[example]]\nname = \"chat\"\npath = \"../chat.rs\"\n"
            ),
            [pair("a", "examples/a.rs")]
        );
        // Edition 2015, as without one.
        assert_eq!(of("[package]\n[[example]]\nname = \"chat\"\n"), [pair(
            "chat",
            "examples/chat.rs"
        )]);
        for odd in [
            "/abs/x.rs",
            "examples\\..\\..\\x.rs",
            "C:\\x.rs",
            "notes.md",
        ] {
            assert_eq!(
                of(&format!(
                    "[package]\nautoexamples = false\n[[example]]\nname = \"x\"\npath = {odd:?}\n"
                )),
                [],
                "{odd}"
            );
        }
    }

    /// `required-features` of a target, as cargo metadata writes it.
    #[cfg(feature = "local")]
    #[test]
    fn a_target_tells_its_required_features() {
        let target: crate::data::metadata::Target = serde_json::from_str(
            r#"{"name":"tls","kind":["example"],"src_path":"/w/examples/tls.rs","required-features":["tls"]}"#,
        )
        .expect("a target");
        assert_eq!(&*target.required_features, [Box::from("tls")]);
    }

    /// Without cargo metadata, the files of `examples/` that Cargo would find and those that
    /// `[[example]]` names, not what starts with a dot nor a directory through a symlink
    /// (decided in M3e, decision 83).
    #[cfg(feature = "local")]
    #[test]
    fn the_files_that_may_be_examples_are_found_in_a_directory() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        // The package is below, so that `../x.rs` is a file outside it.
        std::fs::write(dir.path().join("x.rs"), "fn main() {}\n").expect("written");
        let root = &dir.path().join("pkg");
        for path in [
            "examples/chat.rs",
            "examples/echo/main.rs",
            "examples/echo/util.rs",
            "examples/notes.md",
            "examples/.hidden.rs",
            "examples/.dir/main.rs",
            "demos/tls.rs",
            "outside/main.rs",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            std::fs::write(path, "fn main() {}\n").expect("written");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("outside"), root.join("examples/linked"))
            .expect("linked");
        let manifest = table(
            "[package]\nname = \"demo\"\n[[example]]\nname = \"tls\"\npath = \"demos/tls.rs\"\n\
             [[example]]\nname = \"up\"\npath = \"../x.rs\"\n",
        );
        assert_eq!(files_in(root, &manifest), [
            "demos/tls.rs",
            "examples/chat.rs",
            "examples/echo/main.rs",
        ]);
    }

    /// A member's examples, as cargo metadata tells them, in its directory only.
    #[cfg(feature = "local")]
    #[test]
    fn a_members_examples_are_its_example_targets() {
        use std::path::{Path, PathBuf};

        use crate::data::metadata::Target;

        let target = |name: &str, kind: &str, path: &str, features: &[&str]| Target {
            name: name.into(),
            kind: vec![kind.into()].into(),
            src_path: PathBuf::from(path),
            required_features: features.iter().map(|&feature| Box::from(feature)).collect(),
        };
        let targets = [
            target("demo", "lib", "/w/demo/src/lib.rs", &[]),
            target("tls", "example", "/w/demo/examples/tls.rs", &["tls"]),
            target("chat", "example", "/w/demo/examples/chat/main.rs", &[]),
            target("far", "example", "/w/other/far.rs", &[]),
            target("h", "example", "/w/demo/.x/h.rs", &[]),
            target(
                "back",
                "example",
                "/w/demo/examples/../examples/back.rs",
                &[],
            ),
            target("md", "example", "/w/demo/examples/md.md", &[]),
        ];
        assert_eq!(told(&of_targets(&targets, Path::new("/w/x/../demo"))), [
            ("back", "examples/back.rs", vec![]),
            ("chat", "examples/chat/main.rs", vec![]),
            ("tls", "examples/tls.rs", vec!["tls"]),
        ]);
    }

    /// Without `autoexamples`, only what `[[example]]` names; a path outside the package, under a
    /// hidden directory, or not Rust is not one.
    #[test]
    fn only_the_declared_examples_without_autoexamples() {
        let manifest = table(
            "[package]\nname = \"demo\"\nautoexamples = false\n\n[[example]]\nname = \"echo\"\n\n\
             [[example]]\nname = \"up\"\npath = \"examples/../src/lib.rs\"\n\n[[example]]\nname = \"h\"\n\
             path = \"examples/.hidden.rs\"\n\n[[example]]\nname = \"lib\"\npath = \"src/lib.rs\"\n",
        );
        assert_eq!(told(&of_package(&manifest, FILES)), [
            ("echo", "examples/echo/main.rs", vec![]),
            ("lib", "src/lib.rs", vec![]),
        ]);
    }
}
