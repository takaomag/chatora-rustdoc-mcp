//! A member that builds with `--cfg docsrs`, which the tests break and fix.

/// A function.
pub fn fine() {}

/// Only for docs.rs.
#[cfg(docsrs)]
pub fn docsrs_only() {}
