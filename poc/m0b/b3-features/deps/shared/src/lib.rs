//! Used by app with a different feature for each kind of dependency.

/// Present with the `normal` feature.
#[cfg(feature = "normal")]
pub fn normal() {}
/// Present with the `windows` feature.
#[cfg(feature = "windows")]
pub fn windows() {}
/// Present with the `build` feature.
#[cfg(feature = "build")]
pub fn build() {}
/// Present with the `dev` feature.
#[cfg(feature = "dev")]
pub fn dev() {}
