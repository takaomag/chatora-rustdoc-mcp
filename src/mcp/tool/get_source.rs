//! The `get_source` tool: the source code of an item, or lines of a file of its crate
//! (requirements R-SC-1 to R-SC-5, §3 Q10, decided in M3c).
//!
//! The item is looked up as `get_item` looks it up, following a re-export into the crate that
//! defines it; then the file that its span names is read from the crate's sources, as
//! `source_root` judges them (requirement R-SC-4), and the lines are cut out, marked
//! (requirement R-SEC-7) and cut short (requirement R-SC-5).

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use semver::Version;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Arguments},
};
use crate::data::{
    crate_file::CrateError,
    crate_name::CrateName,
    crate_source::{self, Sources, SourcesError, UnpackError},
    flight::Joined,
    hidden,
    source_root::{self, Refusal, Root},
};
#[cfg(feature = "local")]
use crate::data::{metadata::Resolution, project::Workspace};
#[cfg(feature = "local")]
use crate::query::local::LocalBuilt;

/// The name of the tool.
pub(super) const NAME: &str = "get_source";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("get_source.md");

/// The most lines added before and after (decided in M3c, decision 27).
const MAX_CONTEXT: usize = 200;

/// The most characters of source returned, after the marks (requirement R-SC-5, decided in M3c,
/// decision 26).
const MAX_CHARS: usize = 20_000;

/// How to pass `path` or `file`, for `invalid_argument`.
const WHAT_EXAMPLE: &[&str] = &[
    r#"pass the path of an item, for example {"path": "tokio::sync::Mutex::lock"}"#,
    r#"or a file of the crate, for example {"file": "src/sync/mutex.rs", "lines": "434-466"}"#,
];

/// How to pass `lines`, for `invalid_argument`.
const LINES_EXAMPLE: &[&str] = &[r#"pass a line or a range, for example {"lines": "434-466"}"#];

/// The sources of a crates.io release being fetched and unpacked, shared by the calls that want
/// them at once (requirements R-NW-2 and R-WS-6).
pub(super) type SharedSources = Arc<Result<Sources, SourcesError>>;

/// What is read, besides the item or the crate that `get_item` looks up (decided in M3c,
/// decisions 24 and 27).
#[derive(Debug, Clone)]
pub(super) struct SourceAsk {
    /// Lines added before and after.
    context: usize,
    /// The file asked for, if one was: else the item's.
    file: Option<AskedFile>,
}

/// A file asked for, relative to the package's root, and its lines: all of them if `None`.
#[derive(Debug, Clone)]
struct AskedFile {
    file: Box<str>,
    lines: Option<(usize, usize)>,
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
            "path": {
                "type": "string",
                "description": "The item (tokio::sync::Mutex::lock). The crate name may be left out. \
                                Or pass file.",
            },
            "file": {
                "type": "string",
                "description": "A file of the crate, relative to its package's root \
                                (src/sync/mutex.rs), as source_location.file of get_item. Or pass \
                                path.",
            },
            "lines": {
                "type": "string",
                "description": "With file: the lines (434-466, or 301). The whole file if left out.",
            },
            "context": {
                "type": "integer",
                "minimum": 0,
                "maximum": MAX_CONTEXT,
                "description": "Lines added before and after (default 0).",
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
                    "The target triple to document for.",
                    super::TARGET_DEFAULT
                ),
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Show a Rust item's source")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": get_item::provenance_schema(),
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "item": { "type": "string" },
                    "defined_at": get_item::defined_at_schema(),
                    "kind": { "type": "string" },
                    "file": { "type": "string" },
                    "lines": { "type": "string" },
                    "shown": { "type": "string" },
                    "source": { "type": "string" },
                    "path": { "type": "string" },
                    "url": { "type": "string" },
                    "truncated": { "type": "boolean" },
                },
                "required": ["file", "lines", "shown", "source"],
            },
            "hints": strings,
        },
        "required": ["provenance", "data"],
    }))
}

/// The object inside `value`, which is always an object here.
fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(object) => object,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            JsonObject::new()
        },
    }
}

/// Checks the arguments (requirement R-ARG-1): `path` or `file`, and `lines` and `context`.
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<Arguments, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let text = |name: &'static str| match argument(name) {
        None => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim())),
        Some(_) => Err(InvalidArgument::new(
            format!("`{name}` must be a string that is not empty"),
            WHAT_EXAMPLE,
        )),
    };
    let context = match argument("context") {
        None => 0,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value <= MAX_CONTEXT)
            .ok_or_else(|| {
                InvalidArgument::new(
                    format!("`context` must be a whole number from 0 to {MAX_CONTEXT}"),
                    &[r#"pass a whole number, for example {"context": 5}"#],
                )
            })?,
    };
    let lines = match text("lines")? {
        None => None,
        Some(lines) => Some(parse_lines(lines).ok_or_else(|| {
            InvalidArgument::new(
                format!("`lines` is {lines:?}, not a line or a range of lines"),
                LINES_EXAMPLE,
            )
        })?),
    };
    match (text("path")?, text("file")?) {
        (Some(path), None) => {
            if lines.is_some() {
                return Err(InvalidArgument::new(
                    "`lines` goes with `file`; the lines of `path` are the item's",
                    WHAT_EXAMPLE,
                ));
            }
            Ok(Arguments::for_source(release, path, SourceAsk {
                context,
                file: None,
            }))
        },
        (None, Some(file)) => {
            // The crate's root is looked up, for its release; the file is read.
            let root = release.name.as_str().replace('-', "_");
            Ok(Arguments::for_source(release, &root, SourceAsk {
                context,
                file: Some(AskedFile {
                    file: file.into(),
                    lines,
                }),
            }))
        },
        (Some(_), Some(_)) => Err(InvalidArgument::new(
            "pass either `path` or `file`, not both",
            WHAT_EXAMPLE,
        )),
        (None, None) => Err(InvalidArgument::new(
            "`path` or `file` is required",
            WHAT_EXAMPLE,
        )),
    }
}

