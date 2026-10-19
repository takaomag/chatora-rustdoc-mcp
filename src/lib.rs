//! Internal library of the `chatora-rustdoc-mcp` binary.
//!
//! It is published only because it is part of the same package.
//! Its API is not stable and may change in any release, including patch releases.
//!
//! The code is split into three layers. Each layer depends only on the layers below it:
//! [`mcp`] → [`query`] → [`data`]. [`logging`] and `task` are shared by all of them.

pub mod data;
pub mod logging;
pub mod mcp;
pub mod query;
mod task;
