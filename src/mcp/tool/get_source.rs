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
#[cfg(feature = "local")]
use crate::data::{metadata::Resolution, project::Workspace};
#[cfg(feature = "local")]
use crate::query::local::LocalBuilt;
use crate::{
    data::{
        crate_file::CrateError,
        crate_name::CrateName,
        crate_source::{self, Sources, SourcesError, UnpackError},
        dist::{DistError, RustSrc},
        flight::Joined,
        hidden,
        source_root::{self, Refusal, Root},
        std_json::{self, Chosen},
    },
    query::get_item::Made,
};

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

/// The sources of std of one nightly being had, fetched and unpacked from the distribution server
/// if need be, shared by the calls that want them at once (decided in M3c, C3).
pub(super) type SharedRustSrc = Arc<Result<RustSrc, DistError>>;

/// What is read, besides the item or the crate that `get_item` looks up (decided in M3c,
/// decisions 24 and 27).
#[derive(Debug, Clone)]
pub(super) struct SourceAsk {
    /// Lines added before and after.
    context: usize,
    /// The file asked for, if one was: else the item's.
    file: Option<AskedFile>,
    /// The project asked for, if one was, which decides the version, or for std the nightly, of
    /// what the hint to read the rest reads.
    project: Option<Box<str>>,
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
                    "file_crate": { "type": "string" },
                    "lines": { "type": "string" },
                    "shown": { "type": "string" },
                    "source": { "type": "string" },
                    "path": { "type": "string" },
                    "url": { "type": "string" },
                    "macro": {
                        "type": "string",
                        "enum": ["derive", "invocation"],
                        "description": "A macro made the item: a derive, or a macro called on \
                                        its lines, whose code is not shown.",
                    },
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
    let project = text("project")?.map(Box::from);
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
                project,
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
                project,
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
    pub(super) docs: Source<'a>,
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
    /// Of std, the `rust-src` at `root`, in the cache or else a toolchain's here.
    RustSrc { root: PathBuf, in_cache: bool },
    /// Of a package built here, under the directory of its `Cargo.toml`, `dir`, in the workspace
    /// at `workspace_root`, which its spans outside it are relative to.
    #[cfg(feature = "local")]
    Package {
        package: source_root::Package,
        dir: PathBuf,
        workspace_root: PathBuf,
    },
    /// Of a git dependency built here, in the checkout `checkout` that cargo finished, under
    /// `dir`, in the workspace at `workspace_root`.
    #[cfg(feature = "local")]
    Checkout {
        checkout: PathBuf,
        dir: PathBuf,
        workspace_root: PathBuf,
    },
}

impl Readable {
    /// The directory that this server unpacked the sources into in its cache, which the GC may
    /// take; `None` for sources that cargo or a toolchain here keeps.
    fn unpacked_in_cache(&self) -> Option<PathBuf> {
        match self {
            Self::Release(sources) if sources.origin == crate_source::Origin::Cache => {
                Some(sources.root.clone())
            },
            Self::RustSrc {
                root,
                in_cache: true,
            } => Some(root.clone()),
            Self::Release(_) | Self::RustSrc { .. } => None,
            #[cfg(feature = "local")]
            Self::Package { .. } | Self::Checkout { .. } => None,
        }
    }
}

/// A file read.
struct ReadFile {
    /// What `source_root` judged of it: its real path.
    resolved: source_root::Resolved,
    /// The path asked for, absolute, as its text resolves: as a build's inputs have it.
    asked: PathBuf,
    /// Its contents.
    contents: Vec<u8>,
    /// Its path relative to its package's root, with `/`.
    file: String,
}

/// Where the documentation that the item was found in came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source<'a> {
    /// docs.rs, built from the release's `.crate`.
    DocsRs,
    /// A build here.
    Local,
    /// The std of a nightly, whose JSON this is.
    Std(&'a Chosen),
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
    let mut warnings = warnings.to_vec();
    let where_asked = match where_to_read(ask, &data) {
        Ok(asked) => asked,
        Err(answer) => return *answer,
    };
    let (asked, file_crate, read) = match read_found(
        state,
        &found,
        ask,
        &where_asked,
        (&provenance, &mut warnings),
    )
    .await
    {
        Ok(read) => read,
        Err(answer) => return *answer,
    };
    let ReadFile {
        resolved,
        asked: asked_path,
        contents,
        file,
    } = read;
    // std's is not asked for by version, which the nightly's would be.
    let release = match (&file_crate, found.docs) {
        (Some(of), _) => of.clone(),
        (None, Source::Std(_)) => found.name.to_string(),
        (None, Source::DocsRs | Source::Local) => format!("{}@{}", found.name, found.version),
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
        "crate": release,
        // A dependency's on docs.rs, of its own target.
        "target": if file_crate.is_none() { provenance.get("target") } else { None },
        "project": ask.project,
    });
    let url = file_url(
        found.docs,
        file_crate.as_deref(),
        &file,
        &range(lines.0, lines.1.min(excerpt.lines_in_file)),
    );
    let mut answer = answer(
        (provenance, &item, &data),
        (&file, file_crate.as_deref(), lines, &resolved),
        (&excerpt, url),
        ask,
    );
    if let Some(answer) = answer.as_object_mut() {
        // Whether the file is the one that the documentation was built from (decided in M3c,
        // decisions 36 and 37).
        let built = built_from(&found, &asked_path, &resolved.path, &contents);
        if built == Some(false) {
            let moved = if ask.file.is_none() {
                ": the item's lines may have moved"
            } else {
                ""
            };
            push(
                answer,
                "warnings",
                &format!("{file} has changed since the documentation was built{moved}"),
            );
        }
        if ask.file.is_none() {
            let changed = built.map_or_else(|| provenance_stale(answer), |same| !same)
                || lines.1 > excerpt.lines_in_file;
            made_by_macro(answer, &data, (&contents, lines), changed);
        }
        rest_to_read(answer, call, (&file, lines), &excerpt, ask);
    }
    get_item::add_warnings(&mut answer, &warnings);
    CallToolResult::structured(answer)
}

