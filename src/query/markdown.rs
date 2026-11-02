//! The headings of a Markdown document, as a readme's, and where to cut it into pages at them
//! (requirement §5.9, decided in M3e, decisions 63 to 65).
//!
//! The headings are those of `CommonMark` that a readme uses: ATX (`# Title`) and setext (a
//! paragraph underlined with `===` or `---`), not inside a fenced or an indented code block. The
//! lines of a code block are left as they are: a `# comment` of a shell is not a hidden line of a
//! doctest here.

/// A heading of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Its level, 1 to 6 (setext ones are 1 or 2).
    pub level: u8,
    /// Its text, without its marks.
    pub text: String,
    /// The line it starts at, from 1.
    pub line: usize,
}

/// A fence that opened a code block: its character, and how many.
#[derive(Debug, Clone, Copy)]
struct Fence {
    marker: u8,
    count: usize,
}

/// A paragraph being read, which an underline would make a setext heading: the line it starts at,
/// from 1, and its text.
#[derive(Debug)]
struct Paragraph {
    line: usize,
    text: String,
}

/// The headings of the Markdown document `text`, in order.
#[must_use]
pub fn outline(text: &str) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut fence: Option<Fence> = None;
    let mut paragraph: Option<Paragraph> = None;
    // In an HTML comment, which may hold what looks like a heading.
    let mut comment = false;
    // After a list item, a block quote, an HTML block or a table's row: a line of text goes on
    // it, lazily, and no underline makes a heading of it.
    let mut lazy = false;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for (index, raw) in text.lines().enumerate() {
        let number = index.saturating_add(1);
        let (indent, line) = indented(raw);
        if comment {
            comment = !line.contains("-->");
            continue;
        }
        if let Some(open) = fence {
            if indent <= 3 && closes(line, open) {
                fence = None;
            }
            continue;
        }
        if indent <= 3
            && let Some(opened) = opens(line)
        {
            fence = Some(opened);
            paragraph = None;
            continue;
        }
        if is_blank(line) {
            paragraph = None;
            lazy = false;
            continue;
        }
        if indent <= 3
            && let Some(rest) = line.strip_prefix("<!--")
        {
            comment = !rest.contains("-->");
            paragraph = None;
            lazy = true;
            continue;
        }
        // Indented code, unless it goes on a paragraph.
        if indent >= 4 && paragraph.is_none() {
            continue;
        }
        if indent <= 3 {
            if let Some(level) = underline(line)
                && let Some(Paragraph { line, text }) = paragraph.take()
            {
                headings.push(Heading { level, text, line });
                continue;
            }
            if is_break(line) {
                paragraph = None;
                continue;
            }
            if let Some((level, text)) = atx(line) {
                headings.push(Heading {
                    level,
                    text: text.to_owned(),
                    line: number,
                });
                paragraph = None;
                continue;
            }
            if starts_a_block(line) {
                paragraph = None;
                lazy = true;
                continue;
            }
        }
        if lazy {
            continue;
        }
        match &mut paragraph {
            Some(paragraph) => {
                paragraph.text.push(' ');
                paragraph.text.push_str(line.trim_matches([' ', '\t']));
            },
            None => {
                paragraph = Some(Paragraph {
                    line: number,
                    text: line.trim_matches([' ', '\t']).to_owned(),
                });
            },
        }
    }
    headings
}

/// Whether `line` is blank: spaces and tabs only, as `CommonMark` has it.
fn is_blank(line: &str) -> bool {
    line.bytes().all(|byte| byte == b' ' || byte == b'\t')
}

/// How many columns `line` is indented by, a tab to the next multiple of 4, and the rest.
fn indented(line: &str) -> (usize, &str) {
    let mut columns = 0_usize;
    for (at, byte) in line.bytes().enumerate() {
        match byte {
            b' ' => columns = columns.saturating_add(1),
            b'\t' => {
                columns = columns
                    .saturating_add(1)
                    .checked_next_multiple_of(4)
                    .unwrap_or(usize::MAX);
            },
            _ => return (columns, line.get(at..).unwrap_or_default()),
        }
    }
    (columns, "")
}

/// The fence that `line` opens a code block with: three or more backticks or tildes, of which a
/// backtick fence's info string has none.
fn opens(line: &str) -> Option<Fence> {
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let count = line.bytes().take_while(|&byte| byte == marker).count();
    let info = line.get(count..).unwrap_or_default();
    (count >= 3 && !(marker == b'`' && info.contains('`'))).then_some(Fence { marker, count })
}

