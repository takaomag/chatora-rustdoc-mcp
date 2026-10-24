//! The characters that would hide text from whom reads it, and their removal from what the server
//! answers (requirements R-SEC-4 and R-SEC-7, decided in M2f's F7).
//!
//! What a crate's documentation, names and diagnostics carry is a third party's text, which a
//! control character, a bidirectional control or an invisible character could make read to the
//! model otherwise than it looks to a person, or hide words that only the model reads.

use std::{borrow::Cow, iter::Peekable, str::Chars};

use serde_json::{Map, Value};

/// Whether `c` is a control character, a bidirectional control or mark, or an invisible
/// formatting character: one that is not seen, or changes how the text around it is seen.
#[must_use]
pub const fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E007F}'
            | '\u{E0100}'..='\u{E01EF}')
}

/// Whether `c` goes in an answer: a newline and a tab do, which [`is_hidden`] counts as control
/// characters.
const fn is_kept(c: char) -> bool {
    matches!(c, '\n' | '\t') || !is_hidden(c)
}

/// `text` without the characters that [`is_hidden`] finds but newlines and tabs, nor its ANSI
/// escape sequences, whole; and how many characters were removed.
///
/// A line break other than a newline, as a carriage return alone, U+0085, U+2028 or U+2029,
/// becomes a newline, so that the lines it parts are not joined; one before a newline goes, as in
/// CRLF.
#[must_use]
pub fn strip(text: &str) -> (Cow<'_, str>, usize) {
    if text.chars().all(is_kept) {
        return (Cow::Borrowed(text), 0);
    }
    let mut kept = String::with_capacity(text.len());
    let mut removed = 0_usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let taken = match c {
            '\u{1B}' => escape(&mut chars),
            // CSI, as `ESC [` in eight bits.
            '\u{9B}' => control_sequence(&mut chars),
            // DCS, SOS, OSC, PM and APC, as `ESC P`, `ESC X`, `ESC ]`, `ESC ^` and `ESC _`.
            '\u{90}' | '\u{98}' | '\u{9D}' | '\u{9E}' | '\u{9F}' => string(&mut chars),
            c if is_kept(c) => {
                kept.push(c);
                continue;
            },
            '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                if chars.peek() != Some(&'\n') {
                    kept.push('\n');
                }
                0
            },
            _ => 0,
        };
        removed = removed.saturating_add(taken).saturating_add(1);
    }
    (Cow::Owned(kept), removed)
}

/// Takes the rest of the escape sequence after an `ESC`, and returns how many characters it took.
fn escape(chars: &mut Peekable<Chars<'_>>) -> usize {
    match chars.peek() {
        Some('[') => {
            chars.next();
            control_sequence(chars).saturating_add(1)
        },
        Some(']' | 'P' | 'X' | '^' | '_') => {
            chars.next();
            string(chars).saturating_add(1)
        },
        _ => {
            // Intermediate characters, then the final one.
            let mut taken = take_while(chars, |c| matches!(c, ' '..='/'));
            if chars.next_if(|c| matches!(c, '0'..='~')).is_some() {
                taken = taken.saturating_add(1);
            }
            taken
        },
    }
}

/// Takes the rest of a control sequence after its introducer: its parameters and intermediate
/// characters, then its final character. Returns how many characters it took.
fn control_sequence(chars: &mut Peekable<Chars<'_>>) -> usize {
    let taken = take_while(chars, |c| matches!(c, ' '..='?'));
    if chars.next_if(|c| matches!(c, '@'..='~')).is_some() {
        taken.saturating_add(1)
    } else {
        taken
    }
}

/// Takes the rest of a string, as an operating system command, after its introducer: up to its
/// terminator, a BEL or a string terminator (`ESC \\` or U+009C), or else to the end of the line or
/// to another escape sequence, which is left to be taken whole. Returns how many characters it
/// took.
fn string(chars: &mut Peekable<Chars<'_>>) -> usize {
    let taken = take_while(chars, |c| {
        !matches!(c, '\u{7}' | '\u{1B}' | '\u{9C}' | '\n')
    });
    match chars.peek() {
        Some('\u{7}' | '\u{9C}') => {
            chars.next();
            taken.saturating_add(1)
        },
        Some('\u{1B}') => {
            let mut ahead = chars.clone();
            ahead.next();
            if ahead.peek() == Some(&'\\') {
                chars.next();
                chars.next();
                taken.saturating_add(2)
            } else {
                taken
            }
        },
        _ => taken,
    }
}

/// Takes the characters that `take` holds for, and returns how many.
fn take_while(chars: &mut Peekable<Chars<'_>>, take: impl Fn(&char) -> bool) -> usize {
    let mut taken = 0_usize;
    while chars.next_if(&take).is_some() {
        taken = taken.saturating_add(1);
    }
    taken
}

