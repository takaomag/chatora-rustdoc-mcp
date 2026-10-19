//! The documentation of an item as the responses show it: split into its sections, with the
//! hidden lines of doctests left out (requirements R-OUT-6 and R-OUT-7).

/// Documentation split at its headings.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Docs {
    /// What comes before the first heading.
    pub intro: String,
    /// The sections, in order.
    pub sections: Vec<SectionText>,
}

/// One section of the documentation.
#[derive(Debug, PartialEq, Eq)]
pub struct SectionText {
    /// The heading, without its `#` marks (`Examples`).
    pub heading: String,
    /// The text under the heading, up to the next heading of any level.
    pub text: String,
}

/// Splits `docs` at its ATX headings (`# Examples`), which are not inside code blocks, and leaves
/// out the hidden lines of the doctests in it.
#[must_use]
pub fn split(docs: &str) -> Docs {
    let docs = without_hidden_lines(docs);
    let mut split = Docs::default();
    let mut fence: Option<&str> = None;
    for line in docs.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(marker) = fence_marker(trimmed) {
            fence = match fence {
                Some(open) if marker == open => None,
                Some(open) => Some(open),
                None => Some(marker),
            };
        } else if fence.is_none()
            && let Some(heading) = heading(trimmed)
        {
            split.sections.push(SectionText {
                heading: heading.to_owned(),
                text: String::new(),
            });
            continue;
        }
        let current = match split.sections.last_mut() {
            Some(section) => &mut section.text,
            None => &mut split.intro,
        };
        current.push_str(line);
    }
    split.intro = split.intro.trim().to_owned();
    for section in &mut split.sections {
        section.text = section.text.trim().to_owned();
    }
    split
}

/// The fence that `line` opens or closes a code block with: three or more backticks or tildes.
fn fence_marker(line: &str) -> Option<&str> {
    ["```", "~~~"]
        .into_iter()
        .find(|fence| line.starts_with(fence))
}

/// The text of `line` if it is an ATX heading: one to six `#`, then a space.
fn heading(line: &str) -> Option<&str> {
    let level = line.bytes().take_while(|&byte| byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = line.get(level..)?;
    let text = rest.strip_prefix(' ')?.trim();
    Some(text.trim_end_matches('#').trim_end())
}

/// `docs` without the hidden lines of its Rust code blocks (requirement R-OUT-6).
///
/// A line that is `#` alone or starts with `# ` is left out, and `##` at its start becomes `#`. A
/// code block is
/// Rust unless its info string names another language; `should_panic` and `compile_fail` stay
/// in the info string, where they show.
#[must_use]
pub fn without_hidden_lines(docs: &str) -> String {
    let mut out = String::with_capacity(docs.len());
    let mut fence: Option<(&str, bool)> = None;
    for line in docs.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(marker) = fence_marker(trimmed) {
            fence = match fence {
                Some((open, _)) if open == marker => None,
                Some(open) => Some(open),
                None => Some((
                    marker,
                    is_rust(trimmed.get(marker.len()..).unwrap_or_default()),
                )),
            };
            out.push_str(line);
            continue;
        }
        if let Some((_, true)) = fence {
            let body = trimmed.trim_end_matches(['\n', '\r']);
            if body == "#" || body.starts_with("# ") {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("##") {
                let indent = line.len().saturating_sub(trimmed.len());
                out.push_str(line.get(..indent).unwrap_or_default());
                out.push('#');
                out.push_str(rest);
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

/// Whether a code block with this info string is Rust, as rustdoc decides it: no language, or
/// only the attributes of a doctest.
fn is_rust(info: &str) -> bool {
    info.split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .all(|word| {
            matches!(
                word,
                "rust"
                    | "ignore"
                    | "no_run"
                    | "should_panic"
                    | "compile_fail"
                    | "test_harness"
                    | "standalone_crate"
            ) || word.starts_with("edition")
                || word.starts_with("ignore-")
                || word.starts_with('{')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_are_split_at_their_headings_outside_code() {
        let docs = "Locks it.\n\nMore.\n\n# Examples\n\n```\n# use x::Y;\nlet y = Y;\n```\n\n## Panics\n\nWhen poisoned.\n";
        let split = split(docs);
        assert_eq!(split.intro, "Locks it.\n\nMore.");
        assert_eq!(split.sections, [
            SectionText {
                heading: "Examples".to_owned(),
                text: "```\nlet y = Y;\n```".to_owned(),
            },
            SectionText {
                heading: "Panics".to_owned(),
                text: "When poisoned.".to_owned(),
            },
        ]);
    }

    #[test]
    fn hidden_lines_are_left_out_of_rust_code_only() {
        let docs = "```should_panic\n#\n# fn main() {\n##[derive(Debug)]\nstruct S;\n# }\n```\n\n```text\n# not hidden\n```\n";
        assert_eq!(
            without_hidden_lines(docs),
            "```should_panic\n#[derive(Debug)]\nstruct S;\n```\n\n```text\n# not hidden\n```\n"
        );
    }

    #[test]
    fn a_hash_in_code_is_not_a_heading() {
        let split = split("Intro.\n\n```sh\n# a comment\n```\n");
        assert!(split.sections.is_empty(), "{split:?}");
    }
}
