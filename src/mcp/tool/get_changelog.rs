//! `get_changelog`: the releases of a crate's changelog between two versions, newest first, a page
//! at a time (requirement §5.9, §3 Q14, decided in M3g, decisions 112 to 122).
//!
//! The changelog is read from the `.crate` of the newest version of the range (decision 112), as
//! `get_source` reads its files, and cut into releases at the headings that start with a version
//! (`query::changelog`).

use std::{io::Read as _, path::Path, sync::Arc};

use rmcp::model::{CallToolResult, JsonObject, Tool};
#[cfg(feature = "local")]
use semver::Version;
use serde_json::{Map, Value, json};

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, ReleaseArguments},
    get_source::{self, SourcesOf},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
    resolve_version::{self, Release},
};
use crate::{
    data::{
        cache::sha256_hex,
        changelog,
        crate_file::{self, Declaration},
        crate_source::Sources,
        hidden,
        source_root::{self, Refusal, Root},
        version::VersionSpec,
    },
    query::{
        changelog::{Bound, Release as Entry, Releases, in_range, releases},
        get_item::MAX_RESPONSE_CHARS,
        markdown,
    },
};

/// The name of the tool.
pub(super) const NAME: &str = "get_changelog";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("get_changelog.md");

/// The releases in a page unless `limit` says otherwise (decision 115).
const DEFAULT_LIMIT: usize = 10;

/// The most releases in a page.
const MAX_LIMIT: usize = 50;

/// The most characters of the changelog in one answer, as `get_source` and `get_readme` show
/// (decision 115).
const MAX_CHARS: usize = 20_000;

/// The fewest characters of a page that is made shorter to keep a response within its budget.
const MIN_PAGE_CHARS: usize = 2_000;

/// The longest repository URL shown, in characters (as `crate_metadata`'s, decision 108).
const MAX_URL: usize = 500;

/// The largest changelog read: one larger is refused rather than read in part.
const MAX_CHANGELOG_BYTES: u64 = 8 * 1024 * 1024;

/// How a cursor's position is packed: the release, and the line in it to go on from.
const LINES_PER_RELEASE: usize = 1 << 24;

/// How to pass `from` and `to`, for `invalid_argument`.
const RANGE_EXAMPLE: &[&str] = &[r#"pass versions, for example {"from": "1.40.0", "to": "1.48"}"#];

/// What to call when the changelog is not to be had.
const METADATA: &str = "crate_metadata tells its versions, and where its license, MSRV or edition \
                        changed";

/// What is asked of the changelog, besides the crate.
#[derive(Debug)]
pub(super) struct ChangelogAsk {
    from: Option<Bound>,
    to: Option<Bound>,
    limit: usize,
    /// The release and the line in it to start at, from the cursor.
    position: (usize, usize),
    /// What cursors start with: the form and the digest of the arguments.
    request_digest: Box<str>,
    /// The digest of the changelog that the cursor was made for.
    cursor_data: Option<Box<str>>,
}

/// The arguments checked.
#[derive(Debug)]
pub(super) struct Changelog {
    release: ReleaseArguments,
    ask: ChangelogAsk,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio).",
            },
            "from": {
                "type": "string",
                "description": "The version after which to start (1.40.0, or 1.40 for after every \
                                1.40.x). Defaults to the version the project uses, or the first.",
            },
            "to": {
                "type": "string",
                "description": "The last version to tell (1.48.0, or 1.48 for up to every 1.48.x). \
                                Defaults to the latest release.",
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides `from`. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most releases in a page (default 10).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Read a Rust crate's changelog")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the answer.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let text = json!({ "type": "string" });
    let count = json!({ "type": "integer" });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": provenance_schema(),
            "retry_after_secs": count,
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "file": text,
                    "from": text,
                    "to": text,
                    "releases": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "version": text,
                                "heading": text,
                                "date": text,
                                "yanked": { "type": "boolean" },
                                "text": text,
                                "truncated": {
                                    "type": "boolean",
                                    "description": "Cut at a line: the next page goes on.",
                                },
                            },
                            "required": ["version", "text"],
                        },
                    },
                    "unreleased": {
                        "type": "string",
                        "description": "What the changelog tells of what is not released yet.",
                    },
                    "text": {
                        "type": "string",
                        "description": "Its lines, when it has no headings of versions.",
                    },
                },
                "required": ["file", "to"],
            },
            "page": {
                "type": "object",
                "properties": {
                    "shown": count,
                    "total": count,
                    "has_more": { "type": "boolean" },
                    "next_cursor": text,
                    "lines": {
                        "type": "array",
                        "items": count,
                        "description": "Of a changelog without headings of versions: the lines \
                                        shown, from 1.",
                    },
                    "total_lines": count,
                },
                "required": ["shown", "total", "has_more"],
            },
            "hints": strings,
        },
        "required": ["provenance", "data", "page"],
    }))
}

