//! A member whose build `--cfg docsrs` breaks: the feature was removed from rustdoc (E0557).

#![cfg_attr(docsrs, feature(doc_auto_cfg))]

/// A function.
pub fn works() {}
