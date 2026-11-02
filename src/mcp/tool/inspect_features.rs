//! The `inspect_features` tool: the features of a crate, what each enables and what enables it
//! (requirement §5.9's features, §3 Q11, decided in M3d).
//!
//! The release is resolved as `resolve_version` resolves it, without its documentation nor a
//! build; its features are read from the crates.io index's line of that release (decision 41),
//! and those that docs.rs's build did not enable from its `.crate` (decisions 46 and 47).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use rmcp::model::{CallToolResult, JsonObject, Tool};
use semver::Version;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::{
    ErrorChain,
    InvalidArgument,
    ToolState,
    get_item::{self, Paging, ReleaseArguments},
    list_module::{self, CURSOR_EXAMPLE, CURSOR_FORM},
    resolve_version,
};
#[cfg(feature = "local")]
use super::{project, recorded_git::GitFound};
#[cfg(feature = "local")]
use crate::data::{
    flight::Joined,
    metadata::Resolution,
    packages::DocsRsCheck,
    project::Workspace,
    target::TargetTriple,
};
use crate::{
    data::{
        cache::sha256_hex,
        crate_file,
        crate_name::CrateName,
        feature_tree::FeatureTree,
        sparse_index::{IndexEntry, IndexError},
        version::VersionSpec,
    },
    query::{
        feature_items::{FeatureItem, feature_items},
        get_item::MAX_RESPONSE_CHARS,
        search::edit_distance,
        view::CrateView,
    },
};

/// The name of the tool.
pub(super) const NAME: &str = "inspect_features";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("inspect_features.md");

/// The features in a page unless `limit` says otherwise (decided in M3d, decision 39).
const DEFAULT_LIMIT: usize = 100;

/// The most features in a page, as for the other lists.
const MAX_LIMIT: usize = 200;

/// The most names in one list of a feature, as what enables a feature that thousands of others do
/// (`windows`'s `Win32`): the rest is counted (requirement R-OUT-3).
const MAX_LISTED: usize = 50;

/// How to pass `feature`, for `invalid_argument`.
const FEATURE_EXAMPLE: &[&str] =
    &[r#"pass a feature of the crate, for example {"feature": "net"}"#];

/// What is asked of the features, besides the release (decided in M3d, decisions 38 and 39).
#[derive(Debug, Clone)]
pub(super) struct FeatureAsk {
    /// The feature asked for alone, if one was.
    feature: Option<Box<str>>,
    /// The page of the features.
    paging: Paging,
}

/// What `get_item` is asked for of the items of each feature: until when, whether to forget what
/// failed of the release, and the feature whose items are told.
#[derive(Debug, Clone)]
pub(super) struct FeatureItemsAsk {
    pub(super) until: Instant,
    #[cfg(feature = "local")]
    pub(super) refresh: bool,
    pub(super) feature: Option<Box<str>>,
}

/// The arguments checked: the release's, for `resolve_version`, and what is asked of its
/// features.
#[derive(Debug)]
pub(super) struct Inspect {
    release: ReleaseArguments,
    ask: FeatureAsk,
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
            "feature": {
                "type": "string",
                "description": "One feature alone (net). All of them if left out.",
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
                    "The target triple, for whether docs.rs has its documentation.",
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
                "description": "The most features in a page (default 100).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["crate"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Inspect a Rust crate's features")
        .with_raw_output_schema(Arc::new(super::with_building(output_schema())))
        .with_annotations(super::annotations())
}

/// The schema of `items_pending`: the `building_reason` of the documentation being had.
fn items_pending_schema() -> Value {
    let mut schema = super::building_reason_schema();
    if let Some(pending) = schema.as_object_mut() {
        pending.insert(
            "description".to_owned(),
            json!(
                "The documentation is being had, as this tells: call again after \
                 retry_after_secs for the items."
            ),
        );
    }
    schema
}

/// The schema of the structured content (requirement R-OUT-1).
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let total = json!({
        "type": "integer",
        "description": "How many there are, when the list is cut.",
    });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": resolve_version::provenance_schema(),
            "retry_after_secs": { "type": "integer" },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "default": strings,
                    "default_total": total,
                    "docs_rs_json": {
                        "type": "string",
                        "enum": ["available", "no_json", "no_target_json", "not_on_docs_rs", "unknown"],
                        "description": "Whether docs.rs has the documentation, as resolve_version \
                                        tells.",
                    },
                    "items_pending": items_pending_schema(),
                    "items": {
                        "type": "array",
                        "description": "With feature: its items, entries first.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "kind": { "type": "string" },
                                "required_features": { "type": "string" },
                                "other_cfg": strings,
                                "entry": { "type": "boolean" },
                                "negated": { "type": "boolean" },
                            },
                            "required": ["path", "kind", "entry"],
                        },
                    },
                    "features": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "enables": strings,
                                "enables_total": total,
                                "closure": strings,
                                "closure_total": total,
                                "dependencies": strings,
                                "dependencies_total": total,
                                "enabled_by": strings,
                                "enabled_by_total": total,
                                "default": { "type": "boolean" },
                                "implicit": { "type": "boolean" },
                                "in_docs": { "type": "boolean" },
                                "items": {
                                    "type": "integer",
                                    "description": "How many items its documentation has that \
                                                    need it, when that is known.",
                                },
                                "enabled_in_project": {
                                    "type": "boolean",
                                    "description": "Whether the project's normal build enables \
                                                    it, of a version that it uses.",
                                },
                                "enabled_in_dev_only": {
                                    "type": "boolean",
                                    "description": "Only its build with the dev-dependencies, as \
                                                    of the tests, enables it.",
                                },
                            },
                            "required": ["name", "enables"],
                        },
                    },
                },
                "required": ["default", "features"],
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
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<Inspect, InvalidArgument> {
    let release = get_item::parse_release(arguments)?;
    if get_item::std_crate(&release.name).is_some() {
        return Err(InvalidArgument::new(
            format!("{} has no features to enable", release.name),
            &["std's items that need a feature of the compiler tell it in get_item"],
        ));
    }
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let feature = match argument("feature") {
        None => None,
        Some(Value::String(feature)) if !feature.trim().is_empty() => {
            Some(Box::from(feature.trim()))
        },
        Some(_) => {
            return Err(InvalidArgument::new(
                "`feature` must be a string that is not empty",
                FEATURE_EXAMPLE,
            ));
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
                    &[r#"pass a whole number, for example {"limit": 100}"#],
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
    Ok(Inspect {
        release,
        ask: FeatureAsk {
            feature,
            paging: Paging {
                limit,
                offset,
                request_digest: format!("{CURSOR_FORM}.{request_digest}").into(),
                cursor_data,
            },
        },
    })
}

/// The release that `resolve_version` resolved, as it tells it.
struct Release {
    provenance: Map<String, Value>,
    warnings: Vec<Box<str>>,
    hints: Vec<Value>,
    name: CrateName,
    version: Version,
    /// `crates.io`, `path`, `git` or `registry`.
    source: Box<str>,
    /// Whether it is the release on crates.io, as docs.rs has it: of crates.io, and not a copy of
    /// the project that differs or that the index does not have.
    on_crates_io: bool,
    /// Whether docs.rs has its documentation.
    docs_rs_json: Box<str>,
    /// When to call again, as for a build of git being made again.
    retry_after: Option<Value>,
}

impl Release {
    /// The release that the response `content` of `resolve_version` tells, if it tells one.
    fn of(content: &Value) -> Option<Self> {
        fn text(value: Option<&Value>) -> &str {
            value.and_then(Value::as_str).unwrap_or_default()
        }
        let provenance = content.get("provenance")?.as_object()?.clone();
        let data = content.get("data")?;
        let source = text(data.get("source"));
        let docs_rs_json = text(data.get("docs_rs_json"));
        Some(Self {
            name: text(provenance.get("crate")).parse().ok()?,
            version: Version::parse(text(data.get("version"))).ok()?,
            on_crates_io: source == "crates.io"
                && text(provenance.get("source")) == "crates.io"
                && docs_rs_json != "not_on_docs_rs",
            source: source.into(),
            docs_rs_json: docs_rs_json.into(),
            retry_after: content.get("retry_after_secs").cloned(),
            warnings: content
                .get("warnings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(Box::from)
                .collect(),
            hints: content
                .get("hints")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            provenance,
        })
    }

    /// The target, as the provenance tells it.
    #[cfg(feature = "local")]
    fn target(&self) -> Option<TargetTriple> {
        self.provenance
            .get("target")
            .and_then(Value::as_str)
            .and_then(|target| target.parse().ok())
    }
}

/// What is known of the features of a release.
struct Known {
    tree: FeatureTree,
    /// Those that its documentation does not have, when it is known.
    not_in_docs: Option<Box<[Box<str>]>>,
}

/// The features of a package in the project's build for the target (requirement R-FT-2, decided
/// in M3d, decisions 45 and 54).
pub(super) struct ProjectBuild {
    /// Those of its normal build; `None` when it does not build the package for the target.
    normal: Option<BTreeSet<Box<str>>>,
    /// Those of the build with the members' dev-dependencies.
    with_dev: Option<BTreeSet<Box<str>>>,
}

/// Resolves the release as `resolve_version` does, and tells its features (decided in M3d,
/// decisions 41, 46 to 53): of a crates.io release, from the crates.io index, with those that
/// docs.rs's build did not enable from its `.crate`; of another package of the project, from its
/// `cargo metadata`; of a crate of git that `build_external_docs` recorded, from its `Cargo.toml`;
/// with whether the project's build enables each. Neither its documentation nor a build is needed.
pub(super) async fn execute(inspect: Inspect, state: Arc<ToolState>) -> CallToolResult {
    let Inspect { release, ask } = inspect;
    // One deadline for every step (requirement R-ARCH-6).
    let until = state.until();
    #[cfg(feature = "local")]
    let (refresh, project) = (release.refresh, release.project.clone());
    // For `get_item`, which loads its documentation.
    let asked = (release.clone(), until);
    #[cfg_attr(
        not(feature = "local"),
        expect(
            unused_variables,
            reason = "only a project or a record has more to tell"
        )
    )]
    let (resolved, taken) = resolve_version::resolving(release, Arc::clone(&state), until).await;
    // An error, or the response while the release is resolved, as it is.
    let Some(mut release) = resolved
        .structured_content
        .as_ref()
        .filter(|_| resolved.is_error != Some(true))
        .and_then(Release::of)
    else {
        return resolved;
    };
    #[cfg(feature = "local")]
    if let Some(git) = &taken.git {
        return match git_features(git).await {
            Ok(known) => answer(&state, asked, (&mut release, &known, None), &ask).await,
            Err(answer) => *answer,
        };
    }
    #[cfg(feature = "local")]
    if let Some(found) = taken.found.as_deref()
        && !release.on_crates_io
    {
        let known = match project_features(&state, found, until, &mut release).await {
            Ok(known) => known,
            Err(answer) => return *answer,
        };
        let in_project = in_project(&state, &found.workspace, until, &mut release).await;
        return answer(
            &state,
            asked,
            (&mut release, &known, in_project.as_ref()),
            &ask,
        )
        .await;
    }
    if !release.on_crates_io {
        let of = if release.source.as_ref() == "crates.io" {
            "a copy of the project that the crates.io index does not have"
        } else {
            &release.source
        };
        return not_a_release(&format!("{} {}", release.name, release.version), of);
    }
    #[cfg(feature = "local")]
    let refreshing = refresh;
    #[cfg(not(feature = "local"))]
    let refreshing = false;
    #[cfg_attr(
        not(feature = "local"),
        expect(
            unused_variables,
            reason = "only a project tells whether it uses the release"
        )
    )]
    let (known, entry) = match release_known(&state, &mut release, (refreshing, until)).await {
        Ok(found) => found,
        Err(answer) => return *answer,
    };
    #[cfg(feature = "local")]
    {
        let workspace = match taken.found {
            Some(found) => Some(found.workspace),
            None => {
                project_of_release(&state, project.as_deref(), (&entry, until), &mut release).await
            },
        };
        let in_project = match &workspace {
            Some(workspace) => in_project(&state, workspace, until, &mut release).await,
            None => None,
        };
        answer(
            &state,
            asked,
            (&mut release, &known, in_project.as_ref()),
            &ask,
        )
        .await
    }
    #[cfg(not(feature = "local"))]
    answer(&state, asked, (&mut release, &known, None), &ask).await
}

