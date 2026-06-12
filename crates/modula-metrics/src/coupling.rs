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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use modula_ir::{
        Crate, CrateGraph, CrateId, Module, ModuleId, ModuleKind, SCHEMA_VERSION, Visibility,
    };

    use super::module_coupling;
    use crate::module_graph::ModuleAggregation;

    fn graph_with_modules(n: u32) -> CrateGraph {
        let modules = (0..n)
            .map(|i| Module {
                id: ModuleId(i),
                crate_id: CrateId(0),
                parent: (i != 0).then_some(ModuleId(0)),
                name: format!("m{i}"),
                canonical_path: format!("c::m{i}"),
                depth: u32::from(i != 0),
                visibility: Visibility::Public,
                kind: ModuleKind::Mod,
            })
            .collect();
        CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "c".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules,
            items: Vec::new(),
            edges: Vec::new(),
        }
    }

    #[test]
    fn coupling_computes_instability_and_handles_isolated_nodes() {
        // Node 0: out to 1 and 2 (ce = 2), in from 1 (ca = 1). Node 3: no edges.
        let inter: BTreeMap<(usize, usize), f64> = [((0, 1), 1.0), ((0, 2), 1.0), ((1, 0), 1.0)]
            .into_iter()
            .collect();
        let agg = ModuleAggregation {
            nodes: vec![ModuleId(0), ModuleId(1), ModuleId(2), ModuleId(3)],
            index_of: (0..4u32).map(|i| (ModuleId(i), i as usize)).collect(),
            intra: vec![1.0, 0.0, 0.0, 0.0],
            inter,
        };
        let coupling = module_coupling(&graph_with_modules(4), &agg);

        let node0 = &coupling[0];
        assert_eq!((node0.ca, node0.ce), (1, 2));
        // instability = ce / (ca + ce) = 2/3.
        assert!((node0.instability.unwrap() - 2.0 / 3.0).abs() < 1e-12);
        // cohesion = intra / total = 1 / (1 + 2 + 1) = 0.25.
        assert!((node0.cohesion.unwrap() - 0.25).abs() < 1e-12);

        // The isolated node has no edges, so both ratios are undefined.
        let node3 = &coupling[3];
        assert_eq!(node3.cohesion, None);
        assert_eq!(node3.instability, None);
    }
}
