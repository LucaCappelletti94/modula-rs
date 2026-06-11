//! Phase 1 spike: prove the IR (items, ownership, and a body-level edge) is
//! producible from rust-analyzer on a real fixture crate.
//!
//! Marked `#[ignore]` because it loads a cargo workspace through rust-analyzer,
//! which needs a toolchain. Run with:
//! `cargo test -p modula-extract -- --include-ignored`.

use std::path::PathBuf;

use modula_extract::{ExtractOptions, Extractor, RaExtractor};
use modula_ir::{CrateGraph, RefKind};

fn fixture_manifest(name: &str) -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        name,
        "Cargo.toml",
    ]
    .iter()
    .collect()
}

fn opts(name: &str) -> ExtractOptions {
    ExtractOptions {
        manifest_path: fixture_manifest(name),
        ..Default::default()
    }
}

/// A normalized, id-independent view of the IR for stable snapshots: paths
/// instead of dense numeric ids, everything sorted.
fn normalized(graph: &CrateGraph) -> serde_json::Value {
    let mut modules: Vec<_> = graph
        .modules
        .iter()
        .map(|m| {
            serde_json::json!({
                "path": m.canonical_path,
                "depth": m.depth,
                "visibility": m.visibility,
                "parent": m.parent.map(|p| graph.module(p).canonical_path.clone()),
            })
        })
        .collect();
    modules.sort_by_key(|v| v["path"].as_str().unwrap_or_default().to_owned());

    let mut items: Vec<_> = graph
        .items
        .iter()
        .map(|i| {
            serde_json::json!({
                "path": i.canonical_path,
                "kind": i.kind,
                "visibility": i.visibility,
                "owning_module": graph.module(i.owning_module).canonical_path,
            })
        })
        .collect();
    items.sort_by_key(|v| v["path"].as_str().unwrap_or_default().to_owned());

    let mut edges: Vec<_> = graph
        .edges
        .iter()
        .map(|e| {
            serde_json::json!({
                "from": graph.item(e.from).canonical_path,
                "to": graph.item(e.to).canonical_path,
                "kind": e.kind,
                "weight": e.weight,
            })
        })
        .collect();
    edges.sort_by_key(|v| {
        (
            v["from"].as_str().unwrap_or_default().to_owned(),
            v["to"].as_str().unwrap_or_default().to_owned(),
        )
    });

    serde_json::json!({ "modules": modules, "items": items, "edges": edges })
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn spike_captures_body_level_cross_module_edge() {
    let graph = RaExtractor
        .extract(&opts("spike"))
        .expect("extraction succeeds");

    // Items for both functions are present.
    let f = graph
        .items
        .iter()
        .find(|i| i.canonical_path == "spike::a::f")
        .expect("spike::a::f extracted");
    let g = graph
        .items
        .iter()
        .find(|i| i.canonical_path == "spike::b::g")
        .expect("spike::b::g extracted");

    // The crucial assertion: a Body edge from b::g into a::f, captured despite
    // there being no `use` import. This is the edge cargo-modules cannot see.
    let body_edge = graph
        .edges
        .iter()
        .find(|e| e.from == g.id && e.to == f.id)
        .expect("body-level edge b::g -> a::f captured");
    assert_eq!(body_edge.kind, RefKind::Body);

    // Module tree is present with correct depths.
    let module_a = graph
        .modules
        .iter()
        .find(|m| m.canonical_path == "spike::a")
        .expect("module spike::a");
    assert_eq!(module_a.depth, 1);
    assert_eq!(
        graph.module(module_a.parent.unwrap()).canonical_path,
        "spike"
    );

    insta::assert_json_snapshot!(normalized(&graph));
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn extraction_is_deterministic() {
    let extract = || {
        normalized(
            &RaExtractor
                .extract(&opts("rich"))
                .expect("extraction succeeds"),
        )
    };
    assert_eq!(extract(), extract(), "two extractions must agree");
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn field_access_produces_a_body_edge() {
    let graph = RaExtractor
        .extract(&opts("fields"))
        .expect("extraction succeeds");

    // `read` reaches `Config` only through the `c.value` field access.
    assert!(
        graph.edges.iter().any(|e| {
            graph.item(e.from).canonical_path == "fields::read"
                && graph.item(e.to).canonical_path == "fields::Config"
                && e.kind == RefKind::Body
        }),
        "missing field-access body edge read -> Config"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn trait_items_impl_bounds_and_impl_trait_produce_edges() {
    let graph = RaExtractor
        .extract(&opts("traits"))
        .expect("extraction succeeds");

    // Trait method signature: `Store::describe(&self) -> Record` (associated
    // items are module-qualified, so the path is `traits::describe`).
    assert!(
        has_edge(
            &graph,
            "traits::describe",
            "traits::Record",
            RefKind::Signature
        ),
        "missing trait method signature edge describe -> Record"
    );
    // Trait default method body: `helper` calls `describe`.
    assert!(
        has_edge(&graph, "traits::helper", "traits::describe", RefKind::Body),
        "missing default-body edge helper -> describe"
    );
    // Impl generic bound: `impl<T: Marker> Pair<T>`.
    assert!(
        has_edge(
            &graph,
            "traits::Pair",
            "traits::Marker",
            RefKind::TraitBound
        ),
        "missing impl bound edge Pair -> Marker"
    );
    // impl-Trait return: `provide() -> impl Marker`.
    assert!(
        has_edge(
            &graph,
            "traits::provide",
            "traits::Marker",
            RefKind::Signature
        ),
        "missing impl-Trait return edge provide -> Marker"
    );

    insta::assert_json_snapshot!(normalized(&graph));
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn const_and_static_produce_edges() {
    let graph = RaExtractor
        .extract(&opts("consts"))
        .expect("extraction succeeds");

    // Body edge from a const initializer: `DERIVED = BASE + 1`.
    assert!(
        has_edge(&graph, "consts::DERIVED", "consts::BASE", RefKind::Body),
        "missing initializer body edge DERIVED -> BASE"
    );
    // Signature edge from a static's type: `REGISTRY: Option<Registry>`.
    assert!(
        has_edge(
            &graph,
            "consts::REGISTRY",
            "consts::Registry",
            RefKind::Signature
        ),
        "missing signature edge REGISTRY -> Registry"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn std_generic_wrappers_resolve_to_local_type_arguments() {
    let graph = RaExtractor
        .extract(&opts("stdgen"))
        .expect("extraction succeeds");

    // Each std-library wrapper (`Vec`, `Option`, `Box`, `Result`) resolves with
    // the sysroot enabled, so the type walk descends into the local argument and
    // emits a signature edge to it.
    assert!(
        has_edge(
            &graph,
            "stdgen::Holder",
            "stdgen::Local",
            RefKind::Signature
        ),
        "missing Vec<Local> field edge Holder -> Local"
    );
    assert!(
        has_edge(&graph, "stdgen::first", "stdgen::Local", RefKind::Signature),
        "missing Option<Local> return edge first -> Local"
    );
    assert!(
        has_edge(&graph, "stdgen::boxed", "stdgen::Local", RefKind::Signature),
        "missing Box<Local> return edge boxed -> Local"
    );
    assert!(
        has_edge(
            &graph,
            "stdgen::fallible",
            "stdgen::Local",
            RefKind::Signature
        ),
        "missing Result<Local, _> ok-arm edge fallible -> Local"
    );
    assert!(
        has_edge(
            &graph,
            "stdgen::fallible",
            "stdgen::Failure",
            RefKind::Signature
        ),
        "missing Result<_, Failure> err-arm edge fallible -> Failure"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn trait_bounds_produce_bound_edges() {
    let graph = RaExtractor
        .extract(&opts("bounds"))
        .expect("extraction succeeds");

    assert!(
        has_edge(
            &graph,
            "bounds::f",
            "bounds::LocalTrait",
            RefKind::TraitBound
        ),
        "missing f -> LocalTrait bound edge"
    );
    assert!(
        has_edge(
            &graph,
            "bounds::S",
            "bounds::LocalTrait",
            RefKind::TraitBound
        ),
        "missing S -> LocalTrait bound edge"
    );
    assert!(
        has_edge(&graph, "bounds::Sub", "bounds::Super", RefKind::TraitBound),
        "missing Sub -> Super supertrait edge"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn use_imports_produce_import_edges_from_module_nodes() {
    use modula_ir::ItemKind;

    let graph = RaExtractor
        .extract(&opts("imports"))
        .expect("extraction succeeds");

    // Module `b` is a first-class item.
    let b_module = graph
        .items
        .iter()
        .find(|i| i.canonical_path == "imports::b")
        .expect("module item imports::b");
    assert_eq!(b_module.kind, ItemKind::Module);

    // `use crate::a` and `use crate::a::Thing` produce import edges, even though
    // no item in `b` otherwise references `a`.
    assert!(
        has_edge(&graph, "imports::b", "imports::a", RefKind::Import),
        "missing import edge b -> a"
    );
    assert!(
        has_edge(&graph, "imports::b", "imports::a::Thing", RefKind::Import),
        "missing import edge b -> a::Thing"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn pub_use_reexport_marks_item_as_public_api() {
    let graph = RaExtractor
        .extract(&opts("reexport"))
        .expect("extraction succeeds");

    let is_public = |path: &str| {
        graph
            .items
            .iter()
            .find(|i| i.canonical_path == path)
            .unwrap_or_else(|| panic!("{path} extracted"))
            .reachable_pub_api
    };

    // Every re-export form exposes its target as public API, even though they
    // live in private modules.
    assert!(is_public("reexport::private::Hidden"), "simple pub use");
    assert!(is_public("reexport::private::Nested1"), "nested pub use");
    assert!(is_public("reexport::private::Nested2"), "nested pub use");
    assert!(is_public("reexport::globbed::Globbed"), "glob pub use");
    assert!(
        is_public("reexport::globbed::globbed_fn"),
        "glob pub use fn"
    );
    assert!(is_public("reexport::private::Renamed"), "renamed pub use");

    // Negative cases: pub(crate) and private re-exports do NOT make their
    // targets public API (over-marking would suppress real over-exposure).
    assert!(
        !is_public("reexport::private::OnlyCrate"),
        "pub(crate) use must not be public API"
    );
    assert!(
        !is_public("reexport::private::OnlyPrivate"),
        "private use must not be public API"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn target_filtering_excludes_tests_and_marks_public_api() {
    let graph = RaExtractor
        .extract(&opts("targets"))
        .expect("extraction succeeds");

    // Only the library crate is analyzed; the integration test target is gone.
    assert_eq!(graph.crates.len(), 1);
    assert_eq!(graph.crates[0].name, "targets");
    let in_crate = |path: &str| path == "targets" || path.starts_with("targets::");
    assert!(
        graph.items.iter().all(|i| in_crate(&i.canonical_path)),
        "found a non-library item: {:?}",
        graph.items.iter().find(|i| !in_crate(&i.canonical_path)),
    );
    assert!(
        !graph
            .items
            .iter()
            .any(|i| i.canonical_path.contains("it_works")),
        "integration-test item leaked into the analysis"
    );

    // Public-API detection: `api::visible` is public API, `internal::hidden` is
    // a pub item inside a private module and is not.
    let visible = graph
        .items
        .iter()
        .find(|i| i.canonical_path == "targets::api::visible")
        .expect("api::visible");
    let hidden = graph
        .items
        .iter()
        .find(|i| i.canonical_path == "targets::internal::hidden")
        .expect("internal::hidden");
    assert!(
        visible.reachable_pub_api,
        "api::visible should be public API"
    );
    assert!(
        !hidden.reachable_pub_api,
        "internal::hidden should not be public API"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn derive_macro_generates_impl_edge() {
    let graph = RaExtractor
        .extract(&opts("procderive"))
        .expect("extraction succeeds");

    // `#[derive(Tag)]` expands to `impl Tag for Tagged {}`; with the proc-macro
    // server enabled that generated impl is a real internal `Impl` edge.
    assert!(
        has_edge(
            &graph,
            "procderive::Tagged",
            "procderive::Tag",
            RefKind::Impl
        ),
        "missing derive-generated impl edge Tagged -> Tag"
    );
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn macro_call_body_descent_captures_inner_reference() {
    let graph = RaExtractor
        .extract(&opts("bodymacro"))
        .expect("extraction succeeds");

    // `build`'s only reference to `make_marker` is inside the `marker!()`
    // expansion, so the edge exists only if the body walk descends into macros.
    assert!(
        has_edge(
            &graph,
            "bodymacro::build",
            "bodymacro::make_marker",
            RefKind::Body
        ),
        "missing macro-expansion body edge build -> make_marker"
    );
}

/// Helpers to query edges by canonical path and kind.
fn has_edge(graph: &CrateGraph, from: &str, to: &str, kind: RefKind) -> bool {
    graph.edges.iter().any(|e| {
        graph.item(e.from).canonical_path == from
            && graph.item(e.to).canonical_path == to
            && e.kind == kind
    })
}

fn has_edge_to_containing(
    graph: &CrateGraph,
    from_contains: &str,
    to_contains: &str,
    kind: RefKind,
) -> bool {
    graph.edges.iter().any(|e| {
        graph.item(e.from).canonical_path.contains(from_contains)
            && graph.item(e.to).canonical_path.contains(to_contains)
            && e.kind == kind
    })
}

#[test]
#[ignore = "loads a cargo workspace via rust-analyzer; run with --include-ignored"]
fn rich_captures_signature_impl_and_method_edges() {
    let graph = RaExtractor
        .extract(&opts("rich"))
        .expect("extraction succeeds");

    // Signature edge: struct field `Outer { inner: Inner }`.
    assert!(
        has_edge(
            &graph,
            "rich::types::Outer",
            "rich::types::Inner",
            RefKind::Signature
        ),
        "missing Outer -> Inner signature edge"
    );
    // Impl edge: `impl Greet for Outer`.
    assert!(
        has_edge(
            &graph,
            "rich::types::Outer",
            "rich::types::Greet",
            RefKind::Impl
        ),
        "missing Outer -> Greet impl edge"
    );
    // Signature edge: `fn run(outer: &Outer)`.
    assert!(
        has_edge(
            &graph,
            "rich::logic::run",
            "rich::types::Outer",
            RefKind::Signature
        ),
        "missing run -> Outer signature edge"
    );
    // Body edge: `run` calls `outer.greet()` (method call).
    assert!(
        has_edge_to_containing(&graph, "rich::logic::run", "greet", RefKind::Body),
        "missing run -> greet body edge"
    );
    // Body edge: `greet` calls `self.helper()` (method call).
    assert!(
        has_edge_to_containing(&graph, "greet", "helper", RefKind::Body),
        "missing greet -> helper body edge"
    );

    insta::assert_json_snapshot!(normalized(&graph));
}