/// The provenance's schema: `resolve_version`'s, with the version that the project uses, which the
/// range starts after when `from` is not given.
fn provenance_schema() -> Value {
    let mut provenance = resolve_version::provenance_schema();
    if let Some(Value::Object(properties)) = provenance.get_mut("properties") {
        properties.insert(
            "from_project".to_owned(),
            json!({
                "type": "string",
                "description": "The version that the project uses, which the releases start after.",
            }),
        );
    }
    provenance
}

/// `value` as a JSON object, or an empty one.
fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            JsonObject::new()
        },
    }
}

/// Checks the arguments (requirement R-ARG-1).
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<Changelog, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    if get_item::std_crate(&release.name).is_some() {
        return Err(InvalidArgument::new(
            format!(
                "{}: the release notes of Rust are not looked up here",
                release.name
            ),
            &["get_item tells an item's stability: the version of Rust it is stable since"],
        ));
    }
    // The range says which versions (decision 113).
    if release.version.is_some() || release.target.is_some() {
        return Err(InvalidArgument::new(
            "get_changelog takes no version nor target: the range of versions is from and to",
            RANGE_EXAMPLE,
        ));
    }
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let bound = |name| match argument(name).filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(Value::String(text)) => Bound::parse(text).map(Some).ok_or_else(|| {
            InvalidArgument::new(
                format!("`{name}` must be a version, in full or its first parts (1.48)"),
                RANGE_EXAMPLE,
            )
        }),
        Some(_) => Err(InvalidArgument::new(
            format!("`{name}` must be a string"),
            RANGE_EXAMPLE,
        )),
    };
    let (from, to) = (bound("from")?, bound("to")?);
    if let (Some(Bound::Exact(from)), Some(Bound::Exact(to))) = (&from, &to)
        && from.cmp_precedence(to).is_ge()
    {
        return Err(InvalidArgument::new(
            "`from` must be older than `to`: the range is after from and up to to",
            RANGE_EXAMPLE,
        ));
    }
    let limit = match argument("limit").filter(|value| !value.is_null()) {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=MAX_LIMIT).contains(value))
            .ok_or_else(|| {
                InvalidArgument::new(
                    format!("`limit` must be a whole number from 1 to {MAX_LIMIT}"),
                    &[r#"pass a whole number, for example {"limit": 10}"#],
                )
            })?,
    };
    let request_digest = list_module::request_digest(NAME, arguments);
    let (position, cursor_data) = match argument("cursor") {
        None => ((0, 0), None),
        Some(Value::String(cursor)) => {
            let (offset, data) = list_module::parse_cursor(cursor, &request_digest)?;
            (position_of(offset), data)
        },
        Some(_) => {
            return Err(InvalidArgument::new(
                "`cursor` must be a string",
                CURSOR_EXAMPLE,
            ));
        },
    };
    Ok(Changelog {
        release,
        ask: ChangelogAsk {
            from,
            to,
            limit,
            position,
            request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
            cursor_data,
        },
    })
}

/// The release and the line in it that the offset `offset` of a cursor packs.
fn position_of(offset: usize) -> (usize, usize) {
    (
        offset.checked_div(LINES_PER_RELEASE).unwrap_or_default(),
        offset.checked_rem(LINES_PER_RELEASE).unwrap_or_default(),
    )
}

/// The version spec that `to` asks `resolve_version` for: itself, every version of its first
/// parts (`~0.8`), or else the latest (decision 112).
fn spec_of(to: Option<&Bound>) -> VersionSpec {
    match to {
        Some(Bound::Exact(version)) => VersionSpec::Exact(version.clone()),
        Some(Bound::Parts(parts)) => {
            let requirement = match parts.as_slice() {
                [major] => format!("^{major}"),
                parts => format!(
                    "~{}",
                    parts
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                ),
            };
            requirement.parse().unwrap_or(VersionSpec::Latest)
        },
        None => VersionSpec::Latest,
    }
}

/// Resolves the newest version of the range as `resolve_version` does, and reads the changelog of
/// its `.crate`; `from` defaults to the version that the project uses (decision 114).
pub(super) async fn execute(changelog: Changelog, state: Arc<ToolState>) -> CallToolResult {
    let Changelog { release, ask } = changelog;
    let until = state.until();
    let mut newest = release.clone();
    newest.version = Some(spec_of(ask.to.as_ref()));
    let (resolved, _taken) = resolve_version::resolving(newest, Arc::clone(&state), until).await;
    let Some(newest) = resolved
        .structured_content
        .as_ref()
        .filter(|_| resolved.is_error != Some(true))
        .and_then(Release::of)
    else {
        return resolved;
    };
    if !newest.on_crates_io {
        return not_a_release(&newest);
    }
    #[cfg(feature = "local")]
    let (ask, newest) = match from_project(&state, release, until, (ask, newest)).await {
        Ok(asked) => asked,
        Err(answer) => return *answer,
    };
    #[cfg(not(feature = "local"))]
    drop(release);
    let of = SourcesOf {
        name: &newest.name,
        version: &newest.version,
        checksum: None,
        until,
    };
    let sources =
        match get_source::release_sources(&state, &of, newest.provenance.clone(), &newest.warnings)
            .await
        {
            Ok(sources) => sources,
            Err(answer) => return *answer,
        };
    let answered = crate::task::spawn_blocking(move || {
        let read = read_changelog(&sources);
        respond(&newest, read, &ask)
    })
    .await;
    match answered {
        Ok(answer) => answer,
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the reading of a changelog did not finish");
            super::tool_error(
                "internal_error",
                "the changelog could not be read (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            )
        },
    }
}

