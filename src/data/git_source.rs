//! The git repositories that `build_external_docs` documents crates of, as the arguments `git`,
//! `rev`, `branch` and `tag` name them (§6.4, requirements R-SEC-4 and R-WS-10, decided in M2e's
//! E3).
//!
//! A URL is checked before anything uses it, and normalized as cargo's `CanonicalUrl` does, but for
//! its scheme, to key the records: a record fetched over plain HTTP is not given for HTTPS.

use std::fmt;

use reqwest::Url;

/// The schemes that `git` takes (requirement R-SEC-4).
const SCHEMES: [&str; 4] = ["https", "ssh", "http", "git"];

/// The character of `text` that a value must not have, if any (requirement R-SEC-4).
///
/// It is a control character, a bidirectional control or mark, or an invisible formatting
/// character, which would let a URL or a path look other than it is on the screen that asks to
/// allow a call, in the provenance and in the log.
#[must_use]
pub fn hidden_char(text: &str) -> Option<char> {
    text.chars().find(|&c| super::hidden::is_hidden(c))
}

/// A git repository's URL, checked (requirement R-SEC-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitUrl {
    /// The URL as given, which cargo fetches.
    given: Url,
    /// The URL normalized as cargo does, but for its scheme: the key of its records.
    key: Box<str>,
}

impl GitUrl {
    /// The URL `text`, checked and normalized.
    ///
    /// # Errors
    ///
    /// Returns why it is not taken.
    pub fn parse(text: &str) -> Result<Self, GitUrlError> {
        // Before parsing, which drops tabs and newlines without a word.
        if let Some(c) = hidden_char(text) {
            return Err(GitUrlError::Character(c));
        }
        // Which ends the host of an https URL, after what reads as a user: `evil\@good`.
        if text.contains('\\') {
            return Err(GitUrlError::Backslash);
        }
        let Some((scheme, rest)) = text.split_once("://") else {
            return Err(GitUrlError::NoScheme);
        };
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "file" {
            return Err(GitUrlError::File);
        }
        if !SCHEMES.contains(&scheme.as_str()) {
            return Err(GitUrlError::Scheme);
        }
        // The host as given: `url` makes another of a host in letters that look like others.
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            return Err(GitUrlError::NoHost);
        }
        if !authority.is_ascii() {
            return Err(GitUrlError::NonAsciiHost);
        }
        let given = Url::parse(text).map_err(|_unparsed| GitUrlError::Malformed)?;
        if given.cannot_be_a_base() || given.host_str().is_none_or(str::is_empty) {
            return Err(GitUrlError::NoHost);
        }
        let credentials = if scheme == "ssh" {
            given.password().is_some()
        } else {
            !given.username().is_empty() || given.password().is_some()
        };
        if credentials {
            return Err(GitUrlError::Credentials);
        }
        // What is fetched is the host as written: not one that percent-encoding, a numeric form or
        // a leading `-` (an option of ssh) makes other.
        let written = authority.rsplit('@').next().unwrap_or_default();
        // Without its port: after the last `:` that is not in an IPv6 address's brackets.
        let written = match written.rfind(':') {
            Some(colon) if !written.get(colon..).is_some_and(|port| port.contains(']')) => {
                written.get(..colon).unwrap_or(written)
            },
            Some(_) | None => written,
        };
        let host = given.host_str().unwrap_or_default();
        if !written.eq_ignore_ascii_case(host) {
            return Err(GitUrlError::HostNotAsWritten);
        }
        if host.starts_with('-') || given.username().starts_with('-') {
            return Err(GitUrlError::Dash);
        }
        if given.query().is_some() || given.fragment().is_some() {
            return Err(GitUrlError::QueryOrFragment);
        }
        if given.path().trim_matches('/').is_empty() {
            return Err(GitUrlError::NoPath);
        }
        let key = canonical(&given);
        Ok(Self { given, key })
    }

    /// The URL as given, which cargo fetches.
    #[must_use]
    pub fn as_given(&self) -> &str {
        self.given.as_str()
    }

    /// The key of its records: normalized as cargo does, but for its scheme.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Whether it is fetched in plain text (`http` or `git`), where what is fetched may be changed
    /// on the way, as the response tells (requirement R-SEC-4).
    #[must_use]
    pub fn plaintext(&self) -> bool {
        matches!(self.given.scheme(), "http" | "git")
    }
}