/// The sources of the crate of `found` that `asked` is read from, and the release (`name@version`)
/// that the file is of when it is not the crate's, as a dependency's that docs.rs's build read;
/// `asked` is made the path in them. Else the answer to give.
async fn readable(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    ask: &SourceAsk,
    asked: &mut AskedFile,
    (provenance, warnings): (&Map<String, Value>, &mut Vec<Box<str>>),
) -> Result<(Readable, Option<String>), Box<CallToolResult>> {
    match found.docs {
        Source::DocsRs => docs_rs_sources(state, found, ask, asked, (provenance, warnings))
            .await
            .map(|(sources, of)| (Readable::Release(sources), of)),
        #[cfg(feature = "local")]
        Source::Local => local_package(state, found, provenance.clone(), warnings)
            .await
            .map(|readable| (readable, None)),
        #[cfg(not(feature = "local"))]
        Source::Local => Err(Box::new(not_read_yet())),
        Source::Std(chosen) => std_sources(state, found, chosen, asked, (provenance, warnings))
            .await
            .map(|readable| (readable, None)),
    }
}

/// The file of the crate of `found` that `where_asked` names, read from its sources, with where
/// it was asked for in them and the release it is of when it is not the crate's
/// ([`readable`]); its sources had again once if the GC took them meanwhile, as unpacked in the
/// cache. Else the answer to give.
async fn read_found(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    ask: &SourceAsk,
    where_asked: &AskedFile,
    (provenance, warnings): (&Map<String, Value>, &mut Vec<Box<str>>),
) -> Result<(AskedFile, Option<String>, ReadFile), Box<CallToolResult>> {
    let mut again = true;
    loop {
        let mut asked = where_asked.clone();
        let (readable, file_crate) =
            readable(state, found, ask, &mut asked, (provenance, warnings)).await?;
        let unpacked = readable.unpacked_in_cache();
        match read_file(
            readable,
            &asked.file,
            ask.file.is_none(),
            file_crate.as_deref(),
        )
        .await
        {
            Ok(read) => return Ok((asked, file_crate, read)),
            // Taken by the GC since they were had: had again, once.
            Err(_)
                if again
                    && unpacked.is_some_and(|root| std::fs::symlink_metadata(root).is_err()) =>
            {
                again = false;
            },
            Err(answer) => return Err(answer),
        }
    }
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
    // A dependency's file is read through its release's documentation on docs.rs, of its own
    // target.
    let through = if ask.file.is_none()
        && answer
            .get("data")
            .is_some_and(|data| data.get("file_crate").is_some())
    {
        ", which reads that release's documentation on docs.rs first"
    } else {
        ""
    };
    push(
        answer,
        "hints",
        &format!(
            "the source was cut after line {}: for the rest, call get_source with {call}{through}",
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
        return Err(Box::new(not_read_yet()));
    };
    let record = &built.record;
    let source = super::local::package_source(&record.package);
    if !matches!(source, "path" | "git" | "crates.io") {
        return Err(Box::new(other_registry(found)));
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
    let cargo_home = state.resolver.cargo_home();
    match source {
        // As docs.rs's: of its `.crate`, where cargo unpacked it from crates.io, or else unpacked
        // in the cache. Not where it was unpacked from another source that replaces crates.io,
        // whose files may differ, nor vendored (decided in M3c, decision 31).
        "crates.io" => {
            return if cargo_home.is_some_and(|home| unpacked_from_crates_io(dir, home)) {
                release_sources(state, found, provenance, warnings)
                    .await
                    .map(Readable::Release)
            } else {
                Err(Box::new(other_registry(found)))
            };
        },
        // In its checkout, where cargo checked it out, and only there: not vendored.
        "git" => {
            return cargo_home
                .and_then(|home| checkout_of(dir, home))
                .map(|checkout| Readable::Checkout {
                    checkout,
                    dir: dir.clone(),
                    workspace_root: workspace_root.clone(),
                })
                .ok_or_else(|| Box::new(other_registry(found)));
        },
        _ => {},
    }
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
    let resolution = metadata_of(state, found, provenance, warnings).await?;
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

/// The packages of the workspace that decided the version of `found`, as `cargo metadata` tells
/// them: outside the default project, what `build_external_docs` recorded, as no cargo runs there
/// (decided in M2e's E4b-3); else as the lookup had them, until its deadline, past which the
/// answer is that cargo reads them. `None` when they cannot be had.
#[cfg(feature = "local")]
async fn metadata_of(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    provenance: Map<String, Value>,
    warnings: &[Box<str>],
) -> Result<Option<Arc<Resolution>>, Box<CallToolResult>> {
    let Some(workspace) = found.workspace else {
        return Ok(None);
    };
    if super::recorded_workspace::is_outside(state, workspace) {
        return Ok(super::recorded_workspace::read(state, workspace)
            .await
            .ok()
            .map(|recorded| Arc::clone(&recorded.resolution)));
    }
    match state.resolver.resolve_until(workspace, found.until).await {
        Joined::Done(resolved) => Ok(resolved.ok()),
        Joined::Going(doing) => Err(Box::new(get_item::building_response(
            provenance,
            doing,
            "cargo reads the packages of the workspace",
            warnings,
        ))),
        Joined::Ended => Ok(None),
    }
}

/// Whether cargo unpacked the package at `dir` from crates.io: under
/// `$CARGO_HOME/registry/src/index.crates.io-…/`, in `cargo_home`.
#[cfg(feature = "local")]
fn unpacked_from_crates_io(dir: &Path, cargo_home: &Path) -> bool {
    source_root::lexically(dir)
        .strip_prefix(source_root::lexically(
            &cargo_home.join("registry").join("src"),
        ))
        .ok()
        .and_then(|inside| inside.components().next())
        .is_some_and(|index| {
            index
                .as_os_str()
                .to_string_lossy()
                .starts_with("index.crates.io-")
        })
}

/// The checkout that the package at `dir` is in, `$CARGO_HOME/git/checkouts/<repository>/<rev>`
/// in `cargo_home`, if cargo finished it (its `.cargo-ok`): the root that its files are judged
/// under, and no other's (requirement R-SC-4).
#[cfg(feature = "local")]
fn checkout_of(dir: &Path, cargo_home: &Path) -> Option<PathBuf> {
    let checkouts = source_root::lexically(&cargo_home.join("git").join("checkouts"));
    let inside = source_root::lexically(dir);
    let mut parts = inside.strip_prefix(&checkouts).ok()?.components();
    let checkout = checkouts.join(parts.next()?).join(parts.next()?);
    std::fs::symlink_metadata(checkout.join(".cargo-ok"))
        .is_ok_and(|ok| ok.is_file())
        .then_some(checkout)
}

/// The tool error for a crate of a registry other than crates.io, or vendored, whose sources are
/// not read (decided in M3c, decision 31).
#[cfg(feature = "local")]
fn other_registry(found: &Found<'_>) -> CallToolResult {
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "outside_package",
        "message": format!(
            "{}@{} is of a registry other than crates.io, whose sources get_source does not read",
            found.name, found.version
        ),
        "retryable": false,
        "suggestions": [
            "read the file at source_location.path of get_item, if it has one, with your own tools",
        ],
    }))
}

