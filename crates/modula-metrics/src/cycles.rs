//! Tangle detection on the module dependency graph. The headline measure is the
//! feedback fraction: how much directed dependency weight you would have to
//! remove to make the module graph a layerable DAG, estimated with the
//! Eades-Lin-Smyth (GR) feedback-arc-set heuristic. A pure sink (a shared
//! module everyone uses but that depends on nothing) contributes zero feedback,
//! so usage is free; only genuine mutual dependence (a tangle) is penalized,
//! which is the Acyclic Dependencies Principle. Strongly connected components
//! (Tarjan) and a cyclomatic density are kept as raw diagnostics.

use std::collections::HashSet;

use geometric_traits::{
    impls::{CSR2D, GenericBiMatrix2D, SquareCSR2D, ValuedCSR2D},
    prelude::*,
};
use modula_ir::ModuleId;
use serde::Serialize;

use crate::graph::GraphError;
use crate::module_graph::ModuleAggregation;

/// The tangles (dependency cycles) found in a module graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TangleReport {
    /// Non-trivial strongly connected components (size greater than one), each a
    /// set of mutually dependent modules. Diagnostic only.
    pub sccs: Vec<Vec<ModuleId>>,
    /// `true` when the module graph is a DAG (no feedback edges).
    pub is_acyclic: bool,
    /// Size of the largest non-trivial SCC. Diagnostic only.
    pub largest_scc: usize,
    /// Cyclomatic number (circuit rank) of the tangled subgraph: the number of
    /// independent cycles, summed over the non-trivial SCCs as `E - V + 1` per
    /// component. `0` when acyclic. Diagnostic only.
    pub cyclomatic_number: usize,
    /// Fraction of inter-module dependency weight that points backward in the
    /// GR linear arrangement, i.e. the weight you would cut to layer the graph,
    /// in `[0, 1]`. `0` for a clean DAG. This is the distance-from-DAG measure
    /// that drives the acyclicity score, robust to the dual-role-hub artifact
    /// that inflates SCC membership.
    pub feedback_fraction: f64,
    /// The backward (feedback) module references `(from, to)` GR would cut to
    /// make the graph acyclic: the actionable "break these to layer it" set.
    pub feedback_edges: Vec<(ModuleId, ModuleId)>,
}

impl TangleReport {
    /// An empty, acyclic report (no module nodes).
    fn empty() -> Self {
        Self {
            sccs: Vec::new(),
            is_acyclic: true,
            largest_scc: 0,
            cyclomatic_number: 0,
            feedback_fraction: 0.0,
            feedback_edges: Vec::new(),
        }
    }
}

