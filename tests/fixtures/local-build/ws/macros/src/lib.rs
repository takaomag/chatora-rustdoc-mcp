//! A proc-macro member, which is built for the host.

extern crate proc_macro;

use proc_macro::TokenStream;

/// Returns its input.
#[proc_macro]
pub fn same_tokens(input: TokenStream) -> TokenStream {
    input
}
