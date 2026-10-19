//! Parsing rustdoc JSON of any supported format into the latest `rustdoc_types` (requirements
//! R-FV-1 to R-FV-9).
//!
//! The format, read ahead from the end of the JSON (A5), chooses the types to deserialize into:
//! the latest types for format 57 and from 61, and shims that differ from them only where a change
//! broke reading, for 54 to 56 and for 58 to 60. The JSON is parsed once, as a stream, without
//! going through `serde_json::Value`. Formats before 54 come in M1.

mod v54_to_56;
mod v58_to_60;

use std::ops::RangeInclusive;

use rustdoc_types::{Crate, FORMAT_VERSION};
use serde::de::DeserializeOwned;

use super::format_version::{self, FormatVersionError};

/// The formats this server reads (requirement R-FV-4). Formats 46 to 53 come in M1.
pub const SUPPORTED: RangeInclusive<u32> = 54..=FORMAT_VERSION;

/// rustdoc JSON parsed into the latest types.
#[derive(Debug)]
pub struct ParsedCrate {
    /// The crate.
    pub krate: Crate,
    /// The JSON is in a format newer than the latest types. It was read as if every change were
    /// additive, which is not verified (requirement R-FV-5).
    pub newer_than_supported: bool,
}

/// Why rustdoc JSON could not be parsed.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The JSON has no readable `format_version`.
    #[error(transparent)]
    NoFormatVersion(#[from] FormatVersionError),
    /// The format is outside [`SUPPORTED`], or newer and not readable with the latest types.
    #[error(
        "rustdoc JSON format {found} is not supported (supported: {} to {})",
        supported.start(),
        supported.end()
    )]
    UnsupportedFormat {
        /// The format of the JSON.
        found: u32,
        /// The formats this server reads.
        supported: RangeInclusive<u32>,
    },
    /// The JSON does not match the types of its format.
    #[error("the rustdoc JSON of format {format_version} could not be parsed")]
    Json {
        /// The format of the JSON.
        format_version: u32,
        /// What serde reported.
        #[source]
        source: serde_json::Error,
    },
    /// The parsed `format_version` differs from the one read ahead.
    #[error("format_version was read ahead as {read_ahead} but parsed as {parsed}")]
    FormatMismatch {
        /// The value read ahead.
        read_ahead: u32,
        /// The value in the parsed JSON.
        parsed: u32,
    },
}

/// Parses rustdoc JSON of a supported format into the latest types.
///
/// # Errors
///
/// Returns an error when the format is not supported or the JSON does not match it.
pub fn parse(json: &[u8]) -> Result<ParsedCrate, ParseError> {
    let read_ahead = format_version::read(json)?;
    let json_error = |source| ParseError::Json {
        format_version: read_ahead,
        source,
    };
    let krate = match read_ahead {
        54..=56 => deserialize::<v54_to_56::Crate>(json)
            .map_err(json_error)?
            .into_latest(),
        57 | 61..=FORMAT_VERSION => deserialize::<Crate>(json).map_err(json_error)?,
        58..=60 => deserialize::<v58_to_60::Crate>(json)
            .map_err(json_error)?
            .into_latest(),
        newer if newer > FORMAT_VERSION => {
            // Try the latest types, and give up on the format if they do not fit (requirement
            // R-FV-5).
            deserialize::<Crate>(json).map_err(|error| {
                tracing::debug!(%error, format_version = newer, "the latest types do not fit");
                ParseError::UnsupportedFormat {
                    found: newer,
                    supported: SUPPORTED,
                }
            })?
        },
        older => {
            return Err(ParseError::UnsupportedFormat {
                found: older,
                supported: SUPPORTED,
            });
        },
    };
    if krate.format_version != read_ahead {
        return Err(ParseError::FormatMismatch {
            read_ahead,
            parsed: krate.format_version,
        });
    }
    let newer_than_supported = read_ahead > FORMAT_VERSION;
    if newer_than_supported {
        tracing::warn!(
            format_version = read_ahead,
            supported = FORMAT_VERSION,
            "rustdoc JSON is newer than the supported format; results are unverified"
        );
    }
    Ok(ParsedCrate {
        krate,
        newer_than_supported,
    })
}