/// The lines in `lines`: `434-466` or `301`, from 1, the first not after the last.
fn parse_lines(lines: &str) -> Option<(usize, usize)> {
    let number = |text: &str| text.trim().parse::<usize>().ok().filter(|line| *line > 0);
    let (first, last) = if let Some((first, last)) = lines.split_once('-') {
        (number(first)?, number(last)?)
    } else {
        let line = number(lines)?;
        (line, line)
    };
    (first <= last).then_some((first, last))
}

/// Reads the source as `get_item` looks the item up: the same version, target and cache, and
/// following an item re-exported from another crate.
pub(super) async fn execute(arguments: Arguments, state: Arc<ToolState>) -> CallToolResult {
    get_item::execute(arguments, state).await
}

/// Where the item that `get_item` found is, to read its source from.
pub(super) struct Found<'a> {
    /// The crate, as crates.io spells it.
    pub(super) name: &'a CrateName,
    pub(super) version: &'a Version,
    /// The sha256 of its `.crate` in the crates.io index, when the index was read.
    pub(super) checksum: Option<&'a str>,
    /// Where its documentation came from.
    pub(super) docs: Source,
    /// Until when the call waits for the sources (requirement R-ARCH-6): the lookup's deadline.
    pub(super) until: Instant,
    /// The build here, for [`Source::Local`].
    #[cfg(feature = "local")]
    pub(super) built: Option<Arc<LocalBuilt>>,
    /// The workspace whose `cargo metadata` tells the build's package, when the project decided
    /// the version.
    #[cfg(feature = "local")]
    pub(super) workspace: Option<&'a Workspace>,
}

/// The sources whose files may be read (requirement R-SC-4).
enum Readable {
    /// Of a crates.io release.
    Release(Sources),
    /// Of a package built here, under the directory of its `Cargo.toml`, `dir`, in the workspace
    /// at `workspace_root`, which its spans outside it are relative to.
    #[cfg(feature = "local")]
    Package {
        package: source_root::Package,
        dir: PathBuf,
        workspace_root: PathBuf,
    },
}

/// A file read.
struct ReadFile {
    /// What `source_root` judged of it: its real path.
    resolved: source_root::Resolved,
    /// Its contents.
    contents: Vec<u8>,
    /// Its path relative to its package's root, with `/`.
    file: String,
}

/// Where the documentation that the item was found in came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    /// docs.rs, built from the release's `.crate`.
    DocsRs,
    /// A build here.
    Local,
    /// The std of a nightly.
    Std,
}

/// The response of `get_source` for the item that `get_item` found, `item` (its response), in
/// `found`: its source read, or why not. `warnings` go first among the response's.
pub(super) async fn read(
    state: &Arc<ToolState>,
    found: Found<'_>,
    ask: &SourceAsk,
    item: Value,
    warnings: &[Box<str>],
) -> CallToolResult {
    let provenance = item
        .get("provenance")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let data = item.get("data").cloned().unwrap_or(Value::Null);
    let asked = match where_to_read(ask, &data) {
        Ok(asked) => asked,
        Err(answer) => return *answer,
    };
    let readable = match found.docs {
        Source::DocsRs => {
            match release_sources(state, &found, provenance.clone(), warnings).await {
                Ok(sources) => Readable::Release(sources),
                Err(answer) => return *answer,
            }
        },
        #[cfg(feature = "local")]
        Source::Local => match local_package(state, &found, provenance.clone(), warnings).await {
            Ok(readable) => readable,
            Err(answer) => return *answer,
        },
        #[cfg(not(feature = "local"))]
        Source::Local => return not_read_yet(found.docs),
        Source::Std => return not_read_yet(found.docs),
    };
    let ReadFile {
        resolved,
        contents,
        file,
    } = match read_file(readable, &asked.file, ask.file.is_none()).await {
        Ok(read) => read,
        Err(answer) => return *answer,
    };
    let lines = asked.lines.unwrap_or((1, usize::MAX));
    let excerpt = match excerpt(&contents, lines, ask.context) {
        Ok(excerpt) => excerpt,
        Err(in_file) if ask.file.is_some() => {
            return InvalidArgument::new(format!("{file} has {in_file} lines"), LINES_EXAMPLE)
                .into_result();
        },
        Err(in_file) => {
            return super::error_result(&json!({
                "code": "docs_unavailable",
                "reason": "source_missing",
                "message": format!("{file} has {in_file} lines, fewer than the item's span tells"),
                "retryable": false,
                "suggestions": ["the file may have changed since the documentation was built"],
            }));
        },
    };
    let call = json!({
        "crate": format!("{}@{}", found.name, found.version),
        "target": provenance.get("target"),
    });
    let mut answer = answer(
        (provenance, &item, &data),
        (&file, lines, &resolved),
        &excerpt,
        ask,
    );
    if let Some(answer) = answer.as_object_mut() {
        rest_to_read(answer, call, (&file, lines), &excerpt, ask);
    }
    get_item::add_warnings(&mut answer, warnings);
    CallToolResult::structured(answer)
}

