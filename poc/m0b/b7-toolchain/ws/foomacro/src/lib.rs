//! A proc-macro whose output depends on `--cfg foo` reaching the proc-macro itself.

use proc_macro::TokenStream;

/// Adds `macro_saw_foo()` when the proc-macro was compiled with `--cfg foo`.
#[proc_macro]
pub fn items(_input: TokenStream) -> TokenStream {
    if cfg!(foo) {
        "/// Generated when the proc-macro saw `--cfg foo`.\npub fn macro_saw_foo() {}".parse().unwrap()
    } else {
        TokenStream::new()
    }
}
