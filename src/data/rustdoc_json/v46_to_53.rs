//! Formats 46 to 53 (47 does not exist).
//!
//! - `Item::attrs` is a list of strings (`"#[non_exhaustive]"`); format 54 made it a list of
//!   `Attribute`. The strings are parsed: those that format 54 and later have a variant for become
//!   that variant, and the rest, or what does not parse, stay as `Attribute::Other` with the string
//!   as it is (requirement R-FV-2). Format 48 to 52 changed how some of them are written
//!   (`#[inline]` as `#[attr = Inline(Hint)]`); those without a variant are kept as written.
//! - `ExternalCrate::path` does not exist yet, as in formats 54 to 56.
//! - Format 51 made `AssocItemConstraint::args` and `Type::QualifiedPath::args` optional. The
//!   latest types read the older, plain value as present, so no shim is needed for it.

use std::collections::HashMap;

use rustdoc_types::{
    Attribute,
    AttributeRepr,
    Deprecation,
    Id,
    ItemEnum,
    ItemSummary,
    ReprKind,
    Span,
    Target,
    Visibility,
};
use serde::Deserialize;

use super::v54_to_56::ExternalCrate;

/// `rustdoc_types::Crate` with the old `Item` and `ExternalCrate`.
#[derive(Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "the fields mirror those of `rustdoc_types::Crate`"
)]
pub(super) struct Crate {
    root: Id,
    crate_version: Option<String>,
    includes_private: bool,
    index: HashMap<Id, Item>,
    paths: HashMap<Id, ItemSummary>,
    external_crates: HashMap<u32, ExternalCrate>,
    target: Target,
    format_version: u32,
}

/// `rustdoc_types::Item` with attributes as strings, and without the stability that format 58
/// added.
#[derive(Deserialize)]
struct Item {
    id: Id,
    crate_id: u32,
    name: Option<String>,
    span: Option<Span>,
    visibility: Visibility,
    docs: Option<String>,
    links: HashMap<String, Id>,
    attrs: Vec<String>,
    deprecation: Option<Deprecation>,
    inner: ItemEnum,
}

impl Crate {
    /// Converts to the latest types, field by field so that a new field upstream fails to compile
    /// here (requirement R-FV-9).
    pub(super) fn into_latest(self) -> rustdoc_types::Crate {
        let Self {
            root,
            crate_version,
            includes_private,
            index,
            paths,
            external_crates,
            target,
            format_version,
        } = self;
        rustdoc_types::Crate {
            root,
            crate_version,
            includes_private,
            index: index
                .into_iter()
                .map(|(id, item)| (id, item.into_latest()))
                .collect(),
            paths,
            external_crates: external_crates
                .into_iter()
                .map(|(id, external_crate)| (id, external_crate.into_latest()))
                .collect(),
            target,
            format_version,
        }
    }
}

impl Item {
    fn into_latest(self) -> rustdoc_types::Item {
        let Self {
            id,
            crate_id,
            name,
            span,
            visibility,
            docs,
            links,
            attrs,
            deprecation,
            inner,
        } = self;
        rustdoc_types::Item {
            id,
            crate_id,
            name,
            span,
            visibility,
            docs,
            links,
            attrs: attrs.into_iter().map(attribute).collect(),
            deprecation,
            stability: None,
            const_stability: None,
            inner,
        }
    }
}

/// The attribute that format 54 and later write for `text`, as `#[must_use = "reason"]`, or
/// `Attribute::Other(text)` if it has no variant or does not parse.
fn attribute(text: String) -> Attribute {
    parse_attribute(&text).unwrap_or(Attribute::Other(text))
}

