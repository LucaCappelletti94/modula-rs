//! Per-module coupling and cohesion, including Robert Martin's package metrics.

use modula_ir::{CrateGraph, ModuleId};
use serde::Serialize;

use crate::module_graph::ModuleAggregation;

/// Coupling and cohesion metrics for one module.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModuleCoupling {
    /// The module these metrics describe.
    pub module: ModuleId,
    /// The module's canonical path.
    pub path: String,
    /// Within-module edge weight.
    pub intra_weight: f64,
    /// Outgoing inter-module edge weight.
    pub inter_weight_out: f64,
    /// Incoming inter-module edge weight.
    pub inter_weight_in: f64,
    /// `intra / (intra + inter_out + inter_in)`; `None` when the module has no
    /// edges at all.
    pub cohesion: Option<f64>,
    /// Afferent coupling: number of distinct modules that depend on this one.
    pub ca: usize,
    /// Efferent coupling: number of distinct modules this one depends on.
    pub ce: usize,
    /// Instability `Ce / (Ca + Ce)`; `None` when the module is uncoupled.
    pub instability: Option<f64>,
}

/// Computes per-module coupling and cohesion from a module aggregation.
#[must_use]
pub fn module_coupling(ir: &CrateGraph, agg: &ModuleAggregation) -> Vec<ModuleCoupling> {
    let n = agg.len();
    let mut out_weight = vec![0.0; n];
    let mut in_weight = vec![0.0; n];
    let mut out_degree = vec![0usize; n];
    let mut in_degree = vec![0usize; n];

    // Each `(src, dst)` key is a distinct directed module pair, so counting keys
    // gives the distinct-neighbor counts Ca and Ce.
    for (&(src, dst), &w) in &agg.inter {
        out_weight[src] += w;
        in_weight[dst] += w;
        out_degree[src] += 1;
        in_degree[dst] += 1;
    }

    (0..n)
        .map(|i| {
            let intra = agg.intra[i];
            let total = intra + out_weight[i] + in_weight[i];
            let cohesion = (total > 0.0).then(|| intra / total);
            let (ca, ce) = (in_degree[i], out_degree[i]);
            let instability = (ca + ce > 0).then(|| ce as f64 / (ca + ce) as f64);
            ModuleCoupling {
                module: agg.nodes[i],
                path: ir.module(agg.nodes[i]).canonical_path.clone(),
                intra_weight: intra,
                inter_weight_out: out_weight[i],
                inter_weight_in: in_weight[i],
                cohesion,
                ca,
                ce,
                instability,
            }
        })
        .collect()
}
