//! A member whose build script sets a cfg and a variable, and that shares a dependency with
//! `one`.

/// The dependency's value.
pub fn shared() -> u32 {
    watcher::watched()
}

/// There only with the cfg of the build script.
#[cfg(from_build)]
pub fn configured() {}

/// The variable of the build script.
pub const FROM_BUILD: &str = env!("FROM_BUILD");