/// The variant for `text`, if it has one and parses.
fn parse_attribute(text: &str) -> Option<Attribute> {
    let body = text.strip_prefix("#[")?.strip_suffix(']')?.trim();
    // `#[unsafe(no_mangle)]` in edition 2024.
    let body = body
        .strip_prefix("unsafe(")
        .and_then(|inner| inner.strip_suffix(')'))
        .map_or(body, str::trim);
    let (name, rest) = body
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or((body, ""), |end| body.split_at(end));
    let rest = rest.trim();
    match (name, rest) {
        ("non_exhaustive", "") => Some(Attribute::NonExhaustive),
        ("automatically_derived", "") => Some(Attribute::AutomaticallyDerived),
        ("macro_export", "") => Some(Attribute::MacroExport),
        ("no_mangle", "") => Some(Attribute::NoMangle),
        ("must_use", "") => Some(Attribute::MustUse { reason: None }),
        ("must_use", _) => Some(Attribute::MustUse {
            reason: Some(after_equals(rest)?),
        }),
        ("export_name", _) => after_equals(rest).map(Attribute::ExportName),
        ("link_section", _) => after_equals(rest).map(Attribute::LinkSection),
        ("target_feature", _) => target_feature(in_parentheses(rest)?),
        ("repr", _) => repr(in_parentheses(rest)?).map(Attribute::Repr),
        // Formats 52 and 53: `#[attr = MustUse {reason: "text"}]`.
        ("attr", _) => {
            let parsed = rest.strip_prefix('=')?.trim();
            if parsed == "MustUse" {
                return Some(Attribute::MustUse { reason: None });
            }
            let reason = parsed
                .strip_prefix("MustUse")?
                .trim()
                .strip_prefix('{')?
                .strip_suffix('}')?
                .trim()
                .strip_prefix("reason:")?
                .trim();
            Some(Attribute::MustUse {
                reason: Some(string_literal(reason)?),
            })
        },
        _ => None,
    }
}

/// The string literal after `=` in `rest` (`= "text"`), with nothing after it.
fn after_equals(rest: &str) -> Option<String> {
    string_literal(rest.strip_prefix('=')?.trim())
}

/// What is inside the parentheses that `rest` is (`(C, align(8))`).
fn in_parentheses(rest: &str) -> Option<&str> {
    rest.strip_prefix('(')?.strip_suffix(')')
}

/// The value of `text`, a whole Rust string literal with simple escapes.
fn string_literal(text: &str) -> Option<String> {
    let mut chars = text.strip_prefix('"')?.strip_suffix('"')?.chars();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return None,
            '\\' => value.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                escaped @ ('\\' | '"' | '\'') => escaped,
                _ => return None,
            }),
            c => value.push(c),
        }
    }
    Some(value)
}

/// `TargetFeature` for the inside of `#[target_feature(enable = "a,b", enable = "c")]`, one
/// feature for each name.
fn target_feature(inside: &str) -> Option<Attribute> {
    let mut enable = Vec::new();
    for setting in split_settings(inside)? {
        let value = setting.strip_prefix("enable")?.trim();
        enable.extend(
            after_equals(value)?
                .split(',')
                .map(|feature| feature.trim().to_owned()),
        );
    }
    Some(Attribute::TargetFeature { enable })
}

/// The comma-separated settings of `inside`, where a comma within a string literal does not
/// separate.
fn split_settings(inside: &str) -> Option<Vec<&str>> {
    let mut settings = Vec::new();
    let mut start = 0;
    let mut in_literal = false;
    let mut escaped = false;
    for (at, c) in inside.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_literal => escaped = true,
            '"' => in_literal = !in_literal,
            ',' if !in_literal => {
                settings.push(inside.get(start..at)?.trim());
                start = at.saturating_add(1);
            },
            _ => {},
        }
    }
    settings.push(inside.get(start..)?.trim());
    Some(
        settings
            .into_iter()
            .filter(|setting| !setting.is_empty())
            .collect(),
    )
}

