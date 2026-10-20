//! A crate read in place from its archive (requirement R-PF-1).
//!
//! Queries look items up by id. An item is turned back into the types of rustdoc-types only when
//! it is looked up, and kept for the rest of the query, so that the code that renders it works on
//! those types. The rest of the crate stays in the archive.

use elsa::FrozenMap;
use rkyv::{Archived, rancor};
use rustdoc_types::{ExternalCrate, Id, Item, ItemSummary};

use super::{across::ArchivedIndexed, path_index::ArchivedPathIndex};

/// A crate and its public path index, read from their archive.
pub struct CrateView<'a> {
    archived: &'a ArchivedIndexed,
    items: FrozenMap<Id, Box<Item>>,
    summaries: FrozenMap<Id, Box<ItemSummary>>,
    external_crates: FrozenMap<u32, Box<ExternalCrate>>,
}

impl std::fmt::Debug for CrateView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrateView")
            .field("root", &self.root())
            .field("items_taken_out", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl<'a> CrateView<'a> {
    /// A view of `archived`.
    #[must_use]
    pub fn new(archived: &'a ArchivedIndexed) -> Self {
        Self {
            archived,
            items: FrozenMap::new(),
            summaries: FrozenMap::new(),
            external_crates: FrozenMap::new(),
        }
    }

    /// The public path index.
    #[must_use]
    pub const fn index(&self) -> &'a ArchivedPathIndex {
        &self.archived.index
    }

    /// The root module.
    #[must_use]
    pub const fn root(&self) -> Id {
        native(&self.archived.parsed.krate.root)
    }

    /// What the data was made from (the SHA-256 of the JSON), or empty when not known.
    #[must_use]
    pub fn source_digest(&self) -> &'a str {
        &self.archived.source
    }

    /// The version of the crate, if the JSON has one.
    #[must_use]
    pub fn crate_version(&self) -> Option<&'a str> {
        self.archived
            .parsed
            .krate
            .crate_version
            .as_ref()
            .map(rkyv::string::ArchivedString::as_str)
    }

    /// The format of the JSON the crate was read from.
    #[must_use]
    pub const fn format_version(&self) -> u32 {
        self.archived.parsed.krate.format_version.to_native()
    }

    /// The target the crate was built for.
    #[must_use]
    pub fn target_triple(&self) -> &'a str {
        self.archived.parsed.krate.target.triple.as_str()
    }

    /// The JSON is in a format newer than the latest types (requirement R-FV-5).
    #[must_use]
    pub const fn newer_than_supported(&self) -> bool {
        self.archived.parsed.newer_than_supported
    }

    /// The item `id` of this crate.
    #[must_use]
    pub fn item(&self, id: Id) -> Option<&Item> {
        if let Some(item) = self.items.get(&id) {
            return Some(item);
        }
        let archived = self.archived.parsed.krate.index.get(&archived_id(id))?;
        let item = deserialize::<Item>(archived)?;
        Some(self.items.insert(id, Box::new(item)))
    }

    /// The summary of the item `id`, of this crate or another, with its definition path.
    #[must_use]
    pub fn summary(&self, id: Id) -> Option<&ItemSummary> {
        if let Some(summary) = self.summaries.get(&id) {
            return Some(summary);
        }
        let archived = self.archived.parsed.krate.paths.get(&archived_id(id))?;
        let summary = deserialize::<ItemSummary>(archived)?;
        Some(self.summaries.insert(id, Box::new(summary)))
    }

    /// The crate that `crate_id` (from [`ItemSummary::crate_id`]) stands for.
    #[must_use]
    pub fn external_crate(&self, crate_id: u32) -> Option<&ExternalCrate> {
        if let Some(external) = self.external_crates.get(&crate_id) {
            return Some(external);
        }
        let archived = self
            .archived
            .parsed
            .krate
            .external_crates
            .get(&Archived::<u32>::from_native(crate_id))?;
        let external = deserialize::<ExternalCrate>(archived)?;
        Some(self.external_crates.insert(crate_id, Box::new(external)))
    }
}

/// `id` as it is in an archive.
pub(super) const fn archived_id(id: Id) -> rustdoc_types::ArchivedId {
    rustdoc_types::ArchivedId(Archived::<u32>::from_native(id.0))
}

/// The id that `archived` stands for.
pub(super) const fn native(archived: &rustdoc_types::ArchivedId) -> Id {
    Id(archived.0.to_native())
}

/// `archived` turned back into `T`. The archive was checked when it was opened, so this does not
/// fail; if it did, the item counts as not there.
fn deserialize<T>(archived: &T::Archived) -> Option<T>
where
    T: rkyv::Archive,
    T::Archived: rkyv::Deserialize<T, rkyv::api::high::HighDeserializer<rancor::Error>>,
{
    rkyv::deserialize::<T, rancor::Error>(archived)
        .inspect_err(|error| tracing::error!(%error, "an archived item could not be read"))
        .ok()
}

#[cfg(test)]
pub(super) mod tests {
    use rkyv::util::AlignedVec;
    use rustdoc_types::Crate;

    use super::*;
    use crate::{
        data::rustdoc_json::ParsedCrate,
        query::{across::Indexed, path_index::PathIndex},
    };

    /// `krate` and its index, archived as the cache keeps them.
    pub(in crate::query) fn archive(krate: Crate) -> AlignedVec {
        let indexed = Indexed {
            index: PathIndex::build(&krate),
            parsed: ParsedCrate {
                krate,
                newer_than_supported: false,
            },
            source: "ab".repeat(32).into(),
        };
        rkyv::to_bytes::<rancor::Error>(&indexed).expect("archived")
    }

    /// The archive in `bytes`, checked.
    pub(in crate::query) fn archived(bytes: &AlignedVec) -> &ArchivedIndexed {
        rkyv::access::<ArchivedIndexed, rancor::Error>(bytes).expect("checked")
    }
}