/// `ask` with `from` the version that the project uses when it is not given (decision 114), and
/// `newest` with what tells of it; the answer to give meanwhile while the project is read.
#[cfg(feature = "local")]
async fn from_project(
    state: &Arc<ToolState>,
    release: ReleaseArguments,
    until: tokio::time::Instant,
    (mut ask, mut newest): (ChangelogAsk, Release),
) -> Result<(ChangelogAsk, Release), Box<CallToolResult>> {
    if ask.from.is_none() {
        match used_version(state, release, until).await {
            Used::Version(used) => {
                // Already at the newest of the range or past it: nothing after it to tell.
                if used.cmp_precedence(&newest.version).is_ge() {
                    newest.warnings.push(
                        format!(
                            "the project already uses {} {used}, which is not older than {}: no \
                             release comes after it; pass from for older ones",
                            newest.name, newest.version
                        )
                        .into(),
                    );
                }
                newest
                    .provenance
                    .insert("from_project".to_owned(), json!(used.to_string()));
                ask.from = Some(Bound::Exact(used));
            },
            Used::None => {},
            Used::Unknown(why) => newest.warnings.push(
                format!(
                    "the version that the project uses is not known ({why}): the releases start \
                     at the first; pass from"
                )
                .into(),
            ),
            Used::Resolving(answer) => return Err(answer),
        }
    }
    Ok((ask, newest))
}

/// What is known of the version of the crate that the project uses.
#[cfg(feature = "local")]
enum Used {
    /// Its Cargo.lock has this one.
    Version(Version),
    /// It uses none.
    None,
    /// It could not be told, and why.
    Unknown(String),
    /// The project is being read: the answer to give meanwhile.
    Resolving(Box<CallToolResult>),
}

/// The version of the crate that the project uses, from its Cargo.lock, as the start of the range
/// (decision 114).
#[cfg(feature = "local")]
async fn used_version(
    state: &Arc<ToolState>,
    mut release: ReleaseArguments,
    until: tokio::time::Instant,
) -> Used {
    release.version = None;
    let (resolved, _taken) = resolve_version::resolving(release, Arc::clone(state), until).await;
    let content = resolved.structured_content.as_ref();
    if content.is_some_and(|content| content.get("status").is_some()) {
        return Used::Resolving(Box::new(resolved));
    }
    if resolved.is_error == Some(true) {
        let why = resolved
            .content
            .first()
            .and_then(|content| content.as_text())
            .and_then(|text| serde_json::from_str::<Value>(&text.text).ok())
            .and_then(|body| body.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| "an error".to_owned());
        return Used::Unknown(why);
    }
    let Some(used) = content.and_then(Release::of) else {
        return Used::Unknown("the answer could not be read".to_owned());
    };
    let from_lock = used
        .provenance
        .get("resolved_from")
        .and_then(Value::as_str)
        .is_some_and(|from| from.starts_with("cargo_lock"));
    if from_lock {
        Used::Version(used.version)
    } else {
        Used::None
    }
}

/// The tool error for a release that is not of crates.io, whose changelog is not read yet.
fn not_a_release(release: &Release) -> CallToolResult {
    let of = if release.source.as_ref() == "crates.io" {
        "crates.io, but not the release on crates.io: a copy that differs from it, or that the \
         crates.io index does not have"
    } else {
        &release.source
    };
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "build_unsupported",
        "message": format!(
            "{} {} is of {of}: get_changelog reads the changelog of a release of crates.io, from \
             its .crate",
            release.name, release.version
        ),
        "retryable": false,
        "suggestions": [METADATA],
    }))
}