/// `url` normalized as cargo's `CanonicalUrl` does, but for its scheme (requirement R-WS-10): a
/// trailing `/` taken off, then a trailing `.git`, and a github.com URL in lower case.
fn canonical(url: &Url) -> Box<str> {
    let mut url = url.clone();
    if let Ok(mut segments) = url.path_segments_mut() {
        segments.pop_if_empty();
    }
    if url.host_str() == Some("github.com") {
        let lower = url.path().to_ascii_lowercase();
        url.set_path(&lower);
    }
    if let Some(stripped) = url.path().strip_suffix(".git").map(str::to_owned) {
        url.set_path(&stripped);
    }
    url.as_str().into()
}

/// Why a `git` URL is not taken (requirement R-SEC-4). It never tells the URL, which may have
/// credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitUrlError {
    /// It has a control, bidirectional or invisible character.
    Character(char),
    /// It has no scheme, as the scp form `git@host:owner/repo`.
    NoScheme,
    /// It is a `file://` URL.
    File,
    /// Its scheme is not one that it takes.
    Scheme,
    /// Its host has characters other than ASCII.
    NonAsciiHost,
    /// It is not a URL.
    Malformed,
    /// It has no host.
    NoHost,
    /// It has credentials.
    Credentials,
    /// It has a query or a fragment.
    QueryOrFragment,
    /// It has a backslash.
    Backslash,
    /// Its host is not what is written, as with percent-encoding or a numeric form.
    HostNotAsWritten,
    /// Its host or its user begins with `-`, which ssh would take for an option.
    Dash,
    /// It has no path.
    NoPath,
}

impl fmt::Display for GitUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Character(c) => write!(
                f,
                "`git` has a control, bidirectional or invisible character (U+{:04X})",
                u32::from(*c)
            ),
            Self::NoScheme => f.write_str(
                "`git` must be a URL with a scheme; for git@host:owner/repo, pass \
                 ssh://git@host/owner/repo",
            ),
            Self::File => f.write_str(
                "`git` takes no file:// URL; pass a workspace on this machine as `project`",
            ),
            Self::Scheme => {
                f.write_str("`git` must start with https://, ssh://, http:// or git://")
            },
            Self::NonAsciiHost => f.write_str(
                "the host or the user of `git` has characters other than ASCII; write a host in \
                 punycode (xn--…)",
            ),
            Self::Backslash => f.write_str("`git` must have no backslash; write / between parts"),
            Self::HostNotAsWritten => f.write_str(
                "the host of `git` must be written as it is, without percent-encoding or a \
                 numeric form other than an IP address",
            ),
            Self::Dash => f.write_str("the host and the user of `git` must not begin with -"),
            Self::Malformed => f.write_str("`git` is not a URL"),
            Self::NoHost => f.write_str("`git` has no host"),
            Self::Credentials => f.write_str(
                "`git` must not carry credentials (user@ or user:password@, or a password in an \
                 ssh:// URL): cargo uses git's own",
            ),
            Self::QueryOrFragment => f.write_str(
                "`git` must have no ?query nor #fragment; pass the ref as `branch`, `tag` or `rev`",
            ),
            Self::NoPath => f.write_str("`git` has no repository path"),
        }
    }
}

/// The commit of a git repository that is asked for: by `rev`, `branch` or `tag`, as in cargo's
/// git dependencies, or the default branch (§6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRef {
    /// The default branch.
    DefaultBranch,
    /// A commit, by its hash, in lower case, or a ref under `refs/`.
    Rev(Box<str>),
    /// A branch.
    Branch(Box<str>),
    /// A tag.
    Tag(Box<str>),
}

