//! `get_readme`: a crate's README, as its manifest names it, with the outline of its headings, a page
//! of at most [`MAX_CHARS`] characters cut at a heading (requirement §5.9, decided in M3e,
//! decisions 63 to 68 and 76).
//!
//! The release is resolved as `resolve_version` resolves it; the readme is read from its sources,
//! as `get_source` reads them, without its documentation.

use std::{
    borrow::Cow,
    io::{self, Read as _},
    path::Path,
    sync::Arc,
};

use rmcp::model::{CallToolResult, JsonObject, Tool};
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
        crate_file,
        crate_source::Sources,
        hidden,
        readme::{self, NoReadme},
        source_root::{self, Refusal, Root},
    },
    query::{
        get_item::MAX_RESPONSE_CHARS,
        markdown::{self, Heading, plain},
    },
};

/// The name of the tool.
pub(super) const NAME: &str = "get_readme";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("get_readme.md");

/// The most characters of the readme in one answer, as `get_source` shows (decided in M3e,
/// decision 64).
const MAX_CHARS: usize = 20_000;

/// The fewest characters of a page that is made shorter to keep a response within its budget.
const MIN_PAGE_CHARS: usize = 2_000;

/// The most headings in the outline; the rest is counted (requirement R-OUT-3).
const MAX_OUTLINE: usize = 100;

/// The most characters of a heading in the outline.
const MAX_HEADING_CHARS: usize = 120;

/// The largest readme read: one larger is refused rather than read in part.
const MAX_README_BYTES: u64 = 8 * 1024 * 1024;

/// How to pass `section`, for `invalid_argument`.
const SECTION_EXAMPLE: &[&str] =
    &[r#"pass a heading of the outline, for example {"section": "Getting started"}"#];

/// What to call when the readme is not to be had.
const OVERVIEW: &str =
    "crate_overview gives the crate's root documentation, which often tells how to start";

/// What is asked of the readme, besides the release.
#[derive(Debug)]
pub(super) struct ReadmeAsk {
    /// The heading of the section to start at, if one was asked for.
    section: Option<Box<str>>,
    /// The line to start at, from 0, as the cursor tells.
    offset: Option<usize>,
    /// What cursors start with: the form and the digest of the arguments.
    request_digest: Box<str>,
    /// The digest of the readme that the cursor was made for.
    cursor_data: Option<Box<str>>,
}

/// The arguments checked: the release's, for `resolve_version`, and what is asked of its readme.
#[derive(Debug)]
pub(super) struct Readme {
    release: ReleaseArguments,
    ask: ReadmeAsk,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "crate": {
                "type": "string",
                "description": "The crate (tokio), or the crate and its version as name@version.",
            },
            "section": {
                "type": "string",
                "description": "A heading of the outline to start at (Getting started).",
            },
            "version": {
                "type": "string",
                "description": const_format::concatcp!(
                    "The version: exact, a semver requirement or latest.",
                    super::VERSION_DEFAULT
                ),
            },
            "target": {
                "type": "string",
                "description": const_format::concatcp!(
                    "The target triple, which the version may depend on.",
                    super::TARGET_DEFAULT
                ),
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Read a Rust crate's README")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let line = json!({ "type": "integer", "description": "A line of the readme, from 1." });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": resolve_version::provenance_schema(),
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "file": {
                        "type": "string",
                        "description": "The readme, relative to the package's root.",
                    },
                    "format": { "type": "string", "enum": ["markdown", "text"] },
                    "outline": {
                        "type": "array",
                        "description": "The headings of a Markdown readme, links and images left \
                                        out.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "level": { "type": "integer" },
                                "heading": { "type": "string" },
                                "line": line,
                            },
                            "required": ["level", "heading", "line"],
                        },
                    },
                    "outline_total": {
                        "type": "integer",
                        "description": "How many headings there are, when the outline is cut.",
                    },
                    "text": {
                        "type": "string",
                        "description": "The lines of this page, cut at a heading.",
                    },
                    "truncated": {
                        "type": "boolean",
                        "description": "More of the readme follows: pass next_cursor as cursor.",
                    },
                    "cut_line": {
                        "type": "integer",
                        "description": "A line longer than a page alone, cut within.",
                    },
                },
                "required": ["file", "format", "text", "truncated"],
            },
            "page": {
                "type": "object",
                "properties": {
                    "first_line": line,
                    "last_line": line,
                    "total_lines": { "type": "integer" },
                    "has_more": { "type": "boolean" },
                    "next_cursor": { "type": "string" },
                },
                "required": ["first_line", "last_line", "total_lines", "has_more"],
            },
            "hints": strings,
        },
        "required": ["provenance", "data", "page"],
    }))
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

/// Checks the arguments (requirement R-ARG-1). std's crates have no readme (decided in M3e,
/// decision 68).
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<Readme, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    if get_item::std_crate(&release.name).is_some() {
        return Err(InvalidArgument::new(
            format!(
                "{} has no readme: the standard library's crates are documented at their root",
                release.name
            ),
            &["call crate_overview for the crate's root documentation, or get_item for one item"],
        ));
    }
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let section = match argument("section") {
        None => None,
        Some(Value::String(section)) if !plain(section).is_empty() => {
            Some(Box::from(section.trim()))
        },
        Some(_) => {
            return Err(InvalidArgument::new(
                "`section` must be a heading of the readme",
                SECTION_EXAMPLE,
            ));
        },
    };
    let request_digest = list_module::request_digest(NAME, arguments);
    let (offset, cursor_data) = match argument("cursor") {
        None => (None, None),
        Some(Value::String(cursor)) => {
            let (offset, data) = list_module::parse_cursor(cursor, &request_digest)?;
            (Some(offset), data)
        },
        Some(_) => {
            return Err(InvalidArgument::new(
                "`cursor` must be a string",
                CURSOR_EXAMPLE,
            ));
        },
    };
    Ok(Readme {
        release,
        ask: ReadmeAsk {
            section,
            offset,
            request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
            cursor_data,
        },
    })
}