/// A changelog read, or why none was.
#[derive(Debug)]
enum Read {
    /// Its path in the package, its text, and whether it was not all UTF-8.
    Found(Box<str>, String, bool),
    /// The package has none: what its manifest declares, for where to look instead.
    None(Option<Declaration>),
    /// It could not be read: the reason of the error, and why.
    Failed(&'static str, String),
}

/// The changelog of the release whose sources are `sources`: the first of its root's files that
/// [`changelog::chosen`] takes, judged as `get_source` judges its files (decision 118).
fn read_changelog(sources: &Sources) -> Read {
    let root = Root::Release(sources);
    let names = sources
        .listing
        .files
        .iter()
        .filter(|file| !file.contains('/'))
        .map(String::as_str);
    let none = || {
        let declaration = source_root::resolve(root, Path::new("Cargo.toml"))
            .ok()
            .and_then(|manifest| crate_file::manifest_declaration(&manifest.path).ok())
            .map(|(declaration, _)| declaration);
        Read::None(declaration)
    };
    let Some(file) = changelog::chosen(names) else {
        return none();
    };
    let resolved = match source_root::resolve(root, Path::new(file)) {
        Ok(resolved) => resolved,
        Err(refusal) => {
            let reason = match refusal {
                Refusal::Missing | Refusal::NotAFile => "source_missing",
                Refusal::Outside
                | Refusal::Dotfile
                | Refusal::NotInPackage
                | Refusal::SymlinkOutside
                | Refusal::SymlinkInCache => "outside_package",
            };
            return Read::Failed(
                reason,
                format!("its changelog {file} is not read: {refusal}"),
            );
        },
    };
    // One that leads through a symlink to a file of another name is none.
    if !changelog::is_changelog(&resolved.path) {
        return none();
    }
    let mut contents = Vec::new();
    if let Err(error) = std::fs::File::open(&resolved.path).and_then(|opened| {
        opened
            .take(MAX_CHANGELOG_BYTES.saturating_add(1))
            .read_to_end(&mut contents)
    }) {
        return Read::Failed(
            "source_missing",
            format!("its changelog {file} could not be read: {error}"),
        );
    }
    if u64::try_from(contents.len()).is_ok_and(|len| len > MAX_CHANGELOG_BYTES) {
        return Read::Failed(
            "too_large",
            format!("its changelog {file} is larger than {MAX_CHANGELOG_BYTES} bytes"),
        );
    }
    let text = String::from_utf8_lossy(&contents);
    let lossy = matches!(text, std::borrow::Cow::Owned(_));
    Read::Found(file.into(), text.replace("\r\n", "\n"), lossy)
}

/// The page of a repository's releases on GitHub (`https://github.com/owner/repo/releases`), of
/// its first two parts.
fn github_releases(repository: &str) -> Option<String> {
    let path = repository.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let (owner, repo) = (parts.next()?, parts.next()?);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let named = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (named(owner) && named(repo)).then(|| format!("https://github.com/{owner}/{repo}/releases"))
}

/// The tool error for a crate without a changelog (decision 120): what to call instead, and its
/// repository, which is shown, not fetched.
fn no_changelog(what: &str, declaration: Option<&Declaration>) -> CallToolResult {
    let mut suggestions = vec![json!(METADATA)];
    if let Some(repository) = declaration.and_then(|declaration| declaration.repository.as_deref())
    {
        suggestions.push(json!(github_releases(repository).map_or_else(
            || {
                format!(
                    "its repository may tell its changes: {}",
                    markdown::cut(repository, MAX_URL)
                )
            },
            |releases| format!("its repository may tell its changes in its releases: {releases}"),
        )));
    }
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "no_changelog",
        "message": format!(
            "{what} has no changelog in its package (CHANGELOG, CHANGES, RELEASES, HISTORY or NEWS)"
        ),
        "retryable": false,
        "suggestions": suggestions,
    }))
}

/// What a page tells besides its releases, which [`respond`] puts together.
struct Told {
    data: Map<String, Value>,
    page: Value,
    warnings: Vec<Box<str>>,
}