/// Detects tangles in the module dependency graph.
///
/// # Errors
/// Returns [`GraphError`] if the module graph cannot be built.
pub fn tangles(agg: &ModuleAggregation) -> Result<TangleReport, GraphError> {
    let n = agg.len();
    if n == 0 {
        return Ok(TangleReport::empty());
    }

    let arcs: Vec<(usize, usize)> = agg.inter.keys().copied().collect();
    let graph: SquareCSR2D<CSR2D<usize, usize, usize>> = DiEdgesBuilder::default()
        .expected_number_of_edges(arcs.len())
        .expected_shape(n)
        .edges(arcs.iter().copied())
        .build()
        .map_err(|e| GraphError::Build(format!("{e:?}")))?;

    // Non-trivial strongly connected components, kept as a raw diagnostic.
    let components: Vec<Vec<usize>> = graph
        .tarjan()
        .filter(|component| component.len() > 1)
        .collect();
    let sccs: Vec<Vec<ModuleId>> = components
        .iter()
        .map(|component| component.iter().map(|&node| agg.nodes[node]).collect())
        .collect();
    let largest_scc = sccs.iter().map(Vec::len).max().unwrap_or(0);

    // Cyclomatic number (circuit rank): independent cycles = `E - V + 1` per SCC.
    // A strongly connected component has at least one edge per node, so
    // `edges + 1 >= nodes` and the unsigned arithmetic never underflows.
    let mut cyclomatic_number = 0;
    for component in &components {
        let members: HashSet<usize> = component.iter().copied().collect();
        let edges = arcs
            .iter()
            .filter(|(src, dst)| members.contains(src) && members.contains(dst))
            .count();
        cyclomatic_number += edges + 1 - component.len();
    }

    // Distance from a layerable DAG: the GR feedback fraction over the weighted
    // module graph. Self-loops are already excluded (intra-module weight lives
    // in `agg.intra`, not `agg.inter`), and every `inter` weight is strictly
    // positive (zero-weight edges are dropped during aggregation).
    let valued: ValuedCSR2D<usize, usize, usize, f64> =
        GenericEdgesBuilder::<_, ValuedCSR2D<usize, usize, usize, f64>>::default()
            .expected_number_of_edges(agg.inter.len())
            .expected_shape((n, n))
            .edges(agg.inter.iter().map(|(&(src, dst), &w)| (src, dst, w)))
            .build()
            .map_err(|e| GraphError::Build(format!("{e:?}")))?;
    let gr = GenericBiMatrix2D::new(valued)
        .eades_lin_smyth()
        .map_err(|e| GraphError::Build(format!("{e:?}")))?;
    let feedback_fraction = gr.tangle_fraction();
    let feedback_edges: Vec<(ModuleId, ModuleId)> = gr
        .feedback_edges()
        .iter()
        .map(|&(src, dst)| (agg.nodes[src], agg.nodes[dst]))
        .collect();
    let is_acyclic = feedback_edges.is_empty();

    Ok(TangleReport {
        sccs,
        is_acyclic,
        largest_scc,
        cyclomatic_number,
        feedback_fraction,
        feedback_edges,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use modula_ir::ModuleId;

    use super::tangles;
    use crate::module_graph::ModuleAggregation;

    /// Builds a synthetic aggregation with `n` modules and the given inter-module
    /// arcs (unit weight). Intra weights are zero.
    fn agg(n: usize, arcs: &[(usize, usize)]) -> ModuleAggregation {
        let nodes: Vec<ModuleId> = (0..n as u32).map(ModuleId).collect();
        let index_of: HashMap<ModuleId, usize> =
            nodes.iter().enumerate().map(|(i, &m)| (m, i)).collect();
        let inter: BTreeMap<(usize, usize), f64> = arcs.iter().map(|&arc| (arc, 1.0)).collect();
        ModuleAggregation {
            nodes,
            index_of,
            intra: vec![0.0; n],
            inter,
        }
    }

    #[test]
    fn acyclic_graph_has_no_tangles() {
        // A simple chain 0 -> 1 -> 2 has no cycle.
        let report = tangles(&agg(3, &[(0, 1), (1, 2)])).unwrap();
        assert!(report.is_acyclic);
        assert!(report.sccs.is_empty());
        assert_eq!(report.largest_scc, 0);
        assert_eq!(report.cyclomatic_number, 0);
        // A DAG needs no edges cut to layer it.
        assert_eq!(report.feedback_fraction, 0.0);
        assert!(report.feedback_edges.is_empty());
    }

    #[test]
    fn a_shared_sink_is_not_a_tangle() {
        // Every module depends on module 3 (a shared util), which depends on
        // nothing. That is a clean DAG with a popular sink: zero feedback, even
        // though node 3 has high fan-in. Usage must be free.
        let report = tangles(&agg(4, &[(0, 3), (1, 3), (2, 3)])).unwrap();
        assert!(report.is_acyclic);
        assert_eq!(report.feedback_fraction, 0.0);
    }

    #[test]
    fn a_ring_cuts_one_edge_of_three() {
        // A 3-cycle 0 -> 1 -> 2 -> 0 needs exactly one of its three edges cut.
        let report = tangles(&agg(3, &[(0, 1), (1, 2), (2, 0)])).unwrap();
        assert!(!report.is_acyclic);
        assert_eq!(report.feedback_edges.len(), 1);
        assert!((report.feedback_fraction - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn independent_cycles_each_add_one_to_the_cyclomatic_number() {
        // Two independent 2-cycles: two SCCs, each `E - V + 1 = 2 - 2 + 1 = 1`.
        let report = tangles(&agg(4, &[(0, 1), (1, 0), (2, 3), (3, 2)])).unwrap();
        assert_eq!(report.sccs.len(), 2, "two non-trivial SCCs");
        assert_eq!(report.largest_scc, 2);
        assert_eq!(report.cyclomatic_number, 2);
        // One edge cut per 2-cycle: 2 of 4 edges are feedback.
        assert_eq!(report.feedback_edges.len(), 2);
        assert!((report.feedback_fraction - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_denser_tangle_has_a_higher_cyclomatic_number_than_a_ring() {
        // One SCC {0,1,2} with 4 internal edges: `E - V + 1 = 4 - 3 + 1 = 2`.
        let dense = tangles(&agg(3, &[(0, 1), (1, 0), (1, 2), (2, 0)])).unwrap();
        assert_eq!(dense.sccs.len(), 1);
        assert_eq!(dense.largest_scc, 3);
        assert_eq!(dense.cyclomatic_number, 2);

        // The same three modules as a plain ring (3 edges) have rank 1: one back
        // edge away from acyclic, versus the denser tangle's two independent loops.
        let ring = tangles(&agg(3, &[(0, 1), (1, 2), (2, 0)])).unwrap();
        assert_eq!(ring.cyclomatic_number, 1);
    }
}