/// The key in `data` of the crate's root looked up for `inspect_features` of how many items each
/// feature has, which the answer does not show.
const FEATURE_COUNTS: &str = "feature_counts";

/// The key of the items of the feature asked for, which the answer shows otherwise.
const FEATURE_ITEMS: &str = "feature_items";

/// The key of the features that docs.rs's build left out.
const LACKING: &str = "lacking";

/// The key of the digest of the documentation, for the cursor of the items.
const SOURCE_DIGEST: &str = "source_digest";

/// Adds to `response`, of the root of the crate of `view` looked up for `inspect_features`, how
/// many items each feature has, the items of `feature` if one is asked for, and the features that
/// docs.rs's build left out, `lacking`, if it is docs.rs's (decided in M3d, D3): only those, as a
/// crate may have thousands of features.
pub(super) fn add_items(
    response: &mut Value,
    view: &CrateView<'_>,
    feature: Option<&str>,
    lacking: Option<Box<[Box<str>]>>,
) {
    let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) else {
        return;
    };
    let by_feature = feature_items(view);
    let counts: Map<String, Value> = by_feature
        .iter()
        .map(|(name, items)| (name.to_string(), json!(items.len())))
        .collect();
    data.insert(FEATURE_COUNTS.to_owned(), Value::Object(counts));
    if let Some(items) = feature.and_then(|feature| by_feature.get(feature)) {
        data.insert(
            FEATURE_ITEMS.to_owned(),
            items.iter().map(item_value).collect(),
        );
    }
    if let Some(lacking) = lacking {
        data.insert(LACKING.to_owned(), json!(lacking));
    }
    data.insert(SOURCE_DIGEST.to_owned(), json!(view.source_digest()));
}

/// `item` as the answer shows it.
fn item_value(item: &FeatureItem) -> Value {
    let mut shown = Map::new();
    shown.insert("path".to_owned(), json!(item.path));
    shown.insert("kind".to_owned(), json!(item.kind));
    if let Some(features) = &item.required_features {
        shown.insert("required_features".to_owned(), json!(features));
    }
    if !item.other_cfg.is_empty() {
        shown.insert("other_cfg".to_owned(), json!(item.other_cfg));
    }
    shown.insert("entry".to_owned(), json!(item.entry));
    if item.negated {
        shown.insert("negated".to_owned(), json!(true));
    }
    Value::Object(shown)
}

/// What the documentation tells of the items of each feature (decided in M3d, decisions 42 and
/// 47).
enum Items {
    /// They were found: how many each feature has, and the items of the one asked for, in the
    /// documentation whose digest is `digest`, which lacks the features `lacking` as docs.rs built
    /// it, or was built here with the features `built_with`.
    Found {
        counts: BTreeMap<Box<str>, usize>,
        listed: Vec<Value>,
        digest: Box<str>,
        lacking: Option<Box<[Box<str>]>>,
        built_with: Option<BTreeSet<Box<str>>>,
    },
    /// The documentation is being had, as the `building_reason` tells, which goes on without the
    /// call.
    Pending(Box<str>),
    /// They are not known, which a warning tells.
    Unknown,
}

/// The items of each feature of `release`, from its documentation, which `get_item` loads as
/// for `asked` until `until`, with what failed of the release forgotten if `refresh`: fetched
/// from docs.rs, or built here as it builds them (decided in M3d, decisions 42 and 47). What it
/// warns of is added to `release`.
async fn items_of(
    state: &Arc<ToolState>,
    (mut asked, feature, until): (ReleaseArguments, Option<&str>, Instant),
    release: &mut Release,
) -> Items {
    asked.name = release.name.clone();
    #[cfg(feature = "local")]
    let refresh = asked.refresh;
    // The version resolved, which a crate of git has from its record.
    if asked.git.is_none() {
        asked.version = Some(VersionSpec::Exact(release.version.clone()));
    }
    let mut result = get_item::execute(
        get_item::Arguments::for_feature_items(asked, FeatureItemsAsk {
            until,
            #[cfg(feature = "local")]
            refresh,
            feature: feature.map(Box::from),
        }),
        Arc::clone(state),
    )
    .await;
    if result.is_error == Some(true) {
        not_had(&result, release);
        return Items::Unknown;
    }
    match result.structured_content.take() {
        Some(Value::Object(content)) => found(content, release),
        _ => Items::Unknown,
    }
}