/// Adds to `answer` the hint to read what was cut of `lines` of `file`, and the warning of a line
/// cut within, with the arguments of `call` (decided in M3c, decision 26).
fn rest_to_read(
    answer: &mut Map<String, Value>,
    mut call: Value,
    (file, lines): (&str, (usize, usize)),
    excerpt: &Excerpt,
    ask: &SourceAsk,
) {
    if !excerpt.truncated {
        return;
    }
    let rest = excerpt.shown.1.saturating_add(1);
    let last = lines
        .1
        .saturating_add(ask.context)
        .min(excerpt.lines_in_file);
    if let Some(cut) = excerpt.cut_line {
        push(
            answer,
            "warnings",
            &format!("line {cut} is longer than {MAX_CHARS} characters, so it is cut within"),
        );
    }
    if rest > last {
        return;
    }
    if let Some(call) = call.as_object_mut() {
        call.retain(|_, value| !value.is_null());
        call.insert("file".to_owned(), json!(file));
        call.insert("lines".to_owned(), json!(range(rest, last)));
    }
    push(
        answer,
        "hints",
        &format!(
            "the source was cut after line {}: for the rest, call get_source with {call}",
            excerpt.shown.1
        ),
    );
}

/// Adds `text` to the array of `answer` at `key`.
fn push(answer: &mut Map<String, Value>, key: &str, text: &str) {
    match answer.get_mut(key) {
        Some(Value::Array(texts)) => texts.push(json!(text)),
        Some(_) | None => {
            answer.insert(key.to_owned(), json!([text]));
        },
    }
}

/// The package of the build here of `found`, a member or a path dependency, and what of it may be
/// read (requirement R-SC-4, decided in M3b, B2b): its targets, readme and examples, as the
/// workspace's `cargo metadata` tells them, and the files that rustdoc read, as the build's record
/// tells, under the root of its workspace, or its own directory outside it. Without `cargo
/// metadata`, the files that rustdoc read only. Else the error.
#[cfg(feature = "local")]
async fn local_package(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    provenance: Map<String, Value>,
    warnings: &[Box<str>],
) -> Result<Readable, Box<CallToolResult>> {
    let Some(built) = &found.built else {
        return Err(Box::new(not_read_yet(Source::Local)));
    };
    let record = &built.record;
    // A git or registry dependency's are judged otherwise.
    if super::local::package_source(&record.package) != "path" {
        return Err(Box::new(not_read_yet(Source::Local)));
    }
    let (Some(workspace_root), Some(dir)) = (&record.workspace_root, &record.package_root) else {
        return Err(Box::new(super::error_result(&json!({
            "code": "docs_unavailable",
            "reason": "no_source",
            "message": "the record of the build is from an older version of the server, which did \
                        not keep where its package is",
            "retryable": false,
            "suggestions": [
                "remove the build from the cache with `chatora-rustdoc-mcp cache clear` for the \
                 crate, so that the next lookup builds it again",
            ],
        }))));
    };
    let mut package = source_root::Package {
        root: if dir.starts_with(workspace_root) {
            workspace_root
        } else {
            dir
        }
        .clone(),
        dir: dir.clone(),
        source_files: record.source_files.clone(),
        ..source_root::Package::default()
    };
    let resolution = match found.workspace {
        // Outside the default project, what build_external_docs recorded: no cargo runs there
        // (decided in M2e's E4b-3).
        Some(workspace) if super::recorded_workspace::is_outside(state, workspace) => {
            super::recorded_workspace::read(state, workspace)
                .await
                .ok()
                .map(|recorded| Arc::clone(&recorded.resolution))
        },
        Some(workspace) => match state.resolver.resolve_until(workspace, found.until).await {
            Joined::Done(resolved) => resolved.ok(),
            Joined::Going(doing) => {
                return Err(Box::new(get_item::building_response(
                    provenance,
                    doing,
                    "cargo reads the packages of the workspace",
                    warnings,
                )));
            },
            Joined::Ended => None,
        },
        None => None,
    };
    let dir_text = source_root::lexically(dir);
    if let Some(resolution) = &resolution
        && let Resolution::Metadata { metadata, .. } = &**resolution
        && let Some(listed) = metadata.packages.iter().find(|listed| {
            listed
                .manifest_path
                .parent()
                .is_some_and(|parent| source_root::lexically(parent) == dir_text)
        })
    {
        let files = |example: bool| {
            listed
                .targets
                .iter()
                .filter(|target| {
                    !target.src_path.as_os_str().is_empty()
                        && target.kind.iter().any(|kind| &**kind == "example") == example
                })
                .map(|target| target.src_path.clone())
                .collect()
        };
        package.targets = files(false);
        package.examples = files(true);
        package.readme = listed.readme.as_ref().map(|readme| dir.join(readme));
    }
    Ok(Readable::Package {
        package,
        dir: dir.clone(),
        workspace_root: workspace_root.clone(),
    })
}

