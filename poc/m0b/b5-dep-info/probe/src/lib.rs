#![doc = include_str!("../README.md")]

pub mod plain;

#[path = "elsewhere/pathed.rs"]
pub mod pathed;

#[cfg(docsrs)]
#[path = "docsrs_only.rs"]
pub mod docsrs_only;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

/// Bytes included from a file.
pub const DATA: &[u8] = include_bytes!("../data.bin");
