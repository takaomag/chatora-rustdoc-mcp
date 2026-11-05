//! The reverse index of a crate (requirement R-TI-4, decided in M4a, decision 6).
//!
//! It holds the crate's impls by the type they are for, the implementations of each trait, and the
//! owner of each associated item, variant and field.
//!
//! rustdoc's JSON tells each type its impls and each trait of the crate its implementations, but
//! not the implementations in this crate of another crate's traits, which a lookup across the
//! dependency graph needs (requirement R-TI-3), nor the impls of primitive and unnamed types,
//! which are found otherwise by going through every impl. Traits and types are keyed by their
//! definition path, as the crate's `paths` writes it (`tower_service::Service`), which is the same
//! in the JSON of the crate that defines them.

use std::collections::HashMap;

use rustdoc_types::{Crate, Id, ItemEnum, StructKind, Type, VariantKind};

use super::view;

/// The reverse index of one crate. It is archived with the crate (requirement R-PF-2).
#[derive(Debug, Default, rkyv::Archive, rkyv::Serialize)]
pub struct ReverseIndex {
    /// The impls of this crate, by what they are for: the definition path of a named type, or
    /// the key of a primitive or unnamed one ([`type_key`]).
    impls_by_type: HashMap<Box<str>, Box<[TypeImpl]>>,
    /// The impls of this crate of each trait, by the trait's definition path: negative ones too,
    /// but not the copies of blanket impls that rustdoc writes for each type, nor the impls of
    /// auto traits that the compiler makes (decided in M4a, decision 6), nor those that rustdoc
    /// inlined from another crate.
    implementations_of: HashMap<Box<str>, Box<[TraitImpl]>>,
    /// The impl, trait, enum, struct, union or variant that has each associated item, variant or
    /// field.
    parent_of: HashMap<Id, Id>,
}

/// An impl in [`ReverseIndex`]'s impls by type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize)]
#[rkyv(derive(Debug))]
pub struct TypeImpl {
    /// The impl.
    pub id: Id,
    /// What kind of impl it is.
    pub kind: ImplKind,
    /// It is for a reference to the type (`impl Read for &File`), which rustdoc tells the type
    /// too: it is under the key of the type as well as under `&`.
    pub by_ref: bool,
    /// rustdoc inlined it from another crate, as std does core's impls of primitive types.
    pub inlined: bool,
}

/// An impl in [`ReverseIndex`]'s implementations of a trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize)]
#[rkyv(derive(Debug))]
pub struct TraitImpl {
    /// The impl.
    pub id: Id,
    /// The crate of the trait, by its id in the crate's `external_crates` (0 for this crate):
    /// which version of the trait's crate it is, when the crate depends on two (its
    /// `html_root_url`). `None` when the crate's `paths` do not have the trait.
    pub trait_crate: Option<u32>,
}

/// What kind of impl an impl is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize)]
#[rkyv(derive(Debug, Clone, Copy, PartialEq, Eq))]
pub enum ImplKind {
    /// An impl without a trait.
    Inherent,
    /// An impl of a trait, written for the type.
    Trait,
    /// A copy, for the type, of a blanket impl (`impl<T> Trait for T`).
    Blanket,
    /// An impl of an auto trait that the compiler makes.
    Synthetic,
}

