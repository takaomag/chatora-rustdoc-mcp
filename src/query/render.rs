//! Rendering rustdoc's type trees as Rust code (requirement R-OUT-2).
//!
//! A signature is shown as one line of Rust, not as the JSON tree, which is several times larger.
//! Types of this crate are written with their canonical path. Types of other crates are written as
//! rustdoc gives them until M1 indexes those crates too (R-PR-4). The where clauses, the named
//! fields of a struct and the variants of an enum are rendered separately, one string each
//! (R-OUT-2).

use std::borrow::Cow;

use rustdoc_types::{
    Abi,
    AssocItemConstraint,
    AssocItemConstraintKind,
    DynTrait,
    FunctionHeader,
    FunctionSignature,
    GenericArg,
    GenericArgs,
    GenericBound,
    GenericParamDef,
    GenericParamDefKind,
    Generics,
    Id,
    Impl,
    Item,
    ItemEnum,
    MacroKind,
    Path,
    PreciseCapturingArg,
    Static,
    StructKind,
    Term,
    Trait,
    TraitBoundModifier,
    Type,
    Use,
    Variant,
    VariantKind,
    Visibility,
    WherePredicate,
};

use super::view::CrateView;

/// Renders items and types of one crate.
#[derive(Debug)]
pub struct Renderer<'a> {
    view: &'a CrateView<'a>,
    /// Forms that could not be rendered faithfully (requirement R-OUT-2).
    warnings: Vec<Cow<'static, str>>,
}

impl<'a> Renderer<'a> {
    /// A renderer for the items of the crate in `view`.
    #[must_use]
    pub const fn new(view: &'a CrateView<'a>) -> Self {
        Self {
            view,
            warnings: Vec::new(),
        }
    }

    /// Whether the item `id` is restricted to the module that has it: private, as written without
    /// `pub` (decided in M2's D6).
    fn own_module(&self, id: Id) -> bool {
        self.view.index().is_module_private(id)
    }

    /// What could not be rendered faithfully so far.
    #[must_use]
    pub fn warnings(&self) -> &[Cow<'static, str>] {
        &self.warnings
    }