/// The sources of the release of `found`, documented on docs.rs, to read `asked` from; or, for an
/// item whose span is a file of a dependency that docs.rs's build read, as of a blanket impl
/// copied from it, that release's, with `asked` made the path in it, and that release as
/// `name@version` (decided in M3c, C2). Else the answer to give.
async fn docs_rs_sources(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    ask: &SourceAsk,
    asked: &mut AskedFile,
    (provenance, warnings): (&Map<String, Value>, &[Box<str>]),
) -> Result<(Sources, Option<String>), Box<CallToolResult>> {
    let dependency = if ask.file.is_none() {
        rustwide(&asked.file)
    } else {
        None
    };
    let Some((name, version, path)) = &dependency else {
        let sources = release_sources(state, found, provenance.clone(), warnings).await?;
        return Ok((sources, None));
    };
    let of = Found {
        name,
        version,
        checksum: None,
        docs: Source::DocsRs,
        until: found.until,
        #[cfg(feature = "local")]
        built: None,
        #[cfg(feature = "local")]
        workspace: None,
    };
    asked.file = path.as_str().into();
    let sources = release_sources(state, &of, provenance.clone(), warnings).await?;
    Ok((sources, Some(format!("{name}@{version}"))))
}

/// The tool error for the sources of a local build, which are not read: without local builds, or
/// one without its build.
fn not_read_yet() -> CallToolResult {
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "no_source",
        "message": "get_source does not read the sources of this local build",
        "retryable": false,
        "suggestions": ["read the file at source_location.path of get_item, if it has one"],
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
    of_release: Option<&str>,
) -> Result<ReadFile, Box<CallToolResult>> {
    let file_text = file.to_owned();
    let reading = crate::task::spawn_blocking(move || {
        // A file not there is told so, before that it is not of the package.
        let missing = |refusal, asked: &Path| {
            if refusal == Refusal::NotInPackage && std::fs::symlink_metadata(asked).is_err() {
                Refusal::Missing
            } else {
                refusal
            }
        };
        // A span outside the package is relative to the workspace's root.
        #[cfg(feature = "local")]
        let under = |dir: &Path, workspace_root: &Path| {
            let in_package = dir.join(&file_text);
            if of_span && std::fs::symlink_metadata(&in_package).is_err() {
                workspace_root.join(&file_text)
            } else {
                in_package
            }
        };
        let (resolved, asked, dir) = match &readable {
            Readable::Release(sources) => {
                let asked = sources.root.join(&file_text);
                let resolved = source_root::resolve(Root::Release(sources), Path::new(&file_text))
                    .map_err(|refusal| missing(refusal, &asked))?;
                (resolved, asked, sources.root.as_path())
            },
            Readable::RustSrc { root, in_cache } => {
                let asked = root.join(&file_text);
                let resolved = source_root::resolve(
                    Root::RustSrc {
                        root,
                        in_cache: *in_cache,
                    },
                    Path::new(&file_text),
                )
                .map_err(|refusal| missing(refusal, &asked))?;
                (resolved, asked, root.as_path())
            },
            #[cfg(feature = "local")]
            Readable::Package {
                package,
                dir,
                workspace_root,
            } => {
                let asked = under(dir, workspace_root);
                let resolved = source_root::resolve(Root::Package(package), &asked)
                    .map_err(|refusal| missing(refusal, &asked))?;
                (resolved, asked, dir.as_path())
            },
            #[cfg(feature = "local")]
            Readable::Checkout {
                checkout,
                dir,
                workspace_root,
            } => {
                let asked = under(dir, workspace_root);
                let resolved = source_root::resolve(Root::Checkout(checkout), &asked)
                    .map_err(|refusal| missing(refusal, &asked))?;
                (resolved, asked, dir.as_path())
            },
        };
        let contents = std::fs::read(&resolved.path).map_err(|error| {
            tracing::debug!(path = %resolved.path.display(), %error, "a source file could not be read");
            Refusal::Missing
        })?;
        // Relative to the package's root, which the root judged under may hold.
        let file = relative_to(&asked, dir);
        Ok::<_, Refusal>(ReadFile {
            resolved,
            asked: source_root::lexically(&asked),
            contents,
            file,
        })
    })
    .await;
    match reading {
        Ok(Ok(read)) => Ok(read),
        Ok(Err(refusal)) => Err(Box::new(refused(
            &of_release.map_or_else(|| file.to_owned(), |release| format!("{file} of {release}")),
            refusal,
        ))),
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
    (file, file_crate, lines, resolved): (
        &str,
        Option<&str>,
        (usize, usize),
        &source_root::Resolved,
    ),
    (excerpt, url): (&Excerpt, Option<String>),
    ask: &SourceAsk,
) -> Value {
    let mut answer = Map::new();
    answer.insert("provenance".to_owned(), Value::Object(provenance));
    // Of a build being made again, whose result is newer.
    if let Some(after) = item.get("retry_after_secs") {
        answer.insert("retry_after_secs".to_owned(), after.clone());
    }
    let mut told: Vec<Value> = item
        .get("warnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // That a dependency's file has no URL, which it has as that release's.
    if file_crate.is_some()
        && let Some(span) = data
            .pointer("/source_location/file")
            .and_then(Value::as_str)
    {
        told.retain(|warning| {
            warning.as_str().is_none_or(|warning| {
                !warning.starts_with(&format!("{span} is outside the package"))
            })
        });
    }
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
    if let Some(file_crate) = file_crate {
        shown.insert("file_crate".to_owned(), json!(file_crate));
    }
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
    if let Some(url) = url {
        shown.insert("url".to_owned(), json!(url));
    } else if ask.file.is_none()
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

/// The key in `data` of the item looked up for `get_source` of its own name, which the answer does
/// not show (decided in M3c, decision 35).
const MADE_NAME: &str = "made_name";

/// The key in `data` of the item looked up for `get_source` of whether a derive made it, which the
/// answer does not show.
const MADE_DERIVED: &str = "made_derived";

/// Adds to `response`, of the item looked up for `get_source`, what `made` tells of a macro's
/// output.
pub(super) fn add_made(response: &mut Value, made: Made<'_>) {
    if let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) {
        if let Some(name) = made.name {
            data.insert(MADE_NAME.to_owned(), json!(name));
        }
        data.insert(MADE_DERIVED.to_owned(), json!(made.derived));
    }
}

/// The kinds whose own name is not on their lines although no macro made them: a module of its
/// own file, a primitive, a re-export and a derive macro, whose name is in its attribute.
const NAMES_ELSEWHERE: &[&str] = &[
    "module",
    "primitive",
    "use",
    "extern_crate",
    "impl",
    "proc_derive",
];

/// Marks in `answer` an item that a macro made, as `data` of its lookup tells, with its lines in
/// `contents` (requirement R-SC-3, decided in M3c, decisions 34 and 36): made by a derive, as
/// its attribute tells, or else, unless the file has `changed` since its build, where a macro was
/// called, as its name is not on its lines.
fn made_by_macro(
    answer: &mut Map<String, Value>,
    data: &Value,
    (contents, (first, last)): (&[u8], (usize, usize)),
    changed: bool,
) {
    let made = if data.get(MADE_DERIVED).and_then(Value::as_bool) == Some(true) {
        (
            "derive",
            "the item was made by a derive: its lines are those of the derive's attribute, \
             #[derive(…)], not of the code made"
                .to_owned(),
        )
    } else if let Some(name) = data.get(MADE_NAME).and_then(Value::as_str)
        && !changed
        && named_on_its_lines(data.get("kind").and_then(Value::as_str), name)
        && !String::from_utf8_lossy(contents)
            .lines()
            .skip(first.saturating_sub(1))
            .take(last.saturating_sub(first).saturating_add(1))
            .any(|line| has_word(line, name))
    {
        (
            "invocation",
            format!(
                "{name} is not on its lines: they may be where a macro that made it was called"
            ),
        )
    } else {
        return;
    };
    if let Some(Value::Object(shown)) = answer.get_mut("data") {
        shown.insert("macro".to_owned(), json!(made.0));
    }
    push(answer, "warnings", &made.1);
}

/// Whether an item of `kind` named `name` has its name on its lines unless a macro made it: not
/// of [`NAMES_ELSEWHERE`], nor `_` or a tuple field's number.
fn named_on_its_lines(kind: Option<&str>, name: &str) -> bool {
    kind.is_some_and(|kind| !NAMES_ELSEWHERE.contains(&kind))
        && name
            .chars()
            .next()
            .is_some_and(|first| first.is_alphabetic() || first == '_')
        && name != "_"
}

/// Whether `text` has `word` as a word, as an identifier, raw too (`r#try` has `try`): words end
/// at a space or an ASCII character that no identifier has, as a combining mark goes on in one,
/// and Rust has no other characters outside comments and strings.
fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| {
        c.is_whitespace() || (c.is_ascii() && !(c.is_ascii_alphanumeric() || c == '_'))
    })
    .any(|found| found == word)
}

