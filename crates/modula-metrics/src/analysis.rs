//! Top-level orchestration: run every metric over an IR and assemble the full
//! analysis result, including the composite score.

use modula_ir::CrateGraph;
use serde::Serialize;

use crate::coupling::{ModuleCoupling, module_coupling};
use crate::cycles::{TangleReport, tangles};
use crate::encapsulation::{EncapsulationReport, encapsulation};
use crate::graph::build_item_graphs;
use crate::modularity::{DepthRecord, DivergenceRecord, ModularityConfig, profiles};
use crate::module_graph::ModuleAggregation;
use crate::score::{CompositeScore, CompositeWeights, composite_score};
use crate::weighting::RefKindWeights;

/// Configuration for a full analysis.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnalysisConfig {
    /// Edge weighting policy.
    pub weights: RefKindWeights,
    /// Modularity / detector configuration.
    pub modularity: ModularityConfig,
    /// Composite-score weights.
    pub composite: CompositeWeights,
}

/// An error during analysis.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// A graph could not be built.
    #[error(transparent)]
    Graph(#[from] crate::graph::GraphError),
    /// Modularity scoring or detection failed.
    #[error("modularity analysis failed: {0:?}")]
    Modularity(geometric_traits::traits::algorithms::ModularityError),
    /// Encapsulation analysis failed.
    #[error(transparent)]
    Encapsulation(#[from] crate::encapsulation::EncapsulationError),
}

impl From<geometric_traits::traits::algorithms::ModularityError> for AnalysisError {
    fn from(error: geometric_traits::traits::algorithms::ModularityError) -> Self {
        AnalysisError::Modularity(error)
    }
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
    /// Modularity efficiency profile over depth.
    pub modularity_profile: Vec<DepthRecord>,
    /// Divergence profile over depth.
    pub divergence_profile: Vec<DivergenceRecord>,
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
    let encapsulation = encapsulation(ir)?;

    // Modularity and divergence need the item graphs; a crate with no real
    // items (empty, or a pure module-stub facade) has none.
    let (modularity_profile, divergence_profile) = if n_real_items == 0 {
        (Vec::new(), Vec::new())
    } else {
        let graphs = build_item_graphs(ir, &config.weights)?;
        let p = profiles(ir, &graphs, &config.modularity)?;
        (p.modularity, p.divergence)
    };

    let composite = composite_score(
        &modularity_profile,
        &divergence_profile,
        &tangles,
        n_module_nodes,
        &encapsulation,
        &config.composite,
    );

    Ok(AnalysisResult {
        crate_name,
        n_items,
        n_real_items,
        n_modules,
        n_module_nodes,
        modularity_profile,
        divergence_profile,
        modules,
        tangles,
        encapsulation,
        composite,
    })
}

#[cfg(test)]
mod tests {
    use super::AnalysisError;

    #[test]
    fn modularity_error_converts_into_analysis_error() {
        // The manual `From<ModularityError>` is what `?` uses when the detector
        // rejects an input; exercise it directly so it does not rely on a
        // contrived detector failure.
        use geometric_traits::traits::algorithms::ModularityError;
        let err: AnalysisError = ModularityError::InvalidResolution.into();
        assert!(matches!(err, AnalysisError::Modularity(_)));
    }
}
