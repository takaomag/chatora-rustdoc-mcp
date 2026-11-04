//! Which file of a package is its changelog (decided in M3g, decision 118).
//!
//! No manifest field names it, so it is found by its name, in the package's root only: the first
//! of [`NAMES`] in their order, whatever their case, of a document's extension or none.

use std::path::Path;

/// The names of a changelog, the most likely first; those of a rank are as likely as each other.
pub const NAMES: [&[&str]; 5] = [
    &["CHANGELOG"],
    &["CHANGES"],
    &["RELEASES", "RELEASE-NOTES", "RELEASE_NOTES"],
    &["HISTORY"],
    &["NEWS"],
];

/// The extensions of a changelog, Markdown's first; none is one too.
const EXTENSIONS: [&str; 5] = ["md", "markdown", "txt", "rst", "adoc"];

/// The changelog among the files named `names` in a package's root: of the first rank of
/// [`NAMES`] that one has, the one of the first of a document's extensions (Markdown's first), then
/// none, then by name.
#[must_use]
pub fn chosen<'n>(names: impl IntoIterator<Item = &'n str>) -> Option<&'n str> {
    names
        .into_iter()
        .filter_map(|name| Some((order_of(name)?, name)))
        .min()
        .map(|(_, name)| name)
}

/// Where `name` comes in the order of [`chosen`]: its rank, then its extension; `None` when it is
/// not a changelog's.
fn order_of(name: &str) -> Option<(usize, usize)> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, Some(extension)),
        None => (name, None),
    };
    let rank = NAMES
        .iter()
        .position(|names| names.iter().any(|known| known.eq_ignore_ascii_case(stem)))?;
    let extension = match extension {
        None => EXTENSIONS.len(),
        Some(extension) => EXTENSIONS
            .iter()
            .position(|known| known.eq_ignore_ascii_case(extension))?,
    };
    Some((rank, extension))
}

/// Whether the file at `path` may be a changelog by its name: as [`chosen`] takes it.
#[must_use]
pub fn is_changelog(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(order_of)
        .is_some()
}

/// Whether the changelog `name` is Markdown, whose headings tell its releases; another is read as
/// lines.
#[must_use]
pub fn is_markdown(name: &str) -> bool {
    name.rsplit_once('.').is_none_or(|(_, extension)| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_changelog_is_chosen_by_its_name_then_its_extension() {
        let chosen_of = |names: &[&'static str]| chosen(names.iter().copied());
        assert_eq!(
            chosen_of(&["README.md", "Changes.md", "CHANGELOG.md", "release.toml"]),
            Some("CHANGELOG.md")
        );
        assert_eq!(
            chosen_of(&["changelog", "Changelog.md"]),
            Some("Changelog.md")
        );
        assert_eq!(
            chosen_of(&["CHANGELOG.txt", "CHANGELOG"]),
            Some("CHANGELOG.txt")
        );
        assert_eq!(chosen_of(&["RELEASES.md", "NEWS"]), Some("RELEASES.md"));
        assert_eq!(chosen_of(&["RELEASE_NOTES.md"]), Some("RELEASE_NOTES.md"));
        // Not a document, nor another name.
        assert_eq!(
            chosen_of(&["release.toml", "CHANGELOG.sh", "RELEASE.txt", "src"]),
            None
        );
        assert!(is_changelog(Path::new("pkg/HISTORY.rst")));
        assert!(!is_changelog(Path::new("pkg/CHANGELOG.html")));
    }

    #[test]
    fn markdown_is_told_by_the_extension() {
        assert!(is_markdown("CHANGELOG.md") && is_markdown("CHANGES"));
        assert!(!is_markdown("NEWS.txt") && !is_markdown("HISTORY.rst"));
    }
}
