//! Query layer: resolves paths and re-exports, renders signatures, and builds responses.

pub mod across;
pub mod cfg;
pub mod changelog;
pub mod crate_metadata;
pub mod docs;
pub mod examples;
pub mod feature_items;
pub mod get_item;
pub mod implementors;
pub mod impls;
pub mod list_module;
pub mod load;
#[cfg(feature = "local")]
pub mod local;
pub mod markdown;
pub mod methods;
pub mod overview;
pub mod path_index;
pub mod render;
pub mod search;
pub mod view;