impl GitRef {
    /// The ref that `rev`, `branch` or `tag` names, of which at most one is given.
    ///
    /// # Errors
    ///
    /// Returns why it is not taken.
    pub fn parse(
        rev: Option<&str>,
        branch: Option<&str>,
        tag: Option<&str>,
    ) -> Result<Self, GitRefError> {
        match (rev, branch, tag) {
            (None, None, None) => Ok(Self::DefaultBranch),
            (Some(rev), None, None) => {
                let hash =
                    (7..=64).contains(&rev.len()) && rev.bytes().all(|b| b.is_ascii_hexdigit());
                if hash {
                    Ok(Self::Rev(rev.to_ascii_lowercase().into()))
                } else if rev.starts_with("refs/") && is_ref_name(rev) {
                    Ok(Self::Rev(rev.into()))
                } else {
                    Err(GitRefError::Rev)
                }
            },
            (None, Some(branch), None)
                if branch != "@" && is_ref_name(&format!("refs/heads/{branch}")) =>
            {
                Ok(Self::Branch(branch.into()))
            },
            (None, Some(_), None) => Err(GitRefError::Name("branch")),
            (None, None, Some(tag)) if tag != "@" && is_ref_name(&format!("refs/tags/{tag}")) => {
                Ok(Self::Tag(tag.into()))
            },
            (None, None, Some(_)) => Err(GitRefError::Name("tag")),
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => {
                Err(GitRefError::Several)
            },
        }
    }

    /// The ref as git names it in full, which keys the commit it resolves to: `HEAD` for the
    /// default branch, `refs/heads/<branch>`, `refs/tags/<tag>`, or the rev.
    #[must_use]
    pub fn full_name(&self) -> String {
        match self {
            Self::DefaultBranch => "HEAD".to_owned(),
            Self::Rev(rev) => rev.to_string(),
            Self::Branch(branch) => format!("refs/heads/{branch}"),
            Self::Tag(tag) => format!("refs/tags/{tag}"),
        }
    }

    /// Whether it names a commit by its hash, which the repository cannot move to another
    /// commit, as it can a branch, a tag or a ref (requirement R-WS-10).
    #[must_use]
    pub fn is_hash(&self) -> bool {
        matches!(self, Self::Rev(rev) if rev.bytes().all(|b| b.is_ascii_hexdigit()))
    }
}