/// Whether `line` closes the code block that `open` opened: as many of its character or more, and
/// nothing else.
fn closes(line: &str, open: Fence) -> bool {
    let count = line.bytes().take_while(|&byte| byte == open.marker).count();
    count >= open.count && line.get(count..).is_some_and(|rest| rest.trim().is_empty())
}

/// The level of the setext heading that `line` underlines: 1 for `=`, 2 for `-`.
fn underline(line: &str) -> Option<u8> {
    let line = line.trim_end();
    let marker = *line.as_bytes().first()?;
    let level = match marker {
        b'=' => 1,
        b'-' => 2,
        _ => return None,
    };
    line.bytes().all(|byte| byte == marker).then_some(level)
}

/// Whether `line` is a thematic break: three or more of `-`, `*` or `_`, the same, with spaces.
fn is_break(line: &str) -> bool {
    let mut marks = line.bytes().filter(|byte| !matches!(byte, b' ' | b'\t'));
    let Some(first) = marks.next() else {
        return false;
    };
    matches!(first, b'-' | b'*' | b'_')
        && line
            .bytes()
            .filter(|byte| !matches!(byte, b' ' | b'\t'))
            .try_fold(0_usize, |count, byte| {
                (byte == first).then(|| count.saturating_add(1))
            })
            .is_some_and(|count| count >= 3)
}

/// The level and the text of `line` if it is an ATX heading: one to six `#`, then a space or
/// nothing, without a closing sequence of `#`.
fn atx(line: &str) -> Option<(u8, &str)> {
    let count = line.bytes().take_while(|&byte| byte == b'#').count();
    let level = u8::try_from(count)
        .ok()
        .filter(|level| (1..=6).contains(level))?;
    let rest = line.get(count..)?;
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let text = rest.trim();
    // A closing sequence: `#`s after a space, or the whole text.
    let without = text.trim_end_matches('#');
    let text = if without.is_empty() {
        without
    } else if without.ends_with([' ', '\t']) {
        without.trim_end()
    } else {
        text
    };
    Some((level, text))
}

/// Whether `line` starts a block that no underline makes a heading of: a list item, a block
/// quote, a thematic break, a table's row or an HTML block.
fn starts_a_block(line: &str) -> bool {
    let bytes = line.as_bytes();
    let first = bytes.first().copied().unwrap_or_default();
    if matches!(first, b'>' | b'<' | b'|') {
        return true;
    }
    if matches!(first, b'-' | b'*' | b'+')
        && bytes
            .get(1)
            .is_none_or(|&next| next == b' ' || next == b'\t')
    {
        return true;
    }
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    (1..=9).contains(&digits)
        && matches!(bytes.get(digits), Some(b'.' | b')'))
        && bytes
            .get(digits.saturating_add(1))
            .is_none_or(|&next| next == b' ' || next == b'\t')
}