/// Removes from every string of `value`, the keys of its objects too, what [`strip`] removes, and
/// returns how many characters were removed.
pub fn strip_value(value: &mut Value) -> usize {
    match value {
        Value::String(text) => {
            let (stripped, removed) = strip(text);
            if let Cow::Owned(stripped) = stripped {
                *text = stripped;
            }
            removed
        },
        Value::Array(values) => values
            .iter_mut()
            .map(strip_value)
            .fold(0, usize::saturating_add),
        Value::Object(fields) => {
            let mut removed = 0_usize;
            if fields.keys().any(|key| !key.chars().all(is_kept)) {
                let mut stripped = Map::with_capacity(fields.len());
                for (key, field) in std::mem::take(fields) {
                    let (key, from_key) = strip(&key);
                    removed = removed.saturating_add(from_key);
                    stripped.insert(key.into_owned(), field);
                }
                *fields = stripped;
            }
            fields
                .values_mut()
                .map(strip_value)
                .fold(removed, usize::saturating_add)
        },
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn hidden_characters_are_found() {
        assert!(!is_hidden('a'));
        assert!(!is_hidden('日'));
        assert!(is_hidden('\u{202E}'));
        assert!(is_hidden('\u{85}'));
        assert!(is_hidden('\u{E0041}'));
        assert!(is_hidden('\n'));
    }

    #[test]
    fn text_without_hidden_characters_is_kept_as_it_is() {
        let text = "a doc\n\twith 日本語 and an emoji 🦀";
        assert_eq!(strip(text), (Cow::Borrowed(text), 0));
    }

    #[test]
    fn hidden_characters_are_removed_and_counted() {
        for (text, kept, removed) in [
            ("is_\u{202E}admin", "is_admin", 1),
            ("a\u{200B}b\u{FEFF}", "ab", 2),
            ("crlf\r\n", "crlf\n", 1),
            // Other line breaks part the lines still.
            ("a\rb\u{85}c\u{2028}d\u{2029}e", "a\nb\nc\nd\ne", 4),
            ("bell\u{7} and del\u{7F}", "bell and del", 2),
            ("tags: \u{E0049}\u{E0047}\u{E004E}", "tags: ", 3),
            ("\u{2066}isolated\u{2069}", "isolated", 2),
            ("selector\u{FE0F}", "selector", 1),
        ] {
            assert_eq!(
                strip(text),
                (Cow::Owned(kept.to_owned()), removed),
                "{text:?}"
            );
        }
    }

    #[test]
    fn escape_sequences_are_removed_whole() {
        for (text, kept, removed) in [
            // CSI: colors and a cursor movement.
            ("\u{1B}[1;31merror\u{1B}[0m: x", "error: x", 11),
            ("up\u{1B}[2A", "up", 4),
            // OSC: a title and a hyperlink, ended by BEL and by ST.
            ("\u{1B}]0;title\u{7}text", "text", 10),
            (
                "\u{1B}]8;;https://e.x\u{1B}\\link\u{1B}]8;;\u{1B}\\",
                "link",
                25,
            ),
            // An OSC without its end goes to the end of the line.
            ("a\u{1B}]0;never ended\nnext", "a\nnext", 15),
            // Two characters, and one with an intermediate character.
            ("\u{1B}cclear", "clear", 2),
            ("\u{1B}(Bascii", "ascii", 3),
            // Eight-bit CSI and OSC.
            ("\u{9B}31mred", "red", 4),
            ("\u{9D}0;t\u{9C}x", "x", 5),
            // An escape at the end, and one before a newline.
            ("end\u{1B}", "end", 1),
            ("line\u{1B}\nnext", "line\nnext", 1),
            // A control sequence that a newline cuts.
            ("\u{1B}[31\nnext", "\nnext", 4),
            // An OSC that another escape sequence ends goes, and that one whole too.
            ("\u{1B}]0;t\u{1B}[31mred", "red", 10),
            // An escape, or an introducer of eight bits, takes what follows as its own, as a
            // terminal does: a lone one in a text takes a character, or the rest of the line.
            ("\u{1B}Hello", "ello", 2),
            ("a\u{9D}rest\nnext", "a\nnext", 5),
        ] {
            assert_eq!(
                strip(text),
                (Cow::Owned(kept.to_owned()), removed),
                "{text:?}"
            );
        }
    }

    #[test]
    fn stripping_twice_changes_nothing_more() {
        let text = "\u{1B}[\u{1B}[31m\u{202E}x\u{1B}]\u{1B}\\";
        let (once, _) = strip(text);
        assert_eq!(strip(&once), (Cow::Borrowed(&*once), 0));
    }

    #[test]
    fn every_string_of_a_value_is_stripped_its_keys_too() {
        let mut value = json!({
            "docs": "safe\u{202E}",
            "attrs": ["#[doc(alias = \"a\u{200B}b\")]", 1, null, true],
            "auto_traits": {"Send\u{2060}": {"note": "\u{1B}[1mbold\u{1B}[0m"}},
        });
        assert_eq!(strip_value(&mut value), 11);
        assert_eq!(
            value,
            json!({
                "docs": "safe",
                "attrs": ["#[doc(alias = \"ab\")]", 1, null, true],
                "auto_traits": {"Send": {"note": "bold"}},
            })
        );
    }
}