/// The tool error for the sources of a local build or of std, which are not read yet.
fn not_read_yet(docs: Source) -> CallToolResult {
    let (of, suggestion) = match docs {
        Source::Local | Source::DocsRs => (
            "local builds",
            "read the file at source_location.path of get_item, if it has one",
        ),
        Source::Std => (
            "std",
            "read the file at source_location.url of get_item, on GitHub",
        ),
    };
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "no_source",
        "message": format!("get_source does not read the sources of {of} yet"),
        "retryable": false,
        "suggestions": [suggestion],
    }))
}

/// The file to read and its lines: those asked for, or else those of the item's span, which
/// `data` of `get_item` tells; else the error that it has none.
fn where_to_read(ask: &SourceAsk, data: &Value) -> Result<AskedFile, Box<CallToolResult>> {
    if let Some(asked) = &ask.file {
        return Ok(asked.clone());
    }
    let location = data.get("source_location");
    let file = location
        .and_then(|location| location.get("file"))
        .and_then(Value::as_str);
    let lines = location
        .and_then(|location| location.get("lines"))
        .and_then(Value::as_str)
        .and_then(parse_lines);
    match (file, lines) {
        (Some(file), Some(lines)) => Ok(AskedFile {
            file: file.into(),
            lines: Some(lines),
        }),
        (..) => Err(Box::new(no_span(data))),
    }
}

