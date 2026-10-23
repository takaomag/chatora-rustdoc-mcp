//! Greetings, made of the words of the `words` crate.

pub use words::Word;

/// Greets `name`.
#[must_use]
pub fn greet(name: &str) -> String {
    format!("{} {name}", Word::Hello.as_str())
}