    /// What could not be rendered faithfully, taken out of the renderer.
    #[must_use]
    pub fn into_warnings(self) -> Vec<Cow<'static, str>> {
        self.warnings
    }

    /// The declaration of `item` as Rust, without its where clause, body, named fields or variants
    /// (the fields of a tuple struct or of a variant are part of it):
    /// `pub async fn lock(&self) -> MutexGuard<'_, T>`, `pub struct Mutex<T: ?Sized>`. A
    /// `macro_rules!` macro is shown with its rules, as rustdoc gives them.
    #[must_use]
    pub fn signature(&mut self, item: &Item) -> String {
        let mut out = String::new();
        let name = item.name.as_deref().unwrap_or("_");
        let visibility = &item.visibility;
        match &item.inner {
            ItemEnum::Module(_) => {
                push_visibility(&mut out, visibility, self.own_module(item.id));
                out.push_str("mod ");
                out.push_str(name);
            },
            ItemEnum::ExternCrate { name, rename } => {
                push_visibility(&mut out, visibility, self.own_module(item.id));
                out.push_str("extern crate ");
                out.push_str(name);
                if let Some(rename) = rename {
                    out.push_str(" as ");
                    out.push_str(rename);
                }
                out.push(';');
            },
            ItemEnum::Use(reexport) => {
                push_use(&mut out, visibility, self.own_module(item.id), reexport);
            },
            ItemEnum::Union(contents) => {
                self.push_type_header(&mut out, item, "union", &contents.generics);
            },
            ItemEnum::Struct(contents) => {
                self.push_type_header(&mut out, item, "struct", &contents.generics);
                match &contents.kind {
                    StructKind::Unit => out.push(';'),
                    StructKind::Tuple(fields) => {
                        self.push_tuple_fields(&mut out, fields);
                        out.push(';');
                    },
                    StructKind::Plain { .. } => {},
                }
            },
            ItemEnum::StructField(ty) => {
                push_visibility(&mut out, visibility, self.own_module(item.id));
                out.push_str(name);
                out.push_str(": ");
                self.push_type(&mut out, ty);
            },
            ItemEnum::Enum(contents) => {
                self.push_type_header(&mut out, item, "enum", &contents.generics);
            },
            ItemEnum::Variant(variant) => self.push_variant(&mut out, name, variant),
            ItemEnum::Function(function) => {
                push_visibility(&mut out, visibility, self.own_module(item.id));
                push_header(&mut out, &function.header);
                out.push_str("fn ");
                out.push_str(name);
                self.push_generic_params(&mut out, &function.generics.params);
                self.push_fn_signature(&mut out, &function.sig);
            },
            ItemEnum::Trait(contents) => self.push_trait(&mut out, item, contents),
            ItemEnum::TraitAlias(alias) => {
                push_visibility(&mut out, visibility, self.own_module(item.id));
                out.push_str("trait ");
                out.push_str(name);
                self.push_generic_params(&mut out, &alias.generics.params);
                out.push_str(" = ");
                self.push_bounds(&mut out, &alias.params);
                out.push(';');
            },
            ItemEnum::Impl(contents) => self.push_impl(&mut out, contents),
            ItemEnum::TypeAlias(alias) => {
                self.push_type_item(
                    &mut out,
                    item,
                    Some(&alias.generics),
                    &[],
                    Some(&alias.type_),
                );
            },
            ItemEnum::Constant { type_, const_ } => {
                self.push_const(&mut out, item, type_, Some(&const_.expr));
            },
            ItemEnum::Static(contents) => self.push_static(&mut out, item, contents),
            ItemEnum::ExternType => self.push_type_item(&mut out, item, None, &[], None),
            ItemEnum::Macro(source) => out.push_str(source),
            ItemEnum::ProcMacro(proc_macro) => push_proc_macro(&mut out, name, proc_macro.kind),
            ItemEnum::Primitive(primitive) => out.push_str(&primitive.name),
            ItemEnum::AssocConst { type_, value, .. } => {
                self.push_const(&mut out, item, type_, value.as_deref());
            },
            ItemEnum::AssocType {
                generics,
                bounds,
                type_,
                ..
            } => self.push_type_item(&mut out, item, Some(generics), bounds, type_.as_ref()),
        }
        out
    }

    /// The where clause of `item`, one predicate per string (requirement R-OUT-2).
    #[must_use]
    pub fn where_predicates(&mut self, item: &Item) -> Vec<String> {
        let Some(generics) = generics_of(item) else {
            return Vec::new();
        };
        generics
            .where_predicates
            .iter()
            .map(|predicate| {
                let mut out = String::new();
                match predicate {
                    WherePredicate::BoundPredicate {
                        type_,
                        bounds,
                        generic_params,
                    } => {
                        self.push_for(&mut out, generic_params);
                        self.push_type(&mut out, type_);
                        out.push_str(": ");
                        self.push_bounds(&mut out, bounds);
                    },
                    WherePredicate::LifetimePredicate { lifetime, outlives } => {
                        out.push_str(lifetime);
                        out.push_str(": ");
                        push_joined(&mut out, outlives, " + ");
                    },
                    WherePredicate::EqPredicate { lhs, rhs } => {
                        self.push_type(&mut out, lhs);
                        out.push_str(" = ");
                        self.push_term(&mut out, rhs);
                    },
                }
                out
            })
            .collect()
    }

    /// `ty` as Rust.
    #[must_use]
    pub fn render_type(&mut self, ty: &Type) -> String {
        let mut out = String::new();
        self.push_type(&mut out, ty);
        out
    }

    /// `pub struct Name<T>`, the start of a type declaration.
    fn push_type_header(
        &mut self,
        out: &mut String,
        item: &Item,
        keyword: &str,
        generics: &Generics,
    ) {
        push_visibility(out, &item.visibility, self.own_module(item.id));
        out.push_str(keyword);
        out.push(' ');
        out.push_str(item.name.as_deref().unwrap_or("_"));
        self.push_generic_params(out, &generics.params);
    }

    /// `Name(T)`, `Name { a: T }` or `Name = 4`.
    fn push_variant(&mut self, out: &mut String, name: &str, variant: &Variant) {
        out.push_str(name);
        match &variant.kind {
            VariantKind::Plain => {},
            VariantKind::Tuple(fields) => self.push_tuple_fields(out, fields),
            VariantKind::Struct {
                fields,
                has_stripped_fields,
            } => self.push_struct_fields(out, fields, *has_stripped_fields),
        }
        if let Some(discriminant) = &variant.discriminant {
            out.push_str(" = ");
            out.push_str(&discriminant.expr);
        }
    }

    /// `pub unsafe trait Name<T>: Bound`.
    fn push_trait(&mut self, out: &mut String, item: &Item, contents: &Trait) {
        push_visibility(out, &item.visibility, self.own_module(item.id));
        if contents.is_unsafe {
            out.push_str("unsafe ");
        }
        if contents.is_auto {
            out.push_str("auto ");
        }
        out.push_str("trait ");
        out.push_str(item.name.as_deref().unwrap_or("_"));
        self.push_generic_params(out, &contents.generics.params);
        if !contents.bounds.is_empty() {
            out.push_str(": ");
            self.push_bounds(out, &contents.bounds);
        }
    }

    /// `unsafe impl<T> !Trait for Type`.
    fn push_impl(&mut self, out: &mut String, contents: &Impl) {
        if contents.is_unsafe {
            out.push_str("unsafe ");
        }
        out.push_str("impl");
        self.push_generic_params(out, &contents.generics.params);
        out.push(' ');
        if let Some(trait_) = &contents.trait_ {
            if contents.is_negative {
                out.push('!');
            }
            self.push_path(out, trait_);
            out.push_str(" for ");
        }
        self.push_type(out, &contents.for_);
    }

    /// `pub type Name<T>: Bound = Type;`, a type alias, an associated type or an extern type.
    fn push_type_item(
        &mut self,
        out: &mut String,
        item: &Item,
        generics: Option<&Generics>,
        bounds: &[GenericBound],
        ty: Option<&Type>,
    ) {
        push_visibility(out, &item.visibility, self.own_module(item.id));
        out.push_str("type ");
        out.push_str(item.name.as_deref().unwrap_or("_"));
        if let Some(generics) = generics {
            self.push_generic_params(out, &generics.params);
        }
        if !bounds.is_empty() {
            out.push_str(": ");
            self.push_bounds(out, bounds);
        }
        if let Some(ty) = ty {
            out.push_str(" = ");
            self.push_type(out, ty);
        }
        out.push(';');
    }

    /// `pub const NAME: T = value;`, or without the value when there is none.
    fn push_const(&mut self, out: &mut String, item: &Item, ty: &Type, value: Option<&str>) {
        push_visibility(out, &item.visibility, self.own_module(item.id));
        out.push_str("const ");
        out.push_str(item.name.as_deref().unwrap_or("_"));
        out.push_str(": ");
        self.push_type(out, ty);
        if let Some(value) = value {
            out.push_str(" = ");
            out.push_str(value);
        }
        out.push(';');
    }

    /// `pub static mut NAME: T = value;`.
    fn push_static(&mut self, out: &mut String, item: &Item, contents: &Static) {
        push_visibility(out, &item.visibility, self.own_module(item.id));
        if contents.is_unsafe {
            out.push_str("unsafe ");
        }
        out.push_str("static ");
        if contents.is_mutable {
            out.push_str("mut ");
        }
        out.push_str(item.name.as_deref().unwrap_or("_"));
        out.push_str(": ");
        self.push_type(out, &contents.type_);
        if !contents.expr.is_empty() {
            out.push_str(" = ");
            out.push_str(&contents.expr);
        }
        out.push(';');
    }

    /// `(pub T, U)`, with `_` for a field that rustdoc stripped.
    fn push_tuple_fields(&mut self, out: &mut String, fields: &[Option<Id>]) {
        out.push('(');
        for (position, field) in fields.iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            match field.and_then(|id| self.view.item(id)) {
                Some(Item {
                    id,
                    visibility,
                    inner: ItemEnum::StructField(ty),
                    ..
                }) => {
                    push_visibility(out, visibility, self.own_module(*id));
                    self.push_type(out, ty);
                },
                // A private field.
                _ => out.push('_'),
            }
        }
        out.push(')');
    }

    /// ` { a: T, b: U }`, noting fields that rustdoc stripped.
    fn push_struct_fields(&mut self, out: &mut String, fields: &[Id], has_stripped: bool) {
        out.push_str(" { ");
        let mut first = true;
        for field in fields {
            if let Some(Item {
                name: Some(name),
                inner: ItemEnum::StructField(ty),
                ..
            }) = self.view.item(*field)
            {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                out.push_str(name);
                out.push_str(": ");
                self.push_type(out, ty);
            }
        }
        if has_stripped {
            if !first {
                out.push_str(", ");
            }
            out.push_str("/* private fields */");
        }
        out.push_str(" }");
    }

    /// `(&self, x: u32) -> T`, the parameters and the return type of a function.
    fn push_fn_signature(&mut self, out: &mut String, sig: &FunctionSignature) {
        out.push('(');
        for (position, (name, ty)) in sig.inputs.iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            if name == "self" {
                self.push_self(out, ty);
            } else {
                // A parameter of a function pointer may have no name.
                if !name.is_empty() && name != "_" {
                    out.push_str(name);
                    out.push_str(": ");
                }
                self.push_type(out, ty);
            }
        }
        if sig.is_c_variadic {
            if !sig.inputs.is_empty() {
                out.push_str(", ");
            }
            out.push_str("...");
        }
        out.push(')');
        if let Some(output) = &sig.output {
            out.push_str(" -> ");
            self.push_type(out, output);
        }
    }

    /// The `self` parameter: `self`, `&self`, `&'a mut self`, or `self: Box<Self>`.
    fn push_self(&mut self, out: &mut String, ty: &Type) {
        let is_self = |ty: &Type| matches!(ty, Type::Generic(name) if name == "Self");
        if is_self(ty) {
            out.push_str("self");
        } else if let Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } = ty
            && is_self(type_)
        {
            out.push('&');
            if let Some(lifetime) = lifetime {
                out.push_str(lifetime);
                out.push(' ');
            }
            if *is_mutable {
                out.push_str("mut ");
            }
            out.push_str("self");
        } else {
            out.push_str("self: ");
            self.push_type(out, ty);
        }
    }

    /// `<'a, T: Bound = Default, const N: usize>`, leaving out the parameters that rustdoc made
    /// up for `impl Trait` arguments.
    fn push_generic_params(&mut self, out: &mut String, params: &[GenericParamDef]) {
        let shown: Vec<&GenericParamDef> = params
            .iter()
            .filter(|param| {
                !matches!(param.kind, GenericParamDefKind::Type {
                    is_synthetic: true,
                    ..
                })
            })
            .collect();
        if shown.is_empty() {
            return;
        }
        out.push('<');
        for (position, param) in shown.into_iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            self.push_generic_param(out, param);
        }
        out.push('>');
    }

    fn push_generic_param(&mut self, out: &mut String, param: &GenericParamDef) {
        match &param.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                out.push_str(&param.name);
                if !outlives.is_empty() {
                    out.push_str(": ");
                    push_joined(out, outlives, " + ");
                }
            },
            GenericParamDefKind::Type {
                bounds, default, ..
            } => {
                out.push_str(&param.name);
                if !bounds.is_empty() {
                    out.push_str(": ");
                    self.push_bounds(out, bounds);
                }
                if let Some(default) = default {
                    out.push_str(" = ");
                    self.push_type(out, default);
                }
            },
            GenericParamDefKind::Const { type_, default } => {
                out.push_str("const ");
                out.push_str(&param.name);
                out.push_str(": ");
                self.push_type(out, type_);
                if let Some(default) = default {
                    out.push_str(" = ");
                    out.push_str(default);
                }
            },
        }
    }

    /// `for<'a> `, the binder of higher-ranked lifetimes, if there are any.
    fn push_for(&mut self, out: &mut String, params: &[GenericParamDef]) {
        if params.is_empty() {
            return;
        }
        out.push_str("for<");
        for (position, param) in params.iter().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            self.push_generic_param(out, param);
        }
        out.push_str("> ");
    }

    fn push_bounds(&mut self, out: &mut String, bounds: &[GenericBound]) {
        for (position, bound) in bounds.iter().enumerate() {
            if position > 0 {
                out.push_str(" + ");
            }
            match bound {
                GenericBound::TraitBound {
                    trait_,
                    generic_params,
                    modifier,
                } => {
                    self.push_for(out, generic_params);
                    match modifier {
                        TraitBoundModifier::None => {},
                        TraitBoundModifier::Maybe => out.push('?'),
                        TraitBoundModifier::MaybeConst => out.push_str("~const "),
                    }
                    self.push_path(out, trait_);
                },
                GenericBound::Outlives(lifetime) => out.push_str(lifetime),
                GenericBound::Use(args) => {
                    out.push_str("use<");
                    for (position, arg) in args.iter().enumerate() {
                        if position > 0 {
                            out.push_str(", ");
                        }
                        match arg {
                            PreciseCapturingArg::Lifetime(name)
                            | PreciseCapturingArg::Param(name) => out.push_str(name),
                        }
                    }
                    out.push('>');
                },
            }
        }
    }

    fn push_type(&mut self, out: &mut String, ty: &Type) {
        match ty {
            Type::ResolvedPath(path) => self.push_path(out, path),
            Type::DynTrait(dyn_trait) => self.push_dyn_trait(out, dyn_trait),
            Type::Generic(name) | Type::Primitive(name) => out.push_str(name),
            Type::FunctionPointer(pointer) => {
                self.push_for(out, &pointer.generic_params);
                push_header(out, &pointer.header);
                out.push_str("fn");
                self.push_fn_signature(out, &pointer.sig);
            },
            Type::Tuple(types) => {
                out.push('(');
                for (position, ty) in types.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    self.push_type(out, ty);
                }
                // A tuple of one needs its comma.
                if types.len() == 1 {
                    out.push(',');
                }
                out.push(')');
            },
            Type::Slice(ty) => {
                out.push('[');
                self.push_type(out, ty);
                out.push(']');
            },
            Type::Array { type_, len } => {
                out.push('[');
                self.push_type(out, type_);
                out.push_str("; ");
                out.push_str(len);
                out.push(']');
            },
            Type::Pat { type_, .. } => {
                // rustdoc hides the pattern itself, so only the base type can be shown.
                self.warnings
                    .push(Cow::Borrowed("a pattern type is shown as its base type"));
                self.push_type(out, type_);
            },
            Type::ImplTrait(bounds) => {
                out.push_str("impl ");
                self.push_bounds(out, bounds);
            },
            Type::Infer => out.push('_'),
            Type::RawPointer { is_mutable, type_ } => {
                out.push_str(if *is_mutable { "*mut " } else { "*const " });
                self.push_pointee(out, type_);
            },
            Type::BorrowedRef {
                lifetime,
                is_mutable,
                type_,
            } => {
                out.push('&');
                if let Some(lifetime) = lifetime {
                    out.push_str(lifetime);
                    out.push(' ');
                }
                if *is_mutable {
                    out.push_str("mut ");
                }
                self.push_pointee(out, type_);
            },
            Type::QualifiedPath {
                name,
                args,
                self_type,
                trait_,
            } => {
                if let Some(trait_) = trait_ {
                    out.push('<');
                    self.push_type(out, self_type);
                    out.push_str(" as ");
                    self.push_path(out, trait_);
                    out.push('>');
                } else {
                    self.push_type(out, self_type);
                }
                out.push_str("::");
                out.push_str(name);
                if let Some(args) = args {
                    self.push_generic_args(out, args);
                }
            },
        }
    }

    /// `dyn for<'a> Fn(&'a T) + Send + 'static`.
    fn push_dyn_trait(&mut self, out: &mut String, dyn_trait: &DynTrait) {
        out.push_str("dyn ");
        for (position, poly_trait) in dyn_trait.traits.iter().enumerate() {
            if position > 0 {
                out.push_str(" + ");
            }
            self.push_for(out, &poly_trait.generic_params);
            self.push_path(out, &poly_trait.trait_);
        }
        if let Some(lifetime) = &dyn_trait.lifetime {
            out.push_str(" + ");
            out.push_str(lifetime);
        }
    }

    /// The type behind `&` or `*`, in parentheses when it has `+` (`&(dyn Read + Send)`).
    fn push_pointee(&mut self, out: &mut String, ty: &Type) {
        let needs_parentheses = match ty {
            Type::DynTrait(dyn_trait) => dyn_trait.traits.len() > 1 || dyn_trait.lifetime.is_some(),
            Type::ImplTrait(bounds) => bounds.len() > 1,
            Type::ResolvedPath(_)
            | Type::Generic(_)
            | Type::Primitive(_)
            | Type::FunctionPointer(_)
            | Type::Tuple(_)
            | Type::Slice(_)
            | Type::Array { .. }
            | Type::Pat { .. }
            | Type::Infer
            | Type::RawPointer { .. }
            | Type::BorrowedRef { .. }
            | Type::QualifiedPath { .. } => false,
        };
        if needs_parentheses {
            out.push('(');
            self.push_type(out, ty);
            out.push(')');
        } else {
            self.push_type(out, ty);
        }
    }

    /// A path, by its canonical path if it is an item of this crate, and as rustdoc gives it
    /// otherwise (requirement R-PR-2 for this crate, R-PR-4 for others in M1).
    fn push_path(&mut self, out: &mut String, path: &Path) {
        if let Some(canonical) = self.view.index().canonical_path(path.id) {
            for (position, segment) in canonical.segments().iter().enumerate() {
                if position > 0 {
                    out.push_str("::");
                }
                out.push_str(segment);
            }
        } else if !path.path.is_empty() {
            out.push_str(&path.path);
        } else if let Some(name) = self
            .view
            .summary(path.id)
            .and_then(|summary| summary.path.last())
        {
            // rustdoc leaves the path empty for the trait of `F::Output`, written without it.
            out.push_str(name);
        } else {
            self.warnings.push(Cow::Owned(format!(
                "the path of item {} is unknown",
                path.id.0
            )));
            out.push('_');
        }
        if let Some(args) = &path.args {
            self.push_generic_args(out, args);
        }
    }

    fn push_generic_args(&mut self, out: &mut String, args: &GenericArgs) {
        match args {
            GenericArgs::AngleBracketed { args, constraints } => {
                if args.is_empty() && constraints.is_empty() {
                    return;
                }
                out.push('<');
                let mut first = true;
                for arg in args {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    match arg {
                        GenericArg::Lifetime(lifetime) => out.push_str(lifetime),
                        GenericArg::Type(ty) => self.push_type(out, ty),
                        GenericArg::Const(constant) => out.push_str(&constant.expr),
                        GenericArg::Infer => out.push('_'),
                    }
                }
                for constraint in constraints {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    self.push_constraint(out, constraint);
                }
                out.push('>');
            },
            GenericArgs::Parenthesized { inputs, output } => {
                out.push('(');
                for (position, input) in inputs.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    self.push_type(out, input);
                }
                out.push(')');
                if let Some(output) = output {
                    out.push_str(" -> ");
                    self.push_type(out, output);
                }
            },
            GenericArgs::ReturnTypeNotation => out.push_str("(..)"),
        }
    }

    /// `Item = T` or `Item: Bound` inside generic arguments.
    fn push_constraint(&mut self, out: &mut String, constraint: &AssocItemConstraint) {
        out.push_str(&constraint.name);
        if let Some(args) = &constraint.args {
            self.push_generic_args(out, args);
        }
        match &constraint.binding {
            AssocItemConstraintKind::Equality(term) => {
                out.push_str(" = ");
                self.push_term(out, term);
            },
            AssocItemConstraintKind::Constraint(bounds) => {
                out.push_str(": ");
                self.push_bounds(out, bounds);
            },
        }
    }

    fn push_term(&mut self, out: &mut String, term: &Term) {
        match term {
            Term::Type(ty) => self.push_type(out, ty),
            Term::Constant(constant) => out.push_str(&constant.expr),
        }
    }
}