/// Whether the file read, `contents`, at `asked` (or its real path `real`), is the one that the
/// documentation of `found`, a build here, was built from, as the SHA-256 of its inputs tells;
/// `None` when it cannot be told: not among them, without a digest, or of another source.
#[cfg(feature = "local")]
fn built_from(found: &Found<'_>, asked: &Path, real: &Path, contents: &[u8]) -> Option<bool> {
    as_input(
        found.built.as_ref()?.record.inputs.as_ref()?,
        [asked, real],
        contents,
    )
}

/// Whether `contents`, read at the first of `paths`, as asked for, or its real path, the second, is
/// the file that `inputs` have there, as its SHA-256 tells; `None` without it among them with a
/// digest.
#[cfg(feature = "local")]
fn as_input(
    inputs: &crate::data::freshness::Inputs,
    paths: [&Path; 2],
    contents: &[u8],
) -> Option<bool> {
    let sha256 = paths.into_iter().find_map(|path| {
        inputs
            .files
            .binary_search_by(|file| file.path.as_path().cmp(path))
            .ok()
            .and_then(|at| inputs.files.get(at))
            .and_then(|file| file.sha256.as_deref())
    })?;
    Some(crate::data::cache::sha256_hex(contents).eq_ignore_ascii_case(sha256))
}

