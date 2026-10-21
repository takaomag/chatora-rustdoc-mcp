//! The bin of a package whose lib has the same name.

/// Only in the bin.
pub fn from_bin() {}

fn main() {
    same::from_lib();
    from_bin();
}