/// The answer for the changelog `read` of `newest`, as `ask` asks, within the budget of a
/// response (requirement R-OUT-3): the page is made shorter while it is over.
fn respond(newest: &Release, read: Read, ask: &ChangelogAsk) -> CallToolResult {
    let what = format!("{} {}", newest.name, newest.version);
    let (file, text, lossy) = match read {
        Read::Found(file, text, lossy) => (file, text, lossy),
        Read::None(declaration) => return no_changelog(&what, declaration.as_ref()),
        Read::Failed(reason, why) => {
            return super::error_result(&json!({
                "code": "docs_unavailable",
                "reason": reason,
                "message": format!("{what}: {why}"),
                "retryable": false,
                "suggestions": [METADATA],
            }));
        },
    };
    let digest = sha256_hex(text.as_bytes());
    let digest = digest.get(..16).unwrap_or_default();
    if ask
        .cursor_data
        .as_deref()
        .is_some_and(|data| data != digest)
    {
        return InvalidArgument::new(
            "the changelog changed since the cursor was made; read it again without the cursor",
            CURSOR_EXAMPLE,
        )
        .into_result();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let markdown = changelog::is_markdown(&file);
    let found = if markdown {
        releases(&text, newest.name.as_str())
    } else {
        Releases::default()
    };
    let mut budget = MAX_CHARS;
    loop {
        let told = match page_of(
            newest,
            (&file, &lines, markdown),
            &found,
            (ask, digest, budget),
        ) {
            Ok(told) => told,
            Err(refused) => return *refused,
        };
        let mut answer = Map::new();
        answer.insert(
            "provenance".to_owned(),
            Value::Object(newest.provenance.clone()),
        );
        if let Some(after) = &newest.retry_after {
            answer.insert("retry_after_secs".to_owned(), after.clone());
        }
        answer.insert("data".to_owned(), Value::Object(told.data));
        answer.insert("page".to_owned(), told.page);
        if !newest.hints.is_empty() {
            answer.insert("hints".to_owned(), json!(newest.hints));
        }
        let mut answer = Value::Object(answer);
        let mut warnings = told.warnings;
        if lossy {
            warnings.push("the changelog is not UTF-8: what is not is shown as U+FFFD".into());
        }
        get_item::add_warnings(&mut answer, &warnings);
        if answer.to_string().chars().count() <= MAX_RESPONSE_CHARS || budget <= MIN_PAGE_CHARS {
            return CallToolResult::structured(answer);
        }
        budget = budget
            .checked_div(2)
            .unwrap_or_default()
            .max(MIN_PAGE_CHARS);
    }
}

/// The page that `ask` asks for of the changelog `lines` of `newest`, whose releases are `found`,
/// of at most `budget` characters.
fn page_of(
    newest: &Release,
    (file, lines, markdown): (&str, &[&str], bool),
    found: &Releases,
    (ask, digest, budget): (&ChangelogAsk, &str, usize),
) -> Result<Told, Box<CallToolResult>> {
    let mut warnings = newest.warnings.clone();
    let mut data = Map::new();
    data.insert("file".to_owned(), json!(file));
    if let Some(from) = &ask.from {
        data.insert("from".to_owned(), json!(bound_text(from)));
    }
    data.insert("to".to_owned(), json!(newest.version.to_string()));
    let mut marks = Marks::default();
    let page = if found.all.is_empty() {
        warnings.push(
            if markdown {
                format!(
                    "{file} has no headings of versions to tell its releases apart: its lines are \
                     told from the top"
                )
            } else {
                format!(
                    "{file} is not Markdown, so its releases are not told apart: its lines are told \
                     from the top"
                )
            }
            .into(),
        );
        lines_page(lines, (ask, digest, budget), &mut data, &mut marks)?
    } else {
        if !found
            .all
            .iter()
            .any(|release| release.version.cmp_precedence(&newest.version).is_eq())
        {
            warnings.push(
                format!(
                    "{file} does not tell of {}: it may be kept elsewhere, or not yet for this \
                     version",
                    newest.version
                )
                .into(),
            );
        }
        releases_page(found, lines, (ask, digest, budget), (&mut data, &mut marks))?
    };
    if page.get("total") == Some(&json!(0)) {
        warnings.push(
            "no release of the changelog is after from and up to to: check the range, or pass \
             other versions"
                .into(),
        );
    }
    if marks.hidden > 0 {
        warnings.push(hidden::marked_warning(marks.hidden).into());
    }
    if let Some(line) = marks.cut_line {
        warnings.push(format!("line {line} is longer than a page alone, and was cut").into());
    }
    if marks.unreleased_cut {
        warnings
            .push("what is not released yet is cut: read the changelog's top for the rest".into());
    }
    Ok(Told {
        data,
        page,
        warnings,
    })
}

/// What a page marked or cut, for its warnings.
#[derive(Debug, Default)]
struct Marks {
    /// How many characters were marked (requirement R-SEC-7, as decision 66).
    hidden: usize,
    /// The line, from 1, cut as longer than a page alone, if one was.
    cut_line: Option<usize>,
    /// Whether what is not released yet was cut.
    unreleased_cut: bool,
}

impl Marks {
    /// `text` with what it hides marked, counted.
    fn marked(&mut self, text: &str) -> String {
        let (marked, count) = hidden::mark(text);
        self.hidden = self.hidden.saturating_add(count);
        marked.into_owned()
    }
}

/// `bound` as written.
fn bound_text(bound: &Bound) -> String {
    match bound {
        Bound::Exact(version) => version.to_string(),
        Bound::Parts(parts) => parts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("."),
    }
}

/// The releases of `found` in the range that `ask` asks for, newest first whatever the file's
/// order, each version once (decisions 114 and 115).
fn picked<'f>(found: &'f Releases, ask: &ChangelogAsk) -> Vec<(usize, &'f Entry)> {
    let mut picked: Vec<(usize, &Entry)> = in_range(&found.all, ask.from.as_ref(), ask.to.as_ref())
        .into_iter()
        .filter_map(|index| Some((index, found.all.get(index)?)))
        .collect();
    picked.sort_by(|(_, a), (_, b)| b.version.cmp_precedence(&a.version));
    picked.dedup_by(|(_, a), (_, b)| a.version.cmp_precedence(&b.version).is_eq());
    picked
}