/// Without local builds, no inputs are recorded.
#[cfg(not(feature = "local"))]
const fn built_from(_: &Found<'_>, _: &Path, _: &Path, _: &[u8]) -> Option<bool> {
    None
}

/// Whether the provenance of `answer` tells that the documentation is not of the files as they
/// are: being built again, or its inputs changed.
fn provenance_stale(answer: &Map<String, Value>) -> bool {
    answer
        .get("provenance")
        .and_then(|provenance| provenance.get("stale"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// The URL of `file` at `lines` (`first-last`) of the crate of `docs`: on docs.rs among the
/// sources of `file_crate`, a release, or on GitHub at the commit of std; else none, as the span's
/// own goes.
fn file_url(docs: Source<'_>, file_crate: Option<&str>, file: &str, lines: &str) -> Option<String> {
    match (docs, file_crate) {
        (_, Some(file_crate)) => file_crate.split_once('@').map(|(name, version)| {
            format!("https://docs.rs/crate/{name}/{version}/source/{file}#{lines}")
        }),
        (Source::Std(chosen), None) => Some(format!(
            "https://github.com/rust-lang/rust/blob/{}/{file}#L{}",
            chosen.json.commit_hash(),
            lines.replace('-', "-L")
        )),
        (Source::DocsRs | Source::Local, None) => None,
    }
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

/// The sources of std of `chosen`, the JSON that the item of `found` was found in, to read `asked`
/// from: under their root, as `library/…`, which a span of the JSON, `/rustc/<commit>/library/…`,
/// is made (decided in M3c, decision 32). They are the nightly's here of the JSON's commit, or
/// else fetched and unpacked from the distribution server once, while the call waits (decided in
/// M3b, decisions 20 to 22); what that tells is added to `warnings`. Else the answer to give.
async fn std_sources(
    state: &Arc<ToolState>,
    found: &Found<'_>,
    chosen: &Chosen,
    asked: &mut AskedFile,
    (provenance, warnings): (&Map<String, Value>, &mut Vec<Box<str>>),
) -> Result<Readable, Box<CallToolResult>> {
    let commit = chosen.json.commit_hash();
    if asked.file.starts_with("/rustc/") {
        // Built on Windows, the rest of the span has its separators: `/rustc/<commit>\library\…`.
        let span = asked.file.replace('\\', "/");
        let Some(inside) = source_root::std_span(Path::new(&span), commit) else {
            return Err(Box::new(super::error_result(&json!({
                "code": "docs_unavailable",
                "reason": "outside_package",
                "message": format!(
                    "{} is not of the sources of std of commit {commit}, under its library/",
                    asked.file
                ),
                "retryable": false,
                "suggestions": [
                    "pass a file under library/, as library/core/src/option.rs, or \
                     source_location.file of get_item",
                ],
            }))));
        };
        asked.file = inside
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
            .into();
    } else if !source_root::lexically(Path::new(&*asked.file)).starts_with("library") {
        // Refused before the sources are had.
        return Err(Box::new(refused(&asked.file, Refusal::NotInPackage)));
    }
    // The nightly here is asked of its commit at most once a minute; what is in the cache, read.
    let key = format!("rust-src:{}:{commit}", chosen.json.source());
    let what = format!("the sources of std of {}", chosen.json.toolchain());
    let (cache, dist, json) = (state.cache.clone(), state.dist.clone(), chosen.json.clone());
    let joined = state
        .rust_srcs
        .join(&key, &what, found.until, async move |_stage| {
            Arc::new(std_json::rust_src(&cache, &dist, cache.dir(), &json).await)
        })
        .await;
    match joined {
        Joined::Done(had) => match &*had {
            Ok(rust_src) => {
                warnings.extend(rust_src.warnings.iter().cloned());
                Ok(Readable::RustSrc {
                    root: rust_src.root.clone(),
                    in_cache: rust_src.in_cache,
                })
            },
            Err(error) => {
                // rustup is looked for only to be named.
                #[cfg(feature = "local")]
                let rustup = matches!(error, DistError::Disabled { .. } | DistError::Offline)
                    && crate::data::toolchain::rustup_here(&state.cache).await;
                #[cfg(not(feature = "local"))]
                let rustup = false;
                Err(Box::new(rust_src_error(&what, error, rustup)))
            },
        },
        Joined::Going(doing) => Err(Box::new(get_item::building_response(
            provenance.clone(),
            doing,
            &format!(
                "{what} are looked for in the nightly toolchain here, or fetched and unpacked from \
                 the Rust distribution server"
            ),
            warnings,
        ))),
        Joined::Ended => Err(Box::new(get_item::stopped())),
    }
}

/// The tool error for `what`, the sources of std, that could not be had: as for its JSON, with
/// what to do about it, with rustup's command if `rustup` is here (decided in M3b, decision 23,
/// and in M3c, decision 33).
fn rust_src_error(what: &str, error: &DistError, rustup: bool) -> CallToolResult {
    let message = format!("{what} could not be had: {}", super::ErrorChain(error));
    // Its JSON and sources from the nightly here, which another nightly than this may be.
    let here = "or install them for the nightly toolchain here, whose std is then read, of its \
                own version: rustup component add rust-src rust-docs-json --toolchain nightly";
    let suggestions: Vec<&str> = match error {
        DistError::Disabled { .. } | DistError::Offline => {
            let setting = if matches!(error, DistError::Offline) {
                "turn network.offline off, to fetch them from the Rust distribution server"
            } else {
                "turn std.fetch_from_dist on, to fetch them from the Rust distribution server"
            };
            if rustup {
                vec![setting, here]
            } else {
                vec![setting]
            }
        },
        #[cfg(feature = "local")]
        DistError::NoLocalSource {
            why: crate::data::local_std::NoSourceHere::NotRun { timed_out: true },
        } => vec!["try again in a while"],
        #[cfg(feature = "local")]
        DistError::NoLocalSource { .. } => vec![
            "install them for the nightly toolchain here: rustup component add rust-src \
             --toolchain nightly",
        ],
        DistError::Network(_) | DistError::Status { .. } | DistError::RecentFailure { .. } => {
            vec!["try again in a while"]
        },
        DistError::Cache(_) | DistError::Unpack(UnpackError::Cache(_)) => {
            vec!["check that the cache directory can be written"]
        },
        _ => vec!["read the file on GitHub, at the url of source_location of get_item"],
    };
    super::error_result(&get_item::dist_error_body(error, &message, &suggestions))
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
        None => index_checksum(state, found.name, found.version).await?,
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
            Err(error) => Err(Box::new(sources_error(
                &release,
                error,
                state.index.is_offline(),
            ))),
        },
        Joined::Going(doing) => Err(Box::new(get_item::building_response(
            provenance,
            doing,
            &format!("the sources of {release} are fetched and unpacked"),
            warnings,
        ))),
        Joined::Ended => Err(Box::new(get_item::stopped())),
    }
}

/// The release and the path in it of a file that a span of docs.rs names outside the crate, as
/// `/opt/rustwide/cargo-home/registry/src/index.crates.io-<hash>/<name>-<version>/<path>`: a
/// dependency's from crates.io, which docs.rs's build read (decided in M3c, C2). `None` for any
/// other, as a git dependency's or one generated in the target directory.
fn rustwide(file: &str) -> Option<(CrateName, Version, String)> {
    let rest = file.strip_prefix("/opt/rustwide/cargo-home/registry/src/")?;
    let (index, rest) = rest.split_once('/')?;
    if !index.starts_with("index.crates.io-") {
        return None;
    }
    let (package, path) = rest.split_once('/')?;
    let (name, version) = crate_source::package_name(package)?;
    Some((name, version, path.to_owned()))
}

/// The checksum of `name` at `version` in the crates.io index, read again if the index as kept
/// does not list that version yet, as one published since. `None` when the index has none, or
/// offline cannot be read, which the sources then tell; else the error when the index cannot be
/// read online, which may pass.
async fn index_checksum(
    state: &Arc<ToolState>,
    name: &CrateName,
    version: &Version,
) -> Result<Option<Box<str>>, Box<CallToolResult>> {
    let listed = |entry: &crate::data::sparse_index::IndexEntry| {
        entry
            .versions
            .iter()
            .find(|listed| listed.version == *version)
            .map(|listed| listed.checksum.clone())
    };
    match state.index.entry(name).await {
        Ok(entry) if let Some(checksum) = listed(&entry) => Ok(Some(checksum)),
        Ok(_) => Ok(state
            .index
            .entry_fresh(name)
            .await
            .ok()
            .as_ref()
            .and_then(listed)),
        Err(_) if state.index.is_offline() => Ok(None),
        Err(error) => Err(Box::new(super::tool_error(
            "network_error",
            &format!(
                "the crates.io index could not be read for {name}@{version}: {}",
                super::ErrorChain(&error)
            ),
            true,
            &["try again in a while"],
        ))),
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
fn sources_error(release: &str, error: &SourcesError, offline: bool) -> CallToolResult {
    let message = format!(
        "the sources of {release} could not be had: {}",
        super::ErrorChain(error)
    );
    let (code, reason, retryable): (&str, Option<&str>, bool) = match error {
        // Offline, without the `.crate` at hand nor the checksum to fetch it with.
        SourcesError::Crate(CrateError::Offline) => ("offline_miss", None, false),
        SourcesError::NotCached if offline => ("offline_miss", None, false),
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
            project: None,
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

    /// A crates.io dependency built here is read as a release only where cargo unpacked it from
    /// crates.io, and a git one only in the checkout that cargo finished, its root; not from a
    /// source that replaces crates.io, nor vendored (decided in M3c, decision 31).
    #[cfg(feature = "local")]
    #[test]
    fn a_dependency_built_here_is_read_where_cargo_has_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let home = dir.path().join("cargo-home");
        let src = home.join("registry/src");
        assert!(unpacked_from_crates_io(
            &src.join("index.crates.io-1949cf8c6b5b557f/serde-1.0.0"),
            &home
        ));
        for elsewhere in [
            src.join("mirror.example-0123456789abcdef/serde-1.0.0"),
            dir.path().join("project/vendor/serde"),
        ] {
            assert!(
                !unpacked_from_crates_io(&elsewhere, &home),
                "{}",
                elsewhere.display()
            );
        }
        let checkout = home.join("git/checkouts/repo-0123/abcdef0");
        std::fs::create_dir_all(checkout.join("crates/sub")).expect("made");
        assert_eq!(checkout_of(&checkout.join("crates/sub"), &home), None);
        std::fs::write(checkout.join(".cargo-ok"), "").expect("written");
        assert_eq!(
            checkout_of(&checkout.join("crates/sub"), &home),
            Some(checkout)
        );
        assert_eq!(
            checkout_of(&dir.path().join("project/vendor/dep"), &home),
            None
        );
    }

    /// An item that a macro made is marked: by a derive, as its `impl` tells, or where a macro was
    /// called, as its name is not on its lines, unless the file changed since its build or its
    /// kind has its name elsewhere (requirement R-SC-3, decided in M3c, decisions 34 and 36).
    #[test]
    fn an_item_that_a_macro_made_is_marked() {
        let source = b"#[derive(Clone)]\nstruct A;\nimpl_traits!(B);\nfn r#try() {}\n";
        let marked = |data: Value, lines, changed| {
            let mut answer = Map::new();
            answer.insert("data".to_owned(), json!({}));
            made_by_macro(&mut answer, &data, (source, lines), changed);
            (
                answer["data"].get("macro").cloned(),
                answer.get("warnings").map(Value::to_string),
            )
        };
        let derived = json!({
            "path": "a::A::clone", "kind": "method", MADE_NAME: "clone", MADE_DERIVED: true,
        });
        let (made, told) = marked(derived.clone(), (1, 1), false);
        assert_eq!(made, Some(json!("derive")));
        assert!(told.is_some_and(|told| told.contains("the item was made by a derive")));
        // Told by its attribute, even when the file changed.
        assert_eq!(marked(derived, (1, 1), true).0, Some(json!("derive")));
        let called = json!({
            "path": "a::write", "kind": "function", MADE_NAME: "write", MADE_DERIVED: false,
        });
        let (made, told) = marked(called.clone(), (3, 3), false);
        assert_eq!(made, Some(json!("invocation")));
        assert!(told.is_some_and(|told| told.contains("write is not on its lines")));
        // Not when the lines may have moved.
        assert_eq!(marked(called, (3, 3), true), (None, None));
        for (name, kind, lines) in [
            // On its lines, raw too.
            ("A", "struct", (2, 2)),
            ("try", "function", (4, 4)),
            // Its name is elsewhere, by its kind, or it has none to look for.
            ("module", "module", (3, 3)),
            ("Derive", "proc_derive", (3, 3)),
            ("_", "constant", (3, 3)),
            ("0", "struct_field", (3, 3)),
        ] {
            let data =
                json!({ "path": "a::x", "kind": kind, MADE_NAME: name, MADE_DERIVED: false });
            assert_eq!(marked(data, lines, false), (None, None), "{name}");
        }
        // A word, in its case.
        assert!(has_word("impl Clone for A", "Clone"));
        assert!(!has_word("impl Clone for A", "clone"));
        assert!(!has_word("fn write_all()", "write"));
        // A combining mark goes on in a word.
        assert!(has_word("fn नमस्ते() {}", "नमस्ते"));
        assert!(!has_word("fn नमस्ते() {}", "नमस"));
    }

    /// A file read is told the one built from, or not, by the SHA-256 that a build's inputs have
    /// at its path or its real path; not without (decided in M3c, decisions 36 and 37).
    #[cfg(feature = "local")]
    #[test]
    fn a_file_is_compared_with_the_builds_inputs() {
        use crate::data::freshness::{InputFile, Inputs};
        let input = |path: &str, sha256: Option<&str>| InputFile {
            path: PathBuf::from(path),
            len: 0,
            modified: None,
            sha256: sha256.map(Box::from),
        };
        let digest = crate::data::cache::sha256_hex(b"fn a() {}\n");
        let inputs = Inputs {
            files: vec![
                input("/w/a.rs", Some(&digest)),
                input("/w/b.rs", None),
                input("/w/real.rs", Some(&digest)),
            ],
            taken_at: 0,
        };
        let compared = |asked: &str, real: &str, contents: &[u8]| {
            as_input(&inputs, [Path::new(asked), Path::new(real)], contents)
        };
        assert_eq!(compared("/w/a.rs", "/w/a.rs", b"fn a() {}\n"), Some(true));
        assert_eq!(
            compared("/w/a.rs", "/w/a.rs", b"fn a() { 1 }\n"),
            Some(false)
        );
        // By its real path, through a symlink.
        assert_eq!(
            compared("/w/link.rs", "/w/real.rs", b"fn a() {}\n"),
            Some(true)
        );
        assert_eq!(
            compared("/w/b.rs", "/w/real.rs", b"fn a() {}\n"),
            Some(true)
        );
        // Without a digest, or not among them.
        assert_eq!(compared("/w/b.rs", "/w/b.rs", b""), None);
        assert_eq!(compared("/w/c.rs", "/w/c.rs", b""), None);
    }

    /// The sources of std that cannot be had are told as its JSON is, with what to do: the
    /// settings that keep them from being fetched, and rustup's command if it is here (decided in
    /// M3b, decision 23, and in M3c, decision 33).
    #[test]
    fn the_sources_of_std_that_cannot_be_had_are_told_why() {
        let body = |result: CallToolResult| {
            let text = result.content[0].as_text().expect("text").text.clone();
            serde_json::from_str::<Value>(&text).expect("JSON")
        };
        let disabled = DistError::Disabled { why: "off".into() };
        for (error, rustup, code, reason, suggestions) in [
            (&disabled, true, "toolchain_missing", None, 2),
            (&disabled, false, "toolchain_missing", None, 1),
            (&DistError::Offline, true, "offline_miss", None, 2),
            (
                &DistError::NoSource,
                false,
                "docs_unavailable",
                Some("no_source"),
                1,
            ),
            (
                &DistError::ChecksumMismatch,
                false,
                "docs_unavailable",
                Some("corrupt"),
                1,
            ),
        ] {
            let told = body(rust_src_error("the sources of std", error, rustup));
            assert_eq!(
                (told["code"].as_str(), told["reason"].as_str()),
                (Some(code), reason),
                "{error:?}"
            );
            let given = told["suggestions"].as_array().map_or(0, Vec::len);
            assert_eq!(given, suggestions, "{error:?}: {told}");
        }
        let told = body(rust_src_error("the sources of std", &disabled, true));
        assert!(
            told["suggestions"]
                .to_string()
                .contains("std.fetch_from_dist")
                && told["suggestions"]
                    .to_string()
                    .contains("rustup component add rust-src rust-docs-json --toolchain nightly"),
            "{told}"
        );
        // Of the nightly here, as its run ran too long: again.
        #[cfg(feature = "local")]
        {
            use crate::data::local_std::NoSourceHere;
            for (why, retryable) in [
                (NoSourceHere::NotRun { timed_out: true }, true),
                (NoSourceHere::NoComponent, false),
            ] {
                let error = DistError::NoLocalSource { why };
                let told = body(rust_src_error("the sources of std", &error, true));
                assert_eq!(told["code"], "toolchain_missing", "{told}");
                assert_eq!(told["retryable"], retryable, "{told}");
            }
        }
    }

    /// A file of std is on GitHub at the JSON's commit, and a dependency's that docs.rs's build
    /// read on docs.rs among that release's sources.
    #[test]
    fn a_files_url_is_of_its_crate() {
        let commit = "b6eeb93c9a1b2c3d4e5f60718293a4b5c6d7e8f9";
        let chosen = Chosen {
            json: std_json::StdJson::Dist(crate::data::dist::StdDocs {
                dir: PathBuf::new(),
                nightly: crate::data::dist::Nightly {
                    date: "2026-10-19".into(),
                    version: "1.101.0-nightly (b6eeb93c9 2026-10-18)".into(),
                    commit_hash: commit.into(),
                    triple: "x86_64-unknown-linux-gnu".parse().expect("a triple"),
                },
                warnings: Vec::new(),
            }),
            why: Cow::Borrowed("asked"),
            warnings: Vec::new(),
        };
        let file = "library/core/src/option.rs";
        assert_eq!(
            file_url(Source::Std(&chosen), None, file, "3-5"),
            Some(format!(
                "https://github.com/rust-lang/rust/blob/{commit}/{file}#L3-L5"
            ))
        );
        assert_eq!(
            file_url(Source::Std(&chosen), None, file, "7").as_deref(),
            Some(format!("https://github.com/rust-lang/rust/blob/{commit}/{file}#L7").as_str())
        );
        assert_eq!(
            file_url(Source::DocsRs, Some("dep@1.0.0"), "src/lib.rs", "1-2").as_deref(),
            Some("https://docs.rs/crate/dep/1.0.0/source/src/lib.rs#1-2")
        );
        assert_eq!(file_url(Source::DocsRs, None, "src/lib.rs", "1"), None);
    }

    /// A span of docs.rs in a dependency from crates.io names that release and the path in it;
    /// none of another source (decided in M3c, C2).
    #[test]
    fn a_span_of_docs_rs_in_a_dependency_names_its_release() {
        let named = |file: &str| {
            rustwide(file)
                .map(|(name, version, path)| (name.to_string(), version.to_string(), path))
        };
        let index = "/opt/rustwide/cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f";
        assert_eq!(
            named(&format!("{index}/tracing-0.1.44/src/instrument.rs")),
            Some((
                "tracing".to_owned(),
                "0.1.44".to_owned(),
                "src/instrument.rs".to_owned()
            ))
        );
        assert_eq!(
            named(&format!("{index}/aws-sdk-ec2-1.0.0-beta.1/src/lib.rs")),
            Some((
                "aws-sdk-ec2".to_owned(),
                "1.0.0-beta.1".to_owned(),
                "src/lib.rs".to_owned()
            ))
        );
        assert_eq!(
            named(&format!("{index}/windows-sys-0.59.0+build.1/src/lib.rs")),
            Some((
                "windows-sys".to_owned(),
                "0.59.0+build.1".to_owned(),
                "src/lib.rs".to_owned()
            ))
        );
        // A path that leaves the release, which the judging of the sources refuses.
        assert_eq!(
            named(&format!("{index}/serde-1.0.0/../../x-1.0.0/src/lib.rs")),
            Some((
                "serde".to_owned(),
                "1.0.0".to_owned(),
                "../../x-1.0.0/src/lib.rs".to_owned()
            ))
        );
        for file in [
            "/opt/rustwide/cargo-home/registry/src/github.com-1ecc6299db9ec823/a-1.0.0/src/lib.rs",
            "/opt/rustwide/cargo-home/git/checkouts/a-0123/abcdef0/src/lib.rs",
            "/opt/rustwide/target/debug/build/a-0123/out/gen.rs",
            "src/lib.rs",
            &format!("{index}/not-a-version/src/lib.rs"),
        ] {
            assert_eq!(named(file), None, "{file}");
        }
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
            let told = body(sources_error("a@1.0.0", &error, false));
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
        // Offline, without the `.crate` nor the checksum to fetch it with (decision 15).
        let told = body(sources_error("a@1.0.0", &SourcesError::NotCached, true));
        assert_eq!(told["code"], "offline_miss");
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
