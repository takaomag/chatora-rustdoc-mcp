Read the README of a Rust crate on crates.io: the file that its Cargo.toml names, or else README.md, README.txt or README, from its published `.crate`. Answers questions such as "how do I start with axum?" or "what does this crate's README say about its setup?". For its API, call `crate_overview` or `get_item`.

`crate`, `version`⟨, `project`⟩ and `target` are as for `get_item`. `section` starts the page at a heading of the outline, matched without case, links or badges, or by its start.

Returns `provenance` as `resolve_version` does, and `data`: `file`, `format` (`markdown` or `text`), `outline` (the headings of a Markdown readme with their `level` and `line`, at most 100, with `outline_total`), and `text`, a page of at most 20,000 characters cut at a heading, with `truncated` when more follows: pass `page.next_cursor` as `cursor` for the rest. Invisible characters, as bidirectional controls, are shown as marks such as <U+202E>, with a warning. A crate whose manifest tells that it has none (`readme = false`), or names one that is not a document, gets `docs_unavailable` with the reason `no_readme`.

Neither the documentation nor a build is needed: only the `.crate`.