/// The lines from the line `from` (from 0) of `lines` that fit in `budget` characters, one at
/// least: their text, where they end (from 0, not included), and the line cut, from 1, if one
/// alone was longer than the budget.
fn fitting(lines: &[&str], from: usize, budget: usize) -> (String, usize, Option<usize>) {
    let mut text = String::new();
    let mut end = from;
    for line in lines.iter().skip(from) {
        let room = budget.saturating_sub(text.chars().count());
        let length = line.chars().count();
        if length > room {
            if end == from {
                // A line longer than the page alone: cut.
                text.push_str(&cut_chars(line, room.max(1)));
                return (text, end.saturating_add(1), Some(end.saturating_add(1)));
            }
            break;
        }
        text.push_str(line);
        end = end.saturating_add(1);
    }
    (text, end, None)
}

/// The release `release` as told, with its `text`.
fn release_value(release: &Entry, text: &str, truncated: bool, marks: &mut Marks) -> Value {
    let mut told = Map::new();
    told.insert("version".to_owned(), json!(release.version.to_string()));
    told.insert("heading".to_owned(), json!(marks.marked(&release.heading)));
    if let Some(date) = &release.date {
        told.insert("date".to_owned(), json!(date));
    }
    if release.yanked {
        told.insert("yanked".to_owned(), json!(true));
    }
    told.insert("text".to_owned(), json!(text));
    if truncated {
        told.insert("truncated".to_owned(), json!(true));
    }
    Value::Object(told)
}

/// The page of the releases in the range that `ask` asks for, newest first (decisions 114 and
/// 115), of at most `budget` characters: in `data`, the releases, and what is not released yet on
/// the first page when `to` is not given; the page.
fn releases_page(
    found: &Releases,
    lines: &[&str],
    (ask, digest, budget): (&ChangelogAsk, &str, usize),
    (data, marks): (&mut Map<String, Value>, &mut Marks),
) -> Result<Value, Box<CallToolResult>> {
    let picked = picked(found, ask);
    let total = picked.len();
    let (start, mut line_in) = ask.position;
    if start > total || (start == total && total > 0) {
        return Err(past_the_end());
    }
    let mut room = budget;
    // What is not released yet, first, when the newest is asked for (decision 117).
    if ask.to.is_none()
        && ask.position == (0, 0)
        && let Some((from, to)) = found.lines_of(None, lines.len())
    {
        let body: String = lines
            .get(from.saturating_sub(1)..to.saturating_sub(1))
            .unwrap_or_default()
            .concat();
        if !body.trim().is_empty() {
            let max = budget.checked_div(4).unwrap_or_default();
            let cut = cut_chars(&body, max);
            marks.unreleased_cut = cut.len() < body.len();
            let text = marks.marked(&cut);
            room = room.saturating_sub(text.chars().count());
            data.insert("unreleased".to_owned(), json!(text));
        }
    }
    let mut shown = Vec::new();
    let mut next = None;
    for (place, &(index, release)) in picked.iter().enumerate().skip(start) {
        if shown.len() >= ask.limit {
            next = Some((place, 0));
            break;
        }
        let Some((first, end)) = found.lines_of(Some(index), lines.len()) else {
            continue;
        };
        let body = lines
            .get(first.saturating_sub(1)..end.saturating_sub(1))
            .unwrap_or_default();
        let length: usize = body
            .iter()
            .skip(line_in)
            .map(|line| line.chars().count())
            .sum();
        if length > room && !shown.is_empty() {
            next = Some((place, 0));
            break;
        }
        // One release alone longer than the page: cut at a line, and go on from there.
        let (text, taken, cut_line) = fitting(body, line_in, room);
        if let Some(line) = cut_line {
            marks.cut_line = Some(first.saturating_sub(1).saturating_add(line));
        }
        let truncated = taken < body.len();
        let text = marks.marked(&text);
        room = room.saturating_sub(text.chars().count());
        shown.push(release_value(release, &text, truncated, marks));
        if truncated {
            next = Some((place, taken));
            break;
        }
        line_in = 0;
    }
    let count = shown.len();
    data.insert("releases".to_owned(), Value::Array(shown));
    Ok(page_value(count, total, next, ask, digest))
}

/// The error for a cursor past the end of the changelog.
fn past_the_end() -> Box<CallToolResult> {
    Box::new(
        InvalidArgument::new(
            "the cursor goes past the end of the changelog; read it again without the cursor",
            CURSOR_EXAMPLE,
        )
        .into_result(),
    )
}

