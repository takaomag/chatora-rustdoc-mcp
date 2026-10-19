//! Reading `format_version` before the whole rustdoc JSON is parsed, to choose the shim
//! (requirement R-FV-3).
//!
//! rustdoc writes `format_version` as the last member of the top-level object (checked on every
//! format in `testdata/docs-rs-json.txt`: 46, 48, 53 to 57, 60 and 61), so it is first read from
//! the end of the JSON, which takes the same time for any size. If the JSON does not end with it,
//! the whole JSON is scanned. The value is checked again when the whole JSON is parsed
//! ([`super::rustdoc_json::parse`]), so a misread cannot pick the wrong shim silently.

/// Why `format_version` could not be read.
#[derive(Debug, thiserror::Error)]
#[error("the rustdoc JSON has no readable format_version")]
pub struct FormatVersionError(#[source] serde_json::Error);

/// Reads `format_version` from the end of `json`, or else by scanning the whole of it.
///
/// # Errors
///
/// Returns an error when `json` is not a JSON object with a `format_version` that fits in a `u32`.
pub fn read(json: &[u8]) -> Result<u32, FormatVersionError> {
    if let Some(format_version) = peek(json) {
        return Ok(format_version);
    }
    tracing::warn!("format_version is not the last member of the rustdoc JSON; scanning all of it");
    scan(json)
}

/// Reads `format_version` when it is the last member of the top-level object, and returns `None`
/// otherwise. JSON whitespace around the tokens is allowed.
///
/// Only the end of `json` is read. A `format_version` inside a nested object or a string is not
/// taken: then the byte before the final `}` is a `}` or a `"`, not a digit.
#[must_use]
pub fn peek(json: &[u8]) -> Option<u32> {
    let rest = json.trim_ascii_end().strip_suffix(b"}")?.trim_ascii_end();
    let digit_count = rest
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let (rest, digits) = rest.split_at(rest.len().saturating_sub(digit_count));
    let format_version = std::str::from_utf8(digits).ok()?.parse().ok()?;
    let rest = rest
        .trim_ascii_end()
        .strip_suffix(b":")?
        .trim_ascii_end()
        .strip_suffix(br#""format_version""#)?
        .trim_ascii_end();
    // The member must be one of the top-level object, so a `,` or the opening `{` comes before it.
    matches!(rest.last(), Some(b',' | b'{')).then_some(format_version)
}

/// Reads `format_version` by parsing the whole of `json` and skipping everything else.
///
/// # Errors
///
/// Returns an error when `json` is not a JSON object with a `format_version` that fits in a `u32`.
pub fn scan(json: &[u8]) -> Result<u32, FormatVersionError> {
    #[derive(serde::Deserialize)]
    struct Header {
        format_version: u32,
    }
    serde_json::from_slice::<Header>(json)
        .map(|header| header.format_version)
        .map_err(FormatVersionError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_reads_the_last_member() {
        let cases: [&[u8]; 4] = [
            br#"{"root":1,"index":{},"format_version":61}"#,
            b"{\"root\":1,\"format_version\":46}\n",
            b"{\n  \"root\": 1,\n  \"format_version\" : 57\n}\n",
            br#"{"format_version":53}"#,
        ];
        for json in cases {
            assert_eq!(peek(json), Some(scan(json).expect("valid")), "{json:?}");
            assert_eq!(read(json).ok(), peek(json));
        }
    }

    #[test]
    fn peek_does_not_take_a_nested_or_quoted_format_version() {
        // A nested object that ends with `format_version`: only the scan finds the top-level one.
        let nested = br#"{"format_version":61,"target":{"format_version":5}}"#;
        assert_eq!(peek(nested), None);
        assert_eq!(read(nested).expect("scanned"), 61);
        // A string that ends like the member.
        let quoted = br#"{"format_version":61,"docs":"\"format_version\":5}"}"#;
        assert_eq!(peek(quoted), None);
        assert_eq!(read(quoted).expect("scanned"), 61);
        // Another member with a number.
        let other = br#"{"format_version":61,"root":5}"#;
        assert_eq!(peek(other), None);
        assert_eq!(read(other).expect("scanned"), 61);
    }

    #[test]
    fn read_fails_without_a_format_version() {
        for json in [
            &br#"{"root":1}"#[..],
            br#"{"root":"#,
            br#"{"format_version":99999999999}"#,
            b"",
        ] {
            assert!(read(json).is_err(), "{json:?}");
        }
    }
}
