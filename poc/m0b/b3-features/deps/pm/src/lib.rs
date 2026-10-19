//! A proc-macro that app uses.

use proc_macro::TokenStream;

/// Returns its input.
#[proc_macro_attribute]
pub fn keep(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
