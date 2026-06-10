//! Tests the induced modularity profile: a module layout that matches the
//! dependency clusters scores high efficiency; one that cuts across them scores
//! low. No rust-analyzer (fast tier).

mod common;

use common::{two_cliques_edges, two_module_graph};
use modula_metrics::graph::build_item_graphs;
use modula_metrics::modularity::{ModularityConfig, modularity_profile};
use modula_metrics::weighting::RefKindWeights;

fn profile_at_depth_1(module_of: &[usize]) -> modula_metrics::modularity::DepthRecord {
    let ir = two_module_graph(8, module_of, &two_cliques_edges());
    let graphs = build_item_graphs(&ir, &RefKindWeights::default()).unwrap();
    let profile = modularity_profile(&ir, &graphs, &ModularityConfig::default()).unwrap();
    // Depths 0 and 1 are present (max depth is 1).
    assert_eq!(profile.len(), 2);
    assert_eq!(profile[0].depth, 0);
    *profile
        .iter()
        .find(|r| r.depth == 1)
        .expect("depth-1 record")
}

#[test]
fn aligned_modules_score_high_efficiency() {
    // Declared modules == dependency clusters: items 0-3 in a, 4-7 in b.
    let record = profile_at_depth_1(&[0, 0, 0, 0, 1, 1, 1, 1]);
    assert_eq!(record.communities_declared, 2);

    // The declared partition is (essentially) the detector's own optimum, so
    // its modularity is positive and efficiency is near 1.
    assert!(record.q_declared_undirected > 0.0);
    let eff = record.efficiency_undirected.expect("positive detected Q");
    assert!(eff > 0.95, "expected high efficiency, got {eff}");
}

#[test]
fn crossing_modules_score_low_efficiency() {
    // Declared modules cut across the clusters: a = {0,1,4,5}, b = {2,3,6,7},
    // while the dependencies cluster as {0,1,2,3} and {4,5,6,7}.
    let record = profile_at_depth_1(&[0, 0, 1, 1, 0, 0, 1, 1]);
    assert_eq!(record.communities_declared, 2);

    // The declared partition fights the dependency structure, so its modularity
    // is far below the detector optimum (here even negative).
    let aligned = profile_at_depth_1(&[0, 0, 0, 0, 1, 1, 1, 1]);
    assert!(record.q_declared_undirected < aligned.q_declared_undirected);
    let eff = record.efficiency_undirected.unwrap_or(0.0);
    assert!(eff < 0.5, "expected low efficiency, got {eff}");
}

#[test]
fn directed_profile_is_populated() {
    let record = profile_at_depth_1(&[0, 0, 0, 0, 1, 1, 1, 1]);
    // The directed scoring path is exercised and agrees in spirit with the
    // undirected one on this symmetric-ish graph.
    assert!(record.q_declared_directed > 0.0);
    assert!(record.q_detected_directed > 0.0);
    assert!(record.efficiency_directed.is_some());
}
