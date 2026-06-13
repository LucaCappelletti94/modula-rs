//! End-to-end analysis: composite score behavior, edge cases, gates, and
//! rendering. No rust-analyzer (fast tier).

mod common;

use common::{two_cliques_edges, two_module_graph};
use modula_ir::{
    Crate, CrateGraph, CrateId, Module, ModuleId, ModuleKind, SCHEMA_VERSION, Visibility,
};
use modula_metrics::analysis::{AnalysisConfig, analyze};
use modula_metrics::report::{Gates, evaluate_gates, to_human, to_json};

fn aligned() -> CrateGraph {
    two_module_graph(8, &[0, 0, 0, 0, 1, 1, 1, 1], &two_cliques_edges())
}

fn crossing() -> CrateGraph {
    two_module_graph(8, &[0, 0, 1, 1, 0, 0, 1, 1], &two_cliques_edges())
}

fn empty_crate() -> CrateGraph {
    let krate = CrateId(0);
    CrateGraph {
        schema_version: SCHEMA_VERSION,
        ra_version: String::new(),
        root_crate: krate,
        crates: vec![Crate {
            id: krate,
            name: "empty".to_owned(),
            is_local: true,
            root_module: ModuleId(0),
        }],
        modules: vec![Module {
            id: ModuleId(0),
            crate_id: krate,
            parent: None,
            name: String::new(),
            canonical_path: "empty".to_owned(),
            depth: 0,
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        }],
        items: Vec::new(),
        edges: Vec::new(),
    }
}

#[test]
fn aligned_scores_higher_than_crossing() {
    let clean = analyze(&aligned(), &AnalysisConfig::default()).unwrap();
    let leaky = analyze(&crossing(), &AnalysisConfig::default()).unwrap();
    let clean_h = clean
        .composite
        .headline
        .expect("aligned crate has structure");
    let leaky_h = leaky
        .composite
        .headline
        .expect("crossing crate has structure");
    assert!(clean_h > leaky_h, "clean {clean_h} !> leaky {leaky_h}");
    assert!(clean_h > 0.0 && clean_h <= 1.0);
}

#[test]
fn cyclic_module_graph_lowers_acyclicity() {
    let mut edges = two_cliques_edges();
    edges.push((4, 3)); // back edge b -> a
    let ir = two_module_graph(8, &[0, 0, 0, 0, 1, 1, 1, 1], &edges);
    let result = analyze(&ir, &AnalysisConfig::default()).unwrap();
    assert!(!result.tangles.is_acyclic);
    assert!(result.composite.acyclicity_term < 1.0);
}

#[test]
fn type_level_gives_a_flat_crate_a_partition() {
    use modula_ir::{Edge, Item, ItemId, ItemKind, RefKind};
    // A single `mod` (the crate root) with two type containers Foo and Bar, each
    // owning a struct + a method. Without the type level this would be one
    // community (vacuous high score); with it, the type depth has two.
    let krate = CrateId(0);
    let m = |id: u32, parent: Option<u32>, depth: u32, kind: ModuleKind| Module {
        id: ModuleId(id),
        crate_id: krate,
        parent: parent.map(ModuleId),
        name: format!("m{id}"),
        canonical_path: format!("k::m{id}"),
        depth,
        visibility: Visibility::Public,
        kind,
    };
    let it = |id: u32, owner: u32, kind: ItemKind| Item {
        id: ItemId(id),
        canonical_path: format!("k::i{id}"),
        kind,
        visibility: Visibility::Public,
        owning_module: ModuleId(owner),
        crate_id: krate,
        has_canonical_path: true,
        reachable_pub_api: false,
    };
    let body = |from: u32, to: u32| Edge {
        from: ItemId(from),
        to: ItemId(to),
        kind: RefKind::Body,
        weight: 1,
    };
    let ir = CrateGraph {
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
            m(0, None, 0, ModuleKind::Mod),
            m(1, Some(0), 1, ModuleKind::Type),
            m(2, Some(0), 1, ModuleKind::Type),
        ],
        items: vec![
            it(0, 1, ItemKind::Struct),  // Foo
            it(1, 1, ItemKind::AssocFn), // Foo::method
            it(2, 2, ItemKind::Struct),  // Bar
            it(3, 2, ItemKind::AssocFn), // Bar::method
        ],
        // Intra-type cohesion plus one cross-type edge.
        edges: vec![body(1, 0), body(3, 2), body(3, 1)],
    };
    let result = analyze(&ir, &AnalysisConfig::default()).unwrap();
    // The crate is one real module, but the type depth yields >= 2 communities,
    // so the partition is no longer trivial.
    assert_eq!(result.n_module_nodes, 1, "one real module");
    assert!(
        result
            .modularity_profile
            .iter()
            .any(|r| r.communities_declared >= 2),
        "the type level must give a non-trivial partition"
    );
}

