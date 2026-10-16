//! The `chatora-rustdoc` binary: an MCP server that looks up Rust documentation, plus a small CLI to manage its cache and install the skill.

use chatora_rustdoc as _;

#[cfg_attr(
    test,
    expect(
        clippy::missing_const_for_fn,
        reason = "under cfg(test) `main` is an ordinary function, and clippy flags it while it is empty"
    )
)]
fn main() {}
