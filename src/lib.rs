//! Internal library of the `chatora-rustdoc` binary.
//!
//! It is published only because it is part of the same package.
//! Its API is not stable and may change in any release, including patch releases.
//!
//! The code is split into three layers. Each layer depends only on the layers below it:
//! [`mcp`] → [`query`] → [`data`].

pub mod data;
pub mod mcp;
pub mod query;
