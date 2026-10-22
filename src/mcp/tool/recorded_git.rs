//! The lookups of the query tools in the records of `build_external_docs` of crates of git
//! repositories (requirement R-WS-10, decided in M2e's E3b).
//!
//! A ref is looked up by the commit that it last resolved to, which is used for a day (decided in
//! M2e, 7); a hash, by the recorded commit that it is or begins (decided in M2e, 8). Nothing is
//! resolved or built here (requirement R-MCP-3).

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use rmcp::model::CallToolResult;
use semver::Version;
use serde_json::{Map, Value, json};

use super::{
    ToolState,
    error_result,
    external_git,
    get_item::{GitSpec, RETRY_AFTER_SECS, being, building_response, shutting_down},
    recorded::{self, Found},
};
use crate::{
    data::{
        crate_name::CrateName,
        external::{self, Key, Recorded, Resolution, Resolved},
        flight::Doing,
        git_source,
        metadata::PackageId,
        target::TargetTriple,
    },
    query::local::{LocalBuilt, RECORDED_BY},
};

/// How long what a ref resolved to is used before the responses tell that it may have moved
/// (decided in M2e, 7).
const REF_TTL: Duration = Duration::from_hours(24);

/// What a lookup of a crate of git warns of `refresh`, which it does not act on: the query tools
/// neither resolve nor build it (requirement R-WS-10).
pub(super) const NO_REFRESH: &str = "refresh does not resolve nor build a crate of git again: call \
                                     build_external_docs again for that";

/// A crate of a git repository that `build_external_docs` recorded, to look up in.
#[derive(Debug)]
pub(super) struct GitFound {
    /// Its build.
    pub(super) built: Arc<LocalBuilt>,
    /// The name of its package, as the repository spells it.
    pub(super) package: CrateName,
    /// Its version.
    pub(super) version: Version,
    /// What the response tells of where it came from.
    pub(super) origin: GitOrigin,
}

/// What a response from the record of a crate of git tells of it: its repository, ref and commit,
/// and when the ref was resolved, in the provenance (decided in M2e's E3, (2)), and how to resolve
/// it anew.
#[derive(Debug, Clone, Default)]
pub(super) struct GitOrigin {
    /// The fields of the provenance.
    fields: Map<String, Value>,
    /// The hint to resolve the ref anew, unless a hash was asked for.
    hint: Option<String>,
    /// Whether it is the commit before, while the one that the ref moved to is built: the
    /// response tells when to call again (decided in M2e's E3b-2, 4).
    rebuilding: bool,
}

impl GitOrigin {
    /// Adds what it tells to `response`: its fields to the provenance, and its hint.
    pub(super) fn mark(&self, response: &mut Value) {
        let Some(fields) = response.as_object_mut() else {
            return;
        };
        if let Some(Value::Object(provenance)) = fields.get_mut("provenance") {
            provenance.extend(self.fields.clone());
        }
        // After `provenance`, as the other responses of a build before have it (§7.3).
        if self.rebuilding {
            let old = std::mem::take(fields);
            for (key, value) in old {
                let after = key == "provenance";
                fields.insert(key, value);
                if after {
                    fields.insert("retry_after_secs".to_owned(), json!(RETRY_AFTER_SECS));
                }
            }
        }
        if let Some(hint) = &self.hint
            && let Value::Array(hints) = fields
                .entry("hints")
                .or_insert_with(|| Value::Array(Vec::new()))
        {
            hints.push(json!(hint));
        }
    }
}