/// Resolves the release as `resolve_version` does, and reads its readme from its sources (decided
/// in M3e, decisions 63 to 68): of a crates.io release, its `.crate`'s.
pub(super) async fn execute(readme: Readme, state: Arc<ToolState>) -> CallToolResult {
    let Readme { release, ask } = readme;
    // One deadline for every step (requirement R-ARCH-6).
    let until = state.until();
    #[cfg_attr(
        not(feature = "local"),
        expect(
            unused_variables,
            reason = "only a project or a record has more to tell"
        )
    )]
    let (resolved, taken) = resolve_version::resolving(release, Arc::clone(&state), until).await;
    // An error, or the response while the release is resolved, as it is.
    let Some(release) = resolved
        .structured_content
        .as_ref()
        .filter(|_| resolved.is_error != Some(true))
        .and_then(Release::of)
    else {
        return resolved;
    };
    // A crate of git that `build_external_docs` recorded, in its checkout (decided in M3e,
    // decision 80).
    #[cfg(feature = "local")]
    if let Some(git) = &taken.git {
        return match here::of_git(&state, git) {
            Ok(here) => answered(release, ask, move || here::read(&here)).await,
            Err(answer) => *answer,
        };
    }
    // Another package of the project, where it is (decided in M3e, decisions 78 and 79).
    #[cfg(feature = "local")]
    if let Some(found) = taken.found.as_deref()
        && !release.on_crates_io
        && !here::unpacked_from_crates_io(&state, found)
    {
        let what = format!("{} {}", release.name, release.version);
        return match here::of_project(&state, (found, &release), until, ("readme", OVERVIEW)).await
        {
            Ok(here) => answered(release, ask, move || here::read(&here)).await,
            Err(here::NotHere::Answer(answer)) => *answer,
            Err(here::NotHere::Copy(of)) => *here::copy(&what, &of),
        };
    }
    // As one that the project takes from crates.io, which cargo unpacked as it is (decided in M3e,
    // decision 79), its `.crate`.
    #[cfg(feature = "local")]
    let on_crates_io = release.on_crates_io || taken.found.is_some();
    #[cfg(not(feature = "local"))]
    let on_crates_io = release.on_crates_io;
    if !on_crates_io {
        return not_a_release(&release);
    }
    let of = SourcesOf {
        name: &release.name,
        version: &release.version,
        checksum: None,
        until,
    };
    let sources = match get_source::release_sources(
        &state,
        &of,
        release.provenance.clone(),
        &release.warnings,
    )
    .await
    {
        Ok(sources) => sources,
        Err(answer) => return with_overview(*answer),
    };
    answered(release, ask, move || read_readme(&sources)).await
}

/// The answer for the readme of `release` that `read` reads, as `ask` asks: read, outlined and
/// paged off the runtime's workers, as a readme may be of 8 MiB.
async fn answered(
    release: Release,
    ask: ReadmeAsk,
    read: impl FnOnce() -> Result<ReadReadme, Failure> + Send + 'static,
) -> CallToolResult {
    let what = format!("{} {}", release.name, release.version);
    let answered =
        crate::task::spawn_blocking(move || read().map(|read| respond(&release, &read, &ask)))
            .await;
    let answered = match answered {
        Ok(Ok(answered)) => answered,
        Ok(Err(failure)) => return failure.into_result(&what),
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the reading of a readme did not finish");
            return super::tool_error(
                "internal_error",
                "the readme could not be read (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            );
        },
    };
    answered.map_or_else(|error| *error, CallToolResult::structured)
}

/// `result`, an error of the sources that `get_source` reads too, with what to call instead of a
/// readme that is not to be had: its suggestions are of `get_source`'s files.
fn with_overview(result: CallToolResult) -> CallToolResult {
    if result.is_error != Some(true) {
        return result;
    }
    let body = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str::<Value>(&text.text).ok());
    match body {
        Some(Value::Object(mut body)) if body.get("code") == Some(&json!("docs_unavailable")) => {
            body.insert("suggestions".to_owned(), json!([OVERVIEW]));
            super::error_result(&Value::Object(body))
        },
        Some(_) | None => result,
    }
}

/// The tool error for a release that is not of crates.io, whose readme is not read yet.
fn not_a_release(release: &Release) -> CallToolResult {
    let of = if release.source.as_ref() == "crates.io" {
        "crates.io, but not the release that docs.rs documents: a copy that differs from it, or \
         that the crates.io index does not have"
    } else {
        &release.source
    };
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "build_unsupported",
        "message": format!(
            "{} {} is of {of}: get_readme reads the readme of a release of crates.io, from its \
             .crate",
            release.name, release.version
        ),
        "retryable": false,
        "suggestions": [OVERVIEW],
    }))
}

/// A readme read.
#[derive(Debug)]
struct ReadReadme {
    /// Its path relative to the package's root, with `/`.
    file: Box<str>,
    contents: Vec<u8>,
}

/// Why a readme was not read.
#[derive(Debug)]
enum Failure {
    /// The package has none that may be read (decided in M3e, decisions 68 and 76).
    NoReadme(NoReadme),
    /// The file is refused where it is (requirement R-SC-4).
    Refused(Box<str>, Refusal),
    /// The manifest could not be read.
    Manifest(io::Error),
    /// The file is larger than [`MAX_README_BYTES`].
    TooLarge(Box<str>),
    /// The file could not be read.
    Unreadable(Box<str>, io::Error),
}

