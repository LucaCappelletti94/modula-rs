//! Tangle detection on the module dependency graph: strongly connected
//! components (Tarjan) and elementary circuits (Johnson). A cyclic module
//! dependency violates the Acyclic Dependencies Principle.

use std::collections::HashMap;

use geometric_traits::{
    impls::{CSR2D, SquareCSR2D},
    prelude::*,
};
use modula_ir::ModuleId;
use serde::Serialize;

use crate::graph::GraphError;
use crate::module_graph::ModuleAggregation;

/// Configuration for cycle detection.
#[derive(Clone, Copy, Debug)]
pub struct CyclesConfig {
    /// Maximum number of elementary circuits to enumerate per tangle (SCC).
    /// Johnson is worst-case exponential, so this caps the work; applying it per
    /// tangle keeps every tangle represented rather than letting one large
    /// component exhaust a single global budget.
    pub max_circuits: usize,
}

impl Default for CyclesConfig {
    fn default() -> Self {
        Self {
            max_circuits: 1_000,
        }
    }
}

/// The tangles (dependency cycles) found in a module graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TangleReport {
    /// Non-trivial strongly connected components (size greater than one), each a
    /// set of mutually dependent modules.
    pub sccs: Vec<Vec<ModuleId>>,
    /// Elementary circuits across all tangles (up to the per-tangle cap each).
    pub circuits: Vec<Vec<ModuleId>>,
    /// `true` when no non-trivial SCC exists.
    pub is_acyclic: bool,
    /// Size of the largest non-trivial SCC.
    pub largest_scc: usize,
    /// `true` when any tangle's circuit enumeration hit the per-tangle cap.
    pub circuits_truncated: bool,
}

impl TangleReport {
    /// An empty, acyclic report (no module nodes).
    fn empty() -> Self {
        Self {
            sccs: Vec::new(),
            circuits: Vec::new(),
            is_acyclic: true,
            largest_scc: 0,
            circuits_truncated: false,
        }
    }
}

/// Detects tangles in the module dependency graph.
///
/// # Errors
/// Returns [`GraphError`] if the module graph cannot be built.
pub fn tangles(agg: &ModuleAggregation, config: &CyclesConfig) -> Result<TangleReport, GraphError> {
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

    let to_modules = |component: &[usize]| -> Vec<ModuleId> {
        component.iter().map(|&node| agg.nodes[node]).collect()
    };

    // Elementary circuits only exist within a strongly connected component, so
    // enumerate them per non-trivial SCC. The cap applies per tangle, which keeps
    // every tangle represented instead of letting one large SCC exhaust a single
    // global budget.
    let components: Vec<Vec<usize>> = graph
        .tarjan()
        .filter(|component| component.len() > 1)
        .collect();
    let sccs: Vec<Vec<ModuleId>> = components.iter().map(|c| to_modules(c)).collect();
    let largest_scc = sccs.iter().map(Vec::len).max().unwrap_or(0);
    let is_acyclic = sccs.is_empty();

    let mut circuits = Vec::new();
    let mut circuits_truncated = false;
    for component in &components {
        // Index of a full-graph node within this component's induced subgraph.
        let local_of: HashMap<usize, usize> = component
            .iter()
            .enumerate()
            .map(|(local, &node)| (node, local))
            .collect();
        let mut sub_arcs: Vec<(usize, usize)> = arcs
            .iter()
            .filter(|(src, dst)| local_of.contains_key(src) && local_of.contains_key(dst))
            .map(|(src, dst)| (local_of[src], local_of[dst]))
            .collect();
        sub_arcs.sort_unstable();

        let subgraph: SquareCSR2D<CSR2D<usize, usize, usize>> = DiEdgesBuilder::default()
            .expected_number_of_edges(sub_arcs.len())
            .expected_shape(component.len())
            .edges(sub_arcs.iter().copied())
            .build()
            .map_err(|e| GraphError::Build(format!("{e:?}")))?;

        for (found, circuit) in subgraph.johnson().enumerate() {
            if found >= config.max_circuits {
                circuits_truncated = true;
                break;
            }
            // Map the subgraph's local indices back to module ids.
            circuits.push(
                circuit
                    .iter()
                    .map(|&local| agg.nodes[component[local]])
                    .collect(),
            );
        }
    }

    Ok(TangleReport {
        sccs,
        circuits,
        is_acyclic,
        largest_scc,
        circuits_truncated,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use modula_ir::ModuleId;

    use super::{CyclesConfig, tangles};
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
    fn per_tangle_cap_keeps_every_tangle_represented() {
        // Two independent 2-cycles: two SCCs, each with exactly one circuit.
        let a = agg(4, &[(0, 1), (1, 0), (2, 3), (3, 2)]);
        let report = tangles(&a, &CyclesConfig { max_circuits: 1 }).unwrap();
        assert_eq!(report.sccs.len(), 2, "two non-trivial SCCs");
        // A per-tangle cap lets both tangles contribute their circuit even at
        // max_circuits = 1; a single global cap would surface only one.
        assert_eq!(
            report.circuits.len(),
            2,
            "each tangle should be represented under the cap"
        );
        assert!(!report.circuits_truncated, "neither tangle exceeds the cap");
    }

    #[test]
    fn acyclic_graph_has_no_tangles() {
        // A simple chain 0 -> 1 -> 2 has no cycle.
        let a = agg(3, &[(0, 1), (1, 2)]);
        let report = tangles(&a, &CyclesConfig::default()).unwrap();
        assert!(report.is_acyclic);
        assert!(report.sccs.is_empty());
        assert!(report.circuits.is_empty());
        assert_eq!(report.largest_scc, 0);
    }

    #[test]
    fn per_tangle_cap_flags_truncation() {
        // One SCC {0,1,2} with two elementary circuits: 0->1->0 and 0->1->2->0.
        let a = agg(3, &[(0, 1), (1, 0), (1, 2), (2, 0)]);
        let report = tangles(&a, &CyclesConfig { max_circuits: 1 }).unwrap();
        assert_eq!(report.sccs.len(), 1);
        assert_eq!(report.circuits.len(), 1, "capped at one circuit");
        assert!(report.circuits_truncated, "the tangle exceeds the cap");
    }
}
