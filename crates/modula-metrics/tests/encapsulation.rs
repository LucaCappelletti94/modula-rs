//! Tests encapsulation: the interface-legitimacy leak rate (a cross-module
//! reference to a non-API item is a leak, to a public-API item is not) and
//! over-exposure detection. No rust-analyzer (fast tier).

use modula_ir::{
    Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
    RefKind, SCHEMA_VERSION, Visibility,
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
            kind: ModuleKind::Mod,
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
fn leak_rate_is_fraction_of_cross_module_refs_into_internals() {
    // Tree: k > {a, b}. a::caller references b::api (public API) and b::internal
    // (pub(crate), not API). Only the reach into b's internals is a leak, so the
    // leak rate over the two cross-module references is 1/2.
    let mut ir = build(
        &[("k", None, 0), ("k::a", Some(0), 1), ("k::b", Some(0), 1)],
        &[
            ("k::a::caller", 1, Visibility::Private),
            ("k::b::api", 2, Visibility::Public),
            ("k::b::internal", 2, Visibility::Crate),
        ],
        &[(0, 1), (0, 2)], // caller -> api (legit), caller -> internal (leak)
    );
    // Marks `b::api` reachable_pub_api (public module + public item from root).
    ir.compute_public_api();

    let report = encapsulation(&ir);
    assert_eq!(report.n_cross_module_refs, 2, "both refs are cross-module");
    assert_eq!(report.leaks.len(), 1, "only the internal reach is a leak");
    assert_eq!(
        report.leaks[0].to,
        ItemId(2),
        "the leak targets b::internal"
    );
    assert_eq!(report.leaks[0].target_visibility, Visibility::Crate);
    assert!(
        (report.mean_leak_cost - 0.5).abs() < 1e-9,
        "leak rate = 1/2"
    );
}

#[test]
fn all_cross_module_refs_through_public_api_have_zero_leak_rate() {
    // Every cross-module reference targets a public-API item: MII = 1, leak 0.
    let mut ir = build(
        &[("k", None, 0), ("k::a", Some(0), 1), ("k::b", Some(0), 1)],
        &[
            ("k::a::caller", 1, Visibility::Private),
            ("k::b::api", 2, Visibility::Public),
        ],
        &[(0, 1)],
    );
    ir.compute_public_api();
    let report = encapsulation(&ir);
    assert_eq!(report.n_cross_module_refs, 1);
    assert!(report.leaks.is_empty());
    assert_eq!(report.mean_leak_cost, 0.0);
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

    let report = encapsulation(&ir);
    assert_eq!(report.over_exposed.len(), 1);
    let exposure = &report.over_exposed[0];
    assert_eq!(exposure.item, ItemId(0));
    assert_eq!(exposure.declared, Visibility::Crate);
    assert_eq!(exposure.required, Visibility::Private);
    assert!((report.over_exposed_fraction - 0.5).abs() < 1e-9);

    // The single edge is intra-module, so there are no leaks.
    assert!(report.leaks.is_empty());
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

    let report = encapsulation(&ir);
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

    let report = encapsulation(&ir);
    // f is pub(crate) and genuinely used cross-module; pub(super) would also
    // reach it from b (sibling under the root), so `required` is Super, which is
    // narrower than the declared Crate: f is reported as tightenable.
    // user is private with no consumers, so it is never flagged.
    assert!(report.over_exposed.iter().all(|e| e.item == ItemId(0)));
}
