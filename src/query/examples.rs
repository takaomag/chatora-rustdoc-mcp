//! The examples in a crate's documentation, and how well each matches an item asked for.
//!
//! They are the Rust code blocks of the docs of its public items (requirement §5.9, decided in M3,
//! P13, and M3e, decisions 69 to 71 and 74).
//!
//! A block is Rust as rustdoc decides it: no language, or only the attributes of a doctest. The
//! hidden lines of a doctest are left out, as `get_item` leaves them out (requirement R-OUT-6), and
//! its attributes (`no_run`, `ignore`, `should_panic`, `compile_fail`, …) told. An item matches
//! by the words of the code: its name, and the type, trait or enum that it is a member of, as
//! identifiers (P13: by the text, no type is resolved).

use std::cmp::Ordering;

use rustdoc_types::Id;

use super::{docs, search, view::CrateView};

/// A Rust code block of the docs of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocBlock {
    /// The item whose docs have it, by its canonical path.
    pub item: Box<str>,
    /// The item's id, to tell where it is defined.
    pub id: Id,
    /// The attributes of the doctest, as its info string has them (`no_run`).
    pub attributes: Box<[Box<str>]>,
    /// The code, without the hidden lines of the doctest.
    pub code: String,
}

/// An item asked for, whose examples come first.
#[derive(Debug, Clone, Copy)]
pub struct Asked<'a> {
    /// Its id, whose docs are its own examples: not by its path, which two items may share, as a
    /// module and a macro.
    pub id: Id,
    /// Its canonical path.
    pub path: &'a str,
    /// Its name (`lock`).
    pub name: &'a str,
    /// The type, trait or enum that it is a member of (`Mutex`), if it is one.
    pub parent: Option<&'a str>,
}

/// How well an example matches the item asked for, the best first (decided in M3e, decision 71).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// Of the item's own docs.
    Own,
    /// It names the item and the type it is a member of, or the item if it is a member of none.
    Both,
    /// It names the item, not the type it is a member of.
    Name,
    /// It does not name the item.
    Other,
}

/// The Rust code blocks of the docs of the crate's root and of its public items, the root's first,
/// then by the depth of the path and the path.
#[must_use]
pub fn doc_blocks(view: &CrateView<'_>) -> Vec<DocBlock> {
    let mut blocks = Vec::new();
    let root = view.root();
    if let Some(item) = view.item(root)
        && let Some(name) = item.name.as_deref()
    {
        push_blocks(&mut blocks, name, root, item.docs.as_deref());
    }
    // The docs alone, as archived: not every item taken out.
    for (id, _, path) in search::public_items(view.search_index()) {
        push_blocks(&mut blocks, path, id, view.docs(id));
    }
    // The root's are the first already; a stable sort keeps the order of the blocks of one item.
    blocks.sort_by(|a, b| by_path(&a.item, &b.item));
    blocks
}

/// Orders paths by their depth, then by their text.
fn by_path(a: &str, b: &str) -> Ordering {
    (a.matches("::").count(), a).cmp(&(b.matches("::").count(), b))
}

/// Appends the Rust code blocks of `docs`, of the item `id` at `path`, to `blocks`.
fn push_blocks(blocks: &mut Vec<DocBlock>, path: &str, id: Id, docs: Option<&str>) {
    let Some(docs) = docs else {
        return;
    };
    for (info, code) in fenced(docs) {
        if !docs::is_rust(info) {
            continue;
        }
        let attributes = info
            .split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|word| !word.is_empty() && *word != "rust")
            .map(Box::from)
            .collect();
        blocks.push(DocBlock {
            item: path.into(),
            id,
            attributes,
            code: without_hidden(&code),
        });
    }
}