impl Failure {
    /// The tool error for the readme of `what` (`name version`).
    fn into_result(self, what: &str) -> CallToolResult {
        let (reason, message) = match self {
            Self::NoReadme(why) => ("no_readme", format!("{what} has no readme to read: {why}")),
            Self::Refused(file, refusal) => (
                match refusal {
                    Refusal::Missing | Refusal::NotAFile => "source_missing",
                    Refusal::Outside
                    | Refusal::Dotfile
                    | Refusal::NotInPackage
                    | Refusal::SymlinkOutside
                    | Refusal::SymlinkInCache => "outside_package",
                },
                format!("the readme {file} of {what} is not read: {refusal}"),
            ),
            Self::Manifest(error) => (
                "corrupt",
                format!("a Cargo.toml of {what}, or of its workspace, could not be read: {error}"),
            ),
            Self::TooLarge(file) => (
                "too_large",
                format!("the readme {file} of {what} is larger than {MAX_README_BYTES} bytes"),
            ),
            Self::Unreadable(file, error) => (
                "source_missing",
                format!("the readme {file} of {what} could not be read: {error}"),
            ),
        };
        super::error_result(&json!({
            "code": "docs_unavailable",
            "reason": reason,
            "message": message,
            "retryable": false,
            "suggestions": [OVERVIEW],
        }))
    }
}

/// The readme of the release whose sources are `sources`, as its manifest names it (decided in
/// M3e, decisions 68 and 76): only a file of its `.crate`'s listing, judged as `get_source`
/// judges its files.
fn read_readme(sources: &Sources) -> Result<ReadReadme, Failure> {
    let root = Root::Release(sources);
    let manifest = source_root::resolve(root, Path::new("Cargo.toml"))
        .map_err(|refusal| Failure::Refused("Cargo.toml".into(), refusal))?;
    let declared = crate_file::manifest_readme(&manifest.path).map_err(Failure::Manifest)?;
    let files = &sources.listing.files;
    let file = readme::chosen(&declared, |name| {
        files
            .binary_search_by(|listed| listed.as_str().cmp(name))
            .is_ok()
    })
    .map_err(Failure::NoReadme)?;
    let asked = Path::new(file.as_ref()).to_path_buf();
    read_at(root, &asked, file)
}

/// The readme at `asked` under `root`, judged as `get_source` judges its files, shown as `file`.
fn read_at(root: Root<'_>, asked: &Path, file: Box<str>) -> Result<ReadReadme, Failure> {
    let resolved = source_root::resolve(root, asked).map_err(|refusal| match refusal {
        // A hidden directory on its way (decided in M3e, decision 76).
        Refusal::Dotfile => Failure::NoReadme(NoReadme::Hidden(file.clone())),
        Refusal::Outside
        | Refusal::NotInPackage
        | Refusal::SymlinkOutside
        | Refusal::SymlinkInCache
        | Refusal::Missing
        | Refusal::NotAFile => Failure::Refused(file.clone(), refusal),
    })?;
    // A readme that leads through a symlink to a file that is not a document is not read.
    if !readme::is_document(&resolved.path) {
        return Err(Failure::NoReadme(NoReadme::NotADocument(file)));
    }
    let mut contents = Vec::new();
    std::fs::File::open(&resolved.path)
        .and_then(|opened| {
            opened
                .take(MAX_README_BYTES.saturating_add(1))
                .read_to_end(&mut contents)
        })
        .map_err(|error| Failure::Unreadable(file.clone(), error))?;
    if u64::try_from(contents.len()).is_ok_and(|len| len > MAX_README_BYTES) {
        return Err(Failure::TooLarge(file));
    }
    Ok(ReadReadme { file, contents })
}

/// A page of a readme.
#[derive(Debug, PartialEq, Eq)]
struct Page {
    /// Its lines, marked (requirement R-SEC-7, decided in M3e, decision 66).
    text: String,
    /// The lines it starts and ends at, from 1; 0 and 0 for an empty readme.
    lines: (usize, usize),
    /// How many lines the readme has.
    total: usize,
    /// The line cut within, as longer than a page alone, if one was.
    cut_line: Option<usize>,
    /// How many characters were marked.
    marked: usize,
}

/// The page of `text` from the line `from` (from 0), of at most `max` characters, ending at a line
/// of `starts` (from 0), the starts of its sections, as [`markdown::page_end`] cuts it.
fn page(text: &str, starts: &[usize], from: usize, max: usize) -> Page {
    let lines: Vec<(Cow<'_, str>, usize)> = text.split_inclusive('\n').map(hidden::mark).collect();
    let lengths: Vec<usize> = lines.iter().map(|(line, _)| line.chars().count()).collect();
    let total = lines.len();
    let from = from.min(total);
    let end = markdown::page_end(&lengths, starts, from, max);
    let mut page_text = String::new();
    let mut cut_line = None;
    let mut marked = 0_usize;
    for (line, count) in lines.get(from..end).unwrap_or_default() {
        if line.chars().count() > max {
            page_text.push_str(get_source::cut(line, max));
            cut_line = Some(from.saturating_add(1));
        } else {
            page_text.push_str(line);
        }
        marked = marked.saturating_add(*count);
    }
    Page {
        text: page_text,
        lines: if end > from {
            (from.saturating_add(1), end)
        } else {
            (0, 0)
        },
        total,
        cut_line,
        marked,
    }
}

/// The heading of `outline` that `asked` names: the same, but for case and what [`plain`] leaves
/// out; else the first that starts with it; else the first that has it.
fn section<'h>(outline: &'h [(Heading, String)], asked: &str) -> Option<&'h Heading> {
    let asked = hidden::mark(&plain(asked)).0.to_lowercase();
    let shown: Vec<(&Heading, String)> = outline
        .iter()
        .map(|(heading, shown)| (heading, shown.to_lowercase()))
        .collect();
    let find = |matches: &dyn Fn(&str) -> bool| {
        shown
            .iter()
            .find(|(_, shown)| matches(shown))
            .map(|(heading, _)| *heading)
    };
    find(&|shown| shown == asked)
        .or_else(|| find(&|shown| shown.starts_with(&asked)))
        .or_else(|| find(&|shown| shown.contains(&asked)))
}