/// The crate `name` of the repository and at the ref of `git`, for `target`, as `build_external_docs`
/// recorded it; or the response: it is being made, failed, is broken or is not recorded, or a
/// short hash begins several commits. What the response warns of is added to `warnings`.
pub(super) async fn recorded_git(
    state: &ToolState,
    (name, git, target): (&CrateName, &GitSpec, &TargetTriple),
    warnings: &mut Vec<Box<str>>,
) -> Result<GitFound, Box<CallToolResult>> {
    let asked = Asked {
        name,
        git,
        target,
        what: external_git::what_of(name, git),
        again: external_git::arguments(name.as_str(), git, target),
    };
    let reference = asked.reference();
    if git.url.plaintext() {
        warnings.push(external_git::PLAINTEXT.into());
    }
    if git.reference.is_hash()
        && let Some(commit) = by_hash(state, &asked, &reference).await?
    {
        return of_commit(state, &asked, (&commit, None), warnings).await;
    }
    let (cache, key) = (state.cache.clone(), reference.clone());
    let resolution = crate::task::spawn_blocking(move || external::resolution(&cache, &key))
        .await
        .map_err(|_joined| Box::new(shutting_down()))?;
    match resolution {
        Resolution::Resolved(resolved) => {
            let commit = resolved.commit.clone();
            // A hash's resolution tells the package, not a time that matters.
            let resolved = (!git.reference.is_hash()).then_some(&resolved);
            of_commit(state, &asked, (&commit, resolved), warnings).await
        },
        Resolution::Building => Err(Box::new(asked.building(warnings))),
        Resolution::Failed(failed) => Err(Box::new(recorded::failed_again(
            &failed,
            &asked.call_again(),
        ))),
        Resolution::Broken(why) => Err(Box::new(recorded::broken(
            &asked.what,
            &why,
            &asked.call_again(),
        ))),
        Resolution::Missing => Err(Box::new(asked.no_record())),
    }
}

/// What a lookup asked for, for its responses.
struct Asked<'a> {
    /// The crate.
    name: &'a CrateName,
    /// Its repository and ref.
    git: &'a GitSpec,
    /// The target.
    target: &'a TargetTriple,
    /// What it is, for the messages.
    what: String,
    /// The arguments of `build_external_docs` that build it.
    again: Value,
}

impl Asked<'_> {
    /// The key of the record of the ref asked for.
    fn reference(&self) -> Key {
        Key::GitRef {
            url: self.git.url.key().into(),
            reference: self.git.reference.full_name().into(),
            name: self.name.cache_key().into(),
            target: self.target.as_str().into(),
            hidden: false,
        }
    }

    /// What calls `build_external_docs` to build it again.
    fn call_again(&self) -> String {
        format!(
            "call build_external_docs with {} to build it again",
            self.again
        )
    }

    /// The provenance of the responses while it is being made.
    fn provenance(&self) -> Map<String, Value> {
        let mut provenance = Map::new();
        provenance.insert("source".to_owned(), json!("local"));
        external_git::git_provenance(&mut provenance, self.git);
        provenance.insert("crate".to_owned(), json!(self.name.as_str()));
        provenance.insert("target".to_owned(), json!(self.target.as_str()));
        provenance.insert("recorded_by".to_owned(), json!(RECORDED_BY));
        provenance
    }

    /// `status: building`, as `build_external_docs` resolves or builds it.
    fn building(&self, warnings: &[Box<str>]) -> CallToolResult {
        building_response(
            self.provenance(),
            Doing::Building,
            &being(&self.what, Doing::Building, "build_external_docs"),
            warnings,
        )
    }

    /// `docs_unavailable` with `no_record`: `build_external_docs` has not documented it
    /// (requirement R-MCP-3).
    fn no_record(&self) -> CallToolResult {
        error_result(&json!({
            "code": "docs_unavailable",
            "message": format!("build_external_docs has not documented {}", self.what),
            "retryable": false,
            "suggestions": [format!(
                "call build_external_docs with {}, which runs its build scripts and proc macros, \
                 and then look it up again",
                self.again
            )],
            "reason": "no_record",
        }))
    }
}

