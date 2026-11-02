//! Which file is a package's readme, as its manifest tells (requirements §5.9 and R-SC-4, decided
//! in M3, P8, and M3e).
//!
//! The `readme` of `[package]` names it, relative to the package's root; `true` names
//! `README.md`, and `false` tells that it has none. Without the field, Cargo takes the first of
//! `README.md`, `README.txt` and `README` in the package's root, as for an old `.crate` whose
//! manifest Cargo did not write it into. Only a document is read: of a document's extension, or
//! `README` without one, in any case, and not under a hidden directory (decided in M3e,
//! decision 76).
//!
//! These read the manifest that Cargo wrote into a `.crate`, which has the readme of a workspace
//! that the package inherits copied into it, and its path made relative to the package. The
//! manifest of a checkout or of a workspace's member, as written, may inherit it
//! (`readme.workspace = true`) or name one above the package (`../README.md`): `cargo metadata`
//! tells those.

use std::path::Path;

/// The names that Cargo takes as the readme, in order, when the manifest names none.
pub const DEFAULT_NAMES: [&str; 3] = ["README.md", "README.txt", "README"];

/// The extensions of a document, which alone a readme may have (requirement R-SC-4).
const DOCUMENT_EXTENSIONS: [&str; 5] = ["md", "markdown", "txt", "rst", "adoc"];

/// What the manifest tells of the readme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declared {
    /// The file it names, relative to the package's root, as written.
    Path(Box<str>),
    /// None named: the default names are looked for.
    Default,
    /// `readme = false`: the package has none.
    Disabled,
}

/// Why a package has no readme that may be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoReadme {
    /// Its manifest tells that it has none (`readme = false`).
    #[error("its manifest tells that it has no readme (`readme = false`)")]
    Disabled,
    /// Its manifest names none, and none of the default names is in the package.
    #[error("it has no README.md, README.txt or README, and its manifest names none")]
    NoDefault,
    /// The file that its manifest names is not in the package.
    #[error("its manifest names `{0}` as its readme, which is not in the package")]
    Missing(Box<str>),
    /// The file that its manifest names is not a document, which is not read (requirement
    /// R-SC-4).
    #[error(
        "its manifest names `{0}` as its readme, which is not read: only a document (.md, \
         .markdown, .txt, .rst or .adoc, or README) is"
    )]
    NotADocument(Box<str>),
    /// The file that its manifest names is under a hidden directory, as `.github`, which is not
    /// read (requirement R-SC-4, decided in M3e, decision 76).
    #[error(
        "its manifest names `{0}` as its readme, which is not read: no file under a directory \
         whose name starts with a dot is"
    )]
    Hidden(Box<str>),
}

/// What the manifest `table` tells of the readme. A value of another type than a string or a bool
/// counts as none named, as Cargo would not have published it.
#[must_use]
pub fn declared(table: &toml::Table) -> Declared {
    match table
        .get("package")
        .and_then(|package| package.get("readme"))
    {
        Some(toml::Value::String(path)) => Declared::Path(path.as_str().into()),
        Some(toml::Value::Boolean(true)) => Declared::Path("README.md".into()),
        Some(toml::Value::Boolean(false)) => Declared::Disabled,
        _ => Declared::Default,
    }
}

/// The readme that `declared` tells, relative to the package's root with `/`, of the files of the
/// package, which `has` tells by such a path.
///
/// # Errors
///
/// Returns why there is none that may be read: the manifest tells that there is none, the file
/// that it names is not in the package, not a document or under a hidden directory, or none of
/// the default names is.
pub fn chosen(declared: &Declared, has: impl Fn(&str) -> bool) -> Result<Box<str>, NoReadme> {
    match declared {
        Declared::Disabled => Err(NoReadme::Disabled),
        Declared::Default => DEFAULT_NAMES
            .into_iter()
            .find(|&name| has(name))
            .map(Box::from)
            .ok_or(NoReadme::NoDefault),
        Declared::Path(path) => {
            let Some(relative) = relative(path) else {
                return Err(NoReadme::Missing(path.clone()));
            };
            if !is_document(Path::new(&relative)) {
                return Err(NoReadme::NotADocument(path.clone()));
            }
            if relative.split('/').any(|part| part.starts_with('.')) {
                return Err(NoReadme::Hidden(path.clone()));
            }
            if has(&relative) {
                Ok(relative.into())
            } else {
                Err(NoReadme::Missing(path.clone()))
            }
        },
    }
}