/// Whether the readme at `file` is Markdown, which has an outline (decided in M3e, decision 65).
fn is_markdown(file: &str) -> bool {
    Path::new(file).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

/// The headings of `text` as the outline shows them, if it is Markdown, marked as the text is
/// (decided in M3e, decision 66), and how many characters were marked.
fn outline_of(text: &str, markdown: bool) -> (Vec<(Heading, String)>, usize) {
    let mut marked = 0_usize;
    if !markdown {
        return (Vec::new(), marked);
    }
    let outline = markdown::outline(text)
        .into_iter()
        .map(|heading| {
            let plain = plain(&heading.text);
            let (shown, count) = hidden::mark(&plain);
            marked = marked.saturating_add(count);
            let shown = get_source::cut(&shown, MAX_HEADING_CHARS).to_owned();
            (heading, shown)
        })
        .collect();
    (outline, marked)
}

/// The line (from 0) that the page asked for by `ask` starts at, in a readme of `total` lines
/// whose digest is `digest` and whose headings are `outline`: the cursor's, which must be of the
/// same readme and not at its end or past it; the section's; or the first.
fn start_of(
    ask: &ReadmeAsk,
    (outline, digest, total): (&[(Heading, String)], &str, usize),
) -> Result<usize, Box<CallToolResult>> {
    let refused = |message: &'static str| {
        Err(Box::new(
            InvalidArgument::new(message, CURSOR_EXAMPLE).into_result(),
        ))
    };
    match (ask.offset, &ask.section) {
        (Some(_), _) if ask.cursor_data.as_deref() != Some(digest) => refused(
            "the readme changed since the cursor was made; read it again without the cursor",
        ),
        (Some(offset), _) if offset >= total => {
            refused("the cursor goes past the end of the readme; read it again without the cursor")
        },
        (Some(offset), _) => Ok(offset),
        (None, Some(asked)) => section(outline, asked).map_or_else(
            || Err(Box::new(no_section(asked, outline))),
            |heading| Ok(heading.line.saturating_sub(1)),
        ),
        (None, None) => Ok(0),
    }
}

/// What the answer shows of a readme.
struct Shown<'a> {
    release: &'a Release,
    file: &'a str,
    /// Whether it is Markdown, which has an outline.
    markdown: bool,
    /// Its headings, of which the first `outline_count` are shown, on a page asked for without a
    /// cursor; the rest are counted.
    outline: &'a [(Heading, String)],
    outline_count: Option<usize>,
    page: &'a Page,
    /// The cursor of the next page, if there is one.
    next_cursor: Option<String>,
    /// Whether the readme is not UTF-8.
    lossy: bool,
    /// How many characters the outline marked.
    outline_marked: usize,
}

/// The answer for the readme `read` of `release`, as `ask` asks: the page from the cursor or the
/// section, with the outline when there is no cursor; within the budget of a response, the
/// outline cut first, then the page (decided in M3e, decisions 63 to 66, requirement R-OUT-3).
fn respond(
    release: &Release,
    read: &ReadReadme,
    ask: &ReadmeAsk,
) -> Result<Value, Box<CallToolResult>> {
    let decoded = String::from_utf8_lossy(&read.contents);
    let lossy = matches!(decoded, Cow::Owned(_));
    // A byte order mark is not text, as the outline has it.
    let text = decoded.strip_prefix('\u{feff}').unwrap_or(&decoded);
    let markdown = is_markdown(&read.file);
    let (outline, outline_marked) = outline_of(text, markdown);
    let digest = sha256_hex(&read.contents)
        .get(..16)
        .unwrap_or_default()
        .to_owned();
    let starts: Vec<usize> = outline
        .iter()
        .map(|(heading, _)| heading.line.saturating_sub(1))
        .collect();
    let total = text.split_inclusive('\n').count();
    let from = start_of(ask, (&outline, &digest, total))?;
    let mut max = MAX_CHARS;
    let mut outline_count =
        (markdown && ask.offset.is_none()).then(|| outline.len().min(MAX_OUTLINE));
    loop {
        let page = page(text, &starts, from, max);
        let next_cursor = (page.lines.1 < page.total)
            .then(|| format!("{}.{digest}.{}", ask.request_digest, page.lines.1));
        let answer = answer_of(&Shown {
            release,
            file: &read.file,
            markdown,
            outline: &outline,
            outline_count,
            page: &page,
            next_cursor,
            lossy,
            outline_marked,
        });
        let size = answer.to_string().chars().count();
        if size <= MAX_RESPONSE_CHARS {
            return Ok(answer);
        }
        // The outline first, by half; then the page, in proportion to what is over.
        match outline_count {
            Some(shown) if shown > 0 => outline_count = Some(shown >> 1),
            _ if max > MIN_PAGE_CHARS => {
                let fits = max
                    .saturating_mul(MAX_RESPONSE_CHARS)
                    .checked_div(size)
                    .unwrap_or_default();
                max = fits.clamp(MIN_PAGE_CHARS, max.saturating_sub(1));
            },
            _ => return Ok(answer),
        }
    }
}

