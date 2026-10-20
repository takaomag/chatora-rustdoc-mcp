//! Reads the output of `cargo tree --prefix indent --format '{p}\t{f}' --charset ascii`: each
//! line's package, features, depth and the kind of the edge from its parent (requirement R-MCP-3).
//!
//! `--prefix indent`, not `depth`: only the indented tree has the `[build-dependencies]` and
//! `[dev-dependencies]` headers that tell the kinds of the edges (cargo 1.99.1).

use std::io;

use semver::Version;

/// A line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Line {
    /// 0 for a root.
    pub depth: usize,
    /// The kind of the edge from the parent. [`Edge::Normal`] for a root.
    pub edge: Edge,
    /// The package.
    pub package: TreePackage,
    /// The features enabled on it.
    pub features: Box<[Box<str>]>,
    /// Whether it was shown above, with its dependencies there (` (*)`).
    pub repeated: bool,
}

/// The kind of a dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    /// A normal dependency.
    Normal,
    /// A build-dependency, under `[build-dependencies]`.
    Build,
    /// A dev-dependency, under `[dev-dependencies]`.
    Dev,
}

/// A package as `{p}` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct TreePackage {
    pub name: Box<str>,
    pub version: Version,
    /// Whether it is a proc-macro (` (proc-macro)`).
    pub proc_macro: bool,
    /// Where it comes from.
    pub source: TreeSource,
}

/// Where a package comes from, as `{p}` shows it (cargo 1.99.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum TreeSource {
    /// crates.io, which is not shown.
    CratesIo,
    /// Another registry, by the name the cargo settings give it: ``(registry `alt`)``.
    Registry(Box<str>),
    /// Anything else: the directory of a path dependency, or the URL of a git dependency with its
    /// reference and the first 8 digits of its commit (`https://h/r?branch=main#2c97c54a`).
    Other(Box<str>),
}

/// Reads the tree. An error names the first line that is not of the form asked for.
pub(super) fn parse(text: &str) -> io::Result<Vec<Line>> {
    let mut lines = Vec::new();
    // The kind of the edges to the children of the line above at each depth, which a header
    // changes.
    let mut edges: Vec<Edge> = Vec::new();
    for raw in text.lines() {
        // A blank line separates the roots.
        if raw.trim().is_empty() {
            continue;
        }
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, format!("{raw:?}"));
        let (levels, rest) = strip_levels(raw);
        let (depth, entry) = if let Some(entry) = rest
            .strip_prefix("|-- ")
            .or_else(|| rest.strip_prefix("`-- "))
        {
            (levels.saturating_add(1), entry)
        } else if let Some(edge) = header(rest) {
            // The children of the line at depth `levels` that follow are of this kind.
            edges.truncate(levels.saturating_add(1));
            *edges.get_mut(levels).ok_or_else(invalid)? = edge;
            continue;
        } else if levels == 0 {
            (0, rest)
        } else {
            return Err(invalid());
        };
        if depth > edges.len() {
            return Err(invalid());
        }
        edges.truncate(depth);
        let edge = depth
            .checked_sub(1)
            .and_then(|parent| edges.get(parent).copied())
            .unwrap_or(Edge::Normal);
        edges.push(Edge::Normal);
        lines.push(parse_entry(entry, depth, edge).ok_or_else(invalid)?);
    }
    Ok(lines)
}

/// The number of indentation levels before the rest of `line` (`|   ` and 4 spaces each).
fn strip_levels(line: &str) -> (usize, &str) {
    let mut levels: usize = 0;
    let mut rest = line;
    while let Some(after) = rest
        .strip_prefix("|   ")
        .or_else(|| rest.strip_prefix("    "))
    {
        levels = levels.saturating_add(1);
        rest = after;
    }
    (levels, rest)
}

/// The kind of the edges that a header line starts.
fn header(text: &str) -> Option<Edge> {
    match text {
        "[build-dependencies]" => Some(Edge::Build),
        "[dev-dependencies]" => Some(Edge::Dev),
        _ => None,
    }
}

/// Reads `{p}\t{f}`, with ` (*)` after it when the package was shown above, as the line at
/// `depth` under an edge of kind `edge`.
fn parse_entry(entry: &str, depth: usize, edge: Edge) -> Option<Line> {
    let (package, features) = entry.split_once('\t')?;
    // A package shown before has ` (*)` after its features; with no features, ` (*)` alone.
    let shown_before = features.strip_suffix(" (*)");
    let features = shown_before
        .unwrap_or(features)
        .split(',')
        .filter(|feature| !feature.is_empty())
        .map(Box::from)
        .collect();
    Some(Line {
        depth,
        edge,
        package: parse_package(package)?,
        features,
        repeated: shown_before.is_some(),
    })
}

