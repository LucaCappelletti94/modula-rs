//! Shared hand-built IR fixtures for the metric tests (no rust-analyzer).

use modula_ir::{
    Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
    RefKind, SCHEMA_VERSION, Visibility,
};

/// Builds a crate graph with `n` items split across two depth-1 modules `a` and
/// `b` (chosen by `module_of[i]` in `{0, 1}`), connected by directed `Body`
/// edges (item index pairs).
#[must_use]
pub fn two_module_graph(n: usize, module_of: &[usize], edges: &[(usize, usize)]) -> CrateGraph {
    let krate = CrateId(0);
    let modules = vec![
        Module {
            id: ModuleId(0),
            crate_id: krate,
            parent: None,
            name: String::new(),
            canonical_path: "k".to_owned(),
            depth: 0,
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        },
        Module {
            id: ModuleId(1),
            crate_id: krate,
            parent: Some(ModuleId(0)),
            name: "a".to_owned(),
            canonical_path: "k::a".to_owned(),
            depth: 1,
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        },
        Module {
            id: ModuleId(2),
            crate_id: krate,
            parent: Some(ModuleId(0)),
            name: "b".to_owned(),
            canonical_path: "k::b".to_owned(),
            depth: 1,
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        },
    ];
    let items = (0..n)
        .map(|i| Item {
            id: ItemId(i as u32),
            canonical_path: format!("k::item{i}"),
            kind: ItemKind::Function,
            visibility: Visibility::Crate,
            owning_module: ModuleId(1 + module_of[i] as u32),
            crate_id: krate,
            has_canonical_path: true,
            reachable_pub_api: false,
            visibility_fixed_by_trait: false,
        })
        .collect();
    let edges = edges
        .iter()
        .map(|&(from, to)| Edge {
            from: ItemId(from as u32),
            to: ItemId(to as u32),
            kind: RefKind::Body,
            weight: 1,
        })
        .collect();
    CrateGraph {
        schema_version: SCHEMA_VERSION,
        ra_version: String::new(),
        root_crate: krate,
        crates: vec![Crate {
            id: krate,
            name: "k".to_owned(),
            is_local: true,
            root_module: ModuleId(0),
        }],
        modules,
        items,
        edges,
    }
}

/// Two dense 4-cliques over items `0..4` and `4..8`, joined by a single weak
/// bridge edge: a clear two-community dependency structure.
#[must_use]
pub fn two_cliques_edges() -> Vec<(usize, usize)> {
    let mut edges = Vec::new();
    for cluster in [[0, 1, 2, 3], [4, 5, 6, 7]] {
        for &a in &cluster {
            for &b in &cluster {
                if a != b {
                    edges.push((a, b));
                }
            }
        }
    }
    edges.push((3, 4));
    edges
}