/// The answer that `shown` tells.
fn answer_of(shown: &Shown<'_>) -> Value {
    let Shown { release, page, .. } = shown;
    let mut data = Map::new();
    data.insert("file".to_owned(), json!(shown.file));
    data.insert(
        "format".to_owned(),
        json!(if shown.markdown { "markdown" } else { "text" }),
    );
    if let Some(count) = shown.outline_count {
        let listed: Vec<Value> = shown
            .outline
            .iter()
            .take(count)
            .map(|(heading, text)| {
                json!({ "level": heading.level, "heading": text, "line": heading.line })
            })
            .collect();
        data.insert("outline".to_owned(), Value::Array(listed));
        if shown.outline.len() > count {
            data.insert("outline_total".to_owned(), json!(shown.outline.len()));
        }
    }
    data.insert("text".to_owned(), json!(page.text));
    data.insert(
        "truncated".to_owned(),
        json!(shown.next_cursor.is_some() || page.cut_line.is_some()),
    );
    if let Some(line) = page.cut_line {
        data.insert("cut_line".to_owned(), json!(line));
    }
    let mut paging = Map::new();
    paging.insert("first_line".to_owned(), json!(page.lines.0));
    paging.insert("last_line".to_owned(), json!(page.lines.1));
    paging.insert("total_lines".to_owned(), json!(page.total));
    paging.insert("has_more".to_owned(), json!(shown.next_cursor.is_some()));
    if let Some(cursor) = &shown.next_cursor {
        paging.insert("next_cursor".to_owned(), json!(cursor));
    }
    let mut warnings = release.warnings.clone();
    if shown.lossy {
        warnings.push("the readme is not UTF-8: what is not is shown as U+FFFD".into());
    }
    // Of the page and of the outline, where a heading of the page is again.
    let marked = page.marked.saturating_add(shown.outline_marked);
    if marked > 0 {
        warnings.push(hidden::marked_warning(marked).into());
    }
    if let Some(line) = page.cut_line {
        warnings.push(format!("line {line} is longer than a page alone, and was cut").into());
    }
    let mut answer = Map::new();
    answer.insert(
        "provenance".to_owned(),
        Value::Object(release.provenance.clone()),
    );
    answer.insert("data".to_owned(), Value::Object(data));
    answer.insert("page".to_owned(), Value::Object(paging));
    if !release.hints.is_empty() {
        answer.insert("hints".to_owned(), json!(release.hints));
    }
    let mut answer = Value::Object(answer);
    get_item::add_warnings(&mut answer, &warnings);
    answer
}

/// The tool error for `asked`, a section that `outline` does not have, with its headings.
fn no_section(asked: &str, outline: &[(Heading, String)]) -> CallToolResult {
    let headings: Vec<&str> = outline
        .iter()
        .map(|(_, shown)| shown.as_str())
        .filter(|shown| !shown.is_empty())
        .take(20)
        .collect();
    let suggestion = if headings.is_empty() {
        "the readme has no headings: read it without `section`".to_owned()
    } else {
        format!("its headings: {}", headings.join(" | "))
    };
    super::tool_error(
        "invalid_argument",
        &format!("the readme has no heading like `{asked}`"),
        false,
        &[&suggestion],
    )
}

/// The readmes of the packages built here: the project's members, path and git dependencies, and
/// the crates of git that `build_external_docs` recorded, where they are (decided in M3e, decisions
/// 78 to 80). No cargo is run but what resolved the version.
#[cfg(feature = "local")]
mod here {
    use std::path::{Path, PathBuf};

    use rmcp::model::CallToolResult;

    use super::{Failure, ReadReadme, read_at};
    pub(super) use crate::mcp::tool::here::{
        Here,
        HereRoot,
        NotHere,
        of_git,
        of_project,
        unpacked_from_crates_io,
    };
    use crate::{
        data::{
            crate_file,
            project as workspaces,
            readme::{self, DEFAULT_NAMES, Declared, NoReadme},
            source_root::{self, Root},
        },
        mcp::tool::get_source,
    };

    /// The tool error for the readme of `what`, of a copy from `of`, which is not read (decided in
    /// M3e, decision 79, as in M3c, decision 31).
    pub(super) fn copy(what: &str, of: &str) -> Box<CallToolResult> {
        Box::new(crate::mcp::tool::error_result(&serde_json::json!({
            "code": "docs_unavailable",
            "reason": "outside_package",
            "message": format!("the readme of {what} is not read: the project uses {of}"),
            "retryable": false,
            "suggestions": [super::OVERVIEW],
        })))
    }

    /// The readme of the package `here`: as `cargo metadata` resolved it, or else as its manifest
    /// names it, inherited from its workspace too, or of a default name; judged as `get_source`
    /// judges its files, and shown relative to its directory (`../README.md`).
    pub(super) fn read(here: &Here) -> Result<ReadReadme, Failure> {
        let dir = &here.dir;
        let (named, path, inherited_from) = match &here.readme {
            Some(resolved) => (resolved.display().to_string(), dir.join(resolved), None),
            None => declared(here)?,
        };
        let file = get_source::relative_to(&path, dir);
        readme::judged(&named, &file).map_err(Failure::NoReadme)?;
        let file: Box<str> = file.into();
        match &here.root {
            HereRoot::Package { workspace } => {
                // The project's workspace; else, for one outside it, its own directory, or the
                // workspace that lists it, if it inherits its readme (decided in M3e, decision 78).
                let root = match (workspace, inherited_from) {
                    (Some(root), _) => root.clone(),
                    (None, Some(root)) => root,
                    (None, None) => inherited_root(dir)?.unwrap_or_else(|| dir.clone()),
                };
                let package = source_root::Package {
                    root,
                    dir: dir.clone(),
                    readme: Some(path.clone()),
                    ..source_root::Package::default()
                };
                read_at(Root::Package(&package), &path, file)
            },
            HereRoot::Checkout(checkout) => read_at(Root::Checkout(checkout), &path, file),
        }
    }