/// The recorded commit that the hash of `asked` is, or begins, among those with a record, made or
/// being made or failed (decided in M2e's E3b, 2): `None` when none is, and the record of the ref
/// `reference` is read instead (decided in M2e's E3b, 3). Several are `invalid_argument`, with
/// them (decided in M2e, 8).
async fn by_hash(
    state: &ToolState,
    asked: &Asked<'_>,
    reference: &Key,
) -> Result<Option<Box<str>>, Box<CallToolResult>> {
    let git_source::GitRef::Rev(rev) = &asked.git.reference else {
        return Ok(None);
    };
    let (cache, reference, rev) = (state.cache.clone(), reference.clone(), rev.clone());
    let found = crate::task::spawn_blocking(move || {
        let of = |commit: &str| commit_key(&reference, commit);
        // A full hash is looked for first; a SHA-1 one may begin a SHA-256 one.
        if git_source::is_commit(&rev) && external::exists(&cache, &of(&rev)) {
            return (vec![rev.clone()], rev);
        }
        let matching: Vec<Box<str>> = external::commits(&cache, &reference)
            .into_iter()
            .filter(|commit| commit.starts_with(&*rev) && external::exists(&cache, &of(commit)))
            .collect();
        (matching, rev)
    })
    .await
    .map_err(|_joined| Box::new(shutting_down()))?;
    match found {
        (matching, _) if matching.len() <= 1 => Ok(matching.into_iter().next()),
        (matching, rev) => Err(Box::new(error_result(&json!({
            "code": "invalid_argument",
            "message": format!(
                "`rev` {rev} begins several commits of {} that build_external_docs documented: \
                 give more of the hash",
                asked.git.url.key()
            ),
            "retryable": false,
            "suggestions": ["give one of the candidates as `rev`"],
            "candidates": matching,
        })))),
    }
}

/// The key of the record of `commit` of the crate of the ref `reference`.
fn commit_key(reference: &Key, commit: &str) -> Key {
    match reference {
        Key::GitRef {
            url,
            name,
            target,
            hidden,
            ..
        }
        | Key::Git {
            url,
            name,
            target,
            hidden,
            ..
        } => Key::Git {
            url: url.clone(),
            commit: commit.into(),
            name: name.clone(),
            target: target.clone(),
            hidden: *hidden,
        },
        Key::Release { .. }
        | Key::Target { .. }
        | Key::Workspace { .. }
        | Key::WorkspaceTarget { .. } => reference.clone(),
    }
}

/// The crate of `asked` at `commit`, which the ref resolved to as `resolved` tells, when a ref was
/// asked for: from its record, or else the response.
///
/// While `build_external_docs` holds the lock of the ref, it resolves and builds it: a record of
/// the commit that failed, is broken or is missing is being made again (requirement R-WS-10), and
/// the commit that the ref resolved to before, if it moved, is given meanwhile (decided in M2e's
/// E3b-2, 4).
async fn of_commit(
    state: &ToolState,
    asked: &Asked<'_>,
    (commit, resolved): (&str, Option<&Resolved>),
    warnings: &mut Vec<Box<str>>,
) -> Result<GitFound, Box<CallToolResult>> {
    let reference = asked.reference();
    let key = commit_key(&reference, commit);
    let call_again = asked.call_again();
    let pending = {
        let (cache, key, reference) = (state.cache.clone(), key.clone(), reference.clone());
        crate::task::spawn_blocking(move || match external::read(&cache, &key) {
            Recorded::Building => true,
            Recorded::Built(..) => false,
            Recorded::Failed(_) | Recorded::Broken(_) | Recorded::Missing => {
                external::being_made(&cache, &reference)
            },
        })
        .await
        .map_err(|_joined| Box::new(shutting_down()))?
    };
    if pending {
        if let Some(resolved) = resolved
            && let Some(previous) = &resolved.previous
        {
            let before = commit_key(&reference, previous);
            let found = recorded::recorded_as(
                state,
                (&before, &asked.what, &call_again),
                || asked.provenance(),
                warnings,
            )
            .await;
            if let Found::Built(built) = found {
                let at = (&**previous, Some(resolved));
                return Ok(found_in(asked, built, at, Some(commit), warnings));
            }
        }
        return Err(Box::new(asked.building(warnings)));
    }
    match recorded::recorded_as(
        state,
        (&key, &asked.what, &call_again),
        || asked.provenance(),
        warnings,
    )
    .await
    {
        Found::Built(built) => Ok(found_in(asked, built, (commit, resolved), None, warnings)),
        Found::Response(result) => Err(result),
        Found::Missing => Err(Box::new(asked.no_record())),
    }
}

