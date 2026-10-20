//! The conditions of compilation (`cfg`) of items, as rustdoc JSON keeps them in attributes
//! (requirement R-FT-2).
//!
//! rustdoc JSON has no field for them. They are in the text of `Attribute::Other`, in one of two
//! forms, told apart by the text rather than the format version (requirement R-FT-2):
//!
//! - `#[doc(cfg(...))]`, which the crate writes for its documentation, and the traces of `#[cfg]`
//!   in formats 46 to 53, `#[<cfg>(...)]`, are in the syntax of Rust.
//! - The traces in later formats are rustc's debug output, with positions in the source:
//!   `#[attr = CfgTrace([NameValue { name: "feature", value: Some("rt"), span: … }])]`.
//!
//! The traces of `#[cfg_attr]` (`CfgAttrTrace`, `<cfg_attr>`) are conditions on other attributes,
//! not on the item, and are not read.

use std::fmt;

/// A condition of compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cfg {
    /// `true` or `false`.
    Bool(bool),
    /// A name alone: `unix`, `docsrs`.
    Name(Box<str>),
    /// A name with a value: `feature = "rt"`.
    NameValue(Box<str>, Box<str>),
    /// `all(...)`.
    All(Vec<Self>),
    /// `any(...)`.
    Any(Vec<Self>),
    /// `not(...)`.
    Not(Box<Self>),
}

/// Where a condition was read from, for `cfg_source` (requirement R-FT-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgSource {
    /// `#[doc(cfg(...))]`.
    DocCfg,
    /// A trace of `#[cfg(...)]`.
    CfgTrace,
}

/// What an attribute says about the conditions of its item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfgAttribute {
    /// A condition, from `source`.
    Cfg(CfgSource, Cfg),
    /// An attribute about conditions that could not be read.
    Unreadable,
}

impl Cfg {
    /// The conditions of the attribute `text`, or `None` if it is not about them.
    #[must_use]
    pub fn of_attribute(text: &str) -> Option<CfgAttribute> {
        let text = text.trim();
        let read = |cfg: Option<Self>, source| {
            Some(cfg.map_or(CfgAttribute::Unreadable, |cfg| {
                CfgAttribute::Cfg(source, cfg)
            }))
        };
        if let Some(rest) = text.strip_prefix("#[attr = CfgTrace(") {
            let inner = rest.strip_suffix(")]")?;
            return read(trace::parse(inner), CfgSource::CfgTrace);
        }
        for prefix in ["#[<cfg>", "#[<cfg_trace>", "#[cfg"] {
            if let Some(rest) = text.strip_prefix(prefix)
                && rest.trim_start().starts_with('(')
            {
                let inner = rest.strip_suffix(']')?;
                return read(
                    rust::parse(inner).and_then(|meta| conjunction(&meta)),
                    CfgSource::CfgTrace,
                );
            }
        }
        if let Some(rest) = text.strip_prefix("#[doc")
            && rest.trim_start().starts_with('(')
        {
            let inner = rest.strip_suffix(']')?;
            let metas = rust::parse(inner)?;
            let cfgs: Vec<_> = metas
                .iter()
                .filter_map(|meta| match meta {
                    rust::Meta::List(name, list) if &**name == "cfg" => Some(list),
                    rust::Meta::List(..) | rust::Meta::Word(_) | rust::Meta::NameValue(..) => None,
                })
                .collect();
            if cfgs.is_empty() {
                return None;
            }
            let cfg = cfgs
                .into_iter()
                .map(|list| conjunction(list))
                .collect::<Option<Vec<_>>>()
                .map(Self::all);
            return read(cfg, CfgSource::DocCfg);
        }
        None
    }

    /// `all` of `cfgs`, or the one condition if there is one.
    #[must_use]
    pub fn all(mut cfgs: Vec<Self>) -> Self {
        if cfgs.len() == 1
            && let Some(cfg) = cfgs.pop()
        {
            return cfg;
        }
        Self::All(cfgs)
    }

