Tell what a Rust crate declares (license, MSRV, edition, repository, dependencies), its releases, how much it is used, and where its license, MSRV or edition changed. Answers questions such as "what is serde's MSRV and license?" or "is this crate still maintained?".

`crate`⟨, `project`⟩ and `version` are as for `get_item`⟨; `refresh` asks the crates.io API again too⟩. What is told is declared in the release's `Cargo.toml`, not resolved.⟨ Of the project's other packages, only `declared` and `dependencies` come; of a crate of git, `declared`.⟩

Returns `provenance` as `resolve_version` does, with `api_fetched_at` for the crates.io API, and `data`:
- `declared`: of the release, `description` (cut at 1,000 characters), `license` (SPDX, or old `MIT/Apache-2.0`; none with only `license_file`), `rust_version`, `edition` (none is 2015, unless a warning says inherited), `repository`, `homepage`, `documentation`, `docs_rs_targets` and `docs_rs_default_target`.
- `releases`: `latest_stable`, `latest` (pre-releases too), how many `versions` (yanked too) and `yanked`, and the `last_release`, its `last_release_at`, `days_since_last_release` and `recency` (`within_N_days` for 30, 90 or 365, or `over_365_days`): when, not whether it is maintained.
- `popularity`: `downloads`, `recent_downloads` (90 days) and `dependents`.
- `dependencies`: the normal and build ones, with `req`, `kind`, `optional`, `target`, `package` when renamed and `default_features: false`; `dev_dependencies` is how many dev ones. Their features: `inspect_features`.
- `history`: newest first, the versions where `license`, `rust_version` or `edition` changed (`from`, `to`; one left out was not declared, or a `license_file`) from the version before by semver (a backport within its line), the `yanked` ones, and the `first`; else: `get_changelog`. Paged: pass `next_cursor` as `cursor`.

The crates.io API is asked at most once a second; its answers are kept a day, and used however old when it does not answer. What it never told in time is left out with a warning.