impl ReverseIndex {
    /// Indexes the impls and the items of `krate`.
    #[must_use]
    pub fn build(krate: &Crate) -> Self {
        let mut impls_by_type: HashMap<Box<str>, Vec<TypeImpl>> = HashMap::new();
        let mut implementations_of: HashMap<Box<str>, Vec<TraitImpl>> = HashMap::new();
        let mut parent_of = HashMap::new();
        let mut owns = |owner: Id, items: &[Id]| {
            for &item in items {
                parent_of.insert(item, owner);
            }
        };
        let mut ids: Vec<&Id> = krate.index.keys().collect();
        // In the order of the ids, for the same index from the same JSON.
        ids.sort_unstable_by_key(|id| id.0);
        for &id in ids {
            let Some(item) = krate.index.get(&id) else {
                continue;
            };
            match &item.inner {
                ItemEnum::Impl(found) => {
                    let kind = if found.is_synthetic {
                        ImplKind::Synthetic
                    } else if found.blanket_impl.is_some() {
                        ImplKind::Blanket
                    } else if found.trait_.is_some() {
                        ImplKind::Trait
                    } else {
                        ImplKind::Inherent
                    };
                    // The copies of a blanket impl share its items, whose owner is the impl
                    // written.
                    if matches!(kind, ImplKind::Inherent | ImplKind::Trait) {
                        owns(id, &found.items);
                    }
                    let inlined = item.crate_id != 0;
                    let mut by_type = |key: String, by_ref: bool| {
                        impls_by_type.entry(key.into()).or_default().push(TypeImpl {
                            id,
                            kind,
                            by_ref,
                            inlined,
                        });
                    };
                    by_type(type_key(krate, &found.for_), false);
                    if let Type::BorrowedRef { type_, .. } = &found.for_ {
                        by_type(type_key(krate, type_), true);
                    }
                    if kind == ImplKind::Trait
                        && !inlined
                        && let Some(path) = &found.trait_
                    {
                        implementations_of
                            .entry(definition_path(krate, path.id, &path.path).into())
                            .or_default()
                            .push(TraitImpl {
                                id,
                                trait_crate: krate.paths.get(&path.id).map(|found| found.crate_id),
                            });
                    }
                },
                ItemEnum::Trait(found) => owns(id, &found.items),
                ItemEnum::Enum(found) => owns(id, &found.variants),
                ItemEnum::Struct(found) => match &found.kind {
                    StructKind::Plain { fields, .. } => owns(id, fields),
                    StructKind::Tuple(fields) => {
                        owns(id, &fields.iter().flatten().copied().collect::<Vec<_>>());
                    },
                    StructKind::Unit => {},
                },
                ItemEnum::Union(found) => owns(id, &found.fields),
                ItemEnum::Variant(found) => match &found.kind {
                    VariantKind::Struct { fields, .. } => owns(id, fields),
                    VariantKind::Tuple(fields) => {
                        owns(id, &fields.iter().flatten().copied().collect::<Vec<_>>());
                    },
                    VariantKind::Plain => {},
                },
                ItemEnum::Module(_)
                | ItemEnum::ExternCrate { .. }
                | ItemEnum::Use(_)
                | ItemEnum::StructField(_)
                | ItemEnum::Function(_)
                | ItemEnum::TraitAlias(_)
                | ItemEnum::TypeAlias(_)
                | ItemEnum::Constant { .. }
                | ItemEnum::Static(_)
                | ItemEnum::ExternType
                | ItemEnum::Macro(_)
                | ItemEnum::ProcMacro(_)
                | ItemEnum::Primitive(_)
                | ItemEnum::AssocConst { .. }
                | ItemEnum::AssocType { .. } => {},
            }
        }
        Self {
            impls_by_type: impls_by_type
                .into_iter()
                .map(|(key, impls)| (key, impls.into()))
                .collect(),
            implementations_of: implementations_of
                .into_iter()
                .map(|(key, impls)| (key, impls.into()))
                .collect(),
            parent_of,
        }
    }
}

