//! End-to-end consistency check for the graph builders: our modularity scoring
//! of a detector's partition must equal the modularity the detector reports.
//! This validates dense indexing, weighting, and symmetrization against the
//! `geometric-traits` library's own numbers. No rust-analyzer (fast tier).

mod common;

use approx::assert_abs_diff_eq;
use common::{two_cliques_edges, two_module_graph};
use geometric_traits::prelude::*;
use modula_ir::CrateGraph;
use modula_metrics::graph::build_item_graphs;
use modula_metrics::weighting::RefKindWeights;

/// Two dense 4-cliques (items 0-3 and 4-7) joined by a single cross edge, with
/// the declared modules matching the clusters.
fn clustered() -> CrateGraph {
    two_module_graph(8, &[0, 0, 0, 0, 1, 1, 1, 1], &two_cliques_edges())
}

#[test]
fn undirected_modularity_matches_leiden() {
    let graphs = build_item_graphs(&clustered(), &RefKindWeights::default()).unwrap();
    let result = Leiden::<usize>::leiden(&graphs.undirected, &LeidenConfig::default()).unwrap();
    let scored = UndirectedModularity::<usize>::undirected_modularity(
        &graphs.undirected,
        result.final_partition(),
        1.0,
    )
    .unwrap();
    assert_abs_diff_eq!(scored, result.final_modularity(), epsilon = 1e-9);
}

#[test]
fn undirected_modularity_matches_louvain() {
    let graphs = build_item_graphs(&clustered(), &RefKindWeights::default()).unwrap();
    let result = Louvain::<usize>::louvain(&graphs.undirected, &LouvainConfig::default()).unwrap();
    let scored = UndirectedModularity::<usize>::undirected_modularity(
        &graphs.undirected,
        result.final_partition(),
        1.0,
    )
    .unwrap();
    assert_abs_diff_eq!(scored, result.final_modularity(), epsilon = 1e-9);
}

#[test]
fn directed_modularity_matches_directed_louvain() {
    let graphs = build_item_graphs(&clustered(), &RefKindWeights::default()).unwrap();
    let result =
        DirectedLouvain::<usize>::directed_louvain(&graphs.directed, &LouvainConfig::default())
            .unwrap();
    let scored = DirectedModularity::<usize>::directed_modularity(
        &graphs.directed,
        result.final_partition(),
        1.0,
    )
    .unwrap();
    assert_abs_diff_eq!(scored, result.final_modularity(), epsilon = 1e-9);
}

#[test]
fn detectors_recover_the_two_declared_modules() {
    // Sanity: on this clearly clustered graph the undirected detector should put
    // items 0-3 together and 4-7 together (two communities).
    let graphs = build_item_graphs(&clustered(), &RefKindWeights::default()).unwrap();
    let result = Leiden::<usize>::leiden(&graphs.undirected, &LeidenConfig::default()).unwrap();
    let p = result.final_partition();
    assert_eq!(p[0], p[1]);
    assert_eq!(p[0], p[2]);
    assert_eq!(p[0], p[3]);
    assert_eq!(p[4], p[5]);
    assert_ne!(p[0], p[4]);
}