/// The file at `file` of `readable`, as `source_root` judges it, and its contents, read on a
/// blocking thread; else the error. A file that is not there is told so, before that it is not
/// of the package. `file` is relative to the package's root, or absolute under it, as a span of a
/// local build outside its workspace is; or, `of_span`, as a span outside the package gives it,
/// relative to the workspace's root.
async fn read_file(
    readable: Readable,
    file: &str,
    #[cfg_attr(
        not(feature = "local"),
        expect(
            unused_variables,
            reason = "only a package built here has spans outside it"
        )
    )]
    of_span: bool,
) -> Result<ReadFile, Box<CallToolResult>> {
    let file_text = file.to_owned();
    let reading = crate::task::spawn_blocking(move || {
        let (root, asked, dir) = match &readable {
            Readable::Release(sources) => (
                Root::Release(sources),
                PathBuf::from(&file_text),
                sources.root.as_path(),
            ),
            #[cfg(feature = "local")]
            Readable::Package {
                package,
                dir,
                workspace_root,
            } => {
                let in_package = dir.join(&file_text);
                // A span outside the package is relative to the workspace's root.
                let asked = if of_span && std::fs::symlink_metadata(&in_package).is_err() {
                    workspace_root.join(&file_text)
                } else {
                    in_package
                };
                (Root::Package(package), asked, dir.as_path())
            },
        };
        let resolved = source_root::resolve(root, &asked).map_err(|refusal| {
            let there = std::fs::symlink_metadata(root.dir().join(&asked)).is_ok();
            if refusal == Refusal::NotInPackage && !there {
                Refusal::Missing
            } else {
                refusal
            }
        })?;
        let contents = std::fs::read(&resolved.path).map_err(|error| {
            tracing::debug!(path = %resolved.path.display(), %error, "a source file could not be read");
            Refusal::Missing
        })?;
        // Relative to the package's root, which the root judged under may hold.
        let file = relative_to(&root.dir().join(&asked), dir);
        Ok::<_, Refusal>(ReadFile {
            resolved,
            contents,
            file,
        })
    })
    .await;
    match reading {
        Ok(Ok(read)) => Ok(read),
        Ok(Err(refusal)) => Err(Box::new(refused(file, refusal))),
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the reading of a source file did not finish");
            Err(Box::new(super::tool_error(
                "internal_error",
                "the source file could not be read (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            )))
        },
    }
}

/// `path` relative to `dir`, by their text, with `/`, and `..` for what of `dir` it is not under,
/// as `../README.md` for a readme that a member inherits from its workspace.
fn relative_to(path: &Path, dir: &Path) -> String {
    let (path, dir) = (source_root::lexically(path), source_root::lexically(dir));
    let common = path
        .components()
        .zip(dir.components())
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in dir.components().skip(common) {
        relative.push("..");
    }
    for component in path.components().skip(common) {
        relative.push(component);
    }
    source_root::slash_separated(&relative)
}

/// The answer: `provenance` and the warnings of `item`, the response of `get_item`, whose `data`
/// tells the item; and of `file`, judged as `resolved`, the asked `lines`, `excerpt`.
fn answer(
    (provenance, item, data): (Map<String, Value>, &Value, &Value),
    (file, lines, resolved): (&str, (usize, usize), &source_root::Resolved),
    excerpt: &Excerpt,
    ask: &SourceAsk,
) -> Value {
    let mut answer = Map::new();
    answer.insert("provenance".to_owned(), Value::Object(provenance));
    let mut told: Vec<Value> = item
        .get("warnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if ask.file.is_none() && lines.1 > excerpt.lines_in_file {
        told.push(json!(format!(
            "{file} has {} lines, fewer than the item's span tells: it may have changed since the \
             documentation was built",
            excerpt.lines_in_file
        )));
    }
    if excerpt.lossy {
        told.push(json!(format!(
            "{file} is not UTF-8: what is not is shown as U+FFFD"
        )));
    }
    if excerpt.marked > 0 {
        told.push(json!(hidden::marked_warning(excerpt.marked)));
    }
    if !told.is_empty() {
        answer.insert("warnings".to_owned(), Value::Array(told));
    }
    let mut shown = Map::new();
    if ask.file.is_none() {
        for (from, to) in [
            ("path", "item"),
            ("defined_at", "defined_at"),
            ("kind", "kind"),
        ] {
            if let Some(value) = data.get(from) {
                shown.insert(to.to_owned(), value.clone());
            }
        }
    }
    shown.insert("file".to_owned(), json!(file));
    let shown_range = |first: usize, last: usize| {
        if excerpt.lines_in_file == 0 {
            String::new()
        } else {
            range(first, last.min(excerpt.lines_in_file))
        }
    };
    shown.insert("lines".to_owned(), json!(shown_range(lines.0, lines.1)));
    shown.insert(
        "shown".to_owned(),
        json!(shown_range(excerpt.shown.0, excerpt.shown.1)),
    );
    shown.insert("source".to_owned(), json!(excerpt.source));
    if let Some(path) = shown_path(&resolved.path) {
        shown.insert("path".to_owned(), json!(path));
    }
    if ask.file.is_none()
        && let Some(url) = data.pointer("/source_location/url")
    {
        shown.insert("url".to_owned(), url.clone());
    }
    if excerpt.truncated {
        shown.insert("truncated".to_owned(), json!(true));
    }
    answer.insert("data".to_owned(), Value::Object(shown));
    Value::Object(answer)
}

/// `result`, the answer for an item found through a re-export of another crate's item, at the
/// path asked for, `entry`, with the item's own path in `defined_at` (requirement R-PR-2), as
/// `get_item` shows it.
pub(super) fn at_entry(mut result: CallToolResult, entry: &str) -> CallToolResult {
    if let Some(Value::Object(data)) = result
        .structured_content
        .as_mut()
        .and_then(|answer| answer.get_mut("data"))
        && let Some(Value::String(item)) = data.get("item")
        && item != entry
    {
        let defined = item.clone();
        data.insert("item".to_owned(), json!(entry));
        if !data.contains_key("defined_at") {
            // Right after `item`.
            for (key, value) in std::mem::take(data) {
                let after = key == "item";
                data.insert(key, value);
                if after {
                    data.insert("defined_at".to_owned(), json!(defined));
                }
            }
        }
    }
    result
}

/// `first-last`, or `first` alone when they are the same.
fn range(first: usize, last: usize) -> String {
    if first == last {
        first.to_string()
    } else {
        format!("{first}-{last}")
    }
}

/// `path` to show: on Windows, without the `\\?\` before a real path where it can go.
fn shown_path(path: &Path) -> Option<String> {
    #[cfg(feature = "local")]
    let path = dunce::simplified(path);
    let text = path.to_str()?;
    // Of a drive, as `\\?\C:\`, without dunce.
    #[cfg(not(feature = "local"))]
    let text = text
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.get(1..3) == Some(r":\"))
        .unwrap_or(text);
    Some(text.to_owned())
}

/// The sources of the release of `found`, a crates.io release documented on docs.rs: where cargo
/// unpacked them, or else unpacked in the cache from its `.crate`, fetched if need be, while the
/// call waits; else the answer to give, while they are fetched or when they cannot be had.
async fn release_sources(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    provenance: Map<String, Value>,
    warnings: &[Box<str>],
) -> Result<Sources, Box<CallToolResult>> {
    let release = format!("{}@{}", found.name, found.version);
    let key = format!("sources:{release}");
    let (crates, cache) = (state.static_crates.clone(), state.cache.clone());
    let (name, version) = (found.name.clone(), found.version.clone());
    let checksum = match found.checksum {
        Some(checksum) => Some(Box::<str>::from(checksum)),
        // As the version was decided without the index: from it, as for docs.rs's build.
        None => match state.index.entry(found.name).await {
            Ok(entry) => entry
                .versions
                .iter()
                .find(|listed| listed.version == *found.version)
                .map(|listed| listed.checksum.clone()),
            Err(error) => {
                tracing::debug!(error = %super::ErrorChain(&error), "the index was not read for the sources");
                None
            },
        },
    };
    let max_bytes = state.dist.extract_max_size();
    let what = format!("{name} {version}");
    let joined = state
        .crate_sources
        .join(&key, &what, found.until, async move |_stage| {
            Arc::new(
                crate_source::sources(
                    &crates,
                    &cache,
                    &name,
                    &version,
                    checksum.as_deref(),
                    max_bytes,
                )
                .await,
            )
        })
        .await;
    match joined {
        Joined::Done(sources) => match &*sources {
            Ok(sources) => Ok(sources.clone()),
            Err(error) => Err(Box::new(sources_error(&release, error))),
        },
        Joined::Going(doing) => Err(Box::new(get_item::building_response(
            provenance,
            doing,
            &format!("the sources of {release} are fetched and unpacked"),
            warnings,
        ))),
        Joined::Ended => Err(Box::new(get_item::shutting_down())),
    }
}

/// The tool error for an item without a span: one that a macro or rustdoc made (decided in M3c,
/// decision 28).
fn no_span(data: &Value) -> CallToolResult {
    let item = data
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("the item");
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "no_span",
        "message": format!(
            "{item} has no location in the documentation: a macro or rustdoc made it"
        ),
        "retryable": false,
        "suggestions": [
            "read the source of the type or trait that it is of, or of the macro that made it",
        ],
    }))
}