#[test]
fn graph_with_module_items_scores_cleanly() {
    use modula_ir::{Edge, Item, ItemId, ItemKind, RefKind};
    // Two modules, each represented as a Module item owning itself, plus a
    // function in each, with an import edge between the module items.
    let krate = CrateId(0);
    let module = |id: u32, path: &str, parent: Option<u32>| Module {
        id: ModuleId(id),
        crate_id: krate,
        parent: parent.map(ModuleId),
        name: path.rsplit("::").next().unwrap_or(path).to_owned(),
        canonical_path: path.to_owned(),
        depth: u32::from(parent.is_some()),
        visibility: Visibility::Public,
        kind: ModuleKind::Mod,
    };
    let item = |id: u32, path: &str, owner: u32, kind: ItemKind| Item {
        id: ItemId(id),
        canonical_path: path.to_owned(),
        kind,
        visibility: Visibility::Public,
        owning_module: ModuleId(owner),
        crate_id: krate,
        has_canonical_path: true,
        reachable_pub_api: false,
    };
    let ir = CrateGraph {
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
            module(0, "k", None),
            module(1, "k::a", Some(0)),
            module(2, "k::b", Some(0)),
        ],
        items: vec![
            item(0, "k::a", 1, ItemKind::Module),
            item(1, "k::a::f", 1, ItemKind::Function),
            item(2, "k::b", 2, ItemKind::Module),
            item(3, "k::b::g", 2, ItemKind::Function),
        ],
        edges: vec![
            Edge {
                from: ItemId(2),
                to: ItemId(1),
                kind: RefKind::Import,
                weight: 1,
            },
            Edge {
                from: ItemId(3),
                to: ItemId(1),
                kind: RefKind::Body,
                weight: 1,
            },
        ],
    };

    let result = analyze(&ir, &AnalysisConfig::default()).expect("module-item graph analyzes");
    assert!(
        result
            .composite
            .headline
            .expect("two real items in two modules give structure")
            .is_finite()
    );
    assert_eq!(result.n_items, 4);
    // The two `ItemKind::Module` stubs are excluded; only the two functions are
    // real graph nodes.
    assert_eq!(result.n_real_items, 2);
}

#[test]
fn empty_crate_is_handled() {
    let result = analyze(&empty_crate(), &AnalysisConfig::default()).unwrap();
    assert_eq!(result.n_items, 0);
    assert_eq!(result.n_real_items, 0);
    assert!(result.modularity_profile.is_empty());
    assert!(result.divergence_profile.is_empty());
    assert!(result.tangles.is_acyclic);
    // No measurable structure: the headline is N/A, not a vacuous number.
    assert!(result.composite.headline.is_none());
}

#[test]
fn json_and_human_outputs_render() {
    let result = analyze(&aligned(), &AnalysisConfig::default()).unwrap();

    let json = to_json(&result).unwrap();
    assert!(json.contains("\"headline\""));
    assert!(json.contains("\"modularity_profile\""));

    let human = to_human(&result);
    assert!(human.contains("Headline score"));
    assert!(human.is_ascii(), "human report must be ASCII");
}

#[test]
fn gates_pass_and_fail_as_configured() {
    let result = analyze(&aligned(), &AnalysisConfig::default()).unwrap();

    let lenient = Gates {
        min_headline: Some(0.0),
        require_acyclic: true,
        max_overexposed_fraction: None,
    };
    assert!(evaluate_gates(&result, &lenient).passed);

    let strict = Gates {
        min_headline: Some(1.01),
        ..Default::default()
    };
    let outcome = evaluate_gates(&result, &strict);
    assert!(!outcome.passed);
    assert!(outcome.results.iter().any(|r| !r.passed));
}

#[test]
fn human_report_snapshot() {
    let result = analyze(&aligned(), &AnalysisConfig::default()).unwrap();
    insta::assert_snapshot!(to_human(&result));
}