    /// Whether every name in it is `feature`.
    #[must_use]
    pub fn is_features_only(&self) -> bool {
        match self {
            Self::Bool(_) => true,
            Self::Name(_) => false,
            Self::NameValue(name, _) => &**name == "feature",
            Self::All(cfgs) | Self::Any(cfgs) => cfgs.iter().all(Self::is_features_only),
            Self::Not(cfg) => cfg.is_features_only(),
        }
    }

    /// Whether it mentions no `feature`.
    #[must_use]
    pub fn has_no_features(&self) -> bool {
        match self {
            Self::Bool(_) | Self::Name(_) => true,
            Self::NameValue(name, _) => &**name != "feature",
            Self::All(cfgs) | Self::Any(cfgs) => cfgs.iter().all(Self::has_no_features),
            Self::Not(cfg) => cfg.has_no_features(),
        }
    }

    /// The condition in a normal build, where `doc` and `docsrs` are false (docs.rs sets them
    /// only to document). If it is then false, the condition is kept as it is, so that it shows
    /// that the item is only in documentation.
    #[must_use]
    pub fn for_build(self) -> Self {
        let folded = self.clone().fold();
        if folded == Self::Bool(false) {
            self
        } else {
            folded
        }
    }

    /// Folds `doc` and `docsrs` to false, and the constants away.
    fn fold(self) -> Self {
        match self {
            Self::Name(name) if matches!(&*name, "doc" | "docsrs") => Self::Bool(false),
            Self::Bool(_) | Self::Name(_) | Self::NameValue(..) => self,
            Self::Not(cfg) => {
                let cfg = cfg.fold();
                if let Self::Bool(value) = cfg {
                    Self::Bool(!value)
                } else {
                    Self::Not(Box::new(cfg))
                }
            },
            Self::All(cfgs) => {
                let mut kept = Vec::new();
                for cfg in cfgs.into_iter().map(Self::fold) {
                    match cfg {
                        Self::Bool(false) => return Self::Bool(false),
                        Self::Bool(true) => {},
                        cfg @ (Self::Name(_)
                        | Self::NameValue(..)
                        | Self::All(_)
                        | Self::Any(_)
                        | Self::Not(_)) => kept.push(cfg),
                    }
                }
                if kept.is_empty() {
                    Self::Bool(true)
                } else {
                    Self::all(kept)
                }
            },
            Self::Any(cfgs) => {
                let mut kept = Vec::new();
                for cfg in cfgs.into_iter().map(Self::fold) {
                    match cfg {
                        Self::Bool(true) => return Self::Bool(true),
                        Self::Bool(false) => {},
                        cfg @ (Self::Name(_)
                        | Self::NameValue(..)
                        | Self::All(_)
                        | Self::Any(_)
                        | Self::Not(_)) => kept.push(cfg),
                    }
                }
                match kept.len() {
                    0 => Self::Bool(false),
                    1 => kept.pop().unwrap_or(Self::Bool(false)),
                    _ => Self::Any(kept),
                }
            },
        }
    }

    /// The conditions that must all hold: those of a top-level `all`, or the condition itself.
    #[must_use]
    pub fn conjuncts(self) -> Vec<Self> {
        match self {
            Self::All(cfgs) => cfgs.into_iter().flat_map(Self::conjuncts).collect(),
            Self::Bool(true) => Vec::new(),
            cfg @ (Self::Bool(false)
            | Self::Name(_)
            | Self::NameValue(..)
            | Self::Any(_)
            | Self::Not(_)) => vec![cfg],
        }
    }

    /// The condition with only feature names, as in `any(rt, rt-multi-thread)`, for a condition
    /// that [`Self::is_features_only`].
    #[must_use]
    pub fn features_text(&self) -> String {
        match self {
            Self::NameValue(_, value) => value.to_string(),
            Self::All(cfgs) => format!("all({})", features_list(cfgs)),
            Self::Any(cfgs) => format!("any({})", features_list(cfgs)),
            Self::Not(cfg) => format!("not({})", cfg.features_text()),
            Self::Bool(_) | Self::Name(_) => self.to_string(),
        }
    }

