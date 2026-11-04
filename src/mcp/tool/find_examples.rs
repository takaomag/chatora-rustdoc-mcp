//! `find_examples`: the examples of a crate, or of one of its items first: the files of its
//! `examples/` by their absolute paths, and the Rust code blocks of its docs (requirement §5.9,
//! decided in M3, P13, and M3e, decisions 69 to 75).
//!
//! The release is resolved as `resolve_version` resolves it; the files are found in its sources,
//! as `get_readme` reads them; the blocks are of its documentation, which `get_item` loads, and
//! while that is had, or when there is none, the files come alone (decision 73).

use std::{io::Read as _, path::Path, sync::Arc};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use rustdoc_types::Id;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

#[cfg(feature = "local")]
use super::here;
use super::{
    InvalidArgument,
    ToolState,
    get_item::{self, Paging, ReleaseArguments},
    get_source::{self, SourcesOf},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
    resolve_version::{self, Release},
};
use crate::{
    data::{
        cache::sha256_hex,
        crate_file,
        crate_source::Sources,
        examples::Example,
        hidden,
        source_root::{self, Root},
    },
    query::{
        examples::{self, Asked, DocBlock, Entry, ExampleFile, Rank},
        get_item::{GetItemError, ItemRequest, MAX_RESPONSE_CHARS, examples_of, source_locations},
        view::CrateView,
    },
};

/// The name of the tool.
pub(super) const NAME: &str = "find_examples";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("find_examples.md");

/// The examples in a page unless `limit` says otherwise (decided in M3e, decision 72).
const DEFAULT_LIMIT: usize = 10;

/// The most examples in a page (decided in M3e, decision 72).
const MAX_LIMIT: usize = 50;

/// The most characters of one example's text shown, as `get_source` shows (decided in M3e,
/// decision 70).
const MAX_CHARS: usize = 20_000;

/// The largest file of `examples/` read: one larger is not an example here.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// The most files of `examples/` read, by name.
const MAX_FILES: usize = 1_000;

/// The most bytes of the files of `examples/` read in all: those after are left out.
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// The key in the answer that `get_item` makes that tells that the cursor was refused.
const CURSOR_REFUSED: &str = "cursor_refused";

/// Why a cursor is refused when what it was made for changed.
const CHANGED: &str = "the examples or the documentation changed since the cursor was made, or \
                       the cursor goes past the end; list again without the cursor";

/// Where the items of blocks are defined, each as `get_item` tells it, and what is warned of them.
type Located = (Vec<Option<Value>>, Box<[Box<str>]>);

/// The characters of a response kept for its provenance, warnings and hints, besides the page.
const ROOM: usize = 2_000;

/// The key in `data` of the response of `get_item` that has the answer of `find_examples`.
const ANSWER: &str = "find_examples";

/// What is asked of the examples, besides the release.
#[derive(Debug, Clone)]
pub(super) struct ExamplesAsk {
    /// The item whose examples come first, as given.
    item: Option<Box<str>>,
    /// Whether the text of the files is shown too.
    content: bool,
    /// The page.
    paging: Paging,
}

/// What `get_item` is asked for of the examples in the docs: until when, whether to forget what
/// failed of the release, what is asked, and the files of `examples/`, to be ranked with them.
#[derive(Debug, Clone)]
pub(super) struct DocExamplesAsk {
    pub(super) until: Instant,
    #[cfg(feature = "local")]
    pub(super) refresh: bool,
    pub(super) ask: ExamplesAsk,
    pub(super) files: Arc<[ExampleFile]>,
}

impl DocExamplesAsk {
    /// The item asked for, as given.
    pub(super) fn ask_item(&self) -> Option<&str> {
        self.ask.item.as_deref()
    }
}

