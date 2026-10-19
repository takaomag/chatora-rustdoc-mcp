//! Formats 54 to 56.
//!
//! - `ExternalCrate::path` does not exist yet (it was added in format 57). It becomes an empty
//!   path, which means "unknown".
//! - Format 54 writes `#[macro_export]` as `Attribute::Other`; format 55 added
//!   `Attribute::MacroExport` for it. (The rustdoc-types changelog calls the variant `MacroUse`,
//!   but the type defines `MacroExport`.)

use std::{collections::HashMap, path::PathBuf};

use rustdoc_types::{Attribute, Id, Item, ItemSummary, Target};
use serde::Deserialize;

/// `rustdoc_types::Crate` with the old `ExternalCrate`.
#[derive(Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "the fields mirror those of `rustdoc_types::Crate`"
)]
pub(super) struct Crate {
    root: Id,
    crate_version: Option<String>,
    includes_private: bool,
    index: HashMap<Id, Item>,
    paths: HashMap<Id, ItemSummary>,
    external_crates: HashMap<u32, ExternalCrate>,
    target: Target,
    format_version: u32,
}

/// `rustdoc_types::ExternalCrate` without `path`, as in formats 46 to 56.
#[derive(Deserialize)]
pub(super) struct ExternalCrate {
    name: String,
    html_root_url: Option<String>,
}

impl Crate {
    /// Converts to the latest types, field by field so that a new field upstream fails to compile
    /// here (requirement R-FV-9).
    pub(super) fn into_latest(self) -> rustdoc_types::Crate {
        let Self {
            root,
            crate_version,
            includes_private,
            mut index,
            paths,
            external_crates,
            target,
            format_version,
        } = self;
        if format_version == 54 {
            macro_export_as_its_own_variant(&mut index);
        }
        rustdoc_types::Crate {
            root,
            crate_version,
            includes_private,
            index,
            paths,
            external_crates: external_crates
                .into_iter()
                .map(|(id, external_crate)| (id, external_crate.into_latest()))
                .collect(),
            target,
            format_version,
        }
    }
}

impl ExternalCrate {
    pub(super) fn into_latest(self) -> rustdoc_types::ExternalCrate {
        let Self {
            name,
            html_root_url,
        } = self;
        rustdoc_types::ExternalCrate {
            name,
            html_root_url,
            path: PathBuf::new(),
        }
    }
}

/// Gives `#[macro_export]` the meaning it has from format 55 (requirement R-FV-7).
#[expect(
    clippy::iter_over_hash_type,
    reason = "each item is changed on its own, so the order does not matter"
)]
fn macro_export_as_its_own_variant(index: &mut HashMap<Id, Item>) {
    for item in index.values_mut() {
        for attr in &mut item.attrs {
            if matches!(attr, Attribute::Other(text) if text == "#[macro_export]") {
                *attr = Attribute::MacroExport;
            }
        }
    }
}