/// The tool error for `file` that is not read, and why (requirement R-SC-4, decided in M3c,
/// decision 29).
fn refused(file: &str, refusal: Refusal) -> CallToolResult {
    let reason = match refusal {
        Refusal::Missing | Refusal::NotAFile => "source_missing",
        Refusal::Outside
        | Refusal::Dotfile
        | Refusal::NotInPackage
        | Refusal::SymlinkOutside
        | Refusal::SymlinkInCache => "outside_package",
    };
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": reason,
        "message": format!("{file} is not read: {refusal}"),
        "retryable": false,
        "suggestions": [
            "only the files of the crate's package are read: pass a path relative to its root, as \
             source_location.file of get_item gives it",
        ],
    }))
}

/// The tool error for the sources of `release` (`name@version`) that could not be had (§8,
/// decisions 12, 13 and 15).
fn sources_error(release: &str, error: &SourcesError) -> CallToolResult {
    let message = format!(
        "the sources of {release} could not be had: {}",
        super::ErrorChain(error)
    );
    let (code, reason, retryable): (&str, Option<&str>, bool) = match error {
        SourcesError::Crate(CrateError::Offline) => ("offline_miss", None, false),
        SourcesError::Crate(
            CrateError::Network(_)
            | CrateError::Transient { .. }
            | CrateError::RecentFailure { .. }
            | CrateError::ChecksumMismatch,
        ) => ("network_error", None, true),
        SourcesError::Crate(CrateError::UnexpectedStatus { .. }) => ("network_error", None, false),
        SourcesError::Crate(CrateError::NotFound) | SourcesError::NotCached => {
            ("docs_unavailable", Some("no_source"), false)
        },
        SourcesError::Crate(CrateError::TooLarge { .. })
        | SourcesError::Unpack(UnpackError::TooLarge { .. }) => {
            ("docs_unavailable", Some("too_large"), false)
        },
        SourcesError::Crate(CrateError::Corrupt(_) | CrateError::Manifest(_))
        | SourcesError::Unpack(UnpackError::Archive(_)) => {
            ("docs_unavailable", Some("corrupt"), true)
        },
        // Refused whole, the same each time (decided in M3b, decision 12).
        SourcesError::Unpack(UnpackError::Refused { .. } | UnpackError::TooManyFiles { .. }) => {
            ("docs_unavailable", Some("corrupt"), false)
        },
        SourcesError::Crate(CrateError::Cache(_))
        | SourcesError::Unpack(UnpackError::Cache(_))
        | SourcesError::Cache(_) => ("internal_error", None, false),
    };
    let suggestions: &[&str] = match code {
        "offline_miss" => &[
            "the sources are read from a .crate here: build the project once online, or turn \
             network.offline off",
        ],
        "network_error" => &["try again in a while"],
        "internal_error" => &["check that the cache directory can be written"],
        _ => &["read the file on docs.rs, at the url of source_location of get_item"],
    };
    let mut body = json!({
        "code": code,
        "message": message,
        "retryable": retryable,
        "suggestions": suggestions,
    });
    if let (Some(reason), Some(body)) = (reason, body.as_object_mut()) {
        body.insert("reason".to_owned(), json!(reason));
    }
    if let (SourcesError::Crate(CrateError::RecentFailure { retry_after }), Some(body)) =
        (error, body.as_object_mut())
    {
        body.insert(
            "retry_after_secs".to_owned(),
            json!(retry_after.as_secs().max(1)),
        );
    }
    super::error_result(&body)
}

/// The lines of a file that are shown, as text.
#[derive(Debug, PartialEq, Eq)]
struct Excerpt {
    /// The text of the lines, with their line breaks, marked.
    source: String,
    /// The first and last lines shown, from 1.
    shown: (usize, usize),
    /// Whether it was cut before the last line asked for.
    truncated: bool,
    /// How many characters were marked.
    marked: usize,
    /// Whether the file was not UTF-8, and what was not is shown as U+FFFD.
    lossy: bool,
    /// How many lines the file has.
    lines_in_file: usize,
    /// The line cut within, as longer than [`MAX_CHARS`] alone, if one was.
    cut_line: Option<usize>,
}

/// The lines `first` to `last` of `contents`, with `context` lines before and after, within the
/// file, marked (requirement R-SEC-7) and cut at a line once [`MAX_CHARS`] are passed
/// (requirement R-SC-5); a line longer than that alone is cut within, not within a mark. An empty
/// file shows nothing. The lines that the file has when it has fewer than `first`.
fn excerpt(
    contents: &[u8],
    (first, last): (usize, usize),
    context: usize,
) -> Result<Excerpt, usize> {
    let text = String::from_utf8_lossy(contents);
    let lossy = matches!(text, Cow::Owned(_));
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let lines_in_file = lines.len();
    if lines_in_file == 0 {
        return Ok(Excerpt {
            source: String::new(),
            shown: (0, 0),
            truncated: false,
            marked: 0,
            lossy,
            lines_in_file,
            cut_line: None,
        });
    }
    if first == 0 || first > lines_in_file {
        return Err(lines_in_file);
    }
    let from = first.saturating_sub(context).max(1);
    let to = last.saturating_add(context).min(lines_in_file).max(first);
    let mut source = String::new();
    let mut chars = 0_usize;
    let mut marked = 0_usize;
    let mut shown_to = from;
    let mut truncated = false;
    let mut cut_line = None;
    for (at, line) in lines
        .get(from.saturating_sub(1)..to)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let (line, count) = hidden::mark(line);
        let length = line.chars().count();
        if chars.saturating_add(length) > MAX_CHARS {
            truncated = true;
            if at == 0 {
                source.push_str(cut(&line, MAX_CHARS));
                marked = marked.saturating_add(count);
                cut_line = Some(from);
            }
            break;
        }
        chars = chars.saturating_add(length);
        marked = marked.saturating_add(count);
        source.push_str(&line);
        shown_to = from.saturating_add(at);
    }
    Ok(Excerpt {
        source,
        shown: (from, shown_to),
        truncated,
        marked,
        lossy,
        lines_in_file,
        cut_line,
    })
}