/// The arguments checked: the release's, for `resolve_version`, and what is asked of its examples.
#[derive(Debug)]
pub(super) struct FindExamples {
    release: ReleaseArguments,
    ask: ExamplesAsk,
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
                "description": "An item whose examples come first (sync::Mutex::lock).",
            },
            "content": {
                "type": "boolean",
                "description": "Set true for the text of the files of examples/ too.",
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
                    "The target triple of the documentation.",
                    super::TARGET_DEFAULT
                ),
            },
            "project": {
                "type": "string",
                "description": const_format::concatcp!("The workspace whose Cargo.lock decides the version. Defaults to \
                                the project Claude Code was opened in.", super::PROJECT_IN_A_WORKTREE),
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most examples in a page (default 10).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Find examples of a Rust crate or item")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of the structured content (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": resolve_version::provenance_schema(),
            "retry_after_secs": { "type": "integer" },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "item": {
                        "type": "string",
                        "description": "The canonical path of the item asked for.",
                    },
                    "docs_from": {
                        "type": "string",
                        "description": "The crate and version that the blocks of the docs are of, \
                                        when the item is re-exported from it.",
                    },
                    "docs": {
                        "type": "string",
                        "enum": ["read", "pending", "unavailable"],
                        "description": "Whether the blocks of the docs are among the examples: \
                                        pending while the documentation is had.",
                    },
                    "examples": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": { "type": "string", "enum": ["file", "doc"] },
                                "match": {
                                    "type": "string",
                                    "enum": ["own", "item_and_type", "item", "none"],
                                    "description": "How it names the item asked for.",
                                },
                                "name": { "type": "string" },
                                "file": { "type": "string" },
                                "path": {
                                    "type": "string",
                                    "description": "The file's absolute path, to open.",
                                },
                                "required_features": strings,
                                "lines": {
                                    "type": "array",
                                    "items": { "type": "integer" },
                                    "description": "The lines that name the item.",
                                },
                                "lines_total": { "type": "integer" },
                                "content": { "type": "string" },
                                "item": {
                                    "type": "string",
                                    "description": "The item whose docs have the block.",
                                },
                                "attributes": strings,
                                "code": { "type": "string" },
                                "source_location": {
                                    "type": "object",
                                    "description": "Where the item is defined, as get_item \
                                                    tells it.",
                                },
                                "truncated": { "type": "boolean" },
                            },
                            "required": ["kind"],
                        },
                    },
                },
                "required": ["docs", "examples"],
            },
            "page": {
                "type": "object",
                "properties": {
                    "shown": { "type": "integer" },
                    "total": { "type": "integer" },
                    "has_more": { "type": "boolean" },
                    "next_cursor": { "type": "string" },
                },
                "required": ["shown", "total", "has_more"],
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

/// Checks the arguments (requirement R-ARG-1).
pub(super) fn parse_arguments(
    arguments: Option<&JsonObject>,
) -> Result<FindExamples, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let item = match argument("path") {
        None => None,
        Some(Value::String(path)) if !path.trim().trim_start_matches("::").is_empty() => {
            Some(Box::from(path.trim().trim_start_matches("::")))
        },
        Some(_) => {
            return Err(InvalidArgument::new(
                "`path` must be the path of an item",
                &[r#"pass a path, for example {"path": "sync::Mutex::lock"}"#],
            ));
        },
    };
    let content = match argument("content") {
        None => false,
        Some(Value::Bool(content)) => *content,
        Some(_) => {
            return Err(InvalidArgument::new("`content` must be true or false", &[
                r#"pass a boolean, for example {"content": true}"#,
            ]));
        },
    };
    let limit = match argument("limit") {
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
    let (offset, cursor_data) = match argument("cursor") {
        None => (0, None),
        Some(Value::String(cursor)) => list_module::parse_cursor(cursor, &request_digest)?,
        Some(_) => {
            return Err(InvalidArgument::new(
                "`cursor` must be a string",
                CURSOR_EXAMPLE,
            ));
        },
    };
    Ok(FindExamples {
        release,
        ask: ExamplesAsk {
            item,
            content,
            paging: Paging {
                limit,
                offset,
                request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
                cursor_data,
            },
        },
    })
}

/// Resolves the release as `resolve_version` does, finds the files of its `examples/` in its
/// sources, and ranks them with the blocks of its docs (decided in M3e, decisions 69 to 73).
pub(super) async fn execute(find: FindExamples, state: Arc<ToolState>) -> CallToolResult {
    let FindExamples { release, ask } = find;
    // One deadline for every step (requirement R-ARCH-6).
    let until = state.until();
    let asked_release = release.clone();
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
    // decisions 82 and 83).
    #[cfg(feature = "local")]
    if let Some(git) = &taken.git {
        return match here::of_git(&state, git) {
            Ok(found) => {
                let files = files_here(found).await;
                docs_and_files(&state, (asked_release, until), release, (ask, files)).await
            },
            Err(answer) => *answer,
        };
    }
    // Another package of the project, where it is; of a copy, the docs alone (decided in M3e,
    // decisions 82 and 87).
    #[cfg(feature = "local")]
    if let Some(found) = taken.found.as_deref()
        && !release.on_crates_io
        && !here::unpacked_from_crates_io(&state, found)
    {
        let mut release = release;
        let looked_up =
            here::of_project(&state, (found, &release), until, ("examples", DOCS_ONLY)).await;
        let files = match looked_up {
            Ok(found) => files_here(found).await,
            Err(here::NotHere::Answer(answer)) => return *answer,
            Err(here::NotHere::Copy(of)) => {
                release.warnings.push(
                    format!(
                        "the files of examples/ are not read: the project uses {of}; the blocks \
                         of the docs are of its build"
                    )
                    .into(),
                );
                Vec::new()
            },
        };
        return docs_and_files(&state, (asked_release, until), release, (ask, files)).await;
    }
    // As one that the project takes from crates.io, which cargo unpacked as it is, its `.crate`
    // (decided in M3e, decision 79).
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
        Err(answer) => return *answer,
    };
    let files = match crate::task::spawn_blocking(move || example_files(&sources)).await {
        Ok(files) => files,
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the reading of the examples did not finish");
            return super::tool_error(
                "internal_error",
                "the examples could not be read (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            );
        },
    };
    docs_and_files(&state, (asked_release, until), release, (ask, files)).await
}

/// What to call when the examples of a package here are not known.
#[cfg(feature = "local")]
const DOCS_ONLY: &str = "get_item shows the examples in the docs of an item";

/// The files of the `examples/` of the package `found`, as `cargo metadata` tells them or else as
/// they are found in its directory, read where they are and judged as `get_source` judges its
/// files (decided in M3e, decisions 82, 83 and 86), off the runtime's workers.
#[cfg(feature = "local")]
async fn files_here(found: here::Here) -> Vec<ExampleFile> {
    let reading = crate::task::spawn_blocking(move || {
        let dir = &found.dir;
        let examples = found.examples.as_ref().map_or_else(
            || crate_file::examples_in(dir).unwrap_or_default(),
            Clone::clone,
        );
        let paths: Vec<std::path::PathBuf> = examples
            .iter()
            .map(|example| dir.join(&*example.file))
            .collect();
        match &found.root {
            here::HereRoot::Package { workspace } => {
                let package = source_root::Package {
                    root: workspace.clone().unwrap_or_else(|| dir.clone()),
                    dir: dir.clone(),
                    examples: paths,
                    ..source_root::Package::default()
                };
                read_examples(Root::Package(&package), dir, examples)
            },
            here::HereRoot::Checkout(checkout) => {
                read_examples(Root::Checkout(checkout), dir, examples)
            },
        }
    })
    .await;
    reading.unwrap_or_else(|error| {
        let error = crate::task::resume_if_panic(error);
        tracing::error!(%error, "the reading of the examples did not finish");
        Vec::new()
    })
}

/// The tool error for a release that is not of crates.io, whose examples are not found yet.
fn not_a_release(release: &Release) -> CallToolResult {
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "build_unsupported",
        "message": format!(
            "{} {} is not a release of crates.io as docs.rs documents it (it is of {}, or a copy \
             of it): find_examples finds the examples of such a release",
            release.name, release.version, release.source
        ),
        "retryable": false,
        "suggestions": ["read its examples/ with get_source"],
    }))
}