/// The crate of `asked` in `built`, of `commit`, which the ref resolved to as `resolved` tells, when
/// a ref was asked for; the commit before, while the one that it moved to, `rebuilding`, is built.
fn found_in(
    asked: &Asked<'_>,
    built: Arc<LocalBuilt>,
    (commit, resolved): (&str, Option<&Resolved>),
    rebuilding: Option<&str>,
    warnings: &mut Vec<Box<str>>,
) -> GitFound {
    // Of the commit before, its own record tells.
    let package = package_of(&built, resolved.filter(|_| rebuilding.is_none()));
    let (package, version) = package.unwrap_or_else(|| {
        let version = built
            .record
            .crate_version
            .parse()
            .unwrap_or(Version::new(0, 0, 0));
        (asked.name.clone(), version)
    });
    let origin = origin(
        asked,
        (commit, resolved),
        rebuilding,
        SystemTime::now(),
        warnings,
    );
    GitFound {
        built,
        package,
        version,
        origin,
    }
}

/// The name and the version of the package of `built`: those that the ref resolved to, as
/// `resolved` tells, or else those of its record.
fn package_of(built: &LocalBuilt, resolved: Option<&Resolved>) -> Option<(CrateName, Version)> {
    if let Some(resolved) = resolved {
        return Some((
            resolved.package.parse().ok()?,
            resolved.version.parse().ok()?,
        ));
    }
    let id = PackageId(built.record.package.clone());
    let (name, _) = id.name_and_version()?;
    Some((name.parse().ok()?, built.record.crate_version.parse().ok()?))
}

/// What the response tells of the crate of `asked` at `commit`, resolved as `resolved` tells when
/// a ref was asked for, `now`: when it was resolved, and that it is stale, which `warnings` tells
/// too, while the commit that the ref moved to, `rebuilding`, is built (decided in M2e's E3b-2,
/// 4), or past [`REF_TTL`] (decided in M2e, 7).
fn origin(
    asked: &Asked<'_>,
    (commit, resolved): (&str, Option<&Resolved>),
    rebuilding: Option<&str>,
    now: SystemTime,
    warnings: &mut Vec<Box<str>>,
) -> GitOrigin {
    let mut fields = Map::new();
    external_git::git_provenance(&mut fields, asked.git);
    fields.insert("rev".to_owned(), json!(commit));
    let Some(resolved) = resolved else {
        return GitOrigin {
            fields,
            ..GitOrigin::default()
        };
    };
    fields.insert("resolved_at".to_owned(), json!(resolved.at));
    let reference = asked.git.reference.full_name();
    let stale = if let Some(new) = rebuilding {
        warnings.push(
            format!(
                "{reference} moved to {new}, which build_external_docs builds now: this is the \
                 commit that it resolved to before"
            )
            .into(),
        );
        Some("rebuilding")
    } else {
        expiry(&reference, (commit, &resolved.at), now).map(|why| {
            warnings.push(why.into());
            "ref_ttl_expired"
        })
    };
    if let Some(reason) = stale {
        fields.insert("stale".to_owned(), json!(true));
        fields.insert("stale_reason".to_owned(), json!(reason));
    }
    let hint = format!(
        "this is the commit that {reference} resolved to at {}; to resolve it anew, call \
         build_external_docs with {}",
        resolved.at, asked.again
    );
    GitOrigin {
        fields,
        hint: Some(hint),
        rebuilding: rebuilding.is_some(),
    }
}

/// How much a time may be ahead of the clock before it is taken for wrong.
const CLOCK_SKEW: Duration = Duration::from_mins(5);