/// The fenced code blocks of the Markdown `docs`: each one's info string and its lines, without
/// as much indentation as the opening fence has, as in a list. A fence is indented by 3 at most;
/// one that is not closed goes to the end, as rustdoc reads it.
fn fenced(docs: &str) -> Vec<(&str, String)> {
    // The fence's character, how many, its indentation, the info string, and the lines so far.
    let mut blocks = Vec::new();
    let mut open: Option<(char, usize, usize, &str, String)> = None;
    for line in docs.lines() {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len().saturating_sub(trimmed.len());
        let run = |fence: char| trimmed.chars().take_while(|&c| c == fence).count();
        if let Some((fence, count, opened_at, info, mut code)) = open.take() {
            let closing = run(fence);
            let rest = trimmed.get(closing..).unwrap_or_default();
            if indent <= 3 && closing >= count && rest.trim().is_empty() {
                blocks.push((info, code));
            } else {
                // As much of the indentation as the opening fence's, of spaces.
                let strip = indent.min(opened_at);
                code.push_str(line.get(strip..).unwrap_or(line));
                code.push('\n');
                open = Some((fence, count, opened_at, info, code));
            }
            continue;
        }
        if indent > 3 {
            continue;
        }
        for fence in ['`', '~'] {
            let count = run(fence);
            if count >= 3 {
                let info = trimmed.get(count..).unwrap_or_default().trim();
                if !(fence == '`' && info.contains('`')) {
                    open = Some((fence, count, indent, info, String::new()));
                }
                break;
            }
        }
    }
    if let Some((_, _, _, info, code)) = open {
        blocks.push((info, code));
    }
    blocks
}

/// `code` without the hidden lines of a doctest: `#` alone or `# ` at the start of a line is left
/// out, and `##` becomes `#` (requirement R-OUT-6).
fn without_hidden(code: &str) -> String {
    let mut shown = String::with_capacity(code.len());
    for line in code.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let body = trimmed.trim_end_matches(['\n', '\r']);
        if body == "#" || body.starts_with("# ") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("##") {
            let indent = line.len().saturating_sub(trimmed.len());
            shown.push_str(line.get(..indent).unwrap_or_default());
            shown.push('#');
            shown.push_str(rest);
            continue;
        }
        shown.push_str(line);
    }
    shown
}

/// How well `code`, an example in the docs of the item `of` (`None` for a file of `examples/`),
/// matches `asked` (decided in M3, P13, and M3e, decision 71).
#[must_use]
pub fn rank(asked: &Asked<'_>, of: Option<Id>, code: &str) -> Rank {
    if of == Some(asked.id) {
        return Rank::Own;
    }
    if !has_word(code, asked.name) {
        return Rank::Other;
    }
    match asked.parent {
        Some(parent) if !has_word(code, parent) => Rank::Name,
        Some(_) | None => Rank::Both,
    }
}

/// Whether `text` has `word` as an identifier: not within a longer one.
#[must_use]
pub fn has_word(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        let before = text.get(..at).and_then(|before| before.chars().next_back());
        let after = text
            .get(at.saturating_add(word.len())..)
            .and_then(|after| after.chars().next());
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// A file of a package's `examples/`, read, to be ranked with the blocks of its docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleFile {
    /// Its name, as `cargo run --example` takes it.
    pub name: Box<str>,
    /// Its file, relative to the package's root with `/`.
    pub file: Box<str>,
    /// The features that it needs.
    pub required_features: Box<[Box<str>]>,
    /// Its text.
    pub text: String,
}

/// An example as it is shown: a file of `examples/`, or a block of the docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry<'e> {
    /// A file of `examples/`.
    File {
        /// The file.
        file: &'e ExampleFile,
        /// How well it matches the item asked for.
        rank: Rank,
    },
    /// A block of the docs.
    Block {
        /// The block.
        block: &'e DocBlock,
        /// How well it matches the item asked for.
        rank: Rank,
    },
}

impl Entry<'_> {
    /// How well it matches the item asked for.
    #[must_use]
    pub const fn rank(&self) -> Rank {
        match self {
            Self::File { rank, .. } | Self::Block { rank, .. } => *rank,
        }
    }
}

/// The most lines of a file that are told as naming the item asked for.
pub const MAX_LINES_TOLD: usize = 20;

/// The lines of `text`, from 1, that name `asked`'s name, at most [`MAX_LINES_TOLD`], and how many
/// do.
#[must_use]
pub fn lines_naming(text: &str, asked: &Asked<'_>) -> (Vec<usize>, usize) {
    let mut lines = Vec::new();
    let mut total = 0_usize;
    for (at, line) in text.lines().enumerate() {
        if has_word(line, asked.name) {
            total = total.saturating_add(1);
            if lines.len() < MAX_LINES_TOLD {
                lines.push(at.saturating_add(1));
            }
        }
    }
    (lines, total)
}