/// The files of the `examples/` of the release whose sources are `sources`, as Cargo finds them
/// and its manifest names them, read where they are and judged as `get_source` judges its files:
/// those that cannot be read are left out (requirement R-SC-4).
fn example_files(sources: &Sources) -> Vec<ExampleFile> {
    let root = Root::Release(sources);
    let Ok(manifest) = source_root::resolve(root, Path::new("Cargo.toml")) else {
        return Vec::new();
    };
    let listed = sources.listing.files.iter().map(String::as_str);
    let Ok(found) = crate_file::manifest_examples(&manifest.path, listed) else {
        return Vec::new();
    };
    read_examples(root, &sources.root, found)
}

/// The files of `found`, relative to `dir`, read under `root`, judged as `get_source` judges its
/// files: those that cannot be read, larger than [`MAX_FILE_BYTES`], after [`MAX_FILES`] or past
/// [`MAX_TOTAL_BYTES`] in all are left out (requirement R-SC-4).
fn read_examples(root: Root<'_>, dir: &Path, found: Vec<Example>) -> Vec<ExampleFile> {
    let mut read_in_all = 0_u64;
    found
        .into_iter()
        .take(MAX_FILES)
        .filter_map(|example| {
            if read_in_all >= MAX_TOTAL_BYTES {
                return None;
            }
            let resolved = source_root::resolve(root, &dir.join(&*example.file)).ok()?;
            let mut contents = Vec::new();
            std::fs::File::open(&resolved.path)
                .and_then(|opened| {
                    opened
                        .take(MAX_FILE_BYTES.saturating_add(1))
                        .read_to_end(&mut contents)
                })
                .ok()?;
            let len = u64::try_from(contents.len()).unwrap_or(u64::MAX);
            if len > MAX_FILE_BYTES {
                return None;
            }
            read_in_all = read_in_all.saturating_add(len);
            Some(ExampleFile {
                name: example.name,
                file: example.file,
                path: resolved.path.display().to_string().into(),
                required_features: example.required_features,
                text: String::from_utf8_lossy(&contents).into_owned(),
            })
        })
        .collect()
}

