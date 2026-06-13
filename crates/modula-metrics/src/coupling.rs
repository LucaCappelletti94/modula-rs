//! Per-module coupling and cohesion, including Robert Martin's package metrics.

use std::collections::HashMap;

use modula_ir::{CrateGraph, ItemKind, ModuleId};
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
    /// Abstractness `A`: the fraction of the module's types that are abstract.
    /// In Rust the abstract type is the `trait`; concrete types are
    /// `struct`/`enum`/`union`. `None` when the module declares no types.
    pub abstractness: Option<f64>,
    /// Distance from Martin's main sequence, `D = |A + I - 1|`, in `[0, 1]`.
    /// `0` is a healthy balance (abstract-and-stable or concrete-and-unstable);
    /// `1` is a "zone of pain" / "zone of uselessness" corner. `None` when
    /// either `A` (no types) or `I` (uncoupled) is undefined.
    pub distance_main_sequence: Option<f64>,
}

/// Per real module, `(abstract type count, total type count)`. Types are
/// `struct`/`enum`/`union`/`trait`; the trait is the abstract one. Items are
/// rolled up through `real_module`, so per-type containers count toward their
/// owning `mod`.
fn type_composition(ir: &CrateGraph) -> HashMap<ModuleId, (u32, u32)> {
    let mut comp: HashMap<ModuleId, (u32, u32)> = HashMap::new();
    for item in &ir.items {
        let (is_type, is_abstract) = match item.kind {
            ItemKind::Trait => (true, true),
            ItemKind::Struct | ItemKind::Enum | ItemKind::Union => (true, false),
            _ => (false, false),
        };
        if is_type {
            let entry = comp.entry(ir.real_module(item.owning_module)).or_default();
            entry.1 += 1;
            if is_abstract {
                entry.0 += 1;
            }
        }
    }
    comp
}

/// Computes per-module coupling, cohesion, and Martin's package metrics
/// (instability, abstractness, distance from the main sequence).
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

    let composition = type_composition(ir);

    (0..n)
        .map(|i| {
            let intra = agg.intra[i];
            let total = intra + out_weight[i] + in_weight[i];
            let cohesion = (total > 0.0).then(|| intra / total);
            let (ca, ce) = (in_degree[i], out_degree[i]);
            let instability = (ca + ce > 0).then(|| ce as f64 / (ca + ce) as f64);
            let abstractness = composition
                .get(&agg.nodes[i])
                .filter(|&&(_, types)| types > 0)
                .map(|&(abstr, types)| f64::from(abstr) / f64::from(types));
            let distance_main_sequence = match (abstractness, instability) {
                (Some(a), Some(i)) => Some((a + i - 1.0).abs()),
                _ => None,
            };
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
                abstractness,
                distance_main_sequence,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use modula_ir::{
        Crate, CrateGraph, CrateId, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
        SCHEMA_VERSION, Visibility,
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
        // No items in this fixture, so abstractness and distance are undefined.
        assert_eq!(node0.abstractness, None);
        assert_eq!(node0.distance_main_sequence, None);
    }

    #[test]
    fn abstractness_and_distance_from_main_sequence() {
        // Module 0: a trait + a struct (A = 1/2). Module 1: a struct only (A = 0).
        // The agg couples them both ways, so each has Ca = Ce = 1 -> I = 0.5.
        // D(0) = |0.5 + 0.5 - 1| = 0 (on the main sequence);
        // D(1) = |0.0 + 0.5 - 1| = 0.5 (concrete but as stable as it is unstable).
        let mut ir = graph_with_modules(2);
        let item = |id: u32, owner: u32, kind: ItemKind| Item {
            id: ItemId(id),
            canonical_path: format!("c::i{id}"),
            kind,
            visibility: Visibility::Public,
            owning_module: ModuleId(owner),
            crate_id: CrateId(0),
            has_canonical_path: true,
            reachable_pub_api: true,
        };
        ir.items = vec![
            item(0, 0, ItemKind::Trait),
            item(1, 0, ItemKind::Struct),
            item(2, 1, ItemKind::Struct),
        ];
        let agg = ModuleAggregation {
            nodes: vec![ModuleId(0), ModuleId(1)],
            index_of: [(ModuleId(0), 0), (ModuleId(1), 1)].into_iter().collect(),
            intra: vec![0.0, 0.0],
            inter: [((0, 1), 1.0), ((1, 0), 1.0)].into_iter().collect(),
        };
        let coupling = module_coupling(&ir, &agg);

        assert!((coupling[0].abstractness.unwrap() - 0.5).abs() < 1e-12);
        assert!((coupling[0].instability.unwrap() - 0.5).abs() < 1e-12);
        assert!(coupling[0].distance_main_sequence.unwrap() < 1e-12);
        assert_eq!(coupling[1].abstractness, Some(0.0));
        assert!((coupling[1].distance_main_sequence.unwrap() - 0.5).abs() < 1e-12);
    }
}