/// Deserializes `json` as a whole, with no limit on nesting (requirement R-FV-8): types such as
/// typenum's nest hundreds of levels deep. The stack grows as needed instead of overflowing.
fn deserialize<T: DeserializeOwned>(json: &[u8]) -> Result<T, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(json);
    deserializer.disable_recursion_limit();
    let value = T::deserialize(serde_stacker::Deserializer::new(&mut deserializer))?;
    // Only whitespace may follow the value, as with `serde_json::from_slice`.
    deserializer.end()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use rustdoc_types::{
        Attribute,
        ExternalCrate,
        Generics,
        Id,
        Item,
        ItemEnum,
        Module,
        Stability,
        StabilityLevel,
        Target,
        Type,
        TypeAlias,
        Visibility,
    };
    use serde_json::{Value, json};

    use super::*;

    fn item(id: u32, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: Some(format!("item{id}")),
            span: None,
            visibility: Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner,
        }
    }

    fn module(id: u32) -> Item {
        item(
            id,
            ItemEnum::Module(Module {
                is_crate: id == 0,
                items: Vec::new(),
                is_stripped: false,
            }),
        )
    }

    fn krate(format_version: u32, items: Vec<Item>) -> Crate {
        Crate {
            root: Id(0),
            crate_version: Some("1.0.0".to_owned()),
            includes_private: false,
            index: items.into_iter().map(|item| (item.id, item)).collect(),
            paths: HashMap::new(),
            external_crates: HashMap::from([(1, ExternalCrate {
                name: "std".to_owned(),
                html_root_url: None,
                path: PathBuf::from("/sysroot/lib/libstd.rlib"),
            })]),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".to_owned(),
                target_features: Vec::new(),
            },
            format_version,
        }
    }

    /// `krate` as JSON, edited by `edit`.
    fn json_of(krate: &Crate, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value = serde_json::to_value(krate).expect("serializes");
        edit(&mut value);
        serde_json::to_vec(&value).expect("serializes")
    }

    #[test]
    fn the_latest_format_is_read_as_it_is() {
        let mut stable = module(0);
        stable.stability = Some(Box::new(Stability {
            feature: "core".to_owned(),
            level: StabilityLevel::Stable {
                since: Some("1.0.0".to_owned()),
            },
        }));
        let expected = krate(FORMAT_VERSION, vec![stable]);
        let json = serde_json::to_vec(&expected).expect("serializes");
        let parsed = parse(&json).expect("parses");
        assert_eq!(parsed.krate, expected);
        assert!(!parsed.newer_than_supported);
    }

    #[test]
    fn formats_58_to_60_have_a_flattened_stability_level() {
        let mut item = module(0);
        item.stability = Some(Box::new(Stability {
            feature: "core".to_owned(),
            level: StabilityLevel::Stable {
                since: Some("1.0.0".to_owned()),
            },
        }));
        item.const_stability = Some(Box::new(Stability {
            feature: "const_x".to_owned(),
            level: StabilityLevel::Unstable,
        }));
        for format_version in 58..=60 {
            let expected = krate(format_version, vec![item.clone()]);
            let json = json_of(&expected, |value| {
                value["index"]["0"]["stability"] =
                    json!({ "feature": "core", "level": "stable", "since": "1.0.0" });
                value["index"]["0"]["const_stability"] =
                    json!({ "feature": "const_x", "level": "unstable" });
            });
            assert_eq!(parse(&json).expect("parses").krate, expected);
        }
    }

    #[test]
    fn formats_54_to_56_have_no_external_crate_path() {
        for format_version in 54..=56 {
            let mut expected = krate(format_version, vec![module(0)]);
            let json = json_of(&expected, |value| {
                if let Some(external_crate) = value["external_crates"]["1"].as_object_mut() {
                    external_crate.remove("path");
                }
            });
            expected
                .external_crates
                .get_mut(&1)
                .expect("the crate has std")
                .path = PathBuf::new();
            assert_eq!(parse(&json).expect("parses").krate, expected);
        }
    }

    #[test]
    fn format_54_gets_macro_export_as_its_own_variant() {
        let mut item = module(0);
        item.attrs = vec![
            Attribute::Other("#[macro_export]".to_owned()),
            Attribute::Other("#[inline]".to_owned()),
        ];
        for (format_version, first) in [
            (54, Attribute::MacroExport),
            (55, Attribute::Other("#[macro_export]".to_owned())),
        ] {
            let json = json_of(&krate(format_version, vec![item.clone()]), |value| {
                if let Some(external_crate) = value["external_crates"]["1"].as_object_mut() {
                    external_crate.remove("path");
                }
            });
            let parsed = parse(&json).expect("parses").krate;
            assert_eq!(parsed.index[&Id(0)].attrs, [
                first,
                Attribute::Other("#[inline]".to_owned())
            ]);
        }
    }

    #[test]
    fn older_formats_are_unsupported() {
        let json = serde_json::to_vec(&krate(53, vec![module(0)])).expect("serializes");
        assert!(matches!(
            parse(&json),
            Err(ParseError::UnsupportedFormat { found: 53, .. })
        ));
    }

    #[test]
    fn a_newer_format_is_read_and_flagged_if_it_fits() {
        let newer = FORMAT_VERSION + 1;
        let json = serde_json::to_vec(&krate(newer, vec![module(0)])).expect("serializes");
        assert!(parse(&json).expect("parses").newer_than_supported);
        let json = json_of(&krate(newer, vec![module(0)]), |value| {
            if let Some(object) = value.as_object_mut() {
                object.remove("root");
            }
        });
        assert!(matches!(
            parse(&json),
            Err(ParseError::UnsupportedFormat { found, .. }) if found == newer
        ));
    }

    #[test]
    fn deep_nesting_is_parsed() {
        let deep = (0..300).fold(Type::Primitive("u8".to_owned()), |inner, _| {
            Type::Slice(Box::new(inner))
        });
        let alias = item(
            1,
            ItemEnum::TypeAlias(TypeAlias {
                type_: deep,
                generics: Generics {
                    params: Vec::new(),
                    where_predicates: Vec::new(),
                },
            }),
        );
        let expected = krate(FORMAT_VERSION, vec![module(0), alias]);
        let json = serde_json::to_vec(&expected).expect("serializes");
        assert_eq!(parse(&json).expect("parses").krate, expected);
    }
}