/// Whether `commit` is the full hash of a commit, as cargo writes it: 40 hex digits in lower case,
/// or 64 for a repository of SHA-256.
#[must_use]
pub fn is_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `name` is a ref name that git takes, as `git check-ref-format` tells it (requirement
/// R-SEC-4); a tag that begins with `-` too, which git takes.
fn is_ref_name(name: &str) -> bool {
    !name.is_empty()
        && name != "@"
        && !name.contains("@{")
        && !name.contains("..")
        && !name.contains("//")
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && hidden_char(name).is_none()
        && !name
            .chars()
            .any(|c| matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        && name.split('/').all(|part| {
            !part.is_empty() && !part.starts_with('.') && part.strip_suffix(".lock").is_none()
        })
}

/// Why `rev`, `branch` or `tag` is not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitRefError {
    /// More than one was given.
    Several,
    /// `rev` is neither a commit hash nor a ref under `refs/`.
    Rev,
    /// The branch or the tag, as this names it, is not a ref name.
    Name(&'static str),
}

impl fmt::Display for GitRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Several => f.write_str("give only one of `rev`, `branch` and `tag`"),
            Self::Rev => f.write_str(
                "`rev` must be a commit hash (7 to 64 hex digits) or a ref under refs/; pass a \
                 branch or a tag as `branch` or `tag`",
            ),
            Self::Name(which) => write!(f, "`{which}` is not a name that git takes"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_keyed_as_cargo_keys_it_but_for_its_scheme() {
        let key = |text: &str| GitUrl::parse(text).expect(text).key().to_owned();
        assert_eq!(
            key("https://example.com/o/r.git"),
            "https://example.com/o/r"
        );
        assert_eq!(key("https://example.com/o/r/"), "https://example.com/o/r");
        assert_eq!(
            key("https://example.com/o/r.git/"),
            "https://example.com/o/r"
        );
        assert_eq!(
            key("https://GitHub.com/Owner/Repo"),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            key("https://Example.com/Owner/Repo"),
            "https://example.com/Owner/Repo"
        );
        assert_eq!(key("http://github.com/o/r"), "http://github.com/o/r");
        assert_eq!(
            key("https://example.com:443/o/r"),
            "https://example.com/o/r"
        );
        assert_eq!(
            key("ssh://git@Example.com:2222/o/r.git"),
            "ssh://git@Example.com:2222/o/r"
        );
        assert_eq!(
            key("https://xn--e1afmkfd.example/o/r"),
            "https://xn--e1afmkfd.example/o/r"
        );
        let given = GitUrl::parse("https://example.com/o/r.git/").expect("taken");
        assert_eq!(given.as_given(), "https://example.com/o/r.git/");
        assert!(!given.plaintext());
        assert!(
            GitUrl::parse("git://example.com/o/r")
                .expect("taken")
                .plaintext()
        );
        assert!(
            GitUrl::parse("http://example.com/o/r")
                .expect("taken")
                .plaintext()
        );
    }

    #[test]
    fn a_url_that_could_mislead_or_leak_is_not_taken() {
        let error = |text: &str| GitUrl::parse(text).expect_err(text);
        assert_eq!(error("git@example.com:o/r"), GitUrlError::NoScheme);
        assert_eq!(error("example.com/o/r"), GitUrlError::NoScheme);
        assert_eq!(error("file:///tmp/r"), GitUrlError::File);
        assert_eq!(error("ftp://example.com/o/r"), GitUrlError::Scheme);
        assert_eq!(error("https://exаmple.com/o/r"), GitUrlError::NonAsciiHost);
        assert_eq!(
            error("https://token@example.com/o/r"),
            GitUrlError::Credentials
        );
        assert_eq!(
            error("https://u:p@example.com/o/r"),
            GitUrlError::Credentials
        );
        assert_eq!(
            error("ssh://git:p@example.com/o/r"),
            GitUrlError::Credentials
        );
        assert_eq!(
            error("https://example.com/o/r?branch=x"),
            GitUrlError::QueryOrFragment
        );
        assert_eq!(
            error("https://example.com/o/r#abc"),
            GitUrlError::QueryOrFragment
        );
        assert_eq!(error("https://example.com/"), GitUrlError::NoPath);
        assert_eq!(error("https:///o/r"), GitUrlError::NoHost);
        assert_eq!(
            error("https://example.com/o\t/r"),
            GitUrlError::Character('\t')
        );
        assert_eq!(
            error("https://example.com/o\u{202E}r"),
            GitUrlError::Character('\u{202E}')
        );
        assert_eq!(
            error("https://example.com/o\u{200B}r"),
            GitUrlError::Character('\u{200B}')
        );
        assert_eq!(
            error("https://example.com/o\u{2028}r"),
            GitUrlError::Character('\u{2028}')
        );
        assert_eq!(
            error("https://evil.com\\@good.com/o/r"),
            GitUrlError::Backslash
        );
        assert_eq!(
            error("https://ex%D0%B0mple.com/o/r"),
            GitUrlError::HostNotAsWritten
        );
        assert_eq!(error("https://0x7f.1/o/r"), GitUrlError::HostNotAsWritten);
        assert_eq!(error("ssh://-oProxyCommand=x/o/r"), GitUrlError::Dash);
        assert_eq!(error("ssh://-oProxyCommand=x@host/o/r"), GitUrlError::Dash);
        // Written as parsed: a port, an IPv6 address, a host in upper case.
        GitUrl::parse("https://Example.com:8443/o/r").expect("a port");
        GitUrl::parse("https://Example.com:443/o/r").expect("the default port");
        GitUrl::parse("https://[::1]:8443/o/r").expect("IPv6");
        GitUrl::parse("ssh://git@[::1]/o/r").expect("IPv6 with a user");
        GitUrl::parse("https://127.0.0.1/o/r").expect("IPv4");
        // An ssh user is not a credential.
        GitUrl::parse("ssh://git@example.com/o/r").expect("taken");
        // The message never tells the URL.
        assert!(
            !error("https://secret@example.com/o/r")
                .to_string()
                .contains("secret")
        );
    }

    #[test]
    fn a_ref_is_one_of_rev_branch_and_tag_with_a_name_that_git_takes() {
        assert_eq!(GitRef::parse(None, None, None), Ok(GitRef::DefaultBranch));
        assert_eq!(
            GitRef::parse(Some("ABCDEF0"), None, None),
            Ok(GitRef::Rev("abcdef0".into()))
        );
        assert_eq!(
            GitRef::parse(Some("refs/pull/1/head"), None, None),
            Ok(GitRef::Rev("refs/pull/1/head".into()))
        );
        assert_eq!(
            GitRef::parse(Some("abc"), None, None),
            Err(GitRefError::Rev)
        );
        assert_eq!(
            GitRef::parse(Some("v1.0"), None, None),
            Err(GitRefError::Rev)
        );
        assert_eq!(
            GitRef::parse(None, Some("feature/x"), None),
            Ok(GitRef::Branch("feature/x".into()))
        );
        assert_eq!(
            GitRef::parse(None, None, Some("-v1")),
            Ok(GitRef::Tag("-v1".into()))
        );
        for bad in [
            "a..b", "a b", "a~1", "x.lock", ".x", "a/", "@", "a@{1}", "a:b", "a\u{7}",
        ] {
            assert_eq!(
                GitRef::parse(None, Some(bad), None),
                Err(GitRefError::Name("branch")),
                "{bad:?}"
            );
        }
        assert_eq!(
            GitRef::parse(Some("abcdef0"), Some("main"), None),
            Err(GitRefError::Several)
        );
        assert_eq!(
            GitRef::parse(None, Some("main"), Some("v1")),
            Err(GitRefError::Several)
        );
    }

    #[test]
    fn a_ref_is_named_in_full_and_a_hash_does_not_move() {
        let named = |rev, branch, tag| {
            let reference = GitRef::parse(rev, branch, tag).expect("taken");
            (reference.full_name(), reference.is_hash())
        };
        assert_eq!(named(None, None, None), ("HEAD".to_owned(), false));
        assert_eq!(
            named(Some("ABCDEF0"), None, None),
            ("abcdef0".to_owned(), true)
        );
        assert_eq!(
            named(Some("refs/pull/1/head"), None, None),
            ("refs/pull/1/head".to_owned(), false)
        );
        assert_eq!(
            named(None, Some("main"), None),
            ("refs/heads/main".to_owned(), false)
        );
        assert_eq!(
            named(None, None, Some("v1")),
            ("refs/tags/v1".to_owned(), false)
        );
        let sha1 = "9a41be9f3db7140981e276cfc1fdb993ed8ae07f";
        assert!(is_commit(sha1));
        assert!(is_commit(&"a".repeat(64)));
        for not in [
            "9a41be9f",
            "",
            "../x",
            &sha1.to_ascii_uppercase(),
            &format!("{sha1}0"),
        ] {
            assert!(!is_commit(not), "{not}");
        }
    }

    #[test]
    fn hidden_characters_are_found() {
        assert_eq!(hidden_char("/home/me/app"), None);
        assert_eq!(hidden_char("/home/me/日本語"), None);
        assert_eq!(hidden_char("/home/\u{2066}me"), Some('\u{2066}'));
        assert_eq!(hidden_char("a\u{FEFF}"), Some('\u{FEFF}'));
        assert_eq!(hidden_char("a\u{85}"), Some('\u{85}'));
    }
}