    /// The root of the workspace that lists the package in `dir`, if its manifest inherits the
    /// readme from it (decided in M3e, decision 78).
    fn inherited_root(dir: &Path) -> Result<Option<PathBuf>, Failure> {
        let declared =
            crate_file::manifest_readme(&dir.join("Cargo.toml")).map_err(Failure::Manifest)?;
        if declared != Declared::Workspace {
            return Ok(None);
        }
        workspaces::listing_workspace(dir, None)
            .map_err(|error| Failure::Manifest(std::io::Error::other(error)))
    }

    /// The readme that the manifest of `here` names, as written, where it is, and the root of the
    /// workspace that it is inherited from, if it is: one that lists the package, no higher than
    /// the checkout of a git package (decided in M3e, decision 80); or of a default name in its
    /// directory.
    fn declared(here: &Here) -> Result<(String, PathBuf, Option<PathBuf>), Failure> {
        let dir = &here.dir;
        let declared =
            crate_file::manifest_readme(&dir.join("Cargo.toml")).map_err(Failure::Manifest)?;
        match declared {
            Declared::Path(path) => Ok((path.to_string(), dir.join(&*path), None)),
            Declared::Disabled => Err(Failure::NoReadme(NoReadme::Disabled)),
            Declared::Default => DEFAULT_NAMES
                .into_iter()
                .find(|name| dir.join(name).is_file())
                .map(|name| (name.to_owned(), dir.join(name), None))
                .ok_or(Failure::NoReadme(NoReadme::NoDefault)),
            Declared::Workspace => {
                let root = match &here.root {
                    HereRoot::Checkout(checkout) => {
                        workspaces::listing_workspace(dir, Some(checkout))
                    },
                    HereRoot::Package {
                        workspace: Some(root),
                    } => Ok(Some(root.clone())),
                    HereRoot::Package { workspace: None } => {
                        workspaces::listing_workspace(dir, None)
                    },
                }
                .map_err(|error| Failure::Manifest(std::io::Error::other(error)))?
                .ok_or(Failure::NoReadme(NoReadme::NotInherited))?;
                let inherited = crate_file::workspace_readme(&root.join("Cargo.toml"))
                    .map_err(Failure::Manifest)?;
                match inherited {
                    Some(Declared::Path(path)) => {
                        Ok((path.to_string(), root.join(&*path), Some(root)))
                    },
                    Some(Declared::Disabled) => Err(Failure::NoReadme(NoReadme::Disabled)),
                    Some(Declared::Default | Declared::Workspace) | None => {
                        Err(Failure::NoReadme(NoReadme::NotInherited))
                    },
                }
            },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::data::source_root::Refusal;

        /// Writes `files` under a new temporary directory, whose path is returned as written.
        fn tree(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
            let dir = tempfile::tempdir().expect("a temporary directory");
            for (path, contents) in files {
                let path = dir.path().join(path);
                std::fs::create_dir_all(path.parent().expect("a parent")).expect("made");
                std::fs::write(path, contents).expect("written");
            }
            let root = source_root::lexically(dir.path());
            (dir, root)
        }

        fn outside(dir: PathBuf, resolved: Option<&str>) -> Here {
            Here {
                dir,
                root: HereRoot::Package { workspace: None },
                readme: resolved.map(PathBuf::from),
                examples: None,
            }
        }

        /// A path dependency outside the project reads its readme under its own directory, or,
        /// inherited, under the workspace that lists it; a manifest that names `/` as its
        /// workspace does not widen what is read (decided in M3e, decision 78).
        #[test]
        fn a_path_dependency_outside_reads_no_higher_than_it_may() {
            let (_dir, root) = tree(&[
                ("secret.md", "secret\n"),
                (
                    "dep/Cargo.toml",
                    "[package]\nname = \"dep\"\nworkspace = \"/\"\nreadme = \"../secret.md\"\n",
                ),
                (
                    "ws/Cargo.toml",
                    "[workspace]\nmembers = [\"m\"]\n[workspace.package]\nreadme = \"README.md\"\n",
                ),
                ("ws/README.md", "# Inherited\n"),
                (
                    "ws/m/Cargo.toml",
                    "[package]\nname = \"m\"\nreadme.workspace = true\n",
                ),
            ]);
            // As cargo metadata resolves it, and as its manifest names it.
            for resolved in [Some("../secret.md"), None] {
                let read = read(&outside(root.join("dep"), resolved));
                assert!(
                    matches!(read, Err(Failure::Refused(_, Refusal::Outside))),
                    "{read:?}"
                );
            }
            for resolved in [Some("../README.md"), None] {
                let read = read(&outside(root.join("ws/m"), resolved)).expect("read");
                assert_eq!(&*read.file, "../README.md");
                assert_eq!(read.contents, b"# Inherited\n");
            }
        }

        /// A readme that leads through a symlink to a file that is not a document is not read.
        #[cfg(unix)]
        #[test]
        fn a_readme_that_links_to_another_file_is_not_read() {
            let (_dir, root) = tree(&[
                ("dep/Cargo.toml", "[package]\nname = \"dep\"\n"),
                ("dep/Cargo.lock", "# lock\n"),
            ]);
            std::os::unix::fs::symlink(root.join("dep/Cargo.lock"), root.join("dep/README.md"))
                .expect("linked");
            let read = read(&outside(root.join("dep"), None));
            assert!(
                matches!(read, Err(Failure::NoReadme(NoReadme::NotADocument(_)))),
                "{read:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_fits_the_limit() {
        let len = super::super::for_this_build(DESCRIPTION).chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    /// A heading as the outline shows it: badges and HTML left out, links' text kept, entities
    /// and the marks of emphasis and code left out (decided in M3e, decision 63).
    #[test]
    fn a_heading_is_shown_plain() {
        for (heading, shown) in [
            (
                "Serde &emsp; [![Build Status]][actions] [![Latest Version]][crates.io]",
                "Serde",
            ),
            ("Getting `started`:", "Getting started"),
            ("[Tokio](https://tokio.rs) *runtime*", "Tokio runtime"),
            (r#"<img src="logo.png"> Axum"#, "Axum"),
            ("[not a link] at all", "[not a link] at all"),
            ("serde_json", "serde_json"),
        ] {
            assert_eq!(plain(heading), shown, "{heading}");
        }
    }

    /// `section` names a heading by its plain text without case, or by its start, or by a part.
    #[test]
    fn a_section_is_found_by_its_heading() {
        let outline: Vec<(Heading, String)> = markdown::outline(
            "# Serde [![Build]](x)\n## Getting started\n## Started? No\n### Feature flags\n",
        )
        .into_iter()
        .map(|heading| {
            let shown = plain(&heading.text);
            (heading, shown)
        })
        .collect();
        let line = |asked: &str| section(&outline, asked).map(|heading| heading.line);
        assert_eq!(line("serde"), Some(1));
        assert_eq!(line("GETTING STARTED"), Some(2));
        assert_eq!(line("getting"), Some(2));
        assert_eq!(line("flags"), Some(4));
        assert_eq!(line("started"), Some(3));
        assert_eq!(line("license"), None);
    }

    /// A page ends at a heading within the budget, marks what is hidden, and cuts a line longer
    /// than a page alone (decided in M3e, decisions 64 and 66).
    #[test]
    fn a_page_is_cut_at_a_heading_and_marked() {
        let section = "x".repeat(10_010);
        let text = format!("# A\n{section}\n# B \u{202e}\n{section}\n# C\nend\n");
        let starts = [0, 2, 4];
        let first = page(&text, &starts, 0, MAX_CHARS);
        assert_eq!(first.lines, (1, 2));
        assert_eq!(first.total, 6);
        assert!(first.text.ends_with(&format!("{section}\n")));
        assert_eq!(first.marked, 0);
        let second = page(&text, &starts, 2, MAX_CHARS);
        assert_eq!(second.lines, (3, 6));
        assert!(second.text.starts_with("# B <U+202E>\n"), "{}", second.text);
        assert_eq!(second.marked, 1);
        let long = format!("# A\n{}\n", "y".repeat(MAX_CHARS + 5));
        let cut = page(&long, &[0], 1, MAX_CHARS);
        assert_eq!(cut.lines, (2, 2));
        assert_eq!(cut.cut_line, Some(2));
        assert_eq!(cut.text.chars().count(), MAX_CHARS);
        let empty = page("", &[], 0, MAX_CHARS);
        assert_eq!((empty.lines, empty.total), ((0, 0), 0));
    }

    /// The answer: the outline of a Markdown readme, a page, and the cursor of the rest, which
    /// a readme changed since refuses; a section that it lacks, with its headings.
    #[test]
    fn the_readme_is_answered_a_page_at_a_time() {
        let release = Release {
            provenance: Map::new(),
            warnings: Vec::new(),
            hints: Vec::new(),
            name: "demo".parse().expect("a name"),
            version: semver::Version::new(1, 0, 0),
            source: "crates.io".into(),
            on_crates_io: true,
            docs_rs_json: "available".into(),
            retry_after: None,
        };
        let body = "z".repeat(MAX_CHARS - 10);
        let read = ReadReadme {
            file: "README.md".into(),
            contents: format!("# Demo\nintro\n## Usage\n{body}\n## License\nMIT\n").into_bytes(),
        };
        let ask = |arguments: Value| {
            let Value::Object(arguments) = arguments else {
                panic!("an object");
            };
            parse_arguments(Some(&arguments)).expect("valid").ask
        };
        let first = respond(&release, &read, &ask(json!({ "crate": "demo" }))).expect("answered");
        let data = &first["data"];
        assert_eq!(data["format"], "markdown");
        assert_eq!(
            data["outline"],
            json!([
                { "level": 1, "heading": "Demo", "line": 1 },
                { "level": 2, "heading": "Usage", "line": 3 },
                { "level": 2, "heading": "License", "line": 5 },
            ])
        );
        assert_eq!(data["text"], "# Demo\nintro\n");
        assert_eq!(data["truncated"], true);
        assert_eq!(first["page"]["last_line"], 2);
        let cursor = first["page"]["next_cursor"].as_str().expect("a cursor");
        let rest = respond(
            &release,
            &read,
            &ask(json!({ "crate": "demo", "cursor": cursor })),
        )
        .expect("answered");
        assert_eq!(rest["page"]["first_line"], 3);
        assert_eq!(rest["page"]["has_more"], true);
        // From a section.
        let license = respond(
            &release,
            &read,
            &ask(json!({ "crate": "demo", "section": "license" })),
        )
        .expect("answered");
        assert_eq!(license["data"]["text"], "## License\nMIT\n");
        assert_eq!(license["data"]["truncated"], false);
        assert!(license["page"].get("next_cursor").is_none());
        // The readme changed since the cursor was made.
        let changed = ReadReadme {
            file: "README.md".into(),
            contents: b"# Other\n".to_vec(),
        };
        let refused = respond(
            &release,
            &changed,
            &ask(json!({ "crate": "demo", "cursor": cursor })),
        )
        .expect_err("refused");
        assert!(
            format!("{refused:?}").contains("changed since"),
            "{refused:?}"
        );
        // A cursor at the end, as made by hand, is refused rather than going round.
        let end = cursor
            .rsplit_once('.')
            .map(|(head, _)| format!("{head}.6"))
            .expect("made");
        let past = respond(
            &release,
            &read,
            &ask(json!({ "crate": "demo", "cursor": end })),
        )
        .expect_err("refused");
        assert!(format!("{past:?}").contains("past the end"), "{past:?}");
        // The outline is on the first page only.
        assert!(rest["data"].get("outline").is_none(), "{rest}");
        let missing = respond(
            &release,
            &read,
            &ask(json!({ "crate": "demo", "section": "Changelog" })),
        )
        .expect_err("refused");
        assert!(
            format!("{missing:?}").contains("Demo | Usage | License"),
            "{missing:?}"
        );
        // Not Markdown: no outline.
        let text = ReadReadme {
            file: "README.txt".into(),
            contents: b"# not a heading\n".to_vec(),
        };
        let plain_text =
            respond(&release, &text, &ask(json!({ "crate": "demo" }))).expect("answered");
        assert_eq!(plain_text["data"]["format"], "text");
        assert!(plain_text["data"].get("outline").is_none());
    }

    /// Why a readme is not read, as the reason of `docs_unavailable`: none to read, refused where
    /// it is, or too large (decided in M3e, decisions 68 and 76).
    #[test]
    fn a_readme_not_read_is_told_why() {
        let body = |failure: Failure| -> Value {
            let result = failure.into_result("demo 1.0.0");
            let text = result
                .content
                .first()
                .and_then(|content| content.as_text())
                .map(|text| text.text.clone())
                .unwrap_or_default();
            serde_json::from_str(&text).expect("JSON")
        };
        let hidden = body(Failure::NoReadme(NoReadme::Hidden(
            ".github/README.md".into(),
        )));
        assert_eq!(hidden["code"], "docs_unavailable");
        assert_eq!(hidden["reason"], "no_readme");
        assert_eq!(hidden["retryable"], false);
        assert!(
            hidden["message"]
                .as_str()
                .is_some_and(|message| message.contains("`.github/README.md`")),
            "{hidden}"
        );
        assert!(hidden["suggestions"].to_string().contains("crate_overview"));
        let disabled = body(Failure::NoReadme(NoReadme::Disabled));
        assert!(
            disabled["message"].to_string().contains("readme = false"),
            "{disabled}"
        );
        for (failure, reason) in [
            (
                Failure::Refused("README.md".into(), Refusal::SymlinkOutside),
                "outside_package",
            ),
            (
                Failure::Refused("README.md".into(), Refusal::Missing),
                "source_missing",
            ),
            (Failure::TooLarge("README.md".into()), "too_large"),
        ] {
            assert_eq!(body(failure)["reason"], reason);
        }
    }

    /// A heading of megabytes, nested or not, is made plain at once, of its start only.
    #[test]
    fn a_huge_heading_is_made_plain_quickly() {
        let started = std::time::Instant::now();
        let nested = format!("{}a{}", "[".repeat(200_000), "](x)".repeat(200_000));
        for heading in [nested, "[".repeat(1_000_000), "<&".repeat(500_000)] {
            assert!(plain(&heading).chars().count() <= markdown::MAX_PLAIN_INPUT);
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    /// A readme of long headings and a full page is answered within the budget of a response: the
    /// outline is cut first, then the page (requirement R-OUT-3).
    #[test]
    fn a_large_readme_is_answered_within_the_budget() {
        let release = Release {
            provenance: Map::new(),
            warnings: Vec::new(),
            hints: Vec::new(),
            name: "demo".parse().expect("a name"),
            version: semver::Version::new(1, 0, 0),
            source: "crates.io".into(),
            on_crates_io: true,
            docs_rs_json: "available".into(),
            retry_after: None,
        };
        let mut contents = String::new();
        for n in 0..150 {
            contents.push_str("# ");
            contents.push_str(&n.to_string());
            contents.push(' ');
            contents.push_str(&"h".repeat(200));
            contents.push('\n');
        }
        // Quotes, which JSON writes as two characters each.
        contents.push_str(&"\"".repeat(30_000));
        contents.push('\n');
        let read = ReadReadme {
            file: "README.md".into(),
            contents: contents.into_bytes(),
        };
        let Value::Object(arguments) = json!({ "crate": "demo" }) else {
            panic!("an object");
        };
        let ask = parse_arguments(Some(&arguments)).expect("valid").ask;
        let answer = respond(&release, &read, &ask).expect("answered");
        assert!(answer.to_string().chars().count() <= MAX_RESPONSE_CHARS);
        assert_eq!(answer["data"]["outline_total"], 150);
        let shown = answer["data"]["outline"].as_array().map_or(0, Vec::len);
        assert!(shown < MAX_OUTLINE, "{shown}");
    }

    /// std's crates have no readme; `section` is a heading (decided in M3e, decision 68).
    #[test]
    fn std_and_odd_arguments_are_refused() {
        for (arguments, part) in [
            (json!({ "crate": "std" }), "no readme"),
            (json!({ "crate": "tokio", "section": " ** " }), "`section`"),
            (json!({ "crate": "tokio", "section": 1 }), "`section`"),
            (json!({ "crate": "tokio", "cursor": 1 }), "`cursor`"),
        ] {
            let Value::Object(arguments) = arguments else {
                panic!("an object");
            };
            let error = parse_arguments(Some(&arguments)).expect_err("invalid");
            assert!(error.message.contains(part), "{error:?}");
        }
    }
}