/// `items` with `separator` between them, without building the joined string first.
fn push_joined(out: &mut String, items: &[String], separator: &str) {
    for (position, item) in items.iter().enumerate() {
        if position > 0 {
            out.push_str(separator);
        }
        out.push_str(item);
    }
}

/// The generics of `item`, if it has any.
const fn generics_of(item: &Item) -> Option<&Generics> {
    match &item.inner {
        ItemEnum::Union(contents) => Some(&contents.generics),
        ItemEnum::Struct(contents) => Some(&contents.generics),
        ItemEnum::Enum(contents) => Some(&contents.generics),
        ItemEnum::Function(function) => Some(&function.generics),
        ItemEnum::Trait(contents) => Some(&contents.generics),
        ItemEnum::TraitAlias(alias) => Some(&alias.generics),
        ItemEnum::Impl(contents) => Some(&contents.generics),
        ItemEnum::TypeAlias(alias) => Some(&alias.generics),
        ItemEnum::AssocType { generics, .. } => Some(generics),
        ItemEnum::Module(_)
        | ItemEnum::ExternCrate { .. }
        | ItemEnum::Use(_)
        | ItemEnum::StructField(_)
        | ItemEnum::Variant(_)
        | ItemEnum::Constant { .. }
        | ItemEnum::Static(_)
        | ItemEnum::ExternType
        | ItemEnum::Macro(_)
        | ItemEnum::ProcMacro(_)
        | ItemEnum::Primitive(_)
        | ItemEnum::AssocConst { .. } => None,
    }
}

