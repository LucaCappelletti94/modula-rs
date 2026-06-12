//! Round-trip and accessor tests for the IR, exercised without rust-analyzer.

use modula_ir::{
    Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
    RefKind, SCHEMA_VERSION, Visibility,
};

/// Builds a tiny two-module crate graph by hand:
///
/// ```text
/// my_crate            (module 0, root)
///   |- a              (module 1)
///   |    \- a::f      (item 0, pub(crate) fn)
///   \- b              (module 2)
///        \- b::g      (item 1, private fn) -- body edge --> a::f
/// ```
fn sample() -> CrateGraph {
    let krate = CrateId(0);
    CrateGraph {
        schema_version: SCHEMA_VERSION,
        ra_version: String::new(),
        root_crate: krate,
        crates: vec![Crate {
            id: krate,
            name: "my_crate".to_owned(),
            is_local: true,
            root_module: ModuleId(0),
        }],
        modules: vec![
            Module {
                id: ModuleId(0),
                crate_id: krate,
                parent: None,
                name: String::new(),
                canonical_path: "my_crate".to_owned(),
                depth: 0,
                visibility: Visibility::Public,
                kind: ModuleKind::Mod,
            },
            Module {
                id: ModuleId(1),
                crate_id: krate,
                parent: Some(ModuleId(0)),
                name: "a".to_owned(),
                canonical_path: "my_crate::a".to_owned(),
                depth: 1,
                visibility: Visibility::Crate,
                kind: ModuleKind::Mod,
            },
            Module {
                id: ModuleId(2),
                crate_id: krate,
                parent: Some(ModuleId(0)),
                name: "b".to_owned(),
                canonical_path: "my_crate::b".to_owned(),
                depth: 1,
                visibility: Visibility::Crate,
                kind: ModuleKind::Mod,
            },
        ],
        items: vec![
            Item {
                id: modula_ir::ItemId(0),
                canonical_path: "my_crate::a::f".to_owned(),
                kind: ItemKind::Function,
                visibility: Visibility::Crate,
                owning_module: ModuleId(1),
                crate_id: krate,
                has_canonical_path: true,
                reachable_pub_api: false,
            },
            Item {
                id: modula_ir::ItemId(1),
                canonical_path: "my_crate::b::g".to_owned(),
                kind: ItemKind::Function,
                visibility: Visibility::Private,
                owning_module: ModuleId(2),
                crate_id: krate,
                has_canonical_path: true,
                reachable_pub_api: false,
            },
        ],
        edges: vec![Edge {
            from: modula_ir::ItemId(1),
            to: modula_ir::ItemId(0),
            kind: RefKind::Body,
            weight: 1,
        }],
    }
}

#[test]
fn json_roundtrip_is_lossless() {
    let graph = sample();
    let json = serde_json::to_string_pretty(&graph).expect("serialize");
    let back: CrateGraph = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(graph, back);
}

#[test]
fn accessors_resolve_dense_ids() {
    let graph = sample();
    assert_eq!(graph.krate(graph.root_crate).name, "my_crate");
    assert_eq!(graph.module(ModuleId(1)).canonical_path, "my_crate::a");
    assert_eq!(
        graph.item(modula_ir::ItemId(0)).canonical_path,
        "my_crate::a::f"
    );
    // The single body edge points from b::g into a::f: a cross-module leak.
    let edge = &graph.edges[0];
    assert_eq!(graph.item(edge.from).owning_module, ModuleId(2));
    assert_eq!(graph.item(edge.to).owning_module, ModuleId(1));
    assert_eq!(edge.kind, RefKind::Body);
}

#[test]
fn partition_collapses_at_root_and_splits_by_module() {
    let graph = sample();
    assert_eq!(graph.max_depth(), 1);
    // Depth 0: both items roll up to the crate root, one community.
    assert_eq!(graph.partition_at_depth(0), vec![0, 0]);
    // Depth 1: a::f and b::g sit in different modules, two communities.
    let p1 = graph.partition_at_depth(1);
    assert_eq!(p1.len(), 2);
    assert_ne!(p1[0], p1[1]);
    // Depth beyond the tree clamps to the leaf modules (same as depth 1 here).
    assert_eq!(graph.partition_at_depth(5), p1);
}

#[test]
fn public_api_requires_a_pub_chain_to_the_root() {
    let krate = CrateId(0);
    let module = |id: u32, path: &str, parent: Option<u32>, vis: Visibility| Module {
        id: ModuleId(id),
        crate_id: krate,
        parent: parent.map(ModuleId),
        name: path.rsplit("::").next().unwrap_or(path).to_owned(),
        canonical_path: path.to_owned(),
        depth: u32::from(parent.is_some()),
        visibility: vis,
        kind: ModuleKind::Mod,
    };
    let item = |id: u32, path: &str, owner: u32, vis: Visibility| Item {
        id: ItemId(id),
        canonical_path: path.to_owned(),
        kind: ItemKind::Function,
        visibility: vis,
        owning_module: ModuleId(owner),
        crate_id: krate,
        has_canonical_path: true,
        reachable_pub_api: false,
    };
    let mut graph = CrateGraph {
        schema_version: SCHEMA_VERSION,
        ra_version: String::new(),
        root_crate: krate,
        crates: vec![Crate {
            id: krate,
            name: "k".to_owned(),
            is_local: true,
            root_module: ModuleId(0),
        }],
        modules: vec![
            module(0, "k", None, Visibility::Public),
            module(1, "k::vis", Some(0), Visibility::Public),
            module(2, "k::hidden", Some(0), Visibility::Private),
        ],
        items: vec![
            item(0, "k::vis::api", 1, Visibility::Public),
            item(1, "k::vis::internal", 1, Visibility::Crate),
            item(2, "k::hidden::leaked", 2, Visibility::Public),
        ],
        edges: Vec::new(),
    };

    graph.compute_public_api();
    // pub item in a pub module under the root: public API.
    assert!(graph.item(ItemId(0)).reachable_pub_api);
    // pub(crate) is never public API.
    assert!(!graph.item(ItemId(1)).reachable_pub_api);
    // pub item, but inside a private module: not reachable from outside.
    assert!(!graph.item(ItemId(2)).reachable_pub_api);

    // Re-exporting the hidden item (a `pub use`) makes it public API.
    let reexported = std::collections::HashSet::from([ItemId(2)]);
    graph.compute_public_api_with_reexports(&reexported);
    assert!(graph.item(ItemId(2)).reachable_pub_api);
}

#[test]
fn visibility_restrictiveness_orders_private_below_public() {
    assert!(Visibility::Private.restrictiveness() < Visibility::Super.restrictiveness());
    assert!(Visibility::Super.restrictiveness() < Visibility::Crate.restrictiveness());
    assert!(Visibility::Crate.restrictiveness() < Visibility::Public.restrictiveness());
}
