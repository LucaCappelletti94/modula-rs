//! Tangle detection on the module dependency graph: strongly connected
//! components (Tarjan) plus a cyclomatic density measure. A cyclic module
//! dependency violates the Acyclic Dependencies Principle.

use std::collections::HashSet;

use geometric_traits::{
    impls::{CSR2D, SquareCSR2D},
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
    /// set of mutually dependent modules.
    pub sccs: Vec<Vec<ModuleId>>,
    /// `true` when no non-trivial SCC exists.
    pub is_acyclic: bool,
    /// Size of the largest non-trivial SCC.
    pub largest_scc: usize,
    /// Cyclomatic number (circuit rank) of the tangled subgraph: the number of
    /// independent cycles, summed over the non-trivial SCCs as `E - V + 1` per
    /// component. `0` when acyclic. A single back-edge ring contributes `1`; a
    /// densely cross-referenced tangle contributes more, so it measures how
    /// interwoven the tangles are, not just how large.
    pub cyclomatic_number: usize,
}

impl TangleReport {
    /// An empty, acyclic report (no module nodes).
    fn empty() -> Self {
        Self {
            sccs: Vec::new(),
            is_acyclic: true,
            largest_scc: 0,
            cyclomatic_number: 0,
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

    // Non-trivial strongly connected components are the tangles.
    let components: Vec<Vec<usize>> = graph
        .tarjan()
        .filter(|component| component.len() > 1)
        .collect();
    let sccs: Vec<Vec<ModuleId>> = components
        .iter()
        .map(|component| component.iter().map(|&node| agg.nodes[node]).collect())
        .collect();
    let largest_scc = sccs.iter().map(Vec::len).max().unwrap_or(0);
    let is_acyclic = sccs.is_empty();

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

    Ok(TangleReport {
        sccs,
        is_acyclic,
        largest_scc,
        cyclomatic_number,
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
    }

    #[test]
    fn independent_cycles_each_add_one_to_the_cyclomatic_number() {
        // Two independent 2-cycles: two SCCs, each `E - V + 1 = 2 - 2 + 1 = 1`.
        let report = tangles(&agg(4, &[(0, 1), (1, 0), (2, 3), (3, 2)])).unwrap();
        assert_eq!(report.sccs.len(), 2, "two non-trivial SCCs");
        assert_eq!(report.largest_scc, 2);
        assert_eq!(report.cyclomatic_number, 2);
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