/// The page of the lines of a changelog without headings of versions, from the line that `ask`
/// starts at, of at most `budget` characters, cut at a line (decision 117).
fn lines_page(
    lines: &[&str],
    (ask, digest, budget): (&ChangelogAsk, &str, usize),
    data: &mut Map<String, Value>,
    marks: &mut Marks,
) -> Result<Value, Box<CallToolResult>> {
    let from = ask.position.1;
    if from > lines.len() || (from == lines.len() && from > 0) {
        return Err(past_the_end());
    }
    let (text, end, cut_line) = fitting(lines, from, budget);
    marks.cut_line = cut_line.or(marks.cut_line);
    data.insert("text".to_owned(), json!(marks.marked(&text)));
    data.insert("releases".to_owned(), json!([]));
    let next = (end < lines.len()).then_some((0, end));
    let mut page = page_value(0, 0, next, ask, digest);
    if let Some(page) = page.as_object_mut() {
        page.insert("lines".to_owned(), json!([from.saturating_add(1), end]));
        page.insert("total_lines".to_owned(), json!(lines.len()));
    }
    Ok(page)
}

/// `text`, at most `max` characters.
fn cut_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => text.get(..at).unwrap_or(text).to_owned(),
        None => text.to_owned(),
    }
}

/// The page: `shown` releases of `total`, and the cursor of `next` (the release, the line in it),
/// if any.
fn page_value(
    shown: usize,
    total: usize,
    next: Option<(usize, usize)>,
    ask: &ChangelogAsk,
    digest: &str,
) -> Value {
    let mut page = Map::new();
    page.insert("shown".to_owned(), json!(shown));
    page.insert("total".to_owned(), json!(total));
    page.insert("has_more".to_owned(), json!(next.is_some()));
    if let Some((release, line)) = next {
        let offset = release
            .saturating_mul(LINES_PER_RELEASE)
            .saturating_add(line.min(LINES_PER_RELEASE.saturating_sub(1)));
        page.insert(
            "next_cursor".to_owned(),
            json!(format!("{}.{digest}.{offset}", ask.request_digest)),
        );
    }
    Value::Object(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(
        from: Option<&str>,
        to: Option<&str>,
        limit: usize,
        position: (usize, usize),
    ) -> ChangelogAsk {
        ChangelogAsk {
            from: from.and_then(Bound::parse),
            to: to.and_then(Bound::parse),
            limit,
            position,
            request_digest: "c1.r".into(),
            cursor_data: None,
        }
    }

    fn release_of(version: &str) -> Release {
        Release::of(&json!({
            "provenance": { "crate": "demo", "source": "crates.io", "target": "x" },
            "data": { "version": version, "source": "crates.io", "docs_rs_json": "available" },
        }))
        .expect("a release")
    }

    const CHANGELOG: &str = "# Changelog\n\n## [Unreleased]\n\n- coming\n\n\
                             ## [1.2.0] - 2026-02-01\n\n- two\n\n\
                             ## [1.1.0] - 2026-01-01 [YANKED]\n\n- one \u{202e}\n\n\
                             ## [1.0.0]\n\n- first\n\n[1.0.0]: https://example.invalid\n";

    fn answer(read: Read, ask: &ChangelogAsk) -> Value {
        let result = respond(&release_of("1.2.0"), read, ask);
        let told = format!("{result:?}");
        result
            .structured_content
            .unwrap_or_else(|| json!({ "error": told }))
    }

    #[test]
    fn the_releases_of_a_range_are_told_newest_first() {
        let found = || Read::Found("CHANGELOG.md".into(), CHANGELOG.to_owned(), false);
        let all = answer(found(), &ask(None, None, 10, (0, 0)));
        let data = &all["data"];
        assert_eq!(
            data["unreleased"], "## [Unreleased]\n\n- coming\n\n",
            "{all}"
        );
        let versions: Vec<&str> = data["releases"]
            .as_array()
            .expect("releases")
            .iter()
            .filter_map(|release| release["version"].as_str())
            .collect();
        assert_eq!(versions, ["1.2.0", "1.1.0", "1.0.0"]);
        assert_eq!(data["releases"][1]["yanked"], true);
        assert!(
            data["releases"][1]["text"]
                .as_str()
                .is_some_and(|text| text.contains("<U+202E>"))
        );
        assert_eq!(
            data["releases"][2]["text"], "## [1.0.0]\n\n- first\n\n",
            "without the labels"
        );
        assert_eq!(
            all["page"],
            json!({ "shown": 3, "total": 3, "has_more": false })
        );
        // After 1.0.0 and up to 1.1: without what is not released.
        let range = answer(found(), &ask(Some("1.0.0"), Some("1.1"), 10, (0, 0)));
        assert_eq!(range["data"]["releases"].as_array().map(Vec::len), Some(1));
        assert!(range["data"].get("unreleased").is_none());
        // A page of one, then the next.
        let first = answer(found(), &ask(None, None, 1, (0, 0)));
        assert_eq!(first["page"]["has_more"], true);
        let cursor = first["page"]["next_cursor"]
            .as_str()
            .expect("a cursor")
            .to_owned();
        let offset: usize = cursor
            .rsplit('.')
            .next()
            .and_then(|offset| offset.parse().ok())
            .expect("an offset");
        let second = answer(found(), &ask(None, None, 1, position_of(offset)));
        assert_eq!(second["data"]["releases"][0]["version"], "1.1.0");
        assert!(second["data"].get("unreleased").is_none());
    }

    #[test]
    fn a_release_longer_than_a_page_is_cut_at_a_line_and_goes_on() {
        let long = format!(
            "## 2.0.0\n\n{}## 1.0.0\n\n- one\n",
            "- a change of some words\n".repeat(2_000)
        );
        let read = || Read::Found("CHANGELOG.md".into(), long.clone(), false);
        let first = answer(read(), &ask(None, None, 10, (0, 0)));
        let release = &first["data"]["releases"][0];
        assert_eq!(release["truncated"], true, "{first}");
        assert!(
            release["text"]
                .as_str()
                .is_some_and(|text| text.chars().count() <= MAX_CHARS)
        );
        let cursor = first["page"]["next_cursor"]
            .as_str()
            .expect("a cursor")
            .to_owned();
        let offset: usize = cursor
            .rsplit('.')
            .next()
            .and_then(|offset| offset.parse().ok())
            .expect("an offset");
        let (release, line) = position_of(offset);
        assert_eq!(release, 0, "the same release");
        assert!(line > 0);
    }

    #[test]
    fn a_changelog_without_versions_is_told_as_lines_and_none_is_an_error() {
        let lines = answer(
            Read::Found(
                "NEWS.txt".into(),
                "news
more news
"
                .to_owned(),
                false,
            ),
            &ask(None, None, 10, (0, 0)),
        );
        assert_eq!(lines["data"]["text"], "news\nmore news\n");
        assert!(lines["warnings"].to_string().contains("not Markdown"));
        let declaration = Declaration {
            repository: Some("https://github.com/owner/demo.git".into()),
            ..Declaration::default()
        };
        let none = respond(
            &release_of("1.2.0"),
            Read::None(Some(declaration)),
            &ask(None, None, 10, (0, 0)),
        );
        let body = format!("{none:?}");
        assert!(
            body.contains("no_changelog")
                && body.contains("https://github.com/owner/demo/releases"),
            "{body}"
        );
    }

    #[test]
    fn the_range_and_odd_arguments_are_checked() {
        let parsed = |value: Value| parse_arguments(Some(&object(value)));
        assert!(
            parsed(json!({ "crate": "tokio", "from": "1.48.0", "to": "1.40.0" }))
                .err()
                .is_some()
        );
        assert!(
            parsed(json!({ "crate": "tokio", "to": "1.x" }))
                .err()
                .is_some()
        );
        assert!(
            parsed(json!({ "crate": "tokio@1.48.0" })).err().is_some(),
            "a version"
        );
        assert!(parsed(json!({ "crate": "std" })).err().is_some());
        let ok = parsed(json!({ "crate": "tokio", "from": "1.40", "to": null })).expect("checked");
        assert_eq!(
            (ok.ask.from, ok.ask.to, ok.ask.limit),
            (Some(Bound::Parts(vec![1, 40])), None, 10)
        );
        assert!(matches!(
            spec_of(Bound::parse("0.8").as_ref()),
            VersionSpec::Requirement(_)
        ));
    }

    #[test]
    fn to_is_asked_of_resolve_version_as_a_spec() {
        let spec = |text: &str| spec_of(Bound::parse(text).as_ref()).to_string();
        assert_eq!(spec("0.8"), "~0.8");
        assert_eq!(spec("1"), "^1");
        assert_eq!(spec("1.2.3"), "1.2.3");
        assert_eq!(spec_of(None).to_string(), "latest");
    }

    #[test]
    fn a_cut_release_goes_on_where_it_was_cut_without_lines_again() {
        let long = format!(
            "## 2.0.0\n\n{}## 1.0.0\n\n- one\n",
            (0..3_000)
                .map(|n| format!("- change {n}\n"))
                .collect::<Vec<_>>()
                .concat()
        );
        let read = || Read::Found("CHANGELOG.md".into(), long.clone(), false);
        let mut position = (0, 0);
        let mut texts = String::new();
        for _ in 0..10 {
            let page = answer(read(), &ask(None, None, 10, position));
            for release in page["data"]["releases"].as_array().into_iter().flatten() {
                if release["version"] == "2.0.0" {
                    texts.push_str(release["text"].as_str().unwrap_or_default());
                }
            }
            let Some(cursor) = page["page"]["next_cursor"].as_str() else {
                break;
            };
            let offset: usize = cursor
                .rsplit('.')
                .next()
                .and_then(|offset| offset.parse().ok())
                .expect("an offset");
            position = position_of(offset);
        }
        // Every line of 2.0.0 once, in order.
        let expected: String = long.split_inclusive('\n').take(3_002).collect();
        assert_eq!(texts, expected);
    }
}