/// Reads `{p}`: `name v1.2.3`, then ` (proc-macro)` and ` (<source>)`, if any.
fn parse_package(text: &str) -> Option<TreePackage> {
    let (name, after) = text.split_once(" v")?;
    let (version, rest) = after.split_once(' ').unwrap_or((after, ""));
    let version = version.parse().ok()?;
    let after_marker = rest.strip_prefix("(proc-macro)");
    let rest = after_marker.map_or(rest, str::trim_start);
    let source = if rest.is_empty() {
        TreeSource::CratesIo
    } else {
        let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
        inner
            .strip_prefix("registry `")
            .and_then(|name| name.strip_suffix('`'))
            .map_or_else(
                || TreeSource::Other(inner.into()),
                |name| TreeSource::Registry(name.into()),
            )
    };
    Some(TreePackage {
        name: name.into(),
        version,
        proc_macro: after_marker.is_some(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo tree --workspace -e normal,build,dev --prefix indent --format '{p}\t{f}'` of a
    /// workspace whose member `app` uses `shared` as each kind of dependency, the proc-macro `pm`,
    /// and a crate of another registry (cargo 1.99.1).
    const TREE: &str = "\
app v0.1.0 (/w/app)\t
|-- pm v0.1.0 (proc-macro) (/d/pm)\t
|   `-- pmdep v0.1.0 (/d/pmdep)\tx
|-- regdep v1.0.0 (registry `alt`)\t
`-- shared v0.1.0 (/d/shared)\tnormal
[build-dependencies]
|-- cc v1.2.3\t
|   [build-dependencies]
|   `-- find v0.1.0\t
`-- shared v0.1.0 (/d/shared)\tbuild
[dev-dependencies]
`-- shared v0.1.0 (/d/shared)\tdev,normal (*)

pm v0.1.0 (proc-macro) (/d/pm)\t (*)
";

    fn names_and_edges(lines: &[Line]) -> Vec<(usize, &str, Edge)> {
        lines
            .iter()
            .map(|line| (line.depth, &*line.package.name, line.edge))
            .collect()
    }

    #[test]
    fn headers_give_the_kinds_of_the_edges() {
        let lines = parse(TREE).expect("a tree");
        assert_eq!(names_and_edges(&lines), [
            (0, "app", Edge::Normal),
            (1, "pm", Edge::Normal),
            (2, "pmdep", Edge::Normal),
            (1, "regdep", Edge::Normal),
            (1, "shared", Edge::Normal),
            (1, "cc", Edge::Build),
            (2, "find", Edge::Build),
            (1, "shared", Edge::Build),
            (1, "shared", Edge::Dev),
            (0, "pm", Edge::Normal),
        ]);
    }

    #[test]
    fn entries_give_the_package_and_its_features() {
        let lines = parse(TREE).expect("a tree");
        assert!(lines[1].package.proc_macro);
        assert_eq!(lines[1].package.source, TreeSource::Other("/d/pm".into()));
        assert_eq!(lines[3].package.source, TreeSource::Registry("alt".into()));
        assert_eq!(lines[5].package.source, TreeSource::CratesIo);
        assert_eq!(lines[5].package.version, Version::new(1, 2, 3));
        assert_eq!(&*lines[8].features, [Box::from("dev"), Box::from("normal")]);
        assert!(lines[8].repeated);
        assert!(lines[9].repeated);
        assert!(lines[9].features.is_empty());
        assert!(!lines[0].repeated);
    }

    #[test]
    fn a_git_source_is_kept_as_shown() {
        let line = parse_entry(
            "dep v0.1.0 (https://h/r?branch=main#2c97c54a)\t",
            0,
            Edge::Normal,
        )
        .expect("an entry");
        assert_eq!(
            line.package.source,
            TreeSource::Other("https://h/r?branch=main#2c97c54a".into())
        );
    }

    #[test]
    fn a_line_that_is_not_of_the_tree_is_an_error() {
        parse("app 0.1.0\t\n").expect_err("no version");
        parse("app v0.1.0\n").expect_err("no features");
        parse("app v0.1.0\t\n        `-- dep v0.1.0\t\n").expect_err("no parent");
        parse("[build-dependencies]\n").expect_err("a header with no line above");
        parse("app v0.1.0\t\n[unknown-dependencies]\n").expect_err("an unknown header");
        parse("app v0.1.0 (/w/app\t\n").expect_err("an open source");
    }
}