/// The files of `examples/` and the blocks of the docs, in the order that they are shown.
///
/// That is by their rank for `asked`, the files before the blocks within one, each in its own
/// order; without `asked`, the files, then the blocks, the root's first (decided in M3e, decision
/// 71).
#[must_use]
pub fn arranged<'e>(
    files: &'e [ExampleFile],
    blocks: &'e [DocBlock],
    asked: Option<&Asked<'_>>,
) -> Vec<Entry<'e>> {
    let rank_of =
        |of: Option<Id>, text: &str| asked.map_or(Rank::Other, |asked| rank(asked, of, text));
    // The files by name (decided in M3e, decision 71), whatever order they are given in.
    let mut by_name: Vec<&'e ExampleFile> = files.iter().collect();
    by_name.sort_by(|a, b| a.name.cmp(&b.name));
    let mut entries: Vec<Entry<'e>> = by_name
        .into_iter()
        .map(|file| Entry::File {
            file,
            rank: rank_of(None, &file.text),
        })
        .chain(blocks.iter().map(|block| Entry::Block {
            block,
            rank: rank_of(Some(block.id), &block.code),
        }))
        .collect();
    // Stable: the files before the blocks within a rank, each in its own order.
    entries.sort_by_key(Entry::rank);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(docs: &str) -> Vec<(String, Vec<String>)> {
        let mut blocks = Vec::new();
        push_blocks(&mut blocks, "demo::Mutex", Id(1), Some(docs));
        blocks
            .into_iter()
            .map(|block| {
                (
                    block.code,
                    block.attributes.iter().map(ToString::to_string).collect(),
                )
            })
            .collect()
    }

    /// The fenced blocks of docs: Rust ones by their info string, the hidden lines left out and
    /// the attributes kept; one not closed goes to the end.
    #[test]
    fn the_rust_blocks_of_docs_are_found() {
        let docs = "Intro.\n\n```\nuse demo::Mutex;\n# fn main() {\n  # let hidden = 1;\n\
                    let m = Mutex::new(1);\n## not hidden\n#[derive(Debug)]\n#![allow(x)]\n# }\n```\n\n\
                    ```text\nnot rust\n```\n\n~~~no_run,edition2021\nm.lock();\n~~~\n\n\
                    ```sh\n# a comment\n```\n\n```should_panic\npanic!();\n";
        assert_eq!(found(docs), [
            (
                "use demo::Mutex;\nlet m = Mutex::new(1);\n# not hidden\n#[derive(Debug)]\n\
                 #![allow(x)]\n"
                    .to_owned(),
                vec![]
            ),
            ("m.lock();\n".to_owned(), vec![
                "no_run".to_owned(),
                "edition2021".to_owned()
            ]),
            ("panic!();\n".to_owned(), vec!["should_panic".to_owned()]),
        ]);
    }

    /// A fence in a list, its lines without its indentation; one indented by 4 is not a fence; a
    /// longer fence holds a shorter one; line ends of Windows.
    #[test]
    fn indented_and_nested_fences_are_read_as_commonmark_does() {
        assert_eq!(
            found("1. Step\n\n   ```\n   let a = 1;\n     let b = 2;\n   ```\n"),
            [("let a = 1;\n  let b = 2;\n".to_owned(), vec![])]
        );
        assert_eq!(found("    ```\n    let a = 1;\n    ```\n"), []);
        assert_eq!(found("````\n```\nlet a = 1;\n```\n````\n"), [(
            "```\nlet a = 1;\n```\n".to_owned(),
            vec![]
        )]);
        assert_eq!(found("```\r\nlet a = 1;\r\n```\r\n"), [(
            "let a = 1;\n".to_owned(),
            vec![]
        )]);
    }

    /// A block is ranked by the item's own docs (by its id), by its name and its type's, by its
    /// name alone, or not, as identifiers (decided in M3, P13, and M3e, decision 71).
    #[test]
    fn a_block_is_ranked_by_the_names_that_it_has() {
        let lock = Asked {
            id: Id(7),
            path: "tokio::sync::Mutex::lock",
            name: "lock",
            parent: Some("Mutex"),
        };
        assert_eq!(rank(&lock, Some(Id(7)), ""), Rank::Own);
        // Another item at the same path, as a module and a macro.
        assert_eq!(rank(&lock, Some(Id(8)), ""), Rank::Other);
        assert_eq!(
            rank(&lock, Some(Id(1)), "let g = m.lock().await; // Mutex<T>"),
            Rank::Both
        );
        assert_eq!(rank(&lock, None, "file.lock()"), Rank::Name);
        assert_eq!(rank(&lock, None, "m.locked(); Mutexes"), Rank::Other);
        let spawn = Asked {
            id: Id(9),
            path: "tokio::spawn",
            name: "spawn",
            parent: None,
        };
        assert_eq!(rank(&spawn, None, "tokio::spawn(async {})"), Rank::Both);
        assert!(has_word("a::spawn(", "spawn"));
        assert!(has_word("r#type", "type"));
        assert!(!has_word("spawn_blocking", "spawn"));
        assert!(!has_word("respawn", "spawn"));
        assert!(!has_word("é_spawn", "spawn"));
    }

    /// The blocks are ordered by rank, then as found; `doc_blocks` orders the paths by depth, then
    /// by their text.
    #[test]
    fn the_blocks_are_ranked_for_the_item_asked_for() {
        let block = |item: &str, id: u32, code: &str| DocBlock {
            item: item.into(),
            id: Id(id),
            attributes: Box::default(),
            code: code.into(),
        };
        let blocks = [
            block("demo", 0, "nothing"),
            block("demo::a", 1, "x.lock()"),
            block("demo::Mutex", 2, "Mutex::new().lock()"),
            block("demo::Mutex::lock", 3, "own"),
        ];
        let lock = Asked {
            id: Id(3),
            path: "demo::Mutex::lock",
            name: "lock",
            parent: Some("Mutex"),
        };
        let order: Vec<(Rank, &str)> = arranged(&[], &blocks, Some(&lock))
            .into_iter()
            .map(|entry| match entry {
                Entry::Block { block, rank } => (rank, &*block.item),
                Entry::File { file, rank } => (rank, &*file.file),
            })
            .collect();
        assert_eq!(order, [
            (Rank::Own, "demo::Mutex::lock"),
            (Rank::Both, "demo::Mutex"),
            (Rank::Name, "demo::a"),
            (Rank::Other, "demo"),
        ]);
        // Files of `examples/` before the blocks of the same rank; without an item, all first.
        let file = |name: &str, text: &str| ExampleFile {
            name: name.into(),
            file: format!("examples/{name}.rs").into(),
            required_features: Box::default(),
            text: text.into(),
        };
        let files = [
            file("plain", "fn main() {}"),
            file("locking", "m.lock(); // Mutex"),
        ];
        let order: Vec<(Rank, &str)> = arranged(&files, &blocks, Some(&lock))
            .into_iter()
            .map(|entry| match entry {
                Entry::Block { block, rank } => (rank, &*block.item),
                Entry::File { file, rank } => (rank, &*file.file),
            })
            .collect();
        assert_eq!(order, [
            (Rank::Own, "demo::Mutex::lock"),
            (Rank::Both, "examples/locking.rs"),
            (Rank::Both, "demo::Mutex"),
            (Rank::Name, "demo::a"),
            (Rank::Other, "examples/plain.rs"),
            (Rank::Other, "demo"),
        ]);
        // Without an item: the files by name, then the blocks, the root's first.
        let shown: Vec<&str> = arranged(&files, &blocks, None)
            .into_iter()
            .map(|entry| match entry {
                Entry::Block { block, .. } => &*block.item,
                Entry::File { file, .. } => &*file.file,
            })
            .collect();
        assert_eq!(shown, [
            "examples/locking.rs",
            "examples/plain.rs",
            "demo",
            "demo::a",
            "demo::Mutex",
            "demo::Mutex::lock",
        ]);
        assert_eq!(
            lines_naming("a\r\nm.lock()\r\nlocked\nlock\nMutex::lock\n", &lock),
            (vec![2, 4, 5], 3)
        );
        let many = "lock\n".repeat(MAX_LINES_TOLD + 5);
        let (lines, total) = lines_naming(&many, &lock);
        assert_eq!((lines.len(), total), (MAX_LINES_TOLD, MAX_LINES_TOLD + 5));
        let mut paths = ["demo::b::c", "demo::z", "demo", "demo::a"];
        paths.sort_by(|a, b| by_path(a, b));
        assert_eq!(paths, ["demo", "demo::a", "demo::z", "demo::b::c"]);
    }
}