/// Warns in `release` that the items are not known, as the error `result` of `get_item` tells,
/// with when to call again if it may pass.
fn not_had(result: &CallToolResult, release: &mut Release) {
    let body = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|text| serde_json::from_str::<Value>(&text.text).ok())
        .unwrap_or_default();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or("its documentation could not be had");
    release
        .warnings
        .push(format!("which items each feature enables is not known: {message}").into());
    // Of what may pass, when to call again.
    if body.get("retryable") == Some(&json!(true)) {
        release
            .hints
            .push(json!("call again in a while for the items of each feature"));
        if release.retry_after.is_none() {
            release.retry_after = body.get("retry_after_secs").cloned();
        }
    }
}

/// The items of each feature as the response `content` of `get_item` tells them, of the
/// documentation of `release` or another version, or being had; what it warns of is added to
/// `release`.
fn found(mut content: Map<String, Value>, release: &mut Release) -> Items {
    if content.get("status").and_then(Value::as_str) == Some("building") {
        if release.retry_after.is_none() {
            release.retry_after = content.remove("retry_after_secs");
        }
        let doing = content
            .get("building_reason")
            .and_then(Value::as_str)
            .unwrap_or("fetching");
        return Items::Pending(doing.into());
    }
    let mut provenance = match content.remove("provenance") {
        Some(Value::Object(provenance)) => provenance,
        _ => Map::new(),
    };
    let version = provenance
        .get("version")
        .and_then(Value::as_str)
        .and_then(|version| Version::parse(version).ok());
    if version.as_ref() != Some(&release.version) {
        release.warnings.push(
            "which items each feature enables is not known: the documentation is of another \
             version"
                .into(),
        );
        return Items::Unknown;
    }
    // The build before, while the member is built again.
    if provenance.get("stale_reason").and_then(Value::as_str) == Some("rebuilding") {
        release
            .warnings
            .push("the items are those of the build before, which is being built again".into());
        if release.retry_after.is_none() {
            release.retry_after = content.remove("retry_after_secs");
        }
    }
    let Some(Value::Object(mut data)) = content.remove("data") else {
        return Items::Unknown;
    };
    let strings = |value: Option<Value>| -> Option<Vec<Box<str>>> {
        match value {
            Some(Value::Array(values)) => Some(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(Box::from)
                    .collect(),
            ),
            _ => None,
        }
    };
    let built_with = (provenance.get("source").and_then(Value::as_str) == Some("local"))
        .then(|| strings(provenance.remove("features_effective")))
        .flatten()
        .map(|features| features.into_iter().collect());
    let counts = match data.remove(FEATURE_COUNTS) {
        Some(Value::Object(counts)) => counts
            .into_iter()
            .filter_map(|(name, count)| {
                Some((
                    name.into_boxed_str(),
                    usize::try_from(count.as_u64()?).ok()?,
                ))
            })
            .collect(),
        _ => BTreeMap::new(),
    };
    Items::Found {
        counts,
        listed: match data.remove(FEATURE_ITEMS) {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        digest: data
            .get(SOURCE_DIGEST)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        lacking: strings(data.remove(LACKING)).map(Vec::into_boxed_slice),
        built_with,
    }
}

/// The warnings and the hints of `release`, with those of what its features, `tree`, those that
/// its documentation does not have, `not_in_docs`, `in_project` and `items` tell: the features
/// that the project enables but the documentation does not have, how to read those in neither,
/// those that a build here was without, and when to call again for the items
/// (decided in M3d, decisions 51, 52 and 57).
fn notes(
    release: &Release,
    (tree, not_in_docs, in_project): (&FeatureTree, Option<&[Box<str>]>, Option<&ProjectBuild>),
    items: &Items,
) -> (Vec<Box<str>>, Vec<Value>) {
    let mut warnings = release.warnings.clone();
    let mut hints = release.hints.clone();
    if let Some(missing) = not_in_docs.filter(|missing| !missing.is_empty()) {
        let enabled: Vec<&str> = in_project
            .and_then(|in_project| in_project.normal.as_ref())
            .map(|normal| {
                missing
                    .iter()
                    .map(AsRef::as_ref)
                    .filter(|feature| normal.contains(*feature))
                    .collect()
            })
            .unwrap_or_default();
        if !enabled.is_empty() {
            warnings.push(
                format!(
                    "the project enables {}, whose items get_item does not show: docs.rs's \
                     documentation does not have them",
                    enabled.join(", ")
                )
                .into(),
            );
        }
        hints.push(json!(format!(
            "docs.rs and build_external_docs document {} with the features of its \
             [package.metadata.docs.rs], so the items of a feature with in_docs: false are in \
             neither; read their source with get_source",
            release.name
        )));
    }
    // A build here without some features: their items are not in its documentation. Those that
    // it lacks as docs.rs's does are told above.
    if let Items::Found {
        built_with: Some(built_with),
        ..
    } = items
    {
        let without: Vec<&str> = tree
            .names()
            .filter(|&feature| {
                !built_with.contains(feature)
                    && !not_in_docs
                        .is_some_and(|missing| missing.iter().any(|name| name.as_ref() == feature))
            })
            .collect();
        if !without.is_empty() {
            let more = without.len().saturating_sub(MAX_LISTED);
            warnings.push(
                format!(
                    "the documentation was built without {}{}, whose items are not counted",
                    without
                        .iter()
                        .take(MAX_LISTED)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", "),
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                )
                .into(),
            );
        }
    }
    if let Items::Pending(doing) = items {
        hints.push(json!(format!(
            "the documentation is {}, which goes on without this call; call again in about {} \
             seconds for the items of each feature",
            pending_doing(doing),
            get_item::RETRY_AFTER_SECS
        )));
    }
    (warnings, hints)
}

/// The features that the documentation lacks, as `known` tells, or else as `get_item` tells of
/// docs.rs's build in `items`, when its `.crate` did not tell them.
fn lacking_of<'k>(known: &'k Known, items: &'k Items) -> Option<&'k [Box<str>]> {
    known.not_in_docs.as_deref().or(match items {
        Items::Found { lacking, .. } => lacking.as_deref(),
        Items::Pending(_) | Items::Unknown => None,
    })
}

/// What is done of the documentation, as the `building_reason` `doing` tells.
fn pending_doing(doing: &str) -> &'static str {
    match doing {
        "fetching" => "being fetched",
        "building" => "being built",
        "indexing" => "being indexed",
        "waiting_for_lock" => "waiting for another call that has it",
        "waiting_for_slot" => "waiting for a build to end before it is built",
        _ => "being had",
    }
}

/// The response for `release`, whose features `known` tells, with whether the project's build
/// enables each, `in_project`, and the items of each from its documentation, which `get_item`
/// loads as for `asked` until the deadline, as `ask` asks: those that the project enables but the
/// documentation does not have are warned about, and how to read them told (decided in M3d,
/// decisions 51, 52 and 57).
async fn answer(
    state: &Arc<ToolState>,
    asked: (ReleaseArguments, Instant),
    (release, known, in_project): (&mut Release, &Known, Option<&ProjectBuild>),
    ask: &FeatureAsk,
) -> CallToolResult {
    let (asked, until) = asked;
    let items = items_of(state, (asked, ask.feature.as_deref(), until), release).await;
    let not_in_docs = lacking_of(known, &items);
    let (warnings, hints) = notes(release, (&known.tree, not_in_docs, in_project), &items);
    respond(
        &Told {
            provenance: &release.provenance,
            tree: &known.tree,
            not_in_docs,
            in_project,
            items: &items,
            docs_rs_json: &release.docs_rs_json,
            warnings: &warnings,
            hints: &hints,
            retry_after: release.retry_after.as_ref(),
        },
        ask,
    )
    .map_or_else(|error| *error, CallToolResult::structured)
}

