//! Tests per-module coupling/cohesion and tangle detection. No rust-analyzer.

mod common;

use common::{two_cliques_edges, two_module_graph};
use modula_metrics::coupling::{ModuleCoupling, module_coupling};
use modula_metrics::cycles::tangles;
use modula_metrics::module_graph::ModuleAggregation;
use modula_metrics::weighting::RefKindWeights;

/// Clean layout: items 0-3 in module a, 4-7 in module b, with a single a -> b
/// cross edge (item 3 -> item 4).
fn clean() -> modula_ir::CrateGraph {
    two_module_graph(8, &[0, 0, 0, 0, 1, 1, 1, 1], &two_cliques_edges())
}

fn find<'a>(records: &'a [ModuleCoupling], path: &str) -> &'a ModuleCoupling {
    records
        .iter()
        .find(|r| r.path == path)
        .expect("module record")
}

#[test]
fn coupling_captures_direction_and_instability() {
    let ir = clean();
    let agg = ModuleAggregation::build(&ir, &RefKindWeights::default());
    let records = module_coupling(&ir, &agg);

    let a = find(&records, "k::a");
    let b = find(&records, "k::b");

    // The only cross edge is a -> b, so a is efferent-only and b afferent-only.
    assert_eq!(a.ce, 1);
    assert_eq!(a.ca, 0);
    assert_eq!(a.instability, Some(1.0));
    assert_eq!(b.ca, 1);
    assert_eq!(b.ce, 0);
    assert_eq!(b.instability, Some(0.0));

    // Both modules are internally dense, so cohesion is high.
    assert!(a.cohesion.unwrap() > 0.8, "cohesion = {:?}", a.cohesion);
    assert!(b.cohesion.unwrap() > 0.8);
}

#[test]
fn clean_module_graph_is_acyclic() {
    let ir = clean();
    let agg = ModuleAggregation::build(&ir, &RefKindWeights::default());
    let report = tangles(&agg).unwrap();
    assert!(report.is_acyclic);
    assert_eq!(report.largest_scc, 0);
    assert!(report.sccs.is_empty());
    assert_eq!(report.cyclomatic_number, 0);
}

#[test]
fn mutual_dependency_is_a_tangle() {
    // Add a back edge b -> a (item 4 -> item 3): now a and b depend on each
    // other, forming one strongly connected component.
    let mut edges = two_cliques_edges();
    edges.push((4, 3));
    let ir = two_module_graph(8, &[0, 0, 0, 0, 1, 1, 1, 1], &edges);
    let agg = ModuleAggregation::build(&ir, &RefKindWeights::default());
    let report = tangles(&agg).unwrap();

    assert!(!report.is_acyclic);
    assert_eq!(report.largest_scc, 2);
    assert_eq!(report.sccs.len(), 1);
    assert_eq!(report.sccs[0].len(), 2);
    // The mutual a <-> b dependency is exactly one independent cycle.
    assert_eq!(report.cyclomatic_number, 1);
}
