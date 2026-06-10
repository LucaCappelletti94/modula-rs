//! Builders that lower the IR dependency edges into `geometric-traits` weighted
//! graphs.
//!
//! Item ids in the IR are already dense (`items[i].id == ItemId(i)`), so a node
//! index is just the item id and no separate bijection is needed at item level.
//!
//! Two graphs are produced from the same directed arcs:
//! - the directed item graph `D` (for directed modularity and detectors), and
//! - the symmetrized undirected graph `U` (for undirected modularity and
//!   detectors), where `A_ij = A_ji = w(i,j) + w(j,i)`.

use std::collections::HashMap;

use geometric_traits::{impls::ValuedCSR2D, prelude::*};
use modula_ir::CrateGraph;

use crate::weighting::RefKindWeights;

/// The weighted graph type consumed by the `geometric-traits` algorithms.
pub type WeightedGraph = ValuedCSR2D<usize, usize, usize, f64>;

/// An error building a `geometric-traits` graph from arcs.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// The underlying edge builder rejected the arcs.
    #[error("failed to build graph: {0}")]
    Build(String),
}

/// The directed and undirected item graphs for a crate graph.
pub struct ItemGraphs {
    /// Number of item nodes.
    pub n: usize,
    /// Directed item graph `D` (arc weights are `w(src, dst)`).
    pub directed: WeightedGraph,
    /// Symmetrized undirected graph `U`.
    pub undirected: WeightedGraph,
}

/// Builds the directed and undirected item graphs under the given weighting.
///
/// # Errors
/// Returns [`GraphError`] if the edge builder rejects the arcs.
pub fn build_item_graphs(
    graph: &CrateGraph,
    weights: &RefKindWeights,
) -> Result<ItemGraphs, GraphError> {
    let n = graph.items.len();
    let directed = directed_arcs(graph, weights);
    let undirected = symmetrize(&directed);
    Ok(ItemGraphs {
        n,
        directed: build(n, &directed)?,
        undirected: build(n, &undirected)?,
    })
}

/// Collapses all IR edges into one directed arc per `(src, dst)`, summing
/// weights across reference kinds. Arcs with non-positive weight are dropped
/// (modularity requires strictly positive weights).
fn directed_arcs(graph: &CrateGraph, weights: &RefKindWeights) -> Vec<(usize, usize, f64)> {
    let mut map: HashMap<(usize, usize), f64> = HashMap::new();
    for edge in &graph.edges {
        let w = weights.edge_weight(edge);
        if w > 0.0 {
            *map.entry((edge.from.index(), edge.to.index()))
                .or_insert(0.0) += w;
        }
    }
    sorted(map.into_iter().map(|((s, d), w)| (s, d, w)))
}

/// Symmetrizes directed arcs into an undirected graph: each unordered pair gets
/// the sum of both directions, emitted as two arcs so the matrix is structurally
/// symmetric. Self-loops are emitted once.
fn symmetrize(directed: &[(usize, usize, f64)]) -> Vec<(usize, usize, f64)> {
    let mut pairs: HashMap<(usize, usize), f64> = HashMap::new();
    let mut loops: HashMap<usize, f64> = HashMap::new();
    for &(s, d, w) in directed {
        if s == d {
            *loops.entry(s).or_insert(0.0) += w;
        } else {
            *pairs.entry((s.min(d), s.max(d))).or_insert(0.0) += w;
        }
    }
    let mut arcs = Vec::with_capacity(pairs.len() * 2 + loops.len());
    for ((a, b), w) in pairs {
        arcs.push((a, b, w));
        arcs.push((b, a, w));
    }
    for (s, w) in loops {
        arcs.push((s, s, w));
    }
    sorted(arcs.into_iter())
}

/// Sorts arcs by `(src, dst)` so graph construction is deterministic.
fn sorted(arcs: impl Iterator<Item = (usize, usize, f64)>) -> Vec<(usize, usize, f64)> {
    let mut arcs: Vec<_> = arcs.collect();
    arcs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    arcs
}

/// Builds an `n`-node weighted graph from `(src, dst, weight)` arcs.
fn build(n: usize, arcs: &[(usize, usize, f64)]) -> Result<WeightedGraph, GraphError> {
    GenericEdgesBuilder::<_, WeightedGraph>::default()
        .expected_number_of_edges(arcs.len())
        .expected_shape((n, n))
        .edges(arcs.iter().copied())
        .build()
        .map_err(|e| GraphError::Build(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetrize_sums_both_directions_and_keeps_self_loops() {
        // Asymmetric pair 0<->1 (2.0 + 3.0) plus a self-loop on 2.
        let directed = vec![(0usize, 1usize, 2.0f64), (1, 0, 3.0), (2, 2, 1.0)];
        let u = symmetrize(&directed);
        assert!(u.contains(&(0, 1, 5.0)));
        assert!(u.contains(&(1, 0, 5.0)));
        assert!(u.contains(&(2, 2, 1.0)));
        assert_eq!(u.len(), 3);
    }

    #[test]
    fn symmetrize_is_structurally_symmetric() {
        let directed = vec![(0usize, 1usize, 4.0f64)];
        let u = symmetrize(&directed);
        // A single directed arc becomes both directions with equal weight.
        assert!(u.contains(&(0, 1, 4.0)));
        assert!(u.contains(&(1, 0, 4.0)));
    }
}