/// The features of the crate of git `found` that `build_external_docs` recorded, from the
/// `Cargo.toml` of its checkout, with those that its build did not enable (decided in M3d,
/// decision 49).
#[cfg(feature = "local")]
async fn git_features(found: &GitFound) -> Result<Known, Box<CallToolResult>> {
    let record = &found.built.record;
    let Some(root) = record.package_root.clone() else {
        return Err(Box::new(super::tool_error(
            "docs_unavailable",
            &format!(
                "the record of {} is from before where its package is was kept",
                found.package
            ),
            false,
            &["build it again with build_external_docs"],
        )));
    };
    let read = crate::task::spawn_blocking(move || {
        crate_file::manifest_features(&root.join("Cargo.toml"))
    })
    .await
    .unwrap_or_else(|error| Err(std::io::Error::other(crate::task::resume_if_panic(error))));
    let tree = read.map_err(|error| {
        Box::new(super::tool_error(
            "docs_unavailable",
            &format!(
                "the Cargo.toml of {} could not be read: {}",
                found.package,
                ErrorChain(&error)
            ),
            false,
            &["build it again with build_external_docs"],
        ))
    })?;
    let not_in_docs = record.features_effective.as_ref().map(|effective| {
        tree.names()
            .filter(|feature| !effective.iter().any(|built| built.as_ref() == *feature))
            .map(Box::from)
            .collect()
    });
    Ok(Known { tree, not_in_docs })
}

/// The features of the project's package `found`, from its `cargo metadata` (decided in M3d,
/// decisions 48, 49 and 53); a member whose `[package.metadata.docs.rs]` asks for features is
/// warned about in `release`, which `get_item` documents it with.
#[cfg(feature = "local")]
async fn project_features(
    state: &ToolState,
    found: &project::Found,
    until: Instant,
    release: &mut Release,
) -> Result<Known, Box<CallToolResult>> {
    let what = format!("{} {}", release.name, release.version);
    let resolution = match project::resolve_until(state, &found.workspace, until).await {
        Joined::Done(Ok(resolution)) => resolution,
        Joined::Done(Err(error)) => {
            return Err(Box::new(super::tool_error(
                "docs_unavailable",
                &format!(
                    "the features of {what} are not known: the project could not be read: {}",
                    ErrorChain(&error)
                ),
                false,
                &["read the [features] of its Cargo.toml"],
            )));
        },
        Joined::Going(doing) => {
            let target = match release.target() {
                Some(target) => target,
                None => state.host().await.triple.clone(),
            };
            return Err(Box::new(project::resolving_crate(
                &release.name,
                &target,
                (doing, found.project_from),
                &release.warnings,
            )));
        },
        Joined::Ended => return Err(Box::new(get_item::stopped())),
    };
    let Resolution::Metadata { metadata, .. } = &*resolution else {
        return Err(Box::new(super::tool_error(
            "docs_unavailable",
            &format!("the features of {what} are not known: cargo metadata of the project failed"),
            false,
            &["read the [features] of its Cargo.toml"],
        )));
    };
    let Some(package) = metadata.packages.iter().find(|package| {
        *package.name == *found.package.name
            && package.version == found.package.version
            && found
                .package
                .manifest_path
                .as_ref()
                .is_none_or(|manifest| *manifest == package.manifest_path)
    }) else {
        return Err(Box::new(super::tool_error(
            "docs_unavailable",
            &format!("the features of {what} are not in the project's cargo metadata"),
            false,
            &["read the [features] of its Cargo.toml"],
        )));
    };
    // A member that asks docs.rs for features is documented with them (requirement R-FT-3).
    if metadata.workspace_members.contains(&package.id) {
        let manifest = package.manifest_path.clone();
        let asks = crate::task::spawn_blocking(move || crate_file::member_docs_rs(&manifest))
            .await
            .ok()
            .and_then(Result::ok)
            .is_some_and(|build| build.asks_features);
        if asks {
            release.warnings.push(
                format!(
                    "get_item documents {what} with the features that its \
                     [package.metadata.docs.rs] asks for, which enabled_in_project does not tell"
                )
                .into(),
            );
        }
    }
    Ok(Known {
        tree: package.feature_tree(),
        not_in_docs: None,
    })
}

/// The project's workspace that has the release on crates.io that `entry` lists, at `release`'s
/// version, as `get_item` finds it for a version given (decided in M3d, decisions 45 and 50);
/// `None`, with a warning in `release`, when the project uses other versions of it or a copy that
/// is not the release, or cargo reads it past `until`.
#[cfg(feature = "local")]
async fn project_of_release(
    state: &ToolState,
    project: Option<&std::path::Path>,
    (entry, until): (&IndexEntry, Instant),
    release: &mut Release,
) -> Option<Workspace> {
    let Release {
        name,
        version,
        warnings,
        ..
    } = release;
    let used = match project::crates_io_in_project_or_doing(state, name, project, until).await {
        Ok(used) => used?,
        Err(doing) => {
            warnings.push(get_item::still_reading(doing).into());
            return None;
        },
    };
    let Some(package) = used
        .packages
        .iter()
        .find(|package| package.version == *version)
    else {
        let versions: Vec<String> = used
            .packages
            .iter()
            .map(|package| package.version.to_string())
            .collect();
        warnings.push(
            format!(
                "the project uses {name} {}, not {version}: whether its build enables each \
                 feature is not told",
                versions.join(", ")
            )
            .into(),
        );
        return None;
    };
    let check = package.docs_rs_check(entry);
    if check != DocsRsCheck::Allowed {
        warnings.push(
            format!(
                "the project's {name}@{version} is {}: whether its build enables each feature is \
                 not told",
                get_item::not_on_docs_rs(package, check)
            )
            .into(),
        );
        return None;
    }
    Some(used.workspace)
}