/// The contents of `#[repr(...)]`, as `C, align(8)`. `None` for what format 54 has no field for.
fn repr(inside: &str) -> Option<AttributeRepr> {
    let mut repr = AttributeRepr {
        kind: ReprKind::Rust,
        align: None,
        packed: None,
        int: None,
    };
    for hint in inside.split(',').map(str::trim) {
        match hint {
            "C" => repr.kind = ReprKind::C,
            "transparent" => repr.kind = ReprKind::Transparent,
            "simd" => repr.kind = ReprKind::Simd,
            "Rust" => repr.kind = ReprKind::Rust,
            "packed" => repr.packed = Some(1),
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
            | "i128" | "isize" => repr.int = Some(hint.to_owned()),
            _ => {
                let (name, rest) = hint.split_once('(')?;
                let bytes = rest.strip_suffix(')')?.trim().parse().ok()?;
                match name.trim() {
                    "align" => repr.align = Some(bytes),
                    "packed" => repr.packed = Some(bytes),
                    _ => return None,
                }
            },
        }
    }
    Some(repr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Attribute {
        attribute(text.to_owned())
    }

    #[test]
    fn attributes_with_a_variant_are_parsed() {
        assert_eq!(parsed("#[non_exhaustive]"), Attribute::NonExhaustive);
        assert_eq!(
            parsed("#[automatically_derived]"),
            Attribute::AutomaticallyDerived
        );
        assert_eq!(parsed("#[macro_export]"), Attribute::MacroExport);
        assert_eq!(parsed("#[unsafe(no_mangle)]"), Attribute::NoMangle);
        assert_eq!(parsed("#[must_use]"), Attribute::MustUse { reason: None });
        assert_eq!(
            parsed(r#"#[must_use = "if unused the \"lock\" is released"]"#),
            Attribute::MustUse {
                reason: Some(r#"if unused the "lock" is released"#.to_owned())
            }
        );
        assert_eq!(
            parsed(r#"#[attr = MustUse {reason: "text"}]"#),
            Attribute::MustUse {
                reason: Some("text".to_owned())
            }
        );
        assert_eq!(
            parsed(r#"#[export_name = "foo"]"#),
            Attribute::ExportName("foo".to_owned())
        );
        assert_eq!(
            parsed(r#"#[link_section = ".text"]"#),
            Attribute::LinkSection(".text".to_owned())
        );
        assert_eq!(
            parsed(r#"#[target_feature(enable = "avx2,fma", enable = "sse4.2")]"#),
            Attribute::TargetFeature {
                enable: vec!["avx2".to_owned(), "fma".to_owned(), "sse4.2".to_owned()]
            }
        );
    }

    #[test]
    fn repr_is_parsed_into_its_fields() {
        let repr = |text: &str| match parsed(text) {
            Attribute::Repr(repr) => Some(repr),
            Attribute::NonExhaustive
            | Attribute::MustUse { .. }
            | Attribute::MacroExport
            | Attribute::ExportName(_)
            | Attribute::LinkSection(_)
            | Attribute::AutomaticallyDerived
            | Attribute::NoMangle
            | Attribute::TargetFeature { .. }
            | Attribute::Other(_) => None,
        };
        assert_eq!(
            repr("#[repr(C)]"),
            Some(AttributeRepr {
                kind: ReprKind::C,
                align: None,
                packed: None,
                int: None
            })
        );
        assert_eq!(
            repr("#[repr(transparent)]").map(|repr| repr.kind),
            Some(ReprKind::Transparent)
        );
        assert_eq!(
            repr("#[repr(C, u8, align(8))]"),
            Some(AttributeRepr {
                kind: ReprKind::C,
                align: Some(8),
                packed: None,
                int: Some("u8".to_owned())
            })
        );
        assert_eq!(
            repr("#[repr(packed)]").and_then(|repr| repr.packed),
            Some(1)
        );
        assert_eq!(
            repr("#[repr(packed(2))]").and_then(|repr| repr.packed),
            Some(2)
        );
        assert_eq!(repr("#[repr(weird)]"), None);
    }

    #[test]
    fn other_attributes_are_kept_as_written() {
        for text in [
            "#[inline]",
            "#[attr = Inline(Hint)]",
            r#"#[<cfg>(feature = "sync")]"#,
            "#[doc(hidden)]",
            "#[track_caller]",
            r#"#[must_use = "unclosed]"#,
            r#"#[must_use = "\u{1F600}"]"#,
            "#[repr(weird)]",
            "not an attribute",
        ] {
            assert_eq!(parsed(text), Attribute::Other(text.to_owned()), "{text}");
        }
    }
}