/// The answer: the files of `examples/` ranked with the blocks of the docs, which `get_item`
/// loads as for `asked` until the deadline; while the docs are had, or when there are none, the
/// files alone, as they name the item asked for (decided in M3e, decision 73).
async fn docs_and_files(
    state: &Arc<ToolState>,
    (mut asked, until): (ReleaseArguments, Instant),
    release: Release,
    (ask, files): (ExamplesAsk, Vec<ExampleFile>),
) -> CallToolResult {
    asked.name = release.name.clone();
    #[cfg(feature = "local")]
    let refresh = asked.refresh;
    // The version resolved, which a crate of git has from its record.
    if asked.git.is_none() {
        asked.version = Some(crate::data::version::VersionSpec::Exact(
            release.version.clone(),
        ));
    }
    let files: Arc<[ExampleFile]> = files.into();
    let mut result = get_item::execute(
        get_item::Arguments::for_examples(asked, DocExamplesAsk {
            until,
            #[cfg(feature = "local")]
            refresh,
            ask: ask.clone(),
            files: Arc::clone(&files),
        }),
        Arc::clone(state),
    )
    .await;
    let mut warnings = release.warnings.clone();
    let mut hints = release.hints.clone();
    let mut retry_after = None;
    let docs = if result.is_error == Some(true) {
        let body = error_body(&result);
        let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
        // A path that the crate does not have, or that more than one item has, is the caller's to
        // fix.
        if matches!(
            code,
            "invalid_argument" | "item_not_found" | "ambiguous_path"
        ) {
            return result;
        }
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.is_empty())
            .unwrap_or("its documentation could not be had");
        warnings.push(format!("the examples of the docs are not here: {message}").into());
        if body.get("retryable") == Some(&json!(true)) {
            retry_after = body.get("retry_after_secs").cloned();
            hints.push(json!("call again in a while for the examples of the docs"));
        }
        "unavailable"
    } else {
        match result.structured_content.take() {
            Some(Value::Object(mut content)) => {
                if content.get("status").and_then(Value::as_str) == Some("building") {
                    retry_after = content.remove("retry_after_secs");
                    hints.push(json!(format!(
                        "the documentation is being had, which goes on without this call; call \
                         again in about {} seconds for the examples of the docs",
                        get_item::RETRY_AFTER_SECS
                    )));
                    "pending"
                } else if let Some(Value::Object(mut data)) = content.remove("data")
                    && let Some(Value::Object(mut answer)) = data.remove(ANSWER)
                {
                    if answer.contains_key(CURSOR_REFUSED) {
                        return InvalidArgument::new(CHANGED, CURSOR_EXAMPLE).into_result();
                    }
                    tell_followed(&mut content, (&mut answer, &mut warnings), &release);
                    return finish(&release, answer, (&warnings, &hints), None);
                } else {
                    tracing::error!("get_item answered find_examples without its examples");
                    warnings.push("the examples of the docs could not be had (a bug)".into());
                    "unavailable"
                }
            },
            _ => "unavailable",
        }
    };
    // The files alone, off the runtime's workers.
    let crate_name = release.name.as_str().replace('-', "_");
    let paged = crate::task::spawn_blocking(move || files_alone(&files, &ask, &crate_name)).await;
    let answer = match paged {
        Ok(Ok(mut answer)) => {
            answer.insert("docs".to_owned(), json!(docs));
            answer
        },
        Ok(Err(error)) => return *error,
        Err(error) => {
            let error = crate::task::resume_if_panic(error);
            tracing::error!(%error, "the paging of the examples did not finish");
            return super::tool_error(
                "internal_error",
                "the examples could not be listed (a bug)",
                false,
                &["report the bug to the server's maintainers, with the server's stderr log"],
            );
        },
    };
    finish(&release, answer, (&warnings, &hints), retry_after.as_ref())
}

/// The page of the files of `examples/` alone, as `ask` asks, ranked by the name of the item asked
/// for as given, its last part; nothing asked for the crate's root, named `crate_name` (decided
/// in M3e, decision 73).
fn files_alone(
    files: &[ExampleFile],
    ask: &ExamplesAsk,
    crate_name: &str,
) -> Result<Map<String, Value>, Box<CallToolResult>> {
    let name = ask
        .item
        .as_deref()
        .filter(|item| *item != crate_name)
        .and_then(|item| item.rsplit("::").next());
    let alone = name.map(|name| Asked {
        id: Id(u32::MAX),
        path: "",
        name,
        parent: None,
    });
    let entries = examples::arranged(files, &[], alone.as_ref());
    let digest = digest_of(files, "");
    page(&entries, ask, (alone.as_ref(), &digest), &|_| {
        (Vec::new(), Box::default())
    })
}