/// Whether the project's build of `workspace` enables the features of `release`, for its target
/// (requirement R-FT-2, decided in M3d, decisions 50 and 54): `None`, with a warning in `release`,
/// while cargo reads them past the call's deadline, or when they cannot be read.
#[cfg(feature = "local")]
async fn in_project(
    state: &ToolState,
    workspace: &Workspace,
    until: Instant,
    release: &mut Release,
) -> Option<ProjectBuild> {
    let target = release.target()?;
    let Release {
        name,
        version,
        warnings,
        ..
    } = release;
    let sets = match project::features_until(state, workspace, &target, until).await {
        Joined::Done(Ok(sets)) => sets,
        Joined::Done(Err(error)) => {
            warnings.push(
                format!(
                    "whether the project's build enables the features is not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            return None;
        },
        Joined::Ended => {
            warnings.push(get_item::NOT_READ.into());
            return None;
        },
        Joined::Going(doing) => {
            warnings.push(get_item::still_reading(doing).into());
            return None;
        },
    };
    let normal = sets.normal(name.as_str(), version).cloned();
    if normal.is_none() {
        release
            .warnings
            .push(format!("the project does not build {name} {version} for {target}").into());
    }
    Some(ProjectBuild {
        normal,
        with_dev: sets.with_dev(name.as_str(), version).cloned(),
    })
}

/// The features of `release`, a crates.io release, from its line of the index, with those that
/// docs.rs's build did not enable when docs.rs may have its documentation, and its index entry;
/// what failed of its `.crate` is forgotten first when `refresh` (§6.4); waiting until `until`.
async fn release_known(
    state: &ToolState,
    release: &mut Release,
    (refresh, until): (bool, Instant),
) -> Result<(Known, IndexEntry), Box<CallToolResult>> {
    let (tree, checksum, entry) = release_features(state, &release.name, &release.version).await?;
    // A `.crate` that failed to be fetched a moment ago is asked for again.
    if refresh {
        crate::data::forget_failure_of(
            &state.cache.crate_file_path(&release.name, &release.version),
        );
    }
    let not_in_docs = if matches!(release.docs_rs_json.as_ref(), "available" | "unknown") {
        let Release {
            name,
            version,
            warnings,
            ..
        } = release;
        not_in_docs(state, (name, version), (&checksum, until), warnings).await
    } else {
        None
    };
    Ok((Known { tree, not_in_docs }, entry))
}

/// The tool error for `what`, a crate of `of`, not a crates.io release, whose features are not
/// read yet (decided in M3d, D2).
fn not_a_release(what: &str, of: &str) -> CallToolResult {
    super::error_result(&json!({
        "code": "docs_unavailable",
        "reason": "build_unsupported",
        "message": format!(
            "{what} is of {of}: inspect_features reads the features of crates.io releases"
        ),
        "retryable": false,
        "suggestions": ["read the [features] of its Cargo.toml"],
    }))
}

/// The features of `name` at `version`, from its line of the crates.io index, read again if the
/// index as kept does not list it yet, and the sha256 of its `.crate` (decided in M3d, decision
/// 41); else the error: when the index could not be read again, that it may lack a version
/// published since (requirement R-CA-3).
async fn release_features(
    state: &ToolState,
    name: &CrateName,
    version: &Version,
) -> Result<(FeatureTree, Box<str>, IndexEntry), Box<CallToolResult>> {
    let listed = |entry: &IndexEntry| {
        let tree = entry.features(version)?;
        let checksum = entry
            .versions
            .iter()
            .find(|listed| listed.version == *version)?
            .checksum
            .clone();
        Some((tree, checksum))
    };
    let entry = match state.index.entry(name).await {
        Ok(entry) => entry,
        Err(error) => return Err(Box::new(get_item::index_error(&state.cache, name, &error))),
    };
    if let Some((tree, checksum)) = listed(&entry) {
        return Ok((tree, checksum, entry));
    }
    let what = format!("{name} {version}");
    // Listed, but its line cannot be read: reading the index again does not change it.
    if entry
        .versions
        .iter()
        .any(|listed| listed.version == *version)
    {
        return Err(Box::new(super::tool_error(
            "network_error",
            &format!("the line of {what} in the crates.io index cannot be read"),
            false,
            &[],
        )));
    }
    match state.index.entry_fresh(name).await {
        Ok(entry) if let Some((tree, checksum)) = listed(&entry) => Ok((tree, checksum, entry)),
        Ok(entry) if entry.stale.is_some() => {
            Err(Box::new(get_item::stale_index_error(state, &what, &entry)))
        },
        Ok(_) | Err(IndexError::NotFound) => Err(Box::new(super::tool_error(
            "version_not_found",
            &format!("the crates.io index does not list the features of {what}"),
            false,
            &["check the version with resolve_version"],
        ))),
        Err(error) => Err(Box::new(get_item::index_error(&state.cache, name, &error))),
    }
}

/// The features that docs.rs's build of `name` at `version` did not enable, as its `.crate`
/// tells, whose sha256 is `checksum` (requirement R-FT-1, decided in M3d, decision 46); `None`,
/// with a warning, when the `.crate` cannot be had or read by `until`, the call's deadline
/// (requirement R-ARCH-6): it may be large.
async fn not_in_docs(
    state: &ToolState,
    (name, version): (&CrateName, &Version),
    (checksum, until): (&str, Instant),
    warnings: &mut Vec<Box<str>>,
) -> Option<Box<[Box<str>]>> {
    let fetching = state
        .static_crates
        .crate_file(&state.cache, name, version, Some(checksum));
    let Ok(contents) = tokio::time::timeout_at(until, fetching).await else {
        warnings.push(
            format!(
                "which features docs.rs built {name} {version} without is not known yet: its \
                 .crate is being fetched; call again for it"
            )
            .into(),
        );
        return None;
    };
    // Unpacked until its manifest, which may be far into it: off the async threads.
    let read = match contents {
        Ok(Some(contents)) => {
            let (name, version) = (name.clone(), version.clone());
            crate::task::spawn_blocking(move || {
                crate_file::docs_rs_build(&contents, &name, &version).map(Some)
            })
            .await
            .unwrap_or_else(|error| {
                let error = crate::task::resume_if_panic(error);
                Err(crate_file::CrateError::Cache(std::io::Error::other(error)))
            })
        },
        other => other.map(|_| None),
    };
    match read {
        Ok(Some(build)) => Some(build.missing),
        Ok(None) => None,
        Err(error) => {
            warnings.push(
                format!(
                    "which features docs.rs built {name} {version} without is not known: {}",
                    ErrorChain(&error)
                )
                .into(),
            );
            None
        },
    }
}

/// What the answer tells of the features of a release.
struct Told<'t> {
    provenance: &'t Map<String, Value>,
    tree: &'t FeatureTree,
    /// Those that the documentation does not have, when known.
    not_in_docs: Option<&'t [Box<str>]>,
    /// Whether the project's build enables each.
    in_project: Option<&'t ProjectBuild>,
    /// The items of each.
    items: &'t Items,
    /// Whether docs.rs has the documentation.
    docs_rs_json: &'t str,
    warnings: &'t [Box<str>],
    hints: &'t [Value],
    /// When to call again.
    retry_after: Option<&'t Value>,
}

impl Told<'_> {
    /// How many items of `feature` the documentation tells, if it has them: not of one that it
    /// does not have (decided in M3d, decisions 46 and 57).
    fn count_of(&self, feature: &str) -> Option<usize> {
        let Items::Found {
            counts, built_with, ..
        } = self.items
        else {
            return None;
        };
        let lacking = self
            .not_in_docs
            .is_some_and(|missing| missing.iter().any(|name| name.as_ref() == feature))
            || built_with
                .as_ref()
                .is_some_and(|built_with| !built_with.contains(feature));
        (!lacking).then(|| counts.get(feature).copied().unwrap_or_default())
    }

    /// The items of `feature`, the one asked for, if the documentation has them.
    fn listed(&self, feature: &str) -> Option<&[Value]> {
        let Items::Found { listed, .. } = self.items else {
            return None;
        };
        self.count_of(feature).map(|_| listed.as_slice())
    }
}

/// The response for the features that `told` tells, as `ask` asks: a page of the features, or,
/// for one feature asked for, a page of its items (decided in M3d, decisions 38, 39, 40, 46 and
/// 56); within the budget of a response, with fewer than `limit` if need be (requirement R-OUT-3).
fn respond(told: &Told<'_>, ask: &FeatureAsk) -> Result<Value, Box<CallToolResult>> {
    let tree = told.tree;
    let names = names_asked(tree, ask.feature.as_deref())?;
    // One feature with the items that the documentation tells: the page is of them.
    let listed: Option<&[Value]> = ask
        .feature
        .as_deref()
        .and_then(|feature| told.listed(feature));
    let digest = match (listed, told.items) {
        (Some(_), Items::Found { digest, .. }) => digest_of_items(tree, digest),
        _ => digest_of(tree),
    };
    let paging = &ask.paging;
    let total = listed.map_or(names.len(), <[Value]>::len);
    check_cursor(paging, &digest, total)?;
    let default: BTreeSet<&str> = if tree.contains("default") {
        tree.closure(["default"])
    } else {
        BTreeSet::new()
    };
    let enabled_by = tree.enabled_by();
    let not_in_docs: BTreeSet<&str> = told
        .not_in_docs
        .into_iter()
        .flatten()
        .map(AsRef::as_ref)
        .collect();
    let feature = |name: &str| {
        feature_of(
            tree,
            name,
            (&default, &enabled_by, &not_in_docs),
            (told.in_project, told.count_of(name)),
        )
    };
    let page = |list: &mut dyn Iterator<Item = Value>| -> Vec<Value> {
        list.skip(paging.offset).take(paging.limit).collect()
    };
    let (features, mut paged): (Vec<Value>, Vec<Value>) = listed.map_or_else(
        || {
            (
                Vec::new(),
                page(&mut names.iter().map(|&name| feature(name))),
            )
        },
        |items| {
            (
                names.iter().map(|&name| feature(name)).collect(),
                page(&mut items.iter().cloned()),
            )
        },
    );
    let data = data_of(told, &default);
    let mut hints = told.hints.to_vec();
    // A feature that gathers others has their items, not its own (decided in M3d, decision 58).
    let gathering = |feature: &Value| {
        feature.get("items") == Some(&json!(0)) && feature.get("closure").is_some()
    };
    if features.iter().chain(&paged).any(gathering) {
        hints.push(json!(
            "a feature with items: 0 and a closure enables the items of the features of its \
             closure, which are counted there"
        ));
    }
    let answer = |paged: &[Value]| {
        let page = page_of(paging, (&digest, total), paged.len());
        let mut data = data.clone();
        if listed.is_some() {
            data.insert("features".to_owned(), json!(features));
            data.insert("items".to_owned(), json!(paged));
        } else {
            data.insert("features".to_owned(), json!(paged));
        }
        let mut answer = Map::new();
        answer.insert(
            "provenance".to_owned(),
            Value::Object(told.provenance.clone()),
        );
        if let Some(after) = told.retry_after {
            answer.insert("retry_after_secs".to_owned(), after.clone());
        }
        answer.insert("data".to_owned(), Value::Object(data));
        answer.insert("page".to_owned(), page);
        if !hints.is_empty() {
            answer.insert("hints".to_owned(), json!(hints));
        }
        let mut answer = Value::Object(answer);
        get_item::add_warnings(&mut answer, told.warnings);
        answer
    };
    // Fewer while the response is over its budget, one at least; the rest on the next page.
    loop {
        let response = answer(&paged);
        let size = response.to_string().chars().count();
        if size <= MAX_RESPONSE_CHARS || paged.len() <= 1 {
            return Ok(response);
        }
        // In proportion to what is over, one less at least.
        let fits = paged
            .len()
            .saturating_mul(MAX_RESPONSE_CHARS)
            .checked_div(size)
            .unwrap_or_default();
        paged.truncate(fits.clamp(1, paged.len().saturating_sub(1)));
    }
}

