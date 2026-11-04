//! The `search_crates` tool: crates on crates.io that a query finds (requirement R-SR-6, §3 Q16,
//! decided in M3f, decisions 100 to 103).
//!
//! The search is crates.io's, in its order or another that `sort` picks; each crate comes with
//! what the search tells of it, without asking more of each (decision 101). The query is sent to
//! crates.io, and kept in the cache only as a hash (decision 103).

use std::sync::Arc;

use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use super::{ErrorChain, InvalidArgument, ToolState, get_item};
use crate::{
    data::{
        cache::sha256_hex,
        crates_io::{ApiError, Found, Next, Sort},
        rfc3339,
    },
    query::get_item::MAX_RESPONSE_CHARS,
};

/// The name of the tool.
pub(super) const NAME: &str = "search_crates";

/// The description for `tools/list` (requirement R-MCP-6).
const DESCRIPTION: &str = include_str!("search_crates.md");

/// The crates in a page unless `limit` says otherwise (decision 101).
const DEFAULT_LIMIT: u32 = 10;

/// The most crates in a page (decision 101).
const MAX_LIMIT: u32 = 50;

/// The longest query taken, in characters.
const MAX_QUERY: usize = 200;

/// The longest description told, in characters (as `crate_metadata`'s, decision 108).
const MAX_DESCRIPTION: usize = 1_000;

/// The longest repository URL told, in characters (decision 108).
const MAX_URL: usize = 500;

/// The longest version told, in characters: semver has no limit.
const MAX_VERSION: usize = 100;

/// The longest description told when the answer is over its budget (requirement R-OUT-3).
const SHORT_DESCRIPTION: usize = 200;

/// What cursors of this tool start with: their form.
const CURSOR_FORM: &str = "s1";

/// How to pass `cursor`, for `invalid_argument`.
const CURSOR_EXAMPLE: &[&str] =
    &["pass the next_cursor of the previous page as it is, with the same other arguments"];

/// The arguments checked.
#[derive(Debug)]
pub(super) struct Search {
    query: Box<str>,
    sort: Sort,
    limit: u32,
    /// The digest of what was asked, which the cursor carries.
    digest: Box<str>,
    /// Where the page starts, from the cursor.
    next: Option<Next>,
}

/// Definition of the tool for `tools/list`.
pub(super) fn definition() -> Tool {
    let input_schema = object(json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Words of what the crate does (json parser), or its name.",
            },
            "sort": {
                "type": "string",
                "enum": ["relevance", "downloads", "recent-downloads", "recent-updates", "new"],
                "description": "The order: relevance (default) as crates.io ranks them, \
                                downloads for the most used, recent-downloads for those of the \
                                last 90 days, recent-updates, or new.",
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_LIMIT,
                "description": "The most crates in a page (default 10).",
            },
            "cursor": {
                "type": "string",
                "description": "The next_cursor of the previous page.",
            },
        },
        "required": ["query"],
    }));
    Tool::new(NAME, super::for_this_build(DESCRIPTION), input_schema)
        .with_title("Search crates.io for Rust crates")
        .with_raw_output_schema(Arc::new(output_schema()))
        .with_annotations(annotations())
}

/// The annotations: it reads crates.io and runs nothing, so it is read-only in every build
/// (requirement R-MCP-4).
fn annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(true)
}