    /// The value of the condition, given the value of each name (`None` when not known), in
    /// the logic of three values: `all` is false if one is false, `any` true if one is true.
    #[must_use]
    pub fn eval(&self, value_of: &impl Fn(&Self) -> Option<bool>) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Name(_) | Self::NameValue(..) => value_of(self),
            Self::Not(cfg) => cfg.eval(value_of).map(|value| !value),
            Self::All(cfgs) => {
                let mut known = Some(true);
                for cfg in cfgs {
                    match cfg.eval(value_of) {
                        Some(false) => return Some(false),
                        Some(true) => {},
                        None => known = None,
                    }
                }
                known
            },
            Self::Any(cfgs) => {
                let mut known = Some(false);
                for cfg in cfgs {
                    match cfg.eval(value_of) {
                        Some(true) => return Some(true),
                        Some(false) => {},
                        None => known = None,
                    }
                }
                known
            },
        }
    }
}

/// `cfgs` as features, separated by commas.
fn features_list(cfgs: &[Cfg]) -> String {
    cfgs.iter()
        .map(Cfg::features_text)
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for Cfg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, name: &str, cfgs: &[Self]| {
            write!(f, "{name}(")?;
            for (position, cfg) in cfgs.iter().enumerate() {
                if position > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{cfg}")?;
            }
            f.write_str(")")
        };
        match self {
            Self::Bool(value) => write!(f, "{value}"),
            Self::Name(name) => f.write_str(name),
            Self::NameValue(name, value) => write!(f, "{name} = {value:?}"),
            Self::All(cfgs) => list(f, "all", cfgs),
            Self::Any(cfgs) => list(f, "any", cfgs),
            Self::Not(cfg) => write!(f, "not({cfg})"),
        }
    }
}

/// The predicates of `metas`, which must all hold: `cfg(a, b)` is `all(a, b)`.
fn conjunction(metas: &[rust::Meta]) -> Option<Cfg> {
    metas
        .iter()
        .map(predicate)
        .collect::<Option<Vec<_>>>()
        .map(Cfg::all)
}

/// The predicate that `meta` writes.
fn predicate(meta: &rust::Meta) -> Option<Cfg> {
    match meta {
        rust::Meta::Word(name) => Some(match &**name {
            "true" => Cfg::Bool(true),
            "false" => Cfg::Bool(false),
            _ => Cfg::Name(name.clone()),
        }),
        rust::Meta::NameValue(name, value) => Some(Cfg::NameValue(name.clone(), value.clone())),
        rust::Meta::List(name, list) => {
            let cfgs = list.iter().map(predicate).collect::<Option<Vec<_>>>()?;
            match &**name {
                "all" => Some(Cfg::All(cfgs)),
                "any" => Some(Cfg::Any(cfgs)),
                "not" => match <[Cfg; 1]>::try_from(cfgs) {
                    Ok([cfg]) => Some(Cfg::Not(Box::new(cfg))),
                    Err(_) => None,
                },
                _ => None,
            }
        },
    }
}

/// Attributes in the syntax of Rust: `(feature = "rt", not(unix))`.
mod rust {
    use std::{iter::Peekable, str::CharIndices};