/// Checks the cursor of `paging` against the list of `total` whose digest is `digest`: it was made
/// for the same, and does not go past its end.
fn check_cursor(paging: &Paging, digest: &str, total: usize) -> Result<(), Box<CallToolResult>> {
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
        return refused(
            "the features or their documentation changed since the cursor was made; list again \
             without the cursor",
        );
    }
    if paging.offset > total {
        return refused("the cursor goes past the end of the list; list again without the cursor");
    }
    Ok(())
}

/// What `data` has besides the list that `told` tells: the features that `default` enables,
/// whether docs.rs has the documentation, and that the items are being had.
fn data_of(told: &Told<'_>, default: &BTreeSet<&str>) -> Map<String, Value> {
    let mut data = Map::new();
    let default_list: Vec<&str> = default
        .iter()
        .copied()
        .filter(|&feature| feature != "default")
        .collect();
    insert_listed(&mut data, "default", &default_list);
    if !told.docs_rs_json.is_empty() {
        data.insert("docs_rs_json".to_owned(), json!(told.docs_rs_json));
    }
    if let Items::Pending(doing) = told.items {
        data.insert("items_pending".to_owned(), json!(doing));
    }
    data
}

/// The page of `shown` features after those that `paging` skips, of `total` in the list whose
/// digest is `digest`, with the cursor of the next page if any.
fn page_of(paging: &Paging, (digest, total): (&str, usize), shown: usize) -> Value {
    let next = paging.offset.saturating_add(shown);
    let mut page = Map::new();
    page.insert("shown".to_owned(), json!(shown));
    page.insert("total".to_owned(), json!(total));
    page.insert("has_more".to_owned(), json!(next < total));
    if next < total {
        page.insert(
            "next_cursor".to_owned(),
            json!(format!("{}.{digest}.{next}", paging.request_digest)),
        );
    }
    Value::Object(page)
}