/// The schema of the answer.
fn output_schema() -> JsonObject {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let text = json!({ "type": "string" });
    let count = json!({ "type": "integer" });
    object(json!({
        "type": "object",
        "properties": {
            "provenance": {
                "type": "object",
                "properties": {
                    "source": { "type": "string", "enum": ["crates.io"] },
                    "fetched_at": {
                        "type": "string",
                        "description": "When crates.io answered (RFC 3339).",
                    },
                    "sort": text,
                },
                // Only where it came from: a search has no crate, version or target
                // (decision 102).
                "required": ["source"],
            },
            "warnings": strings,
            "data": {
                "type": "object",
                "properties": {
                    "crates": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": text,
                                "description": text,
                                "latest_stable": text,
                                "latest": text,
                                "downloads": count,
                                "recent_downloads": count,
                                "repository": text,
                                "updated_at": text,
                                "exact_match": { "type": "boolean" },
                            },
                            "required": ["name", "latest", "downloads"],
                        },
                    },
                },
                "required": ["crates"],
            },
            "page": {
                "type": "object",
                "properties": {
                    "shown": count,
                    "total": count,
                    "has_more": { "type": "boolean" },
                    "next_cursor": text,
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
pub(super) fn parse_arguments(arguments: Option<&JsonObject>) -> Result<Search, InvalidArgument> {
    let argument = |name| arguments.and_then(|arguments| arguments.get(name));
    let query = match argument("query") {
        Some(Value::String(query))
            if !query.trim().is_empty() && query.trim().chars().count() <= MAX_QUERY =>
        {
            Box::from(query.trim())
        },
        _ => {
            return Err(InvalidArgument::new(
                format!(
                    "`query` must be words of what the crate does, at most {MAX_QUERY} characters"
                ),
                &[r#"pass words, for example {"query": "json parser"}"#],
            ));
        },
    };
    let sort = match argument("sort").filter(|value| !value.is_null()) {
        None => Sort::Relevance,
        Some(value) => serde_json::from_value(value.clone()).ok().ok_or_else(|| {
            InvalidArgument::new(
                "`sort` must be relevance, downloads, recent-downloads, recent-updates or new",
                &[r#"pass one of them, for example {"sort": "downloads"}"#],
            )
        })?,
    };
    let limit = match argument("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| (1..=MAX_LIMIT).contains(value))
            .ok_or_else(|| {
                InvalidArgument::new(
                    format!("`limit` must be a whole number from 1 to {MAX_LIMIT}"),
                    &[r#"pass a whole number, for example {"limit": 10}"#],
                )
            })?,
    };
    let asked = json!([&*query, sort.as_str(), limit]).to_string();
    let digest: Box<str> = sha256_hex(asked.as_bytes())
        .get(..16)
        .unwrap_or_default()
        .into();
    let next = match argument("cursor").filter(|value| !value.is_null()) {
        None => None,
        Some(Value::String(cursor)) => Some(parse_cursor(cursor, &digest)?),
        Some(_) => {
            return Err(InvalidArgument::new(
                "`cursor` must be a string",
                CURSOR_EXAMPLE,
            ));
        },
    };
    Ok(Search {
        query,
        sort,
        limit,
        digest,
        next,
    })
}

/// Where the page that `cursor` asks for starts, if it was made for the search of `digest`.
fn parse_cursor(cursor: &str, digest: &str) -> Result<Next, InvalidArgument> {
    let refused = |message: &'static str| InvalidArgument::new(message, CURSOR_EXAMPLE);
    let mut parts = cursor.trim().splitn(3, '.');
    let (Some(CURSOR_FORM), Some(made_for), Some(next)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Err(refused("`cursor` is not a next_cursor of search_crates"));
    };
    if made_for != digest {
        return Err(refused(
            "the cursor was made for another search; pass it with the same query, sort and limit",
        ));
    }
    let seek_like = |seek: &str| {
        !seek.is_empty()
            && seek.len() <= 512
            && seek.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'=' | b'+' | b'/')
            })
    };
    match next.split_at_checked(1) {
        Some(("p", page))
            if page.bytes().all(|byte| byte.is_ascii_digit()) && !page.starts_with('0') =>
        {
            page.parse()
                .ok()
                .filter(|page| *page > 1)
                .map(Next::Page)
                .ok_or_else(|| refused("`cursor` is not a next_cursor of search_crates"))
        },
        Some(("s", seek)) if seek_like(seek) => Ok(Next::Seek(seek.into())),
        _ => Err(refused("`cursor` is not a next_cursor of search_crates")),
    }
}

/// The cursor of the page that `next` starts, of the search of `digest`.
fn cursor_of(digest: &str, next: &Next) -> String {
    match next {
        Next::Page(page) => format!("{CURSOR_FORM}.{digest}.p{page}"),
        Next::Seek(seek) => format!("{CURSOR_FORM}.{digest}.s{seek}"),
    }
}

/// Searches crates.io, and tells the page.
pub(super) async fn execute(search: Search, state: Arc<ToolState>) -> CallToolResult {
    let until = state.until();
    let page = match state
        .crates_io
        .search(
            &search.query,
            search.sort,
            search.limit,
            search.next.as_ref(),
            until,
        )
        .await
    {
        Ok(page) => page,
        Err(error) => return failed(&error, search.next.is_some()),
    };
    let mut warnings: Vec<Box<str>> = page.stale.iter().cloned().collect();
    let mut cut = false;
    let crates: Vec<Value> = page
        .value
        .crates
        .iter()
        .map(|found| crate_value(found, &mut cut))
        .collect();
    if cut {
        warnings.push(
            format!(
                "descriptions longer than {MAX_DESCRIPTION} characters, and repositories longer \
                 than {MAX_URL}, are cut"
            )
            .into(),
        );
    }
    let mut provenance = Map::new();
    provenance.insert("source".to_owned(), json!("crates.io"));
    provenance.insert("fetched_at".to_owned(), json!(rfc3339(page.fetched_at)));
    provenance.insert("sort".to_owned(), json!(search.sort.as_str()));
    let mut told = Map::new();
    told.insert("shown".to_owned(), json!(crates.len()));
    told.insert("total".to_owned(), json!(page.value.total));
    let next = page.value.next.as_ref().filter(|_| !crates.is_empty());
    told.insert("has_more".to_owned(), json!(next.is_some()));
    if let Some(next) = next {
        told.insert(
            "next_cursor".to_owned(),
            json!(cursor_of(&search.digest, next)),
        );
    }
    let mut hints = Vec::new();
    if crates.is_empty() && search.next.is_none() {
        hints.push(json!(
            "no crate matched: try fewer words, or other words for what it does"
        ));
    } else if crates.is_empty() {
        hints.push(json!("this is past the last crate found"));
    } else {
        hints.push(json!(
            "crate_metadata tells a crate's MSRV, license, yanked versions, dependents and how \
             long ago it was last released"
        ));
        if search.sort == Sort::Relevance {
            hints.push(json!(
                "for the crates most used for this, search again with sort: downloads"
            ));
        }
    }
    let answer = |crates: &[Value], warnings: &[Box<str>]| {
        let mut answer = json!({
            "provenance": provenance,
            "data": { "crates": crates },
            "page": told,
            "hints": hints,
        });
        get_item::add_warnings(&mut answer, warnings);
        answer
    };
    let response = answer(&crates, &warnings);
    if response.to_string().chars().count() <= MAX_RESPONSE_CHARS {
        return CallToolResult::structured(response);
    }
    // Over the budget (requirement R-OUT-3): the descriptions shorter, as a page of crates.io
    // cannot be cut and gone on with.
    let shorter: Vec<Value> = crates
        .into_iter()
        .map(|mut found| {
            if let Some(description) = found.get_mut("description")
                && let Some(text) = description.as_str()
            {
                *description = json!(shortened(text, SHORT_DESCRIPTION).0);
            }
            found
        })
        .collect();
    warnings.push(
        format!(
            "descriptions are cut to {SHORT_DESCRIPTION} characters, as the answer is long: \
             crate_metadata tells one whole, or pass a smaller limit"
        )
        .into(),
    );
    CallToolResult::structured(answer(&shorter, &warnings))
}

/// A crate that the search found, as told; `cut` is set when a string is cut.
fn crate_value(found: &Found, cut: &mut bool) -> Value {
    let mut told = Map::new();
    told.insert("name".to_owned(), json!(found.name));
    let mut put = |key: &str, value: Option<&str>, max: usize| {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            return;
        };
        let (value, was_cut) = shortened(value, max);
        *cut |= was_cut;
        told.insert(key.to_owned(), json!(value));
    };
    put("description", found.description.as_deref(), MAX_DESCRIPTION);
    put(
        "latest_stable",
        found.max_stable_version.as_deref(),
        MAX_VERSION,
    );
    put("latest", Some(&found.max_version), MAX_VERSION);
    put("repository", found.repository.as_deref(), MAX_URL);
    told.insert("downloads".to_owned(), json!(found.downloads));
    if let Some(recent) = found.recent_downloads {
        told.insert("recent_downloads".to_owned(), json!(recent));
    }
    told.insert("updated_at".to_owned(), json!(found.updated_at));
    if found.exact_match {
        told.insert("exact_match".to_owned(), json!(true));
    }
    Value::Object(told)
}

/// `text` cut to `max` characters with `…`, and whether it was.
fn shortened(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    (cut, true)
}

/// The error for a search that crates.io did not answer, with no answer kept to stand in; or for
/// a cursor that crates.io refused (requirement R-ARG-1).
fn failed(error: &ApiError, with_cursor: bool) -> CallToolResult {
    if with_cursor
        && let ApiError::UnexpectedStatus { status } = error
        && status.is_client_error()
    {
        return InvalidArgument::new(
            "crates.io did not take the cursor; search again without it",
            CURSOR_EXAMPLE,
        )
        .into_result();
    }
    let (code, retryable) = match error {
        ApiError::Offline => ("offline_miss", false),
        ApiError::NoTurn
        | ApiError::RecentFailure { .. }
        | ApiError::Transient { .. }
        | ApiError::Network(_) => ("network_error", true),
        ApiError::NotFound
        | ApiError::UnexpectedStatus { .. }
        | ApiError::TooLarge { .. }
        | ApiError::Malformed(_) => ("network_error", false),
        ApiError::BadUrl(_) | ApiError::Cache(_) => ("internal_error", false),
    };
    let suggestions: &[&str] = match (code, retryable) {
        ("offline_miss", _) => &[
            "the search is answered from the cache only offline: search once online, or turn \
             network.offline off",
        ],
        ("internal_error", _) => &["check that the cache directory can be written"],
        (_, true) => &["call again in a moment"],
        (_, false) => &["crates.io answered in a way this server does not read; try again later"],
    };
    let mut body = json!({
        "code": code,
        "message": format!("crates.io did not answer the search: {}", ErrorChain(error)),
        "retryable": retryable,
        "suggestions": suggestions,
    });
    if let (ApiError::RecentFailure { retry_after }, Some(body)) = (error, body.as_object_mut()) {
        body.insert(
            "retry_after_secs".to_owned(),
            json!(retry_after.as_secs().max(1)),
        );
    }
    super::error_result(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(value: Value) -> Result<Search, InvalidArgument> {
        parse_arguments(Some(&object(value)))
    }

    #[test]
    fn a_cursor_is_only_for_the_same_search() {
        let search = parsed(json!({ "query": "json", "sort": "downloads" })).expect("checked");
        let cursor = cursor_of(&search.digest, &Next::Seek("WyIxIl0".into()));
        let again = parsed(json!({ "query": "json", "sort": "downloads", "cursor": cursor }))
            .expect("the same search");
        assert_eq!(again.next, Some(Next::Seek("WyIxIl0".into())));
        for other in [
            json!({ "query": "json", "sort": "relevance", "cursor": cursor }),
            json!({ "query": "yaml", "sort": "downloads", "cursor": cursor }),
            json!({ "query": "json", "sort": "downloads", "limit": 3, "cursor": cursor }),
        ] {
            assert!(
                parsed(other.clone()).err().is_some(),
                "another search: {other}"
            );
        }
        let paged = cursor_of(&search.digest, &Next::Page(3));
        let again = parsed(json!({ "query": "json", "sort": "downloads", "cursor": paged }))
            .expect("a page");
        assert_eq!(again.next, Some(Next::Page(3)));
        let long = format!("s1.0123.s{}", "a".repeat(513));
        for bad in [
            "s1.x.p3",
            "x",
            "s1.abc",
            "s1.0123.s&q=1",
            "s1.0123.p1",
            "s1.0123.p0",
            "s1.0123.p+3",
            "s1.0123.p03",
            &long,
        ] {
            let cursor = bad.replace("0123", &search.digest);
            let refused =
                parsed(json!({ "query": "json", "sort": "downloads", "cursor": cursor })).err();
            assert!(refused.is_some(), "{bad}");
        }
    }

    #[test]
    fn the_query_and_sort_are_checked() {
        for bad in [
            json!({}),
            json!({ "query": " " }),
            json!({ "query": "x".repeat(201) }),
        ] {
            assert!(parsed(bad).err().is_some());
        }
        assert!(
            parsed(json!({ "query": "json", "sort": "stars" }))
                .err()
                .is_some()
        );
        let null = parsed(json!({ "query": "json", "sort": null, "cursor": null }))
            .expect("null is absent");
        assert_eq!((null.sort, null.next), (Sort::Relevance, None));
        let search = parsed(json!({ "query": " json " })).expect("checked");
        assert_eq!(
            (&*search.query, search.sort, search.limit),
            ("json", Sort::Relevance, 10)
        );
    }

    #[test]
    fn a_found_crate_is_told_with_its_strings_cut() {
        let found = Found {
            name: "serde_json".into(),
            description: Some("d".repeat(MAX_DESCRIPTION + 5).into()),
            max_stable_version: Some("1.0.145".into()),
            max_version: "1.0.145".into(),
            downloads: 9,
            recent_downloads: None,
            repository: Some(" ".into()),
            updated_at: "2026-01-01T00:00:00Z".into(),
            exact_match: false,
        };
        let mut cut = false;
        let told = crate_value(&found, &mut cut);
        assert!(cut);
        assert_eq!(
            told["description"]
                .as_str()
                .map(|text| text.chars().count()),
            Some(MAX_DESCRIPTION)
        );
        assert!(told.get("repository").is_none(), "blank");
        assert!(told.get("exact_match").is_none());
    }
}