/// `line` cut to at most `max` characters, but not within a mark such as `<U+202E>`.
fn cut(line: &str, max: usize) -> &str {
    let end = line
        .char_indices()
        .nth(max)
        .map_or(line.len(), |(at, _)| at);
    let kept = line.get(..end).unwrap_or(line);
    // A mark begun and not ended goes whole.
    match kept.rfind("<U+") {
        Some(at) if !kept.get(at..).unwrap_or_default().contains('>') => {
            kept.get(..at).unwrap_or(kept)
        },
        Some(_) | None => kept,
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

    #[track_caller]
    fn parsed(arguments: Value) -> Result<Arguments, InvalidArgument> {
        let Value::Object(arguments) = arguments else {
            panic!("an object");
        };
        parse_arguments(Some(&arguments))
    }

    /// `path` or `file` is asked for, not both, with `lines` only with `file`, and `context`
    /// within its bounds (decisions 24 and 27).
    #[test]
    fn a_path_or_a_file_is_asked_for() {
        let path = parsed(json!({ "crate": "tokio", "path": "sync::Mutex::lock" })).expect("valid");
        assert_eq!(&*path.path_for_test(), "tokio::sync::Mutex::lock");
        let file = parsed(json!({
            "crate": "tokio",
            "file": "src/sync/mutex.rs",
            "lines": "434-466",
            "context": 3,
        }))
        .expect("valid");
        assert_eq!(&*file.path_for_test(), "tokio");
        for (arguments, part) in [
            (json!({ "crate": "tokio" }), "`path` or `file`"),
            (
                json!({ "crate": "tokio", "path": "a", "file": "src/lib.rs" }),
                "not both",
            ),
            (
                json!({ "crate": "tokio", "path": "a", "lines": "1-2" }),
                "`lines` goes with `file`",
            ),
            (
                json!({ "crate": "tokio", "file": "src/lib.rs", "lines": "9-2" }),
                "`lines`",
            ),
            (
                json!({ "crate": "tokio", "path": "a", "context": 201 }),
                "`context`",
            ),
        ] {
            let error = parsed(arguments).expect_err("invalid");
            assert!(error.message.contains(part), "{error:?}");
        }
    }

    #[test]
    fn lines_are_a_line_or_a_range() {
        assert_eq!(parse_lines("434-466"), Some((434, 466)));
        assert_eq!(parse_lines("301"), Some((301, 301)));
        assert_eq!(parse_lines(" 2 - 3 "), Some((2, 3)));
        for lines in ["0", "3-2", "a", "1-", "-1", "1-2-3"] {
            assert_eq!(parse_lines(lines), None, "{lines}");
        }
    }

    /// The lines of the item with `context` before and after, within the file, with their line
    /// breaks as the file has them, whose CR of a CRLF the answer's sanitizing takes away after
    /// (requirement R-SC-1).
    #[test]
    fn the_lines_and_their_context_are_cut_out() {
        let file = b"one\ntwo\r\nthree\nfour\nfive";
        let shown = |lines, context| {
            excerpt(file, lines, context)
                .ok()
                .map(|excerpt| (excerpt.source, excerpt.shown, excerpt.truncated))
        };
        assert_eq!(
            shown((2, 3), 0),
            Some(("two\r\nthree\n".to_owned(), (2, 3), false))
        );
        assert_eq!(
            shown((2, 3), 1),
            Some(("one\ntwo\r\nthree\nfour\n".to_owned(), (1, 4), false))
        );
        assert_eq!(
            shown((4, usize::MAX), 9),
            Some(("one\ntwo\r\nthree\nfour\nfive".to_owned(), (1, 5), false))
        );
        assert_eq!(shown((6, 6), 0), None);
        assert_eq!(excerpt(file, (6, 6), 0).map(|_| ()), Err(5));
        // An empty file shows nothing.
        let empty = excerpt(b"", (1, usize::MAX), 0).expect("shown");
        assert_eq!((empty.source.as_str(), empty.lines_in_file), ("", 0));
    }

    /// What would hide code is marked and counted (requirement R-SEC-7), and a file that is not
    /// UTF-8 is read with U+FFFD.
    #[test]
    fn hidden_characters_are_marked() {
        let found = excerpt("a\u{202E}b\nc\r\n".as_bytes(), (1, 2), 0).expect("shown");
        assert_eq!(found.source, "a<U+202E>b\nc\r\n");
        assert_eq!((found.marked, found.lossy), (1, false));
        let found = excerpt(b"a\xffb\n", (1, 1), 0).expect("shown");
        assert_eq!((found.source.as_str(), found.lossy), ("a\u{FFFD}b\n", true));
    }

    /// A long source is cut at a line once 20,000 characters are passed, and a line longer than
    /// that alone within it, not within a mark (requirement R-SC-5, decision 26).
    #[test]
    fn a_long_source_is_cut_at_a_line() {
        let line = format!("{}\n", "x".repeat(999));
        let file = line.repeat(30);
        let found = excerpt(file.as_bytes(), (1, 30), 0).expect("shown");
        assert_eq!((found.shown, found.truncated), ((1, 20), true));
        assert_eq!(found.source.chars().count(), 20_000);
        let long = format!("{}\u{202E}tail\n", "y".repeat(MAX_CHARS - 3));
        let found = excerpt(long.as_bytes(), (1, 1), 0).expect("shown");
        assert_eq!(
            (found.shown, found.truncated, found.cut_line),
            ((1, 1), true, Some(1))
        );
        assert_eq!(found.source, "y".repeat(MAX_CHARS - 3));
    }

    /// The hint to read what was cut names the lines after those shown, with the crate, its
    /// target and the file as JSON; none when the line cut within was the last asked for, which
    /// is warned of (decision 26).
    #[test]
    fn the_rest_of_a_cut_source_is_hinted() {
        let line = format!("{}\n", "x".repeat(999));
        let file = line.repeat(30);
        let ask = SourceAsk {
            context: 0,
            file: None,
        };
        let call = json!({ "crate": "a\"b@1.0.0", "target": "x86_64-unknown-linux-gnu" });
        let mut answer = Map::new();
        let found = excerpt(file.as_bytes(), (1, 30), 0).expect("shown");
        rest_to_read(
            &mut answer,
            call.clone(),
            ("src/a\"b.rs", (1, 30)),
            &found,
            &ask,
        );
        let hint = answer["hints"][0].as_str().expect("a hint");
        let (_, hinted) = hint.split_once("with ").expect("a call");
        assert_eq!(
            serde_json::from_str::<Value>(hinted).expect("JSON"),
            json!({
                "crate": "a\"b@1.0.0",
                "target": "x86_64-unknown-linux-gnu",
                "file": "src/a\"b.rs",
                "lines": "21-30",
            })
        );
        let long = format!("{}\n", "y".repeat(MAX_CHARS + 1));
        let found = excerpt(long.as_bytes(), (1, 1), 0).expect("shown");
        let mut answer = Map::new();
        rest_to_read(&mut answer, call, ("src/a.rs", (1, 1)), &found, &ask);
        assert!(answer.get("hints").is_none(), "{answer:?}");
        assert!(
            answer["warnings"][0]
                .as_str()
                .is_some_and(|warning| warning.contains("line 1 is longer")),
            "{answer:?}"
        );
    }

    /// An item found through a re-export of another crate's item is told at the path asked for,
    /// with its own in `defined_at`, and the file's path is left (requirement R-PR-2).
    #[test]
    fn an_item_found_through_a_re_export_is_told_at_its_entry() {
        let answer = CallToolResult::structured(json!({
            "provenance": {},
            "data": {
                "item": "futures_core::stream::Stream",
                "kind": "trait",
                "path": "/cache/src.v1/src/stream.rs",
            },
        }));
        let shown = at_entry(answer, "futures::Stream")
            .structured_content
            .expect("structured");
        assert_eq!(
            shown["data"],
            json!({
                "item": "futures::Stream",
                "defined_at": "futures_core::stream::Stream",
                "kind": "trait",
                "path": "/cache/src.v1/src/stream.rs",
            })
        );
    }

    /// An item without a span, and each failure to have the sources, are told by their codes
    /// and reasons (decisions 28 and 29).
    #[test]
    fn what_cannot_be_read_is_told_why() {
        let body = |result: CallToolResult| {
            let text = result.content[0].as_text().expect("text").text.clone();
            serde_json::from_str::<Value>(&text).expect("JSON")
        };
        let told = body(no_span(&json!({ "path": "a::B" })));
        assert_eq!(
            (told["code"].as_str(), told["reason"].as_str()),
            (Some("docs_unavailable"), Some("no_span"))
        );
        for (error, code, reason, retryable) in [
            (
                SourcesError::Crate(CrateError::Offline),
                "offline_miss",
                None,
                false,
            ),
            (
                SourcesError::Crate(CrateError::ChecksumMismatch),
                "network_error",
                None,
                true,
            ),
            (
                SourcesError::NotCached,
                "docs_unavailable",
                Some("no_source"),
                false,
            ),
            (
                SourcesError::Unpack(UnpackError::TooLarge { limit: 1 }),
                "docs_unavailable",
                Some("too_large"),
                false,
            ),
            (
                SourcesError::Unpack(UnpackError::Archive(std::io::Error::other("cut"))),
                "docs_unavailable",
                Some("corrupt"),
                true,
            ),
            (
                SourcesError::Unpack(UnpackError::Refused {
                    entry: "a/../b".into(),
                    why: "a path with . or ..",
                }),
                "docs_unavailable",
                Some("corrupt"),
                false,
            ),
            (
                SourcesError::Cache(std::io::Error::other("full")),
                "internal_error",
                None,
                false,
            ),
        ] {
            let told = body(sources_error("a@1.0.0", &error));
            assert_eq!(
                (
                    told["code"].as_str(),
                    told["reason"].as_str(),
                    told["retryable"].as_bool()
                ),
                (Some(code), reason, Some(retryable)),
                "{error:?}"
            );
        }
        for (refusal, reason) in [
            (Refusal::Missing, "source_missing"),
            (Refusal::Dotfile, "outside_package"),
            (Refusal::SymlinkOutside, "outside_package"),
        ] {
            let told = body(refused("x", refusal));
            assert_eq!(told["reason"], reason, "{refusal:?}");
        }
    }
}