/// `pub use source;`, `pub use source as name;` or `pub use source::*;`. `own_module` as for
/// [`push_visibility`].
fn push_use(out: &mut String, visibility: &Visibility, own_module: bool, reexport: &Use) {
    push_visibility(out, visibility, own_module);
    out.push_str("use ");
    out.push_str(&reexport.source);
    if reexport.is_glob {
        out.push_str("::*");
    } else if reexport.source.rsplit("::").next() != Some(reexport.name.as_str()) {
        out.push_str(" as ");
        out.push_str(&reexport.name);
    }
    out.push(';');
}

/// How a procedural macro is invoked: `name!()`, `#[name]` or `#[derive(Name)]`.
fn push_proc_macro(out: &mut String, name: &str, kind: MacroKind) {
    let (open, close) = match kind {
        MacroKind::Bang => ("", "!()"),
        MacroKind::Attr => ("#[", "]"),
        MacroKind::Derive => ("#[derive(", ")]"),
    };
    out.push_str(open);
    out.push_str(name);
    out.push_str(close);
}

/// `pub `, `pub(crate) `, `pub(in crate::path) `, or nothing (requirements R-OUT-2 and R-WS-3):
/// nothing for an item restricted to the module that has it, `own_module`, as Rust writes a
/// private item (decided in M2's D6).
fn push_visibility(out: &mut String, visibility: &Visibility, own_module: bool) {
    match visibility {
        Visibility::Public => out.push_str("pub "),
        Visibility::Default => {},
        Visibility::Crate => out.push_str("pub(crate) "),
        Visibility::Restricted { .. } if own_module => {},
        Visibility::Restricted { path, .. } => {
            out.push_str("pub(in ");
            out.push_str(&in_crate(path));
            out.push_str(") ");
        },
    }
}