/// Adds to `answer` and `warnings` what `content`, the response of `get_item`, tells of the docs:
/// its warnings, as of a re-export followed into its crate, and the crate that the docs are of
/// when they are not `release`'s (decided in M3e, decision 81).
fn tell_followed(
    content: &mut Map<String, Value>,
    (answer, warnings): (&mut Map<String, Value>, &mut Vec<Box<str>>),
    release: &Release,
) {
    if let Some(Value::Array(told)) = content.remove("warnings") {
        warnings.extend(told.iter().filter_map(Value::as_str).map(Box::from));
    }
    if let Some(from) = docs_from(content, release) {
        warnings.push(
            format!(
                "the item is re-exported from {from}: the blocks of the docs are of {from}, and \
                 the files of examples/ of {} {}",
                release.name, release.version
            )
            .into(),
        );
        answer.insert("docs_from".to_owned(), json!(from));
    }
}

/// The crate and the version that the docs of `content`, the response of `get_item`, are of, when
/// they are not `release`'s: of the crate that the item asked for is re-exported from.
fn docs_from(content: &Map<String, Value>, release: &Release) -> Option<String> {
    let provenance = content.get("provenance")?.as_object()?;
    let name = provenance.get("crate")?.as_str()?;
    let version = provenance.get("version")?.as_str()?;
    let same = name.replace('-', "_") == release.name.as_str().replace('-', "_")
        && version == release.version.to_string();
    (!same).then(|| format!("{name} {version}"))
}

/// The JSON body of the tool error `result`.
fn error_body(result: &CallToolResult) -> Value {
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str::<Value>(&text.text).ok())
        .unwrap_or_default()
}

/// The answer for `release`: its provenance, the page `answer` (`data` and `page`, with the
/// warnings of the examples), and `warnings` and `hints` before those.
fn finish(
    release: &Release,
    mut answer: Map<String, Value>,
    (warnings, hints): (&[Box<str>], &[Value]),
    retry_after: Option<&Value>,
) -> CallToolResult {
    let mut all_warnings: Vec<Box<str>> = warnings.to_vec();
    if let Some(Value::Array(more)) = answer.remove("warnings") {
        all_warnings.extend(more.iter().filter_map(Value::as_str).map(Box::from));
    }
    let mut response = Map::new();
    response.insert(
        "provenance".to_owned(),
        Value::Object(release.provenance.clone()),
    );
    if let Some(after) = retry_after {
        response.insert("retry_after_secs".to_owned(), after.clone());
    }
    let page = answer.remove("page").unwrap_or_default();
    response.insert("data".to_owned(), Value::Object(answer));
    response.insert("page".to_owned(), page);
    if !hints.is_empty() {
        response.insert("hints".to_owned(), json!(hints));
    }
    let mut response = Value::Object(response);
    get_item::add_warnings(&mut response, &all_warnings);
    CallToolResult::structured(response)
}

/// The digest that a cursor is made for: of the files of `examples/` and of the documentation
/// whose digest is `docs`.
fn digest_of(files: &[ExampleFile], docs: &str) -> String {
    let mut text = String::from(docs);
    for file in files {
        text.push('\n');
        text.push_str(&file.file);
        text.push('\n');
        text.push_str(&sha256_hex(file.text.as_bytes()));
    }
    sha256_hex(text.as_bytes())
        .get(..16)
        .unwrap_or_default()
        .to_owned()
}

/// Adds the answer of `find_examples` to `response`, of the crate of `view` looked up for it as
/// `request` asks: the files of `examples/` that `asked` has, ranked with the blocks of the docs
/// for the item asked for, a page of them with where the items of the blocks are defined (decided
/// in M3e, decisions 69 to 72).
///
/// # Errors
///
/// Returns the error of `get_item` for the item asked for.
pub(super) fn add_answer(
    response: &mut Value,
    (view, request): (&CrateView<'_>, ItemRequest<'_>),
    asked: &DocExamplesAsk,
) -> Result<(), GetItemError> {
    let of = if asked.ask.item.is_some() {
        examples_of(view, request)?
    } else {
        None
    };
    let item = of.as_ref().map(|of| Asked {
        id: of.id,
        path: &of.path,
        name: &of.name,
        parent: of.parent.as_deref(),
    });
    let blocks: Vec<DocBlock> = examples::doc_blocks(view);
    let entries = examples::arranged(&asked.files, &blocks, item.as_ref());
    let digest = digest_of(&asked.files, view.source_digest());
    let located = |ids: &[Id]| source_locations(view, request, ids);
    let Ok(mut answer) = page(&entries, &asked.ask, (item.as_ref(), &digest), &located) else {
        // The cursor, refused, which the tool tells.
        let mut refused = Map::new();
        refused.insert(CURSOR_REFUSED.to_owned(), json!(true));
        insert_answer(response, refused);
        return Ok(());
    };
    answer.insert("docs".to_owned(), json!("read"));
    if let Some(of) = &of {
        answer.insert("item".to_owned(), json!(of.path));
    }
    insert_answer(response, answer);
    Ok(())
}

