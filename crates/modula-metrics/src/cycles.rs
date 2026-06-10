//! Tangle detection on the module dependency graph: strongly connected
//! components (Tarjan) and elementary circuits (Johnson). A cyclic module
//! dependency violates the Acyclic Dependencies Principle.

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
    /// Maximum number of elementary circuits to enumerate (Johnson is worst-case
    /// exponential, so this caps the work).
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
    /// Elementary circuits (up to the configured cap).
    pub circuits: Vec<Vec<ModuleId>>,
    /// `true` when no non-trivial SCC exists.
    pub is_acyclic: bool,
    /// Size of the largest non-trivial SCC.
    pub largest_scc: usize,
    /// `true` when the circuit enumeration hit the cap and stopped early.
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

    let sccs: Vec<Vec<ModuleId>> = graph
        .tarjan()
        .filter(|component| component.len() > 1)
        .map(|component| to_modules(&component))
        .collect();
    let largest_scc = sccs.iter().map(Vec::len).max().unwrap_or(0);
    let is_acyclic = sccs.is_empty();

    let mut circuits = Vec::new();
    let mut circuits_truncated = false;
    for circuit in graph.johnson() {
        if circuits.len() >= config.max_circuits {
            circuits_truncated = true;
            break;
        }
        circuits.push(to_modules(&circuit));
    }

    Ok(TangleReport {
        sccs,
        circuits,
        is_acyclic,
        largest_scc,
        circuits_truncated,
    })
}