/// The path of a module as rustdoc writes it in a visibility, `::query::local`, as Rust writes it
/// in the crate: `crate::query::local`.
#[must_use]
pub fn in_crate(path: &str) -> String {
    let path = path.trim_start_matches("::");
    match path.strip_prefix("crate") {
        Some(rest) if rest.is_empty() || rest.starts_with("::") => format!("crate{rest}"),
        _ if path.is_empty() => "crate".to_owned(),
        _ => format!("crate::{path}"),
    }
}

/// `const async unsafe extern "C" `, in the order Rust writes them.
fn push_header(out: &mut String, header: &FunctionHeader) {
    if header.is_const {
        out.push_str("const ");
    }
    if header.is_async {
        out.push_str("async ");
    }
    if header.is_unsafe {
        out.push_str("unsafe ");
    }
    let (abi, unwind) = match &header.abi {
        Abi::Rust => return,
        Abi::C { unwind } => ("C", *unwind),
        Abi::Cdecl { unwind } => ("cdecl", *unwind),
        Abi::Stdcall { unwind } => ("stdcall", *unwind),
        Abi::Fastcall { unwind } => ("fastcall", *unwind),
        Abi::Aapcs { unwind } => ("aapcs", *unwind),
        Abi::Win64 { unwind } => ("win64", *unwind),
        Abi::SysV64 { unwind } => ("sysv64", *unwind),
        Abi::System { unwind } => ("system", *unwind),
        Abi::Other(abi) => {
            out.push_str("extern \"");
            out.push_str(abi);
            out.push_str("\" ");
            return;
        },
    };
    out.push_str("extern \"");
    out.push_str(abi);
    if unwind {
        out.push_str("-unwind");
    }
    out.push_str("\" ");
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rustdoc_types::{
        Constant,
        Crate,
        Function,
        FunctionPointer,
        ItemKind,
        ItemSummary,
        Module,
        PolyTrait,
        Struct,
        Target,
    };

    use super::*;
    use crate::query::view::tests::{archive, archived};

    fn item(id: u32, name: &str, visibility: Visibility, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: Some(name.to_owned()),
            span: None,
            visibility,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner,
        }
    }

    fn generics(params: Vec<GenericParamDef>, where_predicates: Vec<WherePredicate>) -> Generics {
        Generics {
            params,
            where_predicates,
        }
    }

    fn path(name: &str, id: u32, args: Option<GenericArgs>) -> Path {
        Path {
            path: name.to_owned(),
            id: Id(id),
            args: args.map(Box::new),
        }
    }

    fn angle(types: Vec<Type>) -> GenericArgs {
        GenericArgs::AngleBracketed {
            args: types.into_iter().map(GenericArg::Type).collect(),
            constraints: Vec::new(),
        }
    }

    fn generic(name: &str) -> Type {
        Type::Generic(name.to_owned())
    }

    fn trait_bound(name: &str, id: u32) -> GenericBound {
        GenericBound::TraitBound {
            trait_: path(name, id, None),
            generic_params: Vec::new(),
            modifier: TraitBoundModifier::None,
        }
    }

    fn type_param(name: &str, bounds: Vec<GenericBound>) -> GenericParamDef {
        GenericParamDef {
            name: name.to_owned(),
            kind: GenericParamDefKind::Type {
                bounds,
                default: None,
                is_synthetic: false,
            },
        }
    }

    /// A crate `c` whose `pub mod sync` re-exports the struct `Guard` (id 3) from a private module,
    /// plus `extra` items.
    fn krate(extra: Vec<Item>) -> Crate {
        let mut items = vec![
            item(
                0,
                "c",
                Visibility::Public,
                ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![Id(1)],
                    is_stripped: false,
                }),
            ),
            item(
                1,
                "sync",
                Visibility::Public,
                ItemEnum::Module(Module {
                    is_crate: false,
                    items: vec![Id(2)],
                    is_stripped: false,
                }),
            ),
            Item {
                name: None,
                ..item(
                    2,
                    "",
                    Visibility::Public,
                    ItemEnum::Use(Use {
                        source: "guard::Guard".to_owned(),
                        name: "Guard".to_owned(),
                        id: Some(Id(3)),
                        is_glob: false,
                    }),
                )
            },
            item(
                3,
                "Guard",
                Visibility::Public,
                ItemEnum::Struct(Struct {
                    kind: StructKind::Unit,
                    generics: generics(Vec::new(), Vec::new()),
                    impls: Vec::new(),
                }),
            ),
        ];
        items.extend(extra);
        Crate {
            root: Id(0),
            crate_version: None,
            includes_private: false,
            index: items.into_iter().map(|item| (item.id, item)).collect(),
            paths: HashMap::new(),
            external_crates: HashMap::new(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".to_owned(),
                target_features: Vec::new(),
            },
            format_version: rustdoc_types::FORMAT_VERSION,
        }
    }

    /// Renders `ty` in the crate of [`krate`].
    fn render(ty: &Type) -> String {
        let bytes = archive(krate(Vec::new()));
        let view = CrateView::new(archived(&bytes));
        Renderer::new(&view).render_type(ty)
    }

    /// The signature and where clause of the item `id` added to the crate of [`krate`].
    fn signature(extra: Vec<Item>, id: u32) -> (String, Vec<String>) {
        let bytes = archive(krate(extra));
        let view = CrateView::new(archived(&bytes));
        let mut renderer = Renderer::new(&view);
        let item = view.item(Id(id)).expect("the item");
        (renderer.signature(item), renderer.where_predicates(item))
    }

    #[test]
    fn a_module_in_a_visibility_is_written_from_the_crate() {
        assert_eq!(in_crate("::query::local"), "crate::query::local");
        assert_eq!(in_crate("::"), "crate");
        assert_eq!(in_crate(""), "crate");
        assert_eq!(in_crate("crate::a"), "crate::a");
        assert_eq!(in_crate("crate"), "crate");
        assert_eq!(in_crate("crates::a"), "crate::crates::a");
    }

    #[test]
    fn types_of_this_crate_use_their_canonical_path_and_others_stay_as_given() {
        assert_eq!(
            render(&Type::ResolvedPath(path("Guard", 3, None))),
            "c::sync::Guard"
        );
        assert_eq!(
            render(&Type::ResolvedPath(path(
                "Option",
                99,
                Some(angle(vec![generic("T")]))
            ))),
            "Option<T>"
        );
    }

    #[test]
    fn references_and_pointers_bracket_types_with_plus() {
        let dyn_two = Type::DynTrait(DynTrait {
            traits: vec![
                PolyTrait {
                    trait_: path("Read", 90, None),
                    generic_params: Vec::new(),
                },
                PolyTrait {
                    trait_: path("Send", 91, None),
                    generic_params: Vec::new(),
                },
            ],
            lifetime: Some("'a".to_owned()),
        });
        assert_eq!(
            render(&Type::BorrowedRef {
                lifetime: Some("'a".to_owned()),
                is_mutable: true,
                type_: Box::new(dyn_two),
            }),
            "&'a mut (dyn Read + Send + 'a)"
        );
        assert_eq!(
            render(&Type::RawPointer {
                is_mutable: false,
                type_: Box::new(Type::Primitive("u8".to_owned())),
            }),
            "*const u8"
        );
    }

    #[test]
    fn tuples_slices_arrays_and_function_pointers() {
        let u8_type = Type::Primitive("u8".to_owned());
        assert_eq!(render(&Type::Tuple(Vec::new())), "()");
        assert_eq!(render(&Type::Tuple(vec![u8_type.clone()])), "(u8,)");
        assert_eq!(render(&Type::Slice(Box::new(u8_type.clone()))), "[u8]");
        assert_eq!(
            render(&Type::Array {
                type_: Box::new(u8_type.clone()),
                len: "4".to_owned(),
            }),
            "[u8; 4]"
        );
        let pointer = Type::FunctionPointer(Box::new(FunctionPointer {
            sig: FunctionSignature {
                inputs: vec![("_".to_owned(), u8_type.clone())],
                output: Some(u8_type),
                is_c_variadic: false,
            },
            generic_params: Vec::new(),
            header: FunctionHeader {
                is_const: false,
                is_unsafe: true,
                is_async: false,
                abi: Abi::C { unwind: false },
            },
        }));
        assert_eq!(render(&pointer), "unsafe extern \"C\" fn(u8) -> u8");
    }

    #[test]
    fn a_trait_left_out_of_a_qualified_path_is_named_from_paths() {
        let mut krate = krate(Vec::new());
        krate.paths.insert(Id(98), ItemSummary {
            crate_id: 2,
            path: vec!["core".to_owned(), "future".to_owned(), "Future".to_owned()],
            kind: ItemKind::Trait,
        });
        let bytes = archive(krate);
        let view = CrateView::new(archived(&bytes));
        let mut renderer = Renderer::new(&view);
        let output = |id| Type::QualifiedPath {
            name: "Output".to_owned(),
            args: None,
            self_type: Box::new(generic("F")),
            trait_: Some(path("", id, None)),
        };
        assert_eq!(renderer.render_type(&output(98)), "<F as Future>::Output");
        assert_eq!(renderer.warnings().len(), 0);
        assert_eq!(renderer.render_type(&output(99)), "<F as _>::Output");
        assert_eq!(renderer.warnings().len(), 1);
    }

    #[test]
    fn qualified_paths_and_impl_trait() {
        assert_eq!(
            render(&Type::QualifiedPath {
                name: "Item".to_owned(),
                args: None,
                self_type: Box::new(generic("I")),
                trait_: Some(path("Iterator", 92, None)),
            }),
            "<I as Iterator>::Item"
        );
        assert_eq!(
            render(&Type::ImplTrait(vec![
                trait_bound("Future", 93),
                GenericBound::Use(vec![PreciseCapturingArg::Lifetime("'a".to_owned())]),
            ])),
            "impl Future + use<'a>"
        );
    }

    #[test]
    fn a_method_shows_its_header_self_and_return_type() {
        let lock = item(
            10,
            "lock",
            Visibility::Public,
            ItemEnum::Function(Function {
                sig: FunctionSignature {
                    inputs: vec![
                        ("self".to_owned(), Type::BorrowedRef {
                            lifetime: None,
                            is_mutable: false,
                            type_: Box::new(generic("Self")),
                        }),
                        ("timeout".to_owned(), generic("D")),
                    ],
                    output: Some(Type::ResolvedPath(path(
                        "Guard",
                        3,
                        Some(GenericArgs::AngleBracketed {
                            args: vec![GenericArg::Lifetime("'_".to_owned())],
                            constraints: Vec::new(),
                        }),
                    ))),
                    is_c_variadic: false,
                },
                generics: generics(vec![type_param("D", vec![trait_bound("Into", 94)])], vec![
                    WherePredicate::BoundPredicate {
                        type_: generic("D"),
                        bounds: vec![trait_bound("Send", 91)],
                        generic_params: Vec::new(),
                    },
                ]),
                header: FunctionHeader {
                    is_const: false,
                    is_unsafe: false,
                    is_async: true,
                    abi: Abi::Rust,
                },
                has_body: true,
                default_unstable: None,
            }),
        );
        assert_eq!(
            signature(vec![lock], 10),
            (
                "pub async fn lock<D: Into>(&self, timeout: D) -> c::sync::Guard<'_>".to_owned(),
                vec!["D: Send".to_owned()]
            )
        );
    }

    #[test]
    fn structs_show_tuple_fields_and_visibility() {
        let field = item(
            11,
            "0",
            Visibility::Public,
            ItemEnum::StructField(Type::Primitive("u32".to_owned())),
        );
        let wrapper = item(
            12,
            "Wrapper",
            Visibility::Crate,
            ItemEnum::Struct(Struct {
                kind: StructKind::Tuple(vec![Some(Id(11)), None]),
                generics: generics(
                    vec![GenericParamDef {
                        name: "'a".to_owned(),
                        kind: GenericParamDefKind::Lifetime {
                            outlives: Vec::new(),
                        },
                    }],
                    Vec::new(),
                ),
                impls: Vec::new(),
            }),
        );
        assert_eq!(
            signature(vec![field, wrapper], 12).0,
            "pub(crate) struct Wrapper<'a>(pub u32, _);"
        );
    }

    #[test]
    fn variants_and_constants() {
        let variant = item(
            13,
            "Code",
            Visibility::Default,
            ItemEnum::Variant(Variant {
                kind: VariantKind::Plain,
                discriminant: Some(rustdoc_types::Discriminant {
                    expr: "4".to_owned(),
                    value: "4".to_owned(),
                }),
            }),
        );
        let constant = item(14, "MAX", Visibility::Public, ItemEnum::Constant {
            type_: Type::Primitive("usize".to_owned()),
            const_: Constant {
                expr: "64".to_owned(),
                value: Some("64".to_owned()),
                is_literal: true,
            },
        });
        assert_eq!(signature(vec![variant], 13).0, "Code = 4");
        assert_eq!(
            signature(vec![constant], 14).0,
            "pub const MAX: usize = 64;"
        );
    }

    #[test]
    fn generic_params_hide_synthetic_ones_and_self_by_box() {
        let synthetic = GenericParamDef {
            name: "impl AsRef<str>".to_owned(),
            kind: GenericParamDefKind::Type {
                bounds: vec![trait_bound("AsRef", 95)],
                default: None,
                is_synthetic: true,
            },
        };
        let unsized_bound = GenericBound::TraitBound {
            trait_: path("Sized", 96, None),
            generic_params: Vec::new(),
            modifier: TraitBoundModifier::Maybe,
        };
        let by_box = item(
            15,
            "into_inner",
            Visibility::Default,
            ItemEnum::Function(Function {
                sig: FunctionSignature {
                    inputs: vec![
                        (
                            "self".to_owned(),
                            Type::ResolvedPath(path("Box", 97, Some(angle(vec![generic("Self")])))),
                        ),
                        (
                            "name".to_owned(),
                            Type::ImplTrait(vec![trait_bound("AsRef", 95)]),
                        ),
                    ],
                    output: None,
                    is_c_variadic: false,
                },
                generics: generics(
                    vec![type_param("T", vec![unsized_bound]), synthetic],
                    Vec::new(),
                ),
                header: FunctionHeader {
                    is_const: false,
                    is_unsafe: false,
                    is_async: false,
                    abi: Abi::Rust,
                },
                has_body: false,
                default_unstable: None,
            }),
        );
        assert_eq!(
            signature(vec![by_box], 15).0,
            "fn into_inner<T: ?Sized>(self: Box<Self>, name: impl AsRef)"
        );
    }

    #[test]
    fn re_exports_show_a_rename_and_macros_show_their_rules() {
        let renamed = item(
            16,
            "Other",
            Visibility::Public,
            ItemEnum::Use(Use {
                source: "guard::Guard".to_owned(),
                name: "Other".to_owned(),
                id: Some(Id(3)),
                is_glob: false,
            }),
        );
        let same = item(
            17,
            "Guard",
            Visibility::Public,
            ItemEnum::Use(Use {
                source: "guard::Guard".to_owned(),
                name: "Guard".to_owned(),
                id: Some(Id(3)),
                is_glob: false,
            }),
        );
        let rules = "macro_rules! pin {\n    ($($x:ident),*) => { ... };\n}";
        let pin = item(
            18,
            "pin",
            Visibility::Public,
            ItemEnum::Macro(rules.to_owned()),
        );
        assert_eq!(
            signature(vec![renamed], 16).0,
            "pub use guard::Guard as Other;"
        );
        assert_eq!(signature(vec![same], 17).0, "pub use guard::Guard;");
        assert_eq!(signature(vec![pin], 18).0, rules);
    }
}
