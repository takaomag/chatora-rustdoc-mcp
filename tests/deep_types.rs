//! Archiving, rendering and freeing a crate whose types nest as deep as parsing allows (R-PR-8,
//! doc/m1-plan.md A3). A stack overflow aborts the process, so this test fails by crashing.

#![expect(
    unused_crate_dependencies,
    reason = "a test uses only some of the package's dependencies"
)]

use std::collections::HashMap;

use chatora_rustdoc_mcp::{
    data::{
        archive::Mapped,
        rustdoc_json::{self, MAX_NESTING},
    },
    query::{
        across::Indexed,
        get_item::{Detail, ItemRequest, Origin, ResolvedFrom, get_item},
        view::CrateView,
    },
    task::THREAD_STACK_SIZE,
};
use rustdoc_types::{
    Abi,
    Crate,
    Function,
    FunctionHeader,
    FunctionSignature,
    Generics,
    Id,
    Item,
    ItemEnum,
    Module,
    Target,
    Type,
    Visibility,
};

/// `[[…[u8]…]]` nested `depth` times: of the types, the one with the fewest levels of JSON per
/// level of type, so the most levels of recursion for the depth of the JSON.
fn deep_slice(depth: usize) -> Type {
    (0..depth).fold(Type::Primitive("u8".to_owned()), |inner, _| {
        Type::Slice(Box::new(inner))
    })
}

fn item(id: u32, name: &str, inner: ItemEnum) -> Item {
    Item {
        id: Id(id),
        crate_id: 0,
        name: Some(name.to_owned()),
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

/// A crate `deep` with `pub fn f() -> [[…[u8]…]]`, whose type is nested `depth` times.
fn deep_crate(depth: usize) -> Crate {
    let function = Function {
        sig: FunctionSignature {
            inputs: Vec::new(),
            output: Some(deep_slice(depth)),
            is_c_variadic: false,
        },
        generics: Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        },
        header: FunctionHeader {
            is_const: false,
            is_unsafe: false,
            is_async: false,
            abi: Abi::Rust,
        },
        has_body: true,
        default_unstable: None,
    };
    Crate {
        root: Id(0),
        crate_version: Some("1.0.0".to_owned()),
        includes_private: false,
        index: [
            item(
                0,
                "deep",
                ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![Id(1)],
                    is_stripped: false,
                }),
            ),
            item(1, "f", ItemEnum::Function(function)),
        ]
        .into_iter()
        .map(|item| (item.id, item))
        .collect(),
        paths: HashMap::new(),
        external_crates: HashMap::new(),
        target: Target {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            target_features: Vec::new(),
        },
        format_version: rustdoc_types::FORMAT_VERSION,
    }
}

/// The levels of JSON around the return type of `f`: the crate, `index`, the item, `inner`,
/// `function`, `sig` and `output`.
const LEVELS_AROUND_THE_TYPE: usize = 7;

#[test]
fn a_type_nested_up_to_the_limit_is_rendered_and_freed() {
    let depth = MAX_NESTING - LEVELS_AROUND_THE_TYPE;
    // Building, serializing and dropping the deep type recurse too, so they run on a thread with
    // the same stack as the server's.
    let json = std::thread::Builder::new()
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || serde_json::to_vec(&deep_crate(depth)).expect("serializes"))
        .expect("a thread starts")
        .join()
        .expect("the JSON is built");
    let rendered = std::thread::Builder::new()
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || {
            let parsed = rustdoc_json::parse(&json).expect("the JSON is within the limit");
            let mapped = Mapped::in_memory(&Indexed::new(parsed)).expect("archived");
            let view = CrateView::new(mapped.get());
            let name = "deep".parse().expect("a crate name");
            let version = semver::Version::new(1, 0, 0);
            let response = get_item(&view, ItemRequest {
                name: &name,
                version: &version,
                path: "deep::f",
                origin: Origin::DocsRs {
                    resolved_from: ResolvedFrom::Explicit,
                    project_from: None,
                    build: None,
                },
                detail: Detail::Normal,
                target: None,
            })
            .expect("deep::f is found");
            serde_json::to_value(&response).expect("serializes")["data"]["signature"]
                .as_str()
                .map(str::to_owned)
        })
        .expect("a thread starts")
        .join()
        .expect("rendering and freeing do not panic");
    let expected = format!("pub fn f() -> {}u8{}", "[".repeat(depth), "]".repeat(depth));
    assert!(
        rendered == Some(expected),
        "the signature is rendered in full"
    );
}