/// Inserts `answer` at [`ANSWER`] in the `data` of `response`.
fn insert_answer(response: &mut Value, answer: Map<String, Value>) {
    if let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) {
        data.insert(ANSWER.to_owned(), Value::Object(answer));
    }
}

/// The page of `entries` that `ask` asks for, of the list whose digest is `digest`, for the item
/// `asked`, within the budget of a response: `examples` and `page`, and the warnings of what was
/// shown; `located` tells where the items of the blocks shown are defined.
fn page(
    entries: &[Entry<'_>],
    ask: &ExamplesAsk,
    (asked, digest): (Option<&Asked<'_>>, &str),
    located: &dyn Fn(&[Id]) -> Located,
) -> Result<Map<String, Value>, Box<CallToolResult>> {
    let paging = &ask.paging;
    let refused = |message: &'static str| {
        Err(Box::new(
            InvalidArgument::new(message, CURSOR_EXAMPLE).into_result(),
        ))
    };
    if paging
        .cursor_data
        .as_deref()
        .is_some_and(|data| data != digest)
    {
        return refused(CHANGED);
    }
    let total = entries.len();
    if paging.offset > total {
        return refused(CHANGED);
    }
    let mut count = paging.limit;
    loop {
        let shown: Vec<&Entry<'_>> = entries.iter().skip(paging.offset).take(count).collect();
        let ids: Vec<Id> = shown
            .iter()
            .filter_map(|entry| match entry {
                Entry::Block { block, .. } => Some(block.id),
                Entry::File { .. } => None,
            })
            .collect();
        let (locations, mut warnings) = located(&ids);
        let mut locations = locations.into_iter();
        let mut marked = 0_usize;
        let listed: Vec<Value> = shown
            .iter()
            .map(|entry| {
                let location = match entry {
                    Entry::Block { .. } => locations.next().flatten(),
                    Entry::File { .. } => None,
                };
                let (value, count) = entry_value(entry, (ask.content, asked), location);
                marked = marked.saturating_add(count);
                value
            })
            .collect();
        let next = paging.offset.saturating_add(listed.len());
        let mut page = Map::new();
        page.insert("shown".to_owned(), json!(listed.len()));
        page.insert("total".to_owned(), json!(total));
        page.insert("has_more".to_owned(), json!(next < total));
        if next < total {
            page.insert(
                "next_cursor".to_owned(),
                json!(format!("{}.{digest}.{next}", paging.request_digest)),
            );
        }
        if marked > 0 {
            let mut all = warnings.into_vec();
            all.push(hidden::marked_warning(marked).into());
            warnings = all.into();
        }
        let mut answer = Map::new();
        answer.insert("examples".to_owned(), Value::Array(listed));
        answer.insert("page".to_owned(), Value::Object(page));
        if !warnings.is_empty() {
            answer.insert("warnings".to_owned(), json!(warnings));
        }
        // Fewer while over the budget of a response, one at least (requirement R-OUT-3).
        // The provenance, the release's warnings and hints come after: room is kept for them.
        let size = Value::Object(answer.clone()).to_string().chars().count();
        let budget = MAX_RESPONSE_CHARS.saturating_sub(ROOM);
        let shown_count = answer
            .get("examples")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if size <= budget || shown_count <= 1 {
            return Ok(answer);
        }
        let fits = shown_count
            .saturating_mul(budget)
            .checked_div(size)
            .unwrap_or_default();
        count = fits.clamp(1, shown_count.saturating_sub(1));
    }
}

/// `entry` as the answer shows it, with the text of a file if `content`, and how it names `asked`;
/// `location` is where the item of a block is defined. With how many characters were marked
/// (requirement R-SEC-7).
fn entry_value(
    entry: &Entry<'_>,
    (content, asked): (bool, Option<&Asked<'_>>),
    location: Option<Value>,
) -> (Value, usize) {
    let mut shown = Map::new();
    let mut marked = 0_usize;
    let mut text = |key: &str, text: &str, shown: &mut Map<String, Value>| {
        let (text, count) = hidden::mark(text);
        marked = marked.saturating_add(count);
        let cut = get_source::cut(&text, MAX_CHARS);
        if cut.len() < text.len() {
            shown.insert("truncated".to_owned(), json!(true));
        }
        shown.insert(key.to_owned(), json!(cut));
    };
    match entry {
        Entry::File { file, rank } => {
            shown.insert("kind".to_owned(), json!("file"));
            if asked.is_some() {
                shown.insert("match".to_owned(), json!(match_name(*rank)));
            }
            shown.insert("name".to_owned(), json!(file.name));
            shown.insert("file".to_owned(), json!(file.file));
            shown.insert("path".to_owned(), json!(file.path));
            if !file.required_features.is_empty() {
                shown.insert(
                    "required_features".to_owned(),
                    json!(file.required_features),
                );
            }
            if let Some(asked) = asked {
                let (lines, total) = examples::lines_naming(&file.text, asked);
                if !lines.is_empty() {
                    shown.insert("lines".to_owned(), json!(lines));
                }
                if total > lines.len() {
                    shown.insert("lines_total".to_owned(), json!(total));
                }
            }
            if content {
                text("content", &file.text, &mut shown);
            }
        },
        Entry::Block { block, rank } => {
            shown.insert("kind".to_owned(), json!("doc"));
            if asked.is_some() {
                shown.insert("match".to_owned(), json!(match_name(*rank)));
            }
            shown.insert("item".to_owned(), json!(block.item));
            if !block.attributes.is_empty() {
                shown.insert("attributes".to_owned(), json!(block.attributes));
            }
            text("code", &block.code, &mut shown);
            if let Some(location) = location {
                shown.insert("source_location".to_owned(), location);
            }
        },
    }
    (Value::Object(shown), marked)
}

/// How a rank is told.
const fn match_name(rank: Rank) -> &'static str {
    match rank {
        Rank::Own => "own",
        Rank::Both => "item_and_type",
        Rank::Name => "item",
        Rank::Other => "none",
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

    fn ask(arguments: Value) -> ExamplesAsk {
        let Value::Object(arguments) = arguments else {
            panic!("an object");
        };
        parse_arguments(Some(&arguments)).expect("valid").ask
    }

    fn file(name: &str, text: &str) -> ExampleFile {
        ExampleFile {
            name: name.into(),
            file: format!("examples/{name}.rs").into(),
            path: format!("/p/examples/{name}.rs").into(),
            required_features: Box::default(),
            text: text.into(),
        }
    }

    /// A path dependency outside the project's workspace: its examples under its directory are
    /// read, one that leads outside it through a symlink is not, nor what `[[example]]` names
    /// above it (decided in M3e, decisions 83 and 86, requirement R-SC-4).
    #[cfg(all(feature = "local", unix))]
    #[tokio::test]
    async fn the_examples_of_a_package_outside_are_read_in_it_only() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = source_root::lexically(dir.path());
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("made");
            std::fs::write(path, text).expect("written");
        };
        write("secret.rs", "fn secret() {}\n");
        write(
            "dep/Cargo.toml",
            "[package]\nname = \"dep\"\nedition = \"2021\"\n[[example]]\nname = \"up\"\n\
             path = \"../secret.rs\"\n",
        );
        write("dep/examples/inside.rs", "fn main() {}\n");
        std::os::unix::fs::symlink(root.join("secret.rs"), root.join("dep/examples/leak.rs"))
            .expect("linked");
        // Without cargo metadata, as of a record, and as cargo metadata tells them.
        let dir_of_dep = root.join("dep");
        let listed = |examples| here::Here {
            dir: dir_of_dep.clone(),
            root: here::HereRoot::Package { workspace: None },
            readme: None,
            examples,
        };
        let told = vec![
            Example {
                name: "inside".into(),
                file: "examples/inside.rs".into(),
                required_features: Box::default(),
            },
            Example {
                name: "leak".into(),
                file: "examples/leak.rs".into(),
                required_features: Box::default(),
            },
        ];
        for examples in [None, Some(told)] {
            let read: Vec<String> = files_here(listed(examples))
                .await
                .into_iter()
                .map(|file| file.file.to_string())
                .collect();
            assert_eq!(read, ["examples/inside.rs"]);
        }
    }

    /// `path`, `content`, `limit` and `cursor` are checked.
    #[test]
    fn odd_arguments_are_refused() {
        for (arguments, part) in [
            (json!({ "crate": "tokio", "path": "::" }), "`path`"),
            (json!({ "crate": "tokio", "content": "yes" }), "`content`"),
            (json!({ "crate": "tokio", "limit": 0 }), "`limit`"),
            (json!({ "crate": "tokio", "limit": 51 }), "`limit`"),
            (json!({ "crate": "tokio", "cursor": 1 }), "`cursor`"),
        ] {
            let Value::Object(arguments) = arguments else {
                panic!("an object");
            };
            let error = parse_arguments(Some(&arguments)).expect_err("invalid");
            assert!(error.message.contains(part), "{error:?}");
        }
        let asked = ask(json!({ "crate": "tokio", "path": "::sync::Mutex" }));
        assert_eq!(asked.item.as_deref(), Some("sync::Mutex"));
        assert_eq!(asked.paging.limit, DEFAULT_LIMIT);
    }

    /// The files alone, as without the docs: by how they name the item, then by name; the lines
    /// that do, the features they need, their text when asked; marked; within the budget of a
    /// response, the rest on the next page (decided in M3e, decisions 70, 72 and 73).
    #[test]
    fn a_page_of_files_is_answered_within_the_budget() {
        let mut needs = file("needs", "use demo::Mutex;\nm.lock();\n");
        needs.required_features = Box::from([Box::from("full")]);
        let files = [
            file("big", &"x".repeat(MAX_CHARS + 50)),
            needs,
            file("hidden", "m.lock(); \u{202e}\n"),
        ];
        let lock = Asked {
            id: Id(u32::MAX),
            path: "",
            name: "lock",
            parent: Some("Mutex"),
        };
        let entries = examples::arranged(&files, &[], Some(&lock));
        let asked = ask(json!({ "crate": "demo", "path": "Mutex::lock", "content": true }));
        let none = |_: &[Id]| (Vec::new(), Box::<[Box<str>]>::default());
        let answer = page(&entries, &asked, (Some(&lock), "ab"), &none).expect("answered");
        let examples = answer["examples"].as_array().expect("examples");
        let names: Vec<&str> = examples
            .iter()
            .filter_map(|example| example["name"].as_str())
            .collect();
        assert_eq!(names, ["needs", "hidden", "big"]);
        assert_eq!(examples[0]["match"], "item_and_type");
        assert_eq!(examples[0]["lines"], json!([2]));
        assert_eq!(examples[0]["required_features"], json!(["full"]));
        assert_eq!(examples[1]["match"], "item");
        assert!(
            examples[1]["content"]
                .as_str()
                .is_some_and(|text| text.contains("<U+202E>"))
        );
        assert_eq!(examples[2]["truncated"], true);
        assert!(answer["warnings"].to_string().contains("1 invisible"));
        assert!(Value::Object(answer.clone()).to_string().chars().count() <= MAX_RESPONSE_CHARS);
        // Two of the large: one a page, the other on the next.
        let large = [
            file("a", &"y".repeat(MAX_CHARS)),
            file("b", &"z".repeat(MAX_CHARS)),
        ];
        let entries = examples::arranged(&large, &[], None);
        let all = ask(json!({ "crate": "demo", "content": true }));
        let first = page(&entries, &all, (None, "ab"), &none).expect("answered");
        assert_eq!(first["page"]["shown"], 1);
        assert_eq!(first["page"]["has_more"], true);
        // A cursor of other examples is refused.
        let cursor = first["page"]["next_cursor"].as_str().expect("a cursor");
        let next = ask(json!({ "crate": "demo", "content": true, "cursor": cursor }));
        let refused = page(&entries, &next, (None, "cd"), &none).unwrap_err();
        assert_eq!(refused.is_error, Some(true));
        let rest = page(&entries, &next, (None, "ab"), &none).expect("answered");
        assert_eq!(rest["examples"][0]["name"], "b");
    }

    /// Without the docs, the item's name as given ranks the files; the crate's root asks for
    /// nothing; the docs of another crate are told (decided in M3e, decisions 73 and 81).
    #[test]
    fn the_files_alone_and_the_docs_of_another_crate() {
        let files = [
            file("plain", "fn main() {}"),
            file("uses", "tokio::spawn(x);"),
        ];
        let asked = |item: &str| ask(json!({ "crate": "tokio", "path": item }));
        let names = |answer: &Map<String, Value>| -> Vec<(String, Value)> {
            answer["examples"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|example| {
                    (
                        example["name"].as_str().unwrap_or_default().to_owned(),
                        example.get("match").cloned().unwrap_or_default(),
                    )
                })
                .collect()
        };
        let spawn = files_alone(&files, &asked("spawn"), "tokio").expect("answered");
        assert_eq!(names(&spawn), [
            ("uses".to_owned(), json!("item_and_type")),
            ("plain".to_owned(), json!("none")),
        ]);
        let root = files_alone(&files, &asked("tokio"), "tokio").expect("answered");
        assert_eq!(names(&root), [
            ("plain".to_owned(), Value::Null),
            ("uses".to_owned(), Value::Null),
        ]);
        let release = Release {
            provenance: Map::new(),
            warnings: Vec::new(),
            hints: Vec::new(),
            name: "axum".parse().expect("a name"),
            version: semver::Version::new(0, 8, 9),
            source: "crates.io".into(),
            on_crates_io: true,
            docs_rs_json: "available".into(),
            retry_after: None,
        };
        let content = |name: &str, version: &str| -> Map<String, Value> {
            let Value::Object(content) =
                json!({ "provenance": { "crate": name, "version": version } })
            else {
                panic!("an object");
            };
            content
        };
        assert_eq!(docs_from(&content("axum", "0.8.9"), &release), None);
        assert_eq!(
            docs_from(&content("http", "1.3.1"), &release).as_deref(),
            Some("http 1.3.1")
        );
    }
}
