//! Following a path across crates, through re-exports of another crate's items.
//!
//! std re-exports most of its API from core and alloc: `std::vec::Vec` is `alloc::vec::Vec`, whose
//! documentation is in the JSON of alloc (requirement R-STD-1). Which crates may be loaded is up to
//! the caller; M1 follows only the crates of std (decided in M1).

use std::sync::Arc;

use super::{
    path_index::{PathIndex, Resolution},
    view::CrateView,
};
use crate::data::{archive::Mapped, rustdoc_json::ParsedCrate};

/// Most crates that one lookup goes through. `std::vec::Vec` goes through two.
const MAX_CRATES: usize = 4;

/// The version of the schema of [`Indexed`] in an archive (requirement R-CA-2). Change it with the
/// types that it holds, other than those of rustdoc-types, whose format version is in the header.
pub const ARCHIVE_SCHEMA: u32 = 3;

/// A parsed crate with its path index, which is archived as one (requirements R-PF-1 and R-PF-2).
#[derive(Debug, rkyv::Archive, rkyv::Serialize)]
pub struct Indexed {
    /// The crate.
    pub parsed: ParsedCrate,
    /// Its public paths.
    pub index: PathIndex,
    /// What the data was made from: the SHA-256 of the JSON, or another key of 64 hex digits
    /// that changes with it. Empty when not known. Cursors are bound to it (requirement R-OUT-3).
    pub source: Box<str>,
}

impl Indexed {
    /// Indexes `parsed`.
    #[must_use]
    pub fn new(parsed: ParsedCrate) -> Self {
        let index = PathIndex::build(&parsed.krate);
        Self {
            parsed,
            index,
            source: Box::default(),
        }
    }

    /// The same, made from the data that `source` identifies.
    #[must_use]
    pub fn with_source(mut self, source: &str) -> Self {
        self.source = source.into();
        self
    }
}

/// Where a path was followed to.
#[derive(Debug)]
pub struct Followed {
    /// The archive of the crate where the path stopped.
    pub krate: Arc<Mapped<Indexed>>,
    /// The path in that crate, to resolve there.
    pub path: Box<str>,
    /// The paths it went through, the one asked for first.
    pub through: Vec<Box<str>>,
}

/// Follows `path` from the crate it starts with to the crate that has the item.
///
/// It goes through re-exports of other crates' items. `load` gives the archive of the crate with a
/// name (as `alloc`), or `None` for one that is not followed into. The path stops where it
/// resolves, or where it cannot go on. `None` when `load` does not give the crate that `path`
/// starts with.
pub fn follow(
    path: &str,
    mut load: impl FnMut(&str) -> Option<Arc<Mapped<Indexed>>>,
) -> Option<Followed> {
    let mut path: Box<str> = path.into();
    let mut krate = load(crate_name(&path))?;
    let mut through = vec![path.clone()];
    for _ in 1..MAX_CRATES {
        let view = CrateView::new(krate.get());
        match view.index().resolve(&view, &path) {
            Resolution::ExternalReexport(_) | Resolution::NotFound => {},
            Resolution::Item(_)
            | Resolution::Member { .. }
            | Resolution::Ambiguous(_)
            | Resolution::AmbiguousMembers { .. } => break,
        }
        let Some(crossing) = view.index().crossing(&path) else {
            break;
        };
        let Some(next) = load(crate_name(&crossing.continued)) else {
            break;
        };
        path = crossing.continued;
        through.push(path.clone());
        krate = next;
    }
    Some(Followed {
        krate,
        path,
        through,
    })
}

/// The crate name that `path` starts with.
fn crate_name(path: &str) -> &str {
    path.split("::").next().unwrap_or(path)
}
