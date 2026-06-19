//! Top-level orchestration: run every metric over an IR and assemble the full
//! analysis result, including the composite score.

use modula_ir::CrateGraph;
use serde::Serialize;

use crate::cohesion::cohesion_lift;
use crate::coupling::{ModuleCoupling, module_coupling};
use crate::cycles::{TangleReport, tangles};
use crate::encapsulation::{EncapsulationReport, encapsulation};
use crate::module_graph::ModuleAggregation;
use crate::score::{CompositeScore, CompositeWeights, composite_score};
use crate::weighting::RefKindWeights;

/// Configuration for a full analysis.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnalysisConfig {
    /// Edge weighting policy.
    pub weights: RefKindWeights,
    /// Composite-score weights.
    pub composite: CompositeWeights,
}

/// An error during analysis.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// A graph could not be built.
    #[error(transparent)]
    Graph(#[from] crate::graph::GraphError),
}

/// The complete analysis of a crate graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnalysisResult {
    /// Name of the root crate.
    pub crate_name: String,
    /// Number of items analyzed.
    pub n_items: usize,
    /// Number of real (graph-participating) items: `n_items` minus the
    /// `ItemKind::Module` stub nodes. A crate whose only items are module stubs
    /// (a pure re-export facade) has `n_real_items == 0` and no measurable
    /// internal structure.
    pub n_real_items: usize,
    /// Number of modules in the tree.
    pub n_modules: usize,
    /// Number of module nodes in the dependency graph (modules owning items).
    pub n_module_nodes: usize,
    /// Per-module coupling and cohesion.
    pub modules: Vec<ModuleCoupling>,
    /// Module dependency cycles.
    pub tangles: TangleReport,
    /// Encapsulation report.
    pub encapsulation: EncapsulationReport,
    /// The composite headline score.
    pub composite: CompositeScore,
}

/// Runs the full analysis pipeline over `ir`.
///
/// # Errors
/// Returns [`AnalysisError`] if any stage fails.
pub fn analyze(ir: &CrateGraph, config: &AnalysisConfig) -> Result<AnalysisResult, AnalysisError> {
    let crate_name = ir.krate(ir.root_crate).name.clone();
    let n_items = ir.items.len();
    let n_real_items = ir.n_real_items();
    let n_modules = ir.modules.len();

    let agg = ModuleAggregation::build(ir, &config.weights);
    let n_module_nodes = agg.len();
    let modules = module_coupling(ir, &agg);
    let tangles = tangles(&agg)?;
    let encapsulation = encapsulation(ir);
    let cohesion = cohesion_lift(ir, &agg);

    let composite = composite_score(cohesion, &tangles, &encapsulation, &config.composite);

    Ok(AnalysisResult {
        crate_name,
        n_items,
        n_real_items,
        n_modules,
        n_module_nodes,
        modules,
        tangles,
        encapsulation,
        composite,
    })
}
