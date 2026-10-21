//! A member with one lib, a feature and a dependency whose build script watches
//! `RUSTC_BOOTSTRAP`.

/// A public function.
pub fn answer() -> u32 {
    watcher::watched() + private()
}

/// A private function, which the documentation of a member shows.
fn private() -> u32 {
    1
}
