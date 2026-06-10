//! Tests encapsulation: leak-depth orientation (deeper shared ancestor means a
//! cheaper leak) and over-exposure detection. No rust-analyzer (fast tier).

use modula_ir::{
    Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, RefKind,
    SCHEMA_VERSION, Visibility,
};
use modula_metrics::encapsulation::encapsulation;

/// A module spec: canonical path, parent index, depth.
type ModuleSpec = (&'static str, Option<usize>, u32);
/// An item spec: canonical path, owning-module index, visibility.
type ItemSpec = (&'static str, usize, Visibility);

/// Builds an arbitrary single-crate IR from module, item, and edge specs.
fn build(modules: &[ModuleSpec], items: &[ItemSpec], edges: &[(usize, usize)]) -> CrateGraph {
    let krate = CrateId(0);
    let modules = modules
        .iter()
        .enumerate()
        .map(|(i, &(path, parent, depth))| Module {
            id: ModuleId(i as u32),
            crate_id: krate,
            parent: parent.map(|p| ModuleId(p as u32)),
            name: path.rsplit("::").next().unwrap_or(path).to_owned(),
            canonical_path: path.to_owned(),
            depth,
            visibility: Visibility::Public,
        })
        .collect();
    let items = items
        .iter()
        .enumerate()
        .map(|(i, (path, module, vis))| Item {
            id: ItemId(i as u32),
            canonical_path: (*path).to_owned(),
            kind: ItemKind::Function,
            visibility: vis.clone(),
            owning_module: ModuleId(*module as u32),
            crate_id: krate,
            has_canonical_path: true,
            reachable_pub_api: false,
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

#[test]
fn deeper_shared_ancestor_means_a_cheaper_leak() {
    // Tree: k > {a > {a1, a2}, b}. Item X in a1, Y in a2 (share ancestor a),
    // Z in b (shares only the crate root with X).
    let ir = build(
        &[
            ("k", None, 0),
            ("k::a", Some(0), 1),
            ("k::a::a1", Some(1), 2),
            ("k::a::a2", Some(1), 2),
            ("k::b", Some(0), 1),
        ],
        &[
            ("k::a::a1::x", 2, Visibility::Crate),
            ("k::a::a2::y", 3, Visibility::Crate),
            ("k::b::z", 4, Visibility::Crate),
        ],
        &[(0, 1), (0, 2)], // X -> Y (deep LCA), X -> Z (root LCA)
    );

    let report = encapsulation(&ir).unwrap();
    assert_eq!(report.deepest_leaks.len(), 2);

    let xy = report
        .deepest_leaks
        .iter()
        .find(|l| l.from == ItemId(0) && l.to == ItemId(1))
        .unwrap();
    let xz = report
        .deepest_leaks
        .iter()
        .find(|l| l.from == ItemId(0) && l.to == ItemId(2))
        .unwrap();

    // X -> Z spans the whole tree (LCA is the root, Lin 0), so it costs the most.
    assert!(
        (xz.leak_cost - 1.0).abs() < 1e-9,
        "xz cost = {}",
        xz.leak_cost
    );
    // X -> Y shares the deeper ancestor `a`, so it costs less.
    assert!(xy.leak_cost < xz.leak_cost);
    assert!(xy.leak_cost > 0.0 && xy.leak_cost < 1.0);
    // The deepest leak is ranked first.
    assert_eq!(report.deepest_leaks[0].leak_cost, xz.leak_cost);
}

#[test]
fn crate_visible_item_used_only_locally_is_over_exposed() {
    // f is `pub(crate)` but only `g` (same module) uses it: it should be
    // private. pub(crate) is never public API, so it is always actionable.
    let ir = build(
        &[("k", None, 0), ("k::a", Some(0), 1)],
        &[
            ("k::a::f", 1, Visibility::Crate),
            ("k::a::g", 1, Visibility::Private),
        ],
        &[(1, 0)], // g uses f
    );

    let report = encapsulation(&ir).unwrap();
    assert_eq!(report.over_exposed.len(), 1);
    let exposure = &report.over_exposed[0];
    assert_eq!(exposure.item, ItemId(0));
    assert_eq!(exposure.declared, Visibility::Crate);
    assert_eq!(exposure.required, Visibility::Private);
    assert!((report.over_exposed_fraction - 0.5).abs() < 1e-9);

    // The single edge is intra-module, so there are no leaks.
    assert!(report.deepest_leaks.is_empty());
    assert_eq!(report.mean_leak_cost, 0.0);
}

#[test]
fn genuine_public_api_is_not_flagged() {
    // `api` is `pub` in a `pub` module reachable from the crate root, so it is
    // intended public API and must not be flagged even though it is used only
    // locally. `internal` is pub(crate) used only locally and stays flagged.
    let mut ir = build(
        &[("k", None, 0), ("k::a", Some(0), 1)],
        &[
            ("k::a::api", 1, Visibility::Public),
            ("k::a::internal", 1, Visibility::Crate),
            ("k::a::caller", 1, Visibility::Private),
        ],
        &[(2, 0), (2, 1)], // caller uses api and internal
    );
    // `build` leaves modules public, so `api` becomes public API.
    ir.compute_public_api();

    let report = encapsulation(&ir).unwrap();
    let flagged: Vec<_> = report.over_exposed.iter().map(|e| e.item).collect();
    assert!(
        !flagged.contains(&ItemId(0)),
        "public API must not be flagged"
    );
    assert!(
        flagged.contains(&ItemId(1)),
        "pub(crate) local-only must be flagged"
    );
}

#[test]
fn correctly_scoped_items_are_not_flagged() {
    // g is private and used locally; f is pub(crate) and used from another
    // module. Neither is over-exposed.
    let ir = build(
        &[("k", None, 0), ("k::a", Some(0), 1), ("k::b", Some(0), 1)],
        &[
            ("k::a::f", 1, Visibility::Crate),
            ("k::b::user", 2, Visibility::Private),
        ],
        &[(1, 0)], // b::user uses a::f
    );

    let report = encapsulation(&ir).unwrap();
    // f is pub(crate) and genuinely used cross-module; pub(super) would also
    // reach it from b (sibling under the root), so `required` is Super, which is
    // narrower than the declared Crate: f is reported as tightenable.
    // user is private with no consumers, so it is never flagged.
    assert!(report.over_exposed.iter().all(|e| e.item == ItemId(0)));
}
