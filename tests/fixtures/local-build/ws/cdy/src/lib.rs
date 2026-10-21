//! A member whose lib is only a cdylib.

/// An exported function.
#[no_mangle]
pub extern "C" fn exported() {}
