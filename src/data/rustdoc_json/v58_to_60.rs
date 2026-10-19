//! Formats 58 to 60.
//!
//! `Stability` writes its level flattened into the object, tagged by `level`
//! (`{"feature":"f","level":"stable","since":"1.0"}`). Format 61 nests it instead
//! (`{"feature":"f","level":{"stable":{"since":"1.0"}}}`).

use std::collections::HashMap;

use rustdoc_types::{Attribute, Deprecation, Id, ItemEnum, ItemSummary, Span, Target, Visibility};
use serde::Deserialize;

/// `rustdoc_types::Crate` with the old `Item`.
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
    external_crates: HashMap<u32, rustdoc_types::ExternalCrate>,
    target: Target,
    format_version: u32,
}

/// `rustdoc_types::Item` with the old `Stability`.
#[derive(Deserialize)]
struct Item {
    id: Id,
    crate_id: u32,
    name: Option<String>,
    span: Option<Span>,
    visibility: Visibility,
    docs: Option<String>,
    links: HashMap<String, Id>,
    attrs: Vec<Attribute>,
    deprecation: Option<Deprecation>,
    stability: Option<Box<Stability>>,
    const_stability: Option<Box<Stability>>,
    inner: ItemEnum,
}

#[derive(Deserialize)]
struct Stability {
    feature: String,
    #[serde(flatten)]
    level: StabilityLevel,
}

#[derive(Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
enum StabilityLevel {
    Stable { since: Option<String> },
    Unstable,
}

impl Crate {
    /// Converts to the latest types, field by field so that a new field upstream fails to compile
    /// here (requirement R-FV-9).
    pub(super) fn into_latest(self) -> rustdoc_types::Crate {
        let Self {
            root,
            crate_version,
            includes_private,
            index,
            paths,
            external_crates,
            target,
            format_version,
        } = self;
        rustdoc_types::Crate {
            root,
            crate_version,
            includes_private,
            index: index
                .into_iter()
                .map(|(id, item)| (id, item.into_latest()))
                .collect(),
            paths,
            external_crates,
            target,
            format_version,
        }
    }
}

impl Item {
    fn into_latest(self) -> rustdoc_types::Item {
        let Self {
            id,
            crate_id,
            name,
            span,
            visibility,
            docs,
            links,
            attrs,
            deprecation,
            stability,
            const_stability,
            inner,
        } = self;
        rustdoc_types::Item {
            id,
            crate_id,
            name,
            span,
            visibility,
            docs,
            links,
            attrs,
            deprecation,
            stability: stability.map(|stability| Box::new(stability.into_latest())),
            const_stability: const_stability.map(|stability| Box::new(stability.into_latest())),
            inner,
        }
    }
}

impl Stability {
    fn into_latest(self) -> rustdoc_types::Stability {
        let Self { feature, level } = self;
        rustdoc_types::Stability {
            feature,
            level: match level {
                StabilityLevel::Stable { since } => rustdoc_types::StabilityLevel::Stable { since },
                StabilityLevel::Unstable => rustdoc_types::StabilityLevel::Unstable,
            },
        }
    }
}