/// What the response warns of `reference`, resolved to `commit` at `at`, `now`, when that is more
/// than [`REF_TTL`] ago, or cannot be read, or is ahead of the clock (decided in M2e, 7).
fn expiry(reference: &str, (commit, at): (&str, &str), now: SystemTime) -> Option<String> {
    let Some(then) = crate::data::parse_rfc3339(at) else {
        return Some(format!(
            "{reference} was resolved to {commit} at a time that cannot be read ({at}): it may \
             have moved since"
        ));
    };
    match now.duration_since(then) {
        Ok(age) if age > REF_TTL => Some(format!(
            "{reference} was resolved to {commit} at {at}, more than a day ago: it may have moved \
             since"
        )),
        Err(ahead) if ahead.duration() > CLOCK_SKEW => Some(format!(
            "{reference} was resolved to {commit} at {at}, which is ahead of the clock: it may \
             have moved since"
        )),
        Ok(_) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::git_source::{GitRef, GitUrl};

    const COMMIT: &str = "9a41be9f3db7140981e276cfc1fdb993ed8ae07f";

    fn told(reference: GitRef, at: &str, now: SystemTime) -> (GitOrigin, Vec<Box<str>>) {
        let name: CrateName = "git-dep".parse().expect("a crate name");
        let target: TargetTriple = "x86_64-unknown-linux-gnu".parse().expect("a target");
        let git = GitSpec {
            url: GitUrl::parse("https://example.invalid/dep").expect("taken"),
            reference,
        };
        let asked = Asked {
            name: &name,
            git: &git,
            target: &target,
            what: external_git::what_of(&name, &git),
            again: external_git::arguments(name.as_str(), &git, &target),
        };
        let resolved = Resolved {
            commit: COMMIT.into(),
            package: "git-dep".into(),
            version: "0.3.0".into(),
            at: at.into(),
            previous: None,
        };
        let mut warnings = Vec::new();
        let hash = asked.git.reference.is_hash();
        let origin = origin(
            &asked,
            (COMMIT, (!hash).then_some(&resolved)),
            None,
            now,
            &mut warnings,
        );
        (origin, warnings)
    }

    #[test]
    fn a_ref_resolved_more_than_a_day_ago_is_stale() {
        let at = "2026-10-20T00:00:00Z";
        let then = crate::data::parse_rfc3339(at).expect("a time");
        let branch = || GitRef::Branch("main".into());
        let (fresh, warnings) = told(branch(), at, then + Duration::from_hours(23));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(fresh.fields["resolved_at"], at);
        assert_eq!(fresh.fields["rev"], COMMIT);
        assert_eq!(fresh.fields["ref"], "refs/heads/main");
        assert!(!fresh.fields.contains_key("stale"));
        assert!(
            fresh
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains(r#""branch":"main""#)),
            "{fresh:?}"
        );
        let (old, warnings) = told(branch(), at, then + Duration::from_hours(25));
        assert_eq!(old.fields["stale"], true);
        assert_eq!(old.fields["stale_reason"], "ref_ttl_expired");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // One that cannot be read, or ahead of the clock.
        let (unread, warnings) = told(branch(), "yesterday", then);
        assert_eq!(unread.fields["stale_reason"], "ref_ttl_expired");
        assert!(warnings[0].contains("cannot be read"), "{warnings:?}");
        let (ahead, _) = told(branch(), at, then - Duration::from_mins(10));
        assert_eq!(ahead.fields["stale_reason"], "ref_ttl_expired");
        let (skewed, _) = told(branch(), at, then - Duration::from_mins(1));
        assert!(!skewed.fields.contains_key("stale"));
        // A hash does not move.
        let (hash, warnings) = told(
            GitRef::Rev(COMMIT.into()),
            at,
            then + Duration::from_hours(9 * 24),
        );
        assert_eq!(warnings, Vec::<Box<str>>::new());
        assert!(hash.hint.is_none());
        assert!(!hash.fields.contains_key("resolved_at"));
        assert!(!hash.fields.contains_key("ref"));
    }

    #[test]
    fn what_it_tells_is_added_to_the_provenance_and_the_hints() {
        let (origin, _) = told(
            GitRef::Tag("v1".into()),
            "2026-10-20T00:00:00Z",
            SystemTime::now(),
        );
        let mut response = json!({
            "provenance": { "source": "local", "package_source": "git" },
            "data": {},
            "hints": ["first"],
        });
        origin.mark(&mut response);
        assert_eq!(response["provenance"]["git"], "https://example.invalid/dep");
        assert_eq!(response["provenance"]["ref"], "refs/tags/v1");
        assert_eq!(response["hints"][0], "first");
        assert!(response["hints"][1].is_string(), "{response}");
        let mut without = json!({ "provenance": {} });
        origin.mark(&mut without);
        assert_eq!(without["hints"].as_array().map(Vec::len), Some(1));
    }
}