impl ArchivedReverseIndex {
    /// The impls of this crate for the type keyed `key` ([`type_key`]), in the order of their ids.
    #[must_use]
    pub fn impls_for(&self, key: &str) -> Vec<TypeImpl> {
        self.impls_by_type
            .get(key)
            .map(|impls| {
                impls
                    .iter()
                    .map(|found| TypeImpl {
                        id: view::native(&found.id),
                        kind: match found.kind {
                            ArchivedImplKind::Inherent => ImplKind::Inherent,
                            ArchivedImplKind::Trait => ImplKind::Trait,
                            ArchivedImplKind::Blanket => ImplKind::Blanket,
                            ArchivedImplKind::Synthetic => ImplKind::Synthetic,
                        },
                        by_ref: found.by_ref,
                        inlined: found.inlined,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The impls of this crate of the trait at the definition path `trait_path`
    /// (`tower_service::Service`), negative ones too, in the order of their ids.
    #[must_use]
    pub fn implementations_of(&self, trait_path: &str) -> Vec<TraitImpl> {
        self.implementations_of
            .get(trait_path)
            .map(|impls| {
                impls
                    .iter()
                    .map(|found| TraitImpl {
                        id: view::native(&found.id),
                        trait_crate: found.trait_crate.as_ref().map(|id| id.to_native()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The impl, trait, enum, struct, union or variant that has `id`, if it is an associated
    /// item, a variant or a field.
    #[must_use]
    pub fn parent_of(&self, id: Id) -> Option<Id> {
        self.parent_of.get(&view::archived_id(id)).map(view::native)
    }
}

/// The definition path of the item `id` that a path names, as the crate's `paths` writes it, the
/// crate's name first; or else the path as written.
fn definition_path(krate: &Crate, id: Id, written: &str) -> String {
    krate
        .paths
        .get(&id)
        .map_or_else(|| written.to_owned(), |summary| summary.path.join("::"))
}

/// The key of the type `ty` in [`ReverseIndex`]'s impls by type.
///
/// It is the definition path of a named type; the name of a primitive (`str`, `u8`); `[]` for a
/// slice, `[;]` for an array, `()` for the unit, `(,)` for another tuple, `&` for a reference
/// (whose impls are under the key of the type too), `*` for a raw pointer, `fn` for a function
/// pointer, `dyn` and the definition path of its first trait for a trait object (`dyn
/// core::any::Any`), `impl` for `impl Trait`, `<>` for a qualified path, and `_` for a generic
/// parameter or what is inferred, as for a blanket impl. A type alias is keyed by its own path,
/// not by the type it names.
#[must_use]
pub fn type_key(krate: &Crate, ty: &Type) -> String {
    match ty {
        Type::ResolvedPath(path) => definition_path(krate, path.id, &path.path),
        Type::Primitive(name) => name.clone(),
        Type::Slice(_) => "[]".to_owned(),
        Type::Array { .. } => "[;]".to_owned(),
        Type::Tuple(types) if types.is_empty() => "()".to_owned(),
        Type::Tuple(_) => "(,)".to_owned(),
        Type::BorrowedRef { .. } => "&".to_owned(),
        Type::RawPointer { .. } => "*".to_owned(),
        Type::FunctionPointer(_) => "fn".to_owned(),
        Type::DynTrait(found) => found.traits.first().map_or_else(
            || "dyn".to_owned(),
            |first| {
                format!(
                    "dyn {}",
                    definition_path(krate, first.trait_.id, &first.trait_.path)
                )
            },
        ),
        Type::ImplTrait(_) => "impl".to_owned(),
        Type::QualifiedPath { .. } => "<>".to_owned(),
        Type::Pat { type_, .. } => type_key(krate, type_),
        Type::Generic(_) | Type::Infer => "_".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use rustdoc_types::{
        DynTrait,
        Generics,
        Impl,
        Item,
        ItemKind,
        ItemSummary,
        Path,
        PolyTrait,
        Visibility,
    };

    use super::*;
    use crate::query::{
        get_item::tests::parsed,
        view::tests::{archive, archived},
    };

    fn bare(id: u32, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: None,
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

    fn path(id: u32, name: &str) -> Path {
        Path {
            path: name.to_owned(),
            id: Id(id),
            args: None,
        }
    }

    fn an_impl(id: u32, trait_: Option<Path>, for_: Type, change: impl FnOnce(&mut Impl)) -> Item {
        let mut found = Impl {
            is_unsafe: false,
            generics: Generics {
                params: Vec::new(),
                where_predicates: Vec::new(),
            },
            provided_trait_methods: Vec::new(),
            trait_,
            for_,
            items: Vec::new(),
            is_negative: false,
            is_synthetic: false,
            blanket_impl: None,
        };
        change(&mut found);
        bare(id, ItemEnum::Impl(found))
    }

    /// The test crate of `get_item` with the trait `Lock` (its own, at `demo::Lock`) and these
    /// impls: `Lock` for `Mutex` (31, with the item 39), a copy for `Mutex` of a blanket impl of
    /// `Lock` (32, which shares the item 39), `Send` for `Mutex` that the compiler made (33), a
    /// negative impl of another crate's `tower_service::Service` for `Mutex` (34), an inherent
    /// impl for `str` (35), `Lock` for `&Mutex` (36), an inherent impl for `dyn Lock` (37), and an
    /// impl of `Lock` for `u8` that rustdoc inlined from another crate (38).
    fn with_impls() -> Crate {
        let mut krate = parsed().krate;
        let summary = |crate_id, path: &[&str]| ItemSummary {
            crate_id,
            path: path.iter().map(|&part| part.to_owned()).collect(),
            kind: ItemKind::Trait,
        };
        krate.paths.insert(Id(30), summary(0, &["demo", "Lock"]));
        krate
            .paths
            .insert(Id(40), summary(1, &["core", "marker", "Send"]));
        krate
            .paths
            .insert(Id(41), summary(2, &["tower_service", "Service"]));
        let mutex = || Type::ResolvedPath(path(3, "Mutex"));
        for item in [
            an_impl(31, Some(path(30, "Lock")), mutex(), |found| {
                found.items = vec![Id(39)];
            }),
            an_impl(32, Some(path(30, "Lock")), mutex(), |found| {
                found.blanket_impl = Some(Type::Generic("T".to_owned()));
                found.items = vec![Id(39)];
            }),
            an_impl(33, Some(path(40, "Send")), mutex(), |found| {
                found.is_synthetic = true;
            }),
            an_impl(34, Some(path(41, "Service")), mutex(), |found| {
                found.is_negative = true;
            }),
            an_impl(35, None, Type::Primitive("str".to_owned()), |_| {}),
            an_impl(
                36,
                Some(path(30, "Lock")),
                Type::BorrowedRef {
                    lifetime: None,
                    is_mutable: false,
                    type_: Box::new(mutex()),
                },
                |_| {},
            ),
            an_impl(
                37,
                None,
                Type::DynTrait(DynTrait {
                    traits: vec![PolyTrait {
                        trait_: path(30, "Lock"),
                        generic_params: Vec::new(),
                    }],
                    lifetime: None,
                }),
                |_| {},
            ),
            Item {
                crate_id: 3,
                ..an_impl(
                    38,
                    Some(path(30, "Lock")),
                    Type::Primitive("u8".to_owned()),
                    |_| {},
                )
            },
        ] {
            krate.index.insert(item.id, item);
        }
        krate
    }

    fn ids(impls: &[TraitImpl]) -> Vec<u32> {
        impls.iter().map(|found| found.id.0).collect()
    }

    /// The implementations of a trait are found by its definition path, its own or another
    /// crate's, negative ones too, but not the copies of blanket impls nor what the compiler made;
    /// the impls by type have them all, marked; and an impl's item has the impl as its parent
    /// (decided in M4a, decision 6).
    #[test]
    fn impls_are_found_by_trait_and_by_type() {
        let krate = with_impls();
        let mutex = type_key(&krate, &Type::ResolvedPath(path(3, "Mutex")));
        let bytes = archive(krate);
        let reverse = &archived(&bytes).reverse;
        // Not the copy of the blanket impl (32), nor the impl inlined from another crate (38).
        assert_eq!(ids(&reverse.implementations_of("demo::Lock")), [31, 36]);
        assert_eq!(reverse.implementations_of("tower_service::Service"), [
            TraitImpl {
                id: Id(34),
                trait_crate: Some(2),
            }
        ]);
        assert_eq!(
            ids(&reverse.implementations_of("core::marker::Send")),
            Vec::<u32>::new()
        );
        assert_eq!(ids(&reverse.implementations_of("Lock")), Vec::<u32>::new());
        let kinds: Vec<(u32, ImplKind)> = reverse
            .impls_for(&mutex)
            .iter()
            .map(|found| (found.id.0, found.kind))
            .collect();
        // The crate's own inherent impl of `Mutex` (5) first, and the impl for `&Mutex` too.
        assert_eq!(kinds, [
            (5, ImplKind::Inherent),
            (31, ImplKind::Trait),
            (32, ImplKind::Blanket),
            (33, ImplKind::Synthetic),
            (34, ImplKind::Trait),
            (36, ImplKind::Trait),
        ]);
        let by_ref: Vec<bool> = reverse
            .impls_for(&mutex)
            .iter()
            .map(|found| found.by_ref)
            .collect();
        assert_eq!(by_ref, [false, false, false, false, false, true]);
        assert_eq!(reverse.impls_for("&"), [TypeImpl {
            id: Id(36),
            kind: ImplKind::Trait,
            by_ref: false,
            inlined: false,
        }]);
        assert_eq!(reverse.impls_for("str"), [TypeImpl {
            id: Id(35),
            kind: ImplKind::Inherent,
            by_ref: false,
            inlined: false,
        }]);
        assert_eq!(reverse.impls_for("dyn demo::Lock"), [TypeImpl {
            id: Id(37),
            kind: ImplKind::Inherent,
            by_ref: false,
            inlined: false,
        }]);
        assert_eq!(reverse.impls_for("u8"), [TypeImpl {
            id: Id(38),
            kind: ImplKind::Trait,
            by_ref: false,
            inlined: true,
        }]);
        // The item that the copy of the blanket impl shares is the impl written's.
        assert_eq!(reverse.parent_of(Id(39)), Some(Id(31)));
        assert_eq!(reverse.parent_of(Id(31)), None);
    }

    /// A variant's owner is its enum, and a field's its struct or variant.
    #[test]
    fn variants_and_fields_have_their_owners() {
        let krate = parsed().krate;
        let mut owned = Vec::new();
        let mut ids: Vec<&Id> = krate.index.keys().collect();
        ids.sort_unstable_by_key(|id| id.0);
        for &id in ids {
            let Some(item) = krate.index.get(&id) else {
                continue;
            };
            if let ItemEnum::Enum(found) = &item.inner {
                owned.extend(found.variants.iter().map(|&of| (of, id)));
            } else if let ItemEnum::Struct(found) = &item.inner
                && let StructKind::Plain { fields, .. } = &found.kind
            {
                owned.extend(fields.iter().map(|&of| (of, id)));
            }
        }
        assert!(!owned.is_empty(), "the test crate has an enum");
        let bytes = archive(krate);
        let reverse = &archived(&bytes).reverse;
        for (item, owner) in owned {
            assert_eq!(reverse.parent_of(item), Some(owner), "{item:?}");
        }
    }

    /// The keys of types that have no path of their own.
    #[test]
    fn unnamed_types_have_keys_of_their_shape() {
        let krate = parsed().krate;
        let str_type = || Box::new(Type::Primitive("str".to_owned()));
        for (ty, key) in [
            (Type::Primitive("u8".to_owned()), "u8"),
            (Type::Slice(str_type()), "[]"),
            (
                Type::Array {
                    type_: str_type(),
                    len: "3".to_owned(),
                },
                "[;]",
            ),
            (Type::Tuple(Vec::new()), "()"),
            (Type::Tuple(vec![*str_type(), *str_type()]), "(,)"),
            (
                Type::BorrowedRef {
                    lifetime: None,
                    is_mutable: false,
                    type_: str_type(),
                },
                "&",
            ),
            (Type::Generic("T".to_owned()), "_"),
        ] {
            assert_eq!(type_key(&krate, &ty), key, "{ty:?}");
        }
    }
}
