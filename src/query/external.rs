//! How the items of other crates are shown in a crate's signatures and headers.
//!
//! They are shown by their canonical paths where those crates' archives tell them, else as
//! rustdoc writes them, told apart (requirements R-OUT-2 and R-PR-2, decided in M4b, decisions
//! 18 to 21 and 28 to 33). A [`CrateView`] takes an [`ExternalPaths`]; without one, the items of
//! other crates are written as rustdoc gives them, as before M4b.

use rustdoc_types::{Id, ItemSummary};
use serde::Serialize;

use super::view::CrateView;

/// How the items of other crates are shown, as their crates' archives tell their canonical paths.
///
/// It is asked from the blocking thread that renders, so it opens what it needs there, and as
/// often as an item is met: it remembers what it found itself.
pub trait ExternalPaths: std::fmt::Debug + Sync {
    /// How the item `id`, which `summary` tells of, is shown from the crate of `view`: by a path,
    /// or as rustdoc writes it and told apart; `None` when it is not looked at, as before M4b (by
    /// its canonical path if it is of that crate, as rustdoc writes it otherwise). It is asked of
    /// the items of that crate too, as alloc's, which are shown through std (decided in M4b,
    /// decision 18).
    fn shown(&self, view: &CrateView<'_>, id: Id, summary: &ItemSummary) -> Option<Shown>;
}

/// How an item of another crate is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// By this path: its canonical path, or its name in the prelude (decided in M4b, decision 18).
    Path(Box<str>),
    /// As rustdoc writes it, told in `data.not_canonical` (decided in M4b, decisions 20 and 32).
    /// Its `ty` is left empty: the renderer fills it with what it wrote.
    NotCanonical(NotCanonical),
}

/// An item of another crate that is not shown by its canonical path, and how to look it up, so
/// that the next calls show it by that path (decided in M4b, decision 20).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct NotCanonical {
    /// The type as rustdoc writes it, which the signature shows, where it is first met.
    #[serde(rename = "type")]
    pub ty: Box<str>,
    /// Its crate's package name.
    #[serde(rename = "crate")]
    pub krate: Box<str>,
    /// Its crate's version, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<Box<str>>,
    /// The definition path, which `get_item` looks it up by in its crate.
    pub lookup: Box<str>,
    /// Why it is not shown by its canonical path.
    pub reason: Reason,
}

/// Why an item of another crate is not shown by its canonical path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Its crate's archive is not in the cache: looking it up makes it.
    NotCached,
    /// Its crate's archive does not have it, as when it is of another build.
    NotFound,
    /// It has no public path that is not hidden (decided in M4b, decision 21).
    Unnameable,
}
