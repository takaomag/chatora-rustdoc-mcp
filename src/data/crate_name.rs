//! Crate names checked before they go into a URL or a file path (requirement R-SEC-4).

use std::str::FromStr;

/// Longest crate name crates.io accepts.
const MAX_LEN: usize = 64;

/// A crate name that follows the crates.io rules: 1 to 64 ASCII letters, digits, `-` and `_`,
/// starting with a letter.
///
/// It is made only through [`FromStr`] (`"serde".parse::<CrateName>()`), so every value has been
/// checked. It does not deref to `str`, so that a checked name and a plain string stay apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display, derive_more::AsRef)]
#[as_ref(str)]
pub struct CrateName(Box<str>);

/// Why a string is not a valid crate name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CrateNameError {
    /// The name is empty.
    #[error("a crate name must not be empty")]
    Empty,
    /// The name is longer than crates.io allows.
    #[error("a crate name must be at most {MAX_LEN} characters long")]
    TooLong,
    /// The name does not start with an ASCII letter.
    #[error("a crate name must start with an ASCII letter")]
    BadStart,
    /// The name has a character other than ASCII letters, digits, `-` and `_`.
    #[error("a crate name may contain only ASCII letters, digits, `-` and `_`")]
    BadChar,
}

impl FromStr for CrateName {
    type Err = CrateNameError;

    /// Checks `name` against the crates.io rules and returns the first rule it breaks.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let Some(first) = name.chars().next() else {
            return Err(CrateNameError::Empty);
        };
        if name.len() > MAX_LEN {
            return Err(CrateNameError::TooLong);
        }
        if !first.is_ascii_alphabetic() {
            return Err(CrateNameError::BadStart);
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(CrateNameError::BadChar);
        }
        Ok(Self(name.into()))
    }
}

impl CrateName {
    /// The name as it was given.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_crates_io_names() {
        for name in [
            "serde",
            "tokio-util",
            "rustdoc_types",
            "a",
            "x86_64",
            &"a".repeat(64),
        ] {
            assert_eq!(
                name.parse::<CrateName>().map(|n| n.to_string()),
                Ok(name.to_owned())
            );
        }
    }

    #[test]
    fn rejects_names_that_could_escape_a_url_or_a_path() {
        assert_eq!("".parse::<CrateName>(), Err(CrateNameError::Empty));
        assert_eq!(
            "a".repeat(65).parse::<CrateName>(),
            Err(CrateNameError::TooLong)
        );
        assert_eq!("1serde".parse::<CrateName>(), Err(CrateNameError::BadStart));
        assert_eq!("-serde".parse::<CrateName>(), Err(CrateNameError::BadStart));
        assert_eq!(
            "../serde".parse::<CrateName>(),
            Err(CrateNameError::BadStart)
        );
        for name in [
            "serde/..",
            "serde/json",
            "serde?x",
            "serde%2F",
            "serde json",
            "sérde",
        ] {
            assert_eq!(
                name.parse::<CrateName>(),
                Err(CrateNameError::BadChar),
                "{name}"
            );
        }
    }
}