    /// An item of an attribute's list.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum Meta {
        /// `unix`.
        Word(Box<str>),
        /// `feature = "rt"`.
        NameValue(Box<str>, Box<str>),
        /// `all(...)`.
        List(Box<str>, Vec<Self>),
    }

    /// Parses `text`, a list in parentheses, or `None` if it is not one.
    pub(super) fn parse(text: &str) -> Option<Vec<Meta>> {
        let mut parser = Parser {
            chars: text.char_indices().peekable(),
            text,
        };
        let list = parser.list()?;
        parser.skip_whitespace();
        parser.chars.peek().is_none().then_some(list)
    }

    struct Parser<'a> {
        chars: Peekable<CharIndices<'a>>,
        text: &'a str,
    }

    impl Parser<'_> {
        fn skip_whitespace(&mut self) {
            while self.chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        }

        fn eat(&mut self, expected: char) -> bool {
            self.skip_whitespace();
            self.chars.next_if(|&(_, c)| c == expected).is_some()
        }

        /// `(meta, meta, …)`, with an optional trailing comma.
        fn list(&mut self) -> Option<Vec<Meta>> {
            if !self.eat('(') {
                return None;
            }
            let mut metas = Vec::new();
            loop {
                if self.eat(')') {
                    return Some(metas);
                }
                metas.push(self.meta()?);
                if !self.eat(',') {
                    return self.eat(')').then_some(metas);
                }
            }
        }

        fn meta(&mut self) -> Option<Meta> {
            let name = self.word()?;
            self.skip_whitespace();
            match self.chars.peek() {
                Some((_, '(')) => Some(Meta::List(name, self.list()?)),
                Some((_, '=')) => {
                    self.chars.next();
                    Some(Meta::NameValue(name, self.string()?))
                },
                _ => Some(Meta::Word(name)),
            }
        }

        fn word(&mut self) -> Option<Box<str>> {
            self.skip_whitespace();
            let &(start, first) = self.chars.peek()?;
            if !(first.is_alphabetic() || first == '_') {
                return None;
            }
            let mut end = start;
            while let Some((at, c)) = self
                .chars
                .next_if(|&(_, c)| c.is_alphanumeric() || c == '_' || c == '#')
            {
                end = at.saturating_add(c.len_utf8());
            }
            let word = self.text.get(start..end)?;
            Some(word.strip_prefix("r#").unwrap_or(word).into())
        }

        /// A string literal, with its escapes.
        fn string(&mut self) -> Option<Box<str>> {
            if !self.eat('"') {
                return None;
            }
            let mut value = String::new();
            loop {
                match self.chars.next()?.1 {
                    '"' => return Some(value.into()),
                    '\\' => value.push(match self.chars.next()?.1 {
                        'n' => '\n',
                        't' => '\t',
                        c => c,
                    }),
                    c => value.push(c),
                }
            }
        }
    }
}

/// The debug output of rustc's `CfgEntry`, in the traces of later formats:
/// `[NameValue { name: "feature", value: Some("rt"), span: … }, All([…], span), Not(…, span)]`.
mod trace {
    use std::{iter::Peekable, str::Chars};

    use super::Cfg;

    /// Parses the list of entries `text` (`[…]`), which must all hold.
    pub(super) fn parse(text: &str) -> Option<Cfg> {
        let mut parser = Parser {
            chars: text.chars().peekable(),
        };
        let cfgs = parser.list()?;
        parser.skip_whitespace();
        parser.chars.peek().is_none().then(|| Cfg::all(cfgs))
    }