/// `path`, relative to the package's root, with `/` and without `.`; `None` when it leaves the
/// root or is not relative, as with a drive (`C:`), which no file of the package is at.
fn relative(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        if part.contains(':') {
            return None;
        }
        match part {
            "" if parts.is_empty() && path.starts_with(['/', '\\']) => return None,
            "" | "." => {},
            ".." => return None,
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Whether the readme at `path` is a document (requirement R-SC-4, decided in M3, P8): of the
/// extensions of one, or named `README` without one, in any case.
#[must_use]
pub fn is_document(path: &Path) -> bool {
    path.extension().map_or_else(
        || {
            path.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("README"))
        },
        |extension| {
            DOCUMENT_EXTENSIONS
                .iter()
                .any(|document| extension.eq_ignore_ascii_case(document))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        toml::from_str(text).expect("a manifest")
    }

    /// The field as a path, `true`, `false` or absent, and of another type.
    #[test]
    fn the_manifest_tells_the_readme() {
        assert_eq!(
            declared(&table("[package]\nreadme = \"docs/README.md\"")),
            Declared::Path("docs/README.md".into())
        );
        assert_eq!(
            declared(&table("[package]\nreadme = true")),
            Declared::Path("README.md".into())
        );
        assert_eq!(
            declared(&table("[package]\nreadme = false")),
            Declared::Disabled
        );
        assert_eq!(
            declared(&table("[package]\nname = \"a\"")),
            Declared::Default
        );
        assert_eq!(declared(&table("[package]\nreadme = 1")), Declared::Default);
        assert_eq!(declared(&table("[workspace]")), Declared::Default);
    }

    /// The file named, if it is in the package and a document; else the first default name in
    /// the package; and why there is none.
    #[test]
    fn the_readme_is_chosen_of_the_package_s_files() {
        let files = [
            "README.txt",
            "README",
            "docs/intro.md",
            "README.mkd",
            "notes.html",
        ];
        let has = |name: &str| files.contains(&name);
        let path = |path: &str| chosen(&Declared::Path(path.into()), has);
        assert_eq!(path("./docs//intro.md"), Ok("docs/intro.md".into()));
        assert_eq!(path("docs\\intro.md"), Ok("docs/intro.md".into()));
        assert_eq!(path("README"), Ok("README".into()));
        assert_eq!(
            chosen(&Declared::Path("README.txt".into()), has),
            Ok("README.txt".into())
        );
        // Under a hidden directory, though in the package (decided in M3e, decision 76).
        let hidden = |name: &str| name == ".github/README.md";
        assert_eq!(
            chosen(&Declared::Path(".github/README.md".into()), hidden),
            Err(NoReadme::Hidden(".github/README.md".into()))
        );
        // Not a document comes first, whether it is there or not.
        assert_eq!(
            path("docs/readme.html"),
            Err(NoReadme::NotADocument("docs/readme.html".into()))
        );
        for odd in ["", ".", "C:\\x\\README.md", "c:README.md"] {
            assert_eq!(path(odd), Err(NoReadme::Missing(odd.into())), "{odd}");
        }
        // Cargo copies an inherited readme into the `.crate`; one outside it is not read.
        assert_eq!(
            path("../README.md"),
            Err(NoReadme::Missing("../README.md".into()))
        );
        assert_eq!(
            path("/etc/README.md"),
            Err(NoReadme::Missing("/etc/README.md".into()))
        );
        assert_eq!(
            path("README.md"),
            Err(NoReadme::Missing("README.md".into()))
        );
        assert_eq!(
            path("README.mkd"),
            Err(NoReadme::NotADocument("README.mkd".into()))
        );
        assert_eq!(
            path("notes.html"),
            Err(NoReadme::NotADocument("notes.html".into()))
        );
        // `README.md` is not there: the next default name is.
        assert_eq!(chosen(&Declared::Default, has), Ok("README.txt".into()));
        assert_eq!(
            chosen(&Declared::Default, |name| name == "readme.md"),
            Err(NoReadme::NoDefault)
        );
        assert_eq!(chosen(&Declared::Disabled, has), Err(NoReadme::Disabled));
    }

    /// The extensions of a document in any case, and `README` without one.
    #[test]
    fn a_readme_is_a_document() {
        for (path, is) in [
            ("README.md", true),
            ("docs/Guide.MARKDOWN", true),
            ("README.rst", true),
            ("readme", true),
            ("README.mkd", false),
            ("LICENSE", false),
            (".env", false),
            ("README.html", false),
        ] {
            assert_eq!(is_document(Path::new(path)), is, "{path}");
        }
    }
}