/// Where the page from the line `from` (from 0) ends, exclusive, at most `max` characters long.
///
/// The lines are of `lengths` characters each, their line ends among them. The page ends at the
/// start of a section (a line of `starts`, sorted, from 0) or at the end, the last that fits;
/// else, when the section is longer, after the last line that fits, one line at least, which the
/// caller cuts if it alone is longer. `from` past the end is the end.
///
/// `lengths` are of the lines as [`str::lines`] splits the text, which [`outline`] numbers from 1:
/// a heading at `line` starts a section at `line - 1`.
#[must_use]
pub fn page_end(lengths: &[usize], starts: &[usize], from: usize, max: usize) -> usize {
    let total = lengths.len();
    let from = from.min(total);
    let mut used = 0_usize;
    let mut end = from;
    let mut at_boundary = None;
    for &length in lengths.iter().skip(from) {
        let next = used.saturating_add(length);
        if next > max {
            break;
        }
        used = next;
        end = end.saturating_add(1);
        if end == total || starts.binary_search(&end).is_ok() {
            at_boundary = Some(end);
        }
    }
    at_boundary.unwrap_or_else(|| end.max(from.saturating_add(1)).min(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(text: &str) -> Vec<(u8, String, usize)> {
        outline(text)
            .into_iter()
            .map(|heading| (heading.level, heading.text, heading.line))
            .collect()
    }

    /// ATX and setext headings, with their lines; none in a code block, nor what looks like one in
    /// a list, a quote or a table.
    #[test]
    fn the_headings_are_those_outside_code() {
        let text = "\
# Tokio #

A runtime.

Getting started
===============

```sh
# not a heading
cargo add tokio
```

~~~~
## nor this
~~~
~~~~

    # indented code

## Example ##
### C# and F#

- a list item
---
> quoted
---
***
Not a heading
* * *
| a | b |
|---|---|

Usage
of it
-----
#hashtag
####### seven
";
        assert_eq!(told(text), [
            (1, "Tokio".to_owned(), 1),
            (1, "Getting started".to_owned(), 5),
            (2, "Example".to_owned(), 20),
            (3, "C# and F#".to_owned(), 21),
            (2, "Usage of it".to_owned(), 33),
        ]);
    }

    /// An empty heading, a closing sequence alone, and a fence that a longer one closes.
    #[test]
    fn odd_headings_are_read_as_commonmark_does() {
        assert_eq!(told("#\n## ##\n#\tTabbed\n \t    # tab-indented code"), [
            (1, String::new(), 1),
            (2, String::new(), 2),
            (1, "Tabbed".to_owned(), 3),
        ]);
        // A backtick fence's info string has no backtick: this is text, then a heading.
        assert_eq!(told("``` a`b\n# Heading"), [(1, "Heading".to_owned(), 2)]);
        // Closed by a longer fence of its character only.
        assert_eq!(told("```\n~~~\n# no\n`````\n# yes"), [(
            1,
            "yes".to_owned(),
            5
        )]);
        // Indented by up to 3, the closing one too; one never closed goes to the end.
        assert_eq!(told("   ```\n# no\n  ```\n# yes\n~~~\n# no"), [(
            1,
            "yes".to_owned(),
            4
        )]);
        // `#5` and an escaped `#` are text.
        assert_eq!(told("#5 bolt\n\\# not\n"), []);
    }

    /// What a readme has besides: a byte order mark, line ends of Windows, an HTML comment over
    /// lines, and front matter or text that goes on a list lazily before a thematic break.
    #[test]
    fn a_readme_s_other_lines_are_not_headings() {
        assert_eq!(told("\u{feff}# Title\r\n\r\nText\r\n----\r\n"), [
            (1, "Title".to_owned(), 1),
            (2, "Text".to_owned(), 3),
        ]);
        let text = "\
<!--
## Hidden
-->
<!-- one line -->
# Shown
- item
lazily
---
<div align=\"center\">
badges
===
</div>

\u{3000}
After
=====
";
        assert_eq!(told(text), [
            (1, "Shown".to_owned(), 5),
            (1, "\u{3000} After".to_owned(), 14),
        ]);
    }

    /// A page ends at the start of the last section that fits, or at the end; a section longer
    /// than the page is cut after the last line that fits, one line at least.
    #[test]
    fn a_page_ends_at_a_heading() {
        // Sections start at lines 0, 3 and 5.
        let lengths = [10, 10, 10, 10, 10, 10, 10];
        let starts = [0, 3, 5];
        assert_eq!(page_end(&lengths, &starts, 0, 70), 7);
        assert_eq!(page_end(&lengths, &starts, 0, 69), 5);
        assert_eq!(page_end(&lengths, &starts, 0, 49), 3);
        // The first section alone is longer.
        assert_eq!(page_end(&lengths, &starts, 0, 25), 2);
        assert_eq!(page_end(&lengths, &starts, 0, 5), 1);
        assert_eq!(page_end(&lengths, &starts, 3, 40), 7);
        assert_eq!(page_end(&lengths, &starts, 3, 39), 5);
        // No heading: the lines that fit.
        assert_eq!(page_end(&lengths, &[], 1, 35), 4);
        assert_eq!(page_end(&lengths, &[], 7, 35), 7);
        assert_eq!(page_end(&lengths, &[], 9, 35), 7);
        // The line after a section's start is longer than the page: that one line.
        assert_eq!(page_end(&[10, 100], &[0, 1], 0, 50), 1);
        assert_eq!(page_end(&[10, 100], &[0, 1], 1, 50), 2);
        assert_eq!(page_end(&[], &[], 0, 35), 0);
    }
}