/// The names of the features of `tree` told: `feature` alone if one is asked for, which the
/// crate must have, or else all. One that it does not have is told with those of a name like it
/// (requirement R-PR-7).
fn names_asked<'t>(
    tree: &'t FeatureTree,
    feature: Option<&'t str>,
) -> Result<Vec<&'t str>, Box<CallToolResult>> {
    let Some(feature) = feature else {
        return Ok(tree.names().collect());
    };
    if tree.contains(feature) {
        return Ok(vec![feature]);
    }
    let wanted = feature.to_lowercase();
    let mut near: Vec<(usize, &str)> = tree
        .names()
        .filter_map(|name| {
            let distance = edit_distance(&wanted, &name.to_lowercase());
            let longer = wanted.chars().count().max(name.chars().count());
            // At least 70% alike, and at most 3 edits, as `search_items` suggests names.
            (distance <= 3 && distance.saturating_mul(10) <= longer.saturating_mul(3))
                .then_some((distance, name))
        })
        .collect();
    near.sort_unstable();
    let suggestion = if near.is_empty() {
        "list the features without `feature`".to_owned()
    } else {
        format!(
            "features with a name like it: {}",
            near.iter()
                .take(5)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Err(Box::new(super::tool_error(
        "invalid_argument",
        &format!("the crate has no feature `{feature}`"),
        false,
        &[&suggestion],
    )))
}

/// The feature `name` of `tree` as told: its values, the crate's features that it enables, what
/// it enables of dependencies, and what enables it, as `enabled_by` has it, each list of at most
/// [`MAX_LISTED`] names with its total when cut; whether `default` enables it, an optional
/// dependency makes it, `not_in_docs` has it, and the project's build, `in_project`, or only its
/// build with the dev-dependencies, enables it; and how many `items` its documentation tells.
fn feature_of(
    tree: &FeatureTree,
    name: &str,
    (default, enabled_by, not_in_docs): (
        &BTreeSet<&str>,
        &BTreeMap<&str, BTreeSet<&str>>,
        &BTreeSet<&str>,
    ),
    (in_project, items): (Option<&ProjectBuild>, Option<usize>),
) -> Value {
    let closure = tree.closure([name]);
    let mut feature = Map::new();
    feature.insert("name".to_owned(), json!(name));
    let enables: Vec<&str> = tree
        .values(name)
        .unwrap_or_default()
        .iter()
        .map(AsRef::as_ref)
        .collect();
    insert_listed(&mut feature, "enables", &enables);
    let lists: [(&str, Vec<&str>); 3] = [
        (
            "closure",
            closure
                .iter()
                .copied()
                .filter(|&other| other != name)
                .collect(),
        ),
        (
            "dependencies",
            tree.dependencies(&closure).into_iter().collect(),
        ),
        (
            "enabled_by",
            enabled_by
                .get(name)
                .map(|by| by.iter().copied().collect())
                .unwrap_or_default(),
        ),
    ];
    for (key, list) in lists {
        if !list.is_empty() {
            insert_listed(&mut feature, key, &list);
        }
    }
    for (key, value, told) in [
        ("default", true, default.contains(name)),
        ("implicit", true, tree.is_implicit(name)),
        ("in_docs", false, not_in_docs.contains(name)),
    ] {
        if told {
            feature.insert(key.to_owned(), json!(value));
        }
    }
    if let Some(items) = items {
        feature.insert("items".to_owned(), json!(items));
    }
    if let Some(in_project) = in_project {
        let has =
            |set: &Option<BTreeSet<Box<str>>>| set.as_ref().is_some_and(|set| set.contains(name));
        let in_build = has(&in_project.normal);
        feature.insert("enabled_in_project".to_owned(), json!(in_build));
        if !in_build && has(&in_project.with_dev) {
            feature.insert("enabled_in_dev_only".to_owned(), json!(true));
        }
    }
    Value::Object(feature)
}

/// Inserts `list` at `key` of `object`, at most [`MAX_LISTED`] of it, with its total at
/// `<key>_total` when it is cut.
fn insert_listed(object: &mut Map<String, Value>, key: &str, list: &[&str]) {
    object.insert(
        key.to_owned(),
        json!(list.iter().take(MAX_LISTED).collect::<Vec<_>>()),
    );
    if list.len() > MAX_LISTED {
        object.insert(format!("{key}_total"), json!(list.len()));
    }
}

/// The digest of the items of a feature of `tree` that a cursor is made for, in the documentation
/// whose digest is `documentation`: the list goes on only over the same.
fn digest_of_items(tree: &FeatureTree, documentation: &str) -> String {
    sha256_hex(format!("{}\n{documentation}", digest_of(tree)).as_bytes())
        .get(..16)
        .unwrap_or_default()
        .to_owned()
}

/// The digest of the features `tree` that a cursor is made for: the list goes on only over the
/// same features.
fn digest_of(tree: &FeatureTree) -> String {
    let mut text = String::new();
    for name in tree.names() {
        text.push_str(name);
        for value in tree.values(name).unwrap_or_default() {
            text.push(' ');
            text.push_str(value);
        }
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
        .get(..16)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_fits_the_limit() {
        let len = super::super::for_this_build(DESCRIPTION).chars().count();
        assert!(len <= 2048, "{len} characters");
    }

    fn tree() -> FeatureTree {
        FeatureTree::new(
            [
                ("default", vec!["rt"]),
                ("full", vec!["net", "rt", "hyper?/http1"]),
                ("net", vec!["mio/net"]),
                ("rt", vec![]),
            ],
            ["mio", "hyper"],
        )
    }

    /// demo 1.0.0 of crates.io, as `resolve_version` tells it.
    fn release() -> Release {
        Release {
            provenance: Map::new(),
            warnings: Vec::new(),
            hints: Vec::new(),
            name: "demo".parse().expect("a name"),
            version: Version::new(1, 0, 0),
            source: "crates.io".into(),
            on_crates_io: true,
            docs_rs_json: "available".into(),
            retry_after: None,
        }
    }

    fn asked(arguments: Value) -> FeatureAsk {
        let Value::Object(arguments) = arguments else {
            panic!("an object");
        };
        parse_arguments(Some(&arguments)).expect("valid").ask
    }

    /// The response for the features of `tree`, of which `not_in_docs` the documentation does not
    /// have, and whose items `items` tells, as `ask` asks.
    fn respond_with(
        (tree, not_in_docs, in_project): (&FeatureTree, Option<&[Box<str>]>, Option<&ProjectBuild>),
        ask: &FeatureAsk,
        (docs_rs_json, items): (&str, &Items),
    ) -> Result<Value, Box<CallToolResult>> {
        respond(
            &Told {
                provenance: &Map::new(),
                tree,
                not_in_docs,
                in_project,
                items,
                docs_rs_json,
                warnings: &[],
                hints: &[],
                retry_after: None,
            },
            ask,
        )
    }

    /// [`respond_with`] without the items.
    fn respond_to(
        told: (&FeatureTree, Option<&[Box<str>]>, Option<&ProjectBuild>),
        ask: &FeatureAsk,
        docs_rs_json: &str,
    ) -> Result<Value, Box<CallToolResult>> {
        respond_with(told, ask, (docs_rs_json, &Items::Unknown))
    }

    /// Each feature is told with its values, the crate's features that it enables, what it
    /// enables of dependencies and what enables it; whether `default` enables it, an optional
    /// dependency makes it, and whether docs.rs's build had it (decided in M3d, decisions 38,
    /// 40 and 46).
    #[test]
    fn each_feature_tells_what_it_enables_and_what_enables_it() {
        let ask = asked(json!({ "crate": "demo" }));
        let answer = respond_to(
            (&tree(), Some(&[Box::from("net")]), None),
            &ask,
            "available",
        )
        .expect("answered");
        let data = &answer["data"];
        assert_eq!(data["default"], json!(["rt"]));
        assert_eq!(data["docs_rs_json"], "available");
        let features = data["features"].as_array().expect("features");
        let names: Vec<&str> = features
            .iter()
            .filter_map(|feature| feature["name"].as_str())
            .collect();
        assert_eq!(names, ["default", "full", "hyper", "mio", "net", "rt"]);
        assert_eq!(
            features[1],
            json!({
                "name": "full",
                "enables": ["net", "rt", "hyper?/http1"],
                "closure": ["mio", "net", "rt"],
                "dependencies": ["dep:mio", "hyper?/http1", "mio/net"],
            })
        );
        let net = &features[4];
        assert_eq!(net["enabled_by"], json!(["full"]));
        assert_eq!(net["in_docs"], false);
        assert_eq!(features[3]["implicit"], true);
        assert_eq!(features[5]["default"], true);
        assert_eq!(features[5]["enabled_by"], json!(["default", "full"]));
        assert_eq!(
            answer["page"],
            json!({ "shown": 6, "total": 6, "has_more": false })
        );
    }

    /// The features are paged with a cursor made for them, and one is asked for alone; a feature
    /// that the crate does not have is told with those named like it (decided in M3d, decision
    /// 39, requirement R-PR-7).
    #[test]
    fn the_features_are_paged_and_one_is_asked_for_alone() {
        let first = respond_to(
            (&tree(), None, None),
            &asked(json!({ "crate": "demo", "limit": 4 })),
            "",
        )
        .expect("answered");
        let cursor = first["page"]["next_cursor"].as_str().expect("a cursor");
        let rest = respond_to(
            (&tree(), None, None),
            &asked(json!({ "crate": "demo", "limit": 4, "cursor": cursor })),
            "",
        )
        .expect("answered");
        assert_eq!(
            rest["page"],
            json!({ "shown": 2, "total": 6, "has_more": false })
        );
        assert_eq!(rest["data"]["features"][0]["name"], "net");
        // Of other features, the cursor is refused.
        let other = FeatureTree::new([("x", vec![])], []);
        let refused = respond_to(
            (&other, None, None),
            &asked(json!({ "crate": "demo", "limit": 4, "cursor": cursor })),
            "",
        )
        .expect_err("refused");
        assert_eq!(refused.is_error, Some(true));
        let one = respond_to(
            (&tree(), None, None),
            &asked(json!({ "crate": "demo", "feature": "net" })),
            "",
        )
        .expect("answered");
        assert_eq!(one["data"]["features"].as_array().map(Vec::len), Some(1));
        for (asked_for, near) in [("Full", true), ("fulll", true), ("c", false)] {
            let missing = respond_to(
                (&tree(), None, None),
                &asked(json!({ "crate": "demo", "feature": asked_for })),
                "",
            )
            .expect_err("not a feature");
            let text = format!("{:?}", missing.content);
            assert!(text.contains("no feature"), "{text}");
            assert_eq!(text.contains("like it: full"), near, "{asked_for}: {text}");
        }
    }

    /// A response is kept within its budget: a long list of a feature is cut, with its total,
    /// and the page has fewer features, the rest after its cursor (requirement R-OUT-3).
    #[test]
    fn a_large_response_is_kept_within_its_budget() {
        let names: Vec<String> = (0..3_000)
            .map(|n| format!("Win32_Feature_{n:04}"))
            .collect();
        // Each enables a few dependencies too, so that a page of them is over the budget.
        let dependencies: Vec<String> = (0..8)
            .map(|n| format!("dep:a_dependency_with_a_long_name_{n}"))
            .collect();
        let values: Vec<&str> = std::iter::once("Win32")
            .chain(dependencies.iter().map(String::as_str))
            .collect();
        let tree = FeatureTree::new(
            names
                .iter()
                .map(|name| (name.as_str(), values.clone()))
                .chain([("Win32", vec![])]),
            [],
        );
        let answer = respond_to(
            (&tree, None, None),
            &asked(json!({ "crate": "windows", "limit": 200 })),
            "",
        )
        .expect("answered");
        assert!(answer.to_string().chars().count() <= MAX_RESPONSE_CHARS);
        let shown = answer["page"]["shown"].as_u64().expect("a number");
        assert!((1..200).contains(&shown), "{shown}");
        assert_eq!(answer["page"]["has_more"], true);
        let win32 = respond_to(
            (&tree, None, None),
            &asked(json!({ "crate": "windows", "feature": "Win32" })),
            "",
        )
        .expect("answered");
        let feature = &win32["data"]["features"][0];
        assert_eq!(
            feature["enabled_by"].as_array().map(Vec::len),
            Some(MAX_LISTED)
        );
        assert_eq!(feature["enabled_by_total"], 3_000);
    }

    /// Whether the project's build enables each feature, or only its build with the
    /// dev-dependencies, or none when it does not build the package for the target; those that
    /// it enables but the documentation does not have are warned about, and how to read them
    /// told (decided in M3d, decisions 45, 51, 52 and 54).
    #[test]
    fn whether_the_project_enables_each_feature_is_told() {
        let release = release();
        let known = Known {
            tree: tree(),
            not_in_docs: Some(Box::from([Box::from("net"), Box::from("full")])),
        };
        let set = |names: &[&str]| Some(names.iter().map(|&name| Box::from(name)).collect());
        let in_project = ProjectBuild {
            normal: set(&["default", "rt", "net", "mio"]),
            with_dev: set(&["default", "rt", "net", "mio", "full"]),
        };
        let ask = asked(json!({ "crate": "demo" }));
        let answered = |in_project: &ProjectBuild| {
            respond_to(
                (&known.tree, known.not_in_docs.as_deref(), Some(in_project)),
                &ask,
                &release.docs_rs_json,
            )
            .expect("answered")
        };
        let told = answered(&in_project);
        let features = told["data"]["features"].as_array().expect("features");
        let of = |name: &str| {
            features
                .iter()
                .find(|feature| feature["name"] == name)
                .expect("a feature")
        };
        assert_eq!(of("net")["enabled_in_project"], true);
        assert_eq!(of("hyper")["enabled_in_project"], false);
        assert_eq!(of("full")["enabled_in_project"], false);
        assert_eq!(of("full")["enabled_in_dev_only"], true);
        assert!(of("rt").get("enabled_in_dev_only").is_none());
        let (warnings, hints) = notes(
            &release,
            (&known.tree, known.not_in_docs.as_deref(), Some(&in_project)),
            &Items::Unknown,
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("the project enables net, whose items")),
            "{warnings:?}"
        );
        assert!(
            json!(hints)
                .to_string()
                .contains("read their source with get_source")
        );
        // Not built for the target: none is enabled.
        let none = ProjectBuild {
            normal: None,
            with_dev: None,
        };
        let told = answered(&none);
        assert!(
            told["data"]["features"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|feature| feature["enabled_in_project"] == false),
            "{told}"
        );
    }

    /// The items of each feature that the documentation tells: their number by each feature, not
    /// of one that a build here was without; one feature's, entries first, paged with a cursor
    /// made for that documentation; a feature that gathers others told so; and, while the
    /// documentation is had, none, but that they are coming (decided in M3d, decisions 42, 56, 57
    /// and 58).
    #[test]
    fn the_items_of_each_feature_are_told() {
        let item =
            |path: &str, entry: bool| json!({ "path": path, "kind": "function", "entry": entry });
        let found = |built_with: Option<&[&str]>| Items::Found {
            counts: [(Box::from("net"), 2), (Box::from("rt"), 1)]
                .into_iter()
                .collect(),
            listed: vec![item("a::net", true), item("a::net::x", false)],
            digest: "ab".into(),
            lacking: None,
            built_with: built_with.map(|names| names.iter().map(|&name| Box::from(name)).collect()),
        };
        let all = asked(json!({ "crate": "demo" }));
        let told = respond_with((&tree(), None, None), &all, ("", &found(None))).expect("answered");
        let count = |told: &Value, name: &str| {
            told["data"]["features"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|feature| feature["name"] == name)
                .and_then(|feature| feature.get("items").cloned())
        };
        assert_eq!(count(&told, "net"), Some(json!(2)));
        assert_eq!(count(&told, "full"), Some(json!(0)));
        assert!(
            told["hints"].to_string().contains("items: 0 and a closure"),
            "{told}"
        );
        // Built here without `full`: its items are not counted.
        let built = found(Some(&["default", "rt", "net", "mio"]));
        let told = respond_with((&tree(), None, None), &all, ("", &built)).expect("answered");
        assert_eq!(count(&told, "full"), None);
        assert_eq!(count(&told, "rt"), Some(json!(1)));
        // One feature: a page of its items.
        let found = found(None);
        let net = asked(json!({ "crate": "demo", "feature": "net", "limit": 1 }));
        let first = respond_with((&tree(), None, None), &net, ("", &found)).expect("answered");
        assert_eq!(first["data"]["items"], json!([item("a::net", true)]));
        assert_eq!(first["data"]["features"][0]["items"], 2);
        let cursor = first["page"]["next_cursor"].as_str().expect("a cursor");
        let second =
            asked(json!({ "crate": "demo", "feature": "net", "limit": 1, "cursor": cursor }));
        let rest = respond_with((&tree(), None, None), &second, ("", &found)).expect("answered");
        assert_eq!(rest["data"]["items"], json!([item("a::net::x", false)]));
        assert_eq!(rest["page"]["has_more"], false);
        // The cursor is of that documentation.
        let other = Items::Found {
            counts: BTreeMap::new(),
            listed: Vec::new(),
            digest: "cd".into(),
            lacking: None,
            built_with: None,
        };
        let refused =
            respond_with((&tree(), None, None), &second, ("", &other)).expect_err("refused");
        assert_eq!(refused.is_error, Some(true));
        // While the documentation is fetched.
        let pending = Items::Pending("fetching".into());
        let told = respond_with((&tree(), None, None), &all, ("", &pending)).expect("answered");
        assert_eq!(told["data"]["items_pending"], "fetching");
        assert_eq!(count(&told, "net"), None);
    }

    /// The features that the documentation lacks are those of its `.crate`, or else those that
    /// `get_item` tells of docs.rs's build; a build here without features is warned of without
    /// those, its list cut; and what is done of the documentation is told as it is.
    #[test]
    fn what_the_documentation_lacks_is_told() {
        let found = |lacking: Option<&[&str]>, built_with: Option<&[&str]>| Items::Found {
            counts: BTreeMap::new(),
            listed: Vec::new(),
            digest: "ab".into(),
            lacking: lacking.map(|names| names.iter().map(|&name| Box::from(name)).collect()),
            built_with: built_with.map(|names| names.iter().map(|&name| Box::from(name)).collect()),
        };
        let unknown = Known {
            tree: tree(),
            not_in_docs: None,
        };
        let from_get_item = found(Some(&["net"]), None);
        assert_eq!(
            lacking_of(&unknown, &from_get_item),
            Some(&[Box::from("net")][..])
        );
        let known = Known {
            tree: tree(),
            not_in_docs: Some(Box::from([Box::from("full")])),
        };
        assert_eq!(
            lacking_of(&known, &from_get_item),
            Some(&[Box::from("full")][..])
        );
        assert_eq!(lacking_of(&unknown, &Items::Unknown), None);
        // Built here without `full` and `net`, of which `full` is told as not in the docs.
        let built = found(None, Some(&["default", "rt", "mio", "hyper"]));
        let (warnings, _) = notes(
            &release(),
            (&known.tree, known.not_in_docs.as_deref(), None),
            &built,
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("built without net, whose items")),
            "{warnings:?}"
        );
        // Of many features, with the one that `mio` makes, the list is cut.
        let names: Vec<String> = (0..MAX_LISTED + 3).map(|n| format!("f{n}")).collect();
        let many = FeatureTree::new(names.iter().map(|name| (name.as_str(), vec![])), ["mio"]);
        let (warnings, _) = notes(&release(), (&many, None, None), &found(None, Some(&[])));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains(" and 4 more")),
            "{warnings:?}"
        );
        for (doing, told) in [
            ("indexing", "being indexed"),
            ("waiting_for_slot", "waiting for a build"),
        ] {
            let (_, hints) = notes(
                &release(),
                (&known.tree, None, None),
                &Items::Pending(doing.into()),
            );
            assert!(json!(hints).to_string().contains(told), "{hints:?}");
        }
    }

    /// std has no features to enable; `feature` is a name, and `limit` within its bounds.
    #[test]
    fn std_and_odd_arguments_are_refused() {
        for (arguments, part) in [
            (json!({ "crate": "std" }), "no features"),
            (json!({ "crate": "tokio", "feature": "" }), "`feature`"),
            (json!({ "crate": "tokio", "limit": 0 }), "`limit`"),
            (json!({ "crate": "tokio", "limit": 201 }), "`limit`"),
        ] {
            let Value::Object(arguments) = arguments else {
                panic!("an object");
            };
            let error = parse_arguments(Some(&arguments)).expect_err("invalid");
            assert!(error.message.contains(part), "{error:?}");
        }
    }
}
