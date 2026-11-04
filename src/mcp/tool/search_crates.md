Search crates.io for Rust crates by what they do or by name. Answers questions such as "which crate should I use to parse JSON?" or "is there a crate for rate limiting?".

`query` is words of what the crate does, or its name; it is sent to crates.io. `sort` picks the order: `relevance` (the default) as crates.io ranks them, with a crate of that very name first; `downloads` for the most used, which is often what "which crate should I use" asks; `recent-downloads` (the last 90 days); `recent-updates`; or `new`.

Returns `provenance` with `source: "crates.io"`, `fetched_at` and `sort`, and `data.crates`, each with `name`, `description`, `latest_stable` (none when every version is a pre-release or yanked), `latest`, `downloads`, `recent_downloads`, `repository`, `updated_at`, and `exact_match` when its name is the query. The list is paged: `page.total` is how many crates.io found, and `next_cursor`, passed as `cursor` with the same other arguments, gives the next page.

To choose among them, `crate_metadata` tells a crate's license, MSRV, dependents, releases and how long ago the last was; `crate_overview` and `get_readme` tell what it is. A search is kept an hour, and used however old when crates.io does not answer.
