//! The versions of the formats of what the cache keeps, in one place (decided in M3a).
//!
//! A version is in the path of the places of its format, `v<n>`, so that servers of different
//! releases share the cache without reading each other's records (requirement R-CA-4); what has
//! no version in its path is of a format that every release reads.
//!
//! Change a version with the format that it names. A debug build for the tests makes them all
//! one newer (`crate::testing::format_bump`).

/// The records of the local builds, under `local/` (decided in C1 and M2's D4).
const LOCAL: u32 = 1;
/// The records of `build_external_docs`, under `ext/`.
const EXTERNAL: u32 = 1;
/// The records of the children of the servers, under `procs/`.
const RECORDS: u32 = 1;
/// What the probes of toolchains found, under `toolchains/` (decided in M2's C1).
const TOOLCHAINS: u32 = 1;
/// The sources unpacked from a `.crate`, and the listing of its files, beside it under
/// `static.crates.io/` (decided in M3b, B1-1).
const SOURCES: u32 = 1;
/// The schema of the archives of rustdoc JSON (requirement R-CA-2): change it with the types that
/// an archive holds, other than those of rustdoc-types, whose `FORMAT_VERSION` is in the header and
/// the name of the archive too.
const ARCHIVE: u32 = 8;

/// The directory of the local builds' records of this format: `v<n>`.
#[must_use]
pub fn local_dir() -> String {
    dir(LOCAL)
}

/// The directory of the records of `build_external_docs` of this format: `v<n>`.
#[must_use]
pub fn external_dir() -> String {
    dir(EXTERNAL)
}

/// The directory of the records of the children of this format: `v<n>`.
#[must_use]
pub fn records_dir() -> String {
    dir(RECORDS)
}

/// The directory of what the probes of toolchains found, of this format: `v<n>`.
#[must_use]
pub fn toolchains_dir() -> String {
    dir(TOOLCHAINS)
}

/// The directory of the sources unpacked from a `.crate`, of this format: `src.v<n>`. Its listing
/// is beside it, with `.json` added.
#[must_use]
pub fn sources_dir() -> String {
    format!("src.{}", dir(SOURCES))
}

/// The schema of the archives of rustdoc JSON.
#[must_use]
#[cfg_attr(
    not(debug_assertions),
    expect(
        clippy::missing_const_for_fn,
        reason = "a debug build reads the hooks for the tests, which no const fn can"
    )
)]
pub fn archive_schema() -> u32 {
    ARCHIVE.saturating_add(bump())
}

/// The directory of the format whose version is `version`, made newer for the tests.
fn dir(version: u32) -> String {
    format!("v{}", version.saturating_add(bump()))
}

/// How many versions newer the formats are: 1 in a debug build for the tests of servers of
/// different versions, else 0.
#[cfg(debug_assertions)]
fn bump() -> u32 {
    u32::from(crate::testing::format_bump())
}

/// How many versions newer the formats are: none in a release build.
#[cfg(not(debug_assertions))]
const fn bump() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The formats are as their constants tell, with the hooks for the tests off, as the unit
    /// tests run.
    #[test]
    fn the_directories_have_the_versions() {
        assert_eq!(local_dir(), "v1");
        assert_eq!(external_dir(), "v1");
        assert_eq!(records_dir(), "v1");
        assert_eq!(toolchains_dir(), "v1");
        assert_eq!(sources_dir(), "src.v1");
        assert_eq!(archive_schema(), 8);
    }
}