    struct Parser<'a> {
        chars: Peekable<Chars<'a>>,
    }

    impl Parser<'_> {
        fn skip_whitespace(&mut self) {
            while self.chars.next_if(|c| c.is_whitespace()).is_some() {}
        }

        fn eat(&mut self, expected: char) -> bool {
            self.skip_whitespace();
            self.chars.next_if_eq(&expected).is_some()
        }

        fn eat_word(&mut self, expected: &str) -> bool {
            self.word().as_deref() == Some(expected)
        }

        fn word(&mut self) -> Option<String> {
            self.skip_whitespace();
            let mut word = String::new();
            while let Some(c) = self.chars.next_if(|&c| c.is_alphanumeric() || c == '_') {
                word.push(c);
            }
            (!word.is_empty()).then_some(word)
        }

        /// `[entry, entry, …]`.
        fn list(&mut self) -> Option<Vec<Cfg>> {
            if !self.eat('[') {
                return None;
            }
            let mut cfgs = Vec::new();
            loop {
                if self.eat(']') {
                    return Some(cfgs);
                }
                cfgs.push(self.entry()?);
                if !self.eat(',') {
                    return self.eat(']').then_some(cfgs);
                }
            }
        }

        fn entry(&mut self) -> Option<Cfg> {
            match self.word()?.as_str() {
                "NameValue" => self.name_value(),
                "All" => self.combined(Cfg::All),
                "Any" => self.combined(Cfg::Any),
                "Not" => {
                    if !self.eat('(') {
                        return None;
                    }
                    let cfg = self.entry()?;
                    self.close(')')?;
                    Some(Cfg::Not(Box::new(cfg)))
                },
                "Bool" => {
                    if !self.eat('(') {
                        return None;
                    }
                    let value = match self.word()?.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => return None,
                    };
                    self.close(')')?;
                    Some(Cfg::Bool(value))
                },
                _ => None,
            }
        }

        /// `All([…], span)` or `Any([…], span)`.
        fn combined(&mut self, make: fn(Vec<Cfg>) -> Cfg) -> Option<Cfg> {
            if !self.eat('(') {
                return None;
            }
            let cfgs = self.list()?;
            self.close(')')?;
            Some(make(cfgs))
        }

        /// `{ name: "feature", value: Some("rt"), span: … }`.
        fn name_value(&mut self) -> Option<Cfg> {
            if !(self.eat('{') && self.eat_word("name") && self.eat(':')) {
                return None;
            }
            let name = self.string_or_word()?;
            if !(self.eat(',') && self.eat_word("value") && self.eat(':')) {
                return None;
            }
            let value = match self.word()?.as_str() {
                "None" => None,
                "Some" => {
                    if !self.eat('(') {
                        return None;
                    }
                    let value = self.string_or_word()?;
                    if !self.eat(')') {
                        return None;
                    }
                    Some(value)
                },
                _ => return None,
            };
            self.close('}')?;
            Some(value.map_or_else(
                || Cfg::Name(name.as_str().into()),
                |value| Cfg::NameValue(name.as_str().into(), value.into()),
            ))
        }

        /// Skips what is left before `close` (a span, as `, span: src/lib.rs:1:2: 1:9 (#0)`),
        /// then `close` itself.
        fn close(&mut self, close: char) -> Option<()> {
            let mut depth = 0_usize;
            loop {
                let c = self.chars.next()?;
                match c {
                    '(' | '[' | '{' => depth = depth.saturating_add(1),
                    ')' | ']' | '}' if depth == 0 => return (c == close).then_some(()),
                    ')' | ']' | '}' => depth = depth.saturating_sub(1),
                    '"' => self.skip_string()?,
                    _ => {},
                }
            }
        }

        fn skip_string(&mut self) -> Option<()> {
            loop {
                match self.chars.next()? {
                    '"' => return Some(()),
                    '\\' => {
                        self.chars.next()?;
                    },
                    _ => {},
                }
            }
        }

        /// `"feature"` with its escapes, or `feature` written without quotes.
        fn string_or_word(&mut self) -> Option<String> {
            if !self.eat('"') {
                return self.word();
            }
            let mut value = String::new();
            loop {
                match self.chars.next()? {
                    '"' => return Some(value),
                    '\\' => value.push(self.chars.next()?),
                    c => value.push(c),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<(CfgSource, String)> {
        match Cfg::of_attribute(text)? {
            CfgAttribute::Cfg(source, cfg) => Some((source, cfg.to_string())),
            CfgAttribute::Unreadable => Some((CfgSource::CfgTrace, "unreadable".to_owned())),
        }
    }

    #[test]
    fn doc_cfg_is_read_in_the_syntax_of_rust() {
        assert_eq!(
            read(r#"#[doc(cfg(feature = "rt"))]"#),
            Some((CfgSource::DocCfg, r#"feature = "rt""#.to_owned()))
        );
        assert_eq!(
            read("#[doc(cfg(all(reqwest_unstable, feature = \"http3\")))]"),
            Some((
                CfgSource::DocCfg,
                r#"all(reqwest_unstable, feature = "http3")"#.to_owned()
            ))
        );
        assert_eq!(read(r#"#[doc(alias = "lock")]"#), None);
        assert_eq!(read("#[must_use]"), None);
    }

    #[test]
    fn traces_of_old_formats_are_read() {
        assert_eq!(
            read("#[<cfg>(any(feature = \"derive\",\nfeature = \"full\"))]"),
            Some((
                CfgSource::CfgTrace,
                r#"any(feature = "derive", feature = "full")"#.to_owned()
            ))
        );
        assert_eq!(
            read("#[<cfg>(not(all(loom, test)))]"),
            Some((CfgSource::CfgTrace, "not(all(loom, test))".to_owned()))
        );
        // A condition on another attribute, not on the item.
        assert_eq!(read("#[<cfg_attr>(docsrs, doc(cfg(windows)))]"), None);
    }

    #[test]
    fn traces_of_later_formats_are_read_without_their_spans() {
        assert_eq!(
            read(
                r#"#[attr = CfgTrace([NameValue { name: "feature", value: Some("rt"), span: src/macros/cfg.rs:480:19: 480:33 (#4230) }])]"#
            ),
            Some((CfgSource::CfgTrace, r#"feature = "rt""#.to_owned()))
        );
        assert_eq!(
            read(
                r#"#[attr = CfgTrace([NameValue { name: "target_has_atomic", value: Some("ptr"), span: src/lib.rs:1:2: 1:9 (#0) },
NameValue { name: "feature", value: Some("alloc"), span: src/lib.rs:2:2: 2:9 (#0) }])]"#
            ),
            Some((
                CfgSource::CfgTrace,
                r#"all(target_has_atomic = "ptr", feature = "alloc")"#.to_owned()
            ))
        );
        assert_eq!(
            read(
                r#"#[attr = CfgTrace([Not(All([NameValue { name: "target_arch", value: Some("wasm32"), span: src/lib.rs:274:50: 274:60 (#37) }, Any([NameValue { name: "target_os", value: Some("unknown"), span: src/lib.rs:1:1: 1:2 (#37) }, NameValue { name: "unix", value: None, span: src/lib.rs:1:1: 1:2 (#37) }], src/lib.rs:274:50: 274:93 (#37))], src/lib.rs:274:22: 274:94 (#37)), src/lib.rs:274:18: 274:95 (#37))])]"#
            ),
            Some((
                CfgSource::CfgTrace,
                r#"not(all(target_arch = "wasm32", any(target_os = "unknown", unix)))"#.to_owned()
            ))
        );
        assert_eq!(read("#[attr = CfgAttrTrace]"), None);
        assert_eq!(
            read(r#"#[attr = CfgTrace([Version { since: "1.80" }])]"#),
            Some((CfgSource::CfgTrace, "unreadable".to_owned()))
        );
    }

    #[test]
    fn a_normal_build_has_no_doc_or_docsrs() {
        let cfg = |text: &str| match Cfg::of_attribute(text) {
            Some(CfgAttribute::Cfg(_, cfg)) => cfg,
            other => panic!("{text}: {other:?}"),
        };
        assert_eq!(
            cfg("#[<cfg>(any(all(doc, docsrs), windows))]")
                .for_build()
                .to_string(),
            "windows"
        );
        // An item only in the documentation keeps its condition.
        assert_eq!(cfg("#[<cfg>(docsrs)]").for_build().to_string(), "docsrs");
    }

    #[test]
    fn features_are_written_by_name_and_evaluated_in_three_values() {
        let cfg = Cfg::All(vec![
            Cfg::Any(vec![
                Cfg::NameValue("feature".into(), "rt".into()),
                Cfg::NameValue("feature".into(), "rt-multi-thread".into()),
            ]),
            Cfg::Name("tokio_unstable".into()),
        ]);
        let conjuncts = cfg.clone().conjuncts();
        assert!(conjuncts[0].is_features_only());
        assert_eq!(conjuncts[0].features_text(), "any(rt, rt-multi-thread)");
        assert!(conjuncts[1].has_no_features());
        let features = |enabled: &'static [&'static str]| {
            move |cfg: &Cfg| match cfg {
                Cfg::NameValue(name, value) if &**name == "feature" => {
                    Some(enabled.contains(&&**value))
                },
                Cfg::Bool(_) | Cfg::Name(_) | Cfg::NameValue(..) | Cfg::All(_) | Cfg::Any(_) => {
                    None
                },
                Cfg::Not(_) => None,
            }
        };
        assert_eq!(cfg.eval(&features(&[])), Some(false));
        assert_eq!(cfg.eval(&features(&["rt"])), None);
    }
}
