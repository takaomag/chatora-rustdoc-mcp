//! The words that greetings are made of.

/// A word of a greeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    /// "Hello".
    Hello,
    /// "Goodbye".
    Goodbye,
}

impl Word {
    /// The word as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hello => "Hello",
            Self::Goodbye => "Goodbye",
        }
    }
}
