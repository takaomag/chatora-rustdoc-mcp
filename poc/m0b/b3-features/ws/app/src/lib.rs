//! The member that uses everything.

/// Uses the proc-macro.
#[pm::keep]
pub fn app() {
    shared::normal();
}
