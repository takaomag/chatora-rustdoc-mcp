//! Its items show where `--cfg foo` from RUSTFLAGS reached.

/// Present when the crate itself was compiled with `--cfg foo`.
#[cfg(foo)]
pub fn lib_saw_foo() {}

/// Present when the build script was compiled with `--cfg foo`.
#[cfg(build_saw_foo)]
pub fn build_saw_foo() {}

foomacro::items!();
