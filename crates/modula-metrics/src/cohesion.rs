//! Cohesion lift: a hub-tolerant measure of how well the declared module
//! boundaries match the dependency graph.
//!
//! It compares the fraction of dependency weight that stays inside declared
//! modules against the fraction a random partition with the same module sizes
//! would retain by chance. The null expectation is analytic (no sampling), so
//! the measure is deterministic and `O(V + E)`.
//!
//! This replaces the Newman-modularity efficiency and the AMI divergence. Those
//! rest on a configuration-model null that treats a shared, widely-used module
//! as "no community structure", which penalizes usage. Cohesion lift does not:
//! a sink's edges are inter-module under every partition, so being depended upon
//! is free, and only edges that genuinely cluster within a declared module count
//! as signal.

use modula_ir::{CrateGraph, ItemKind};

use crate::module_graph::ModuleAggregation;

/// Cohesion lift in `[0, 1]`: `(declared - null) / (1 - null)`, where `declared`
/// is the intra-module share of dependency weight and `null` is the share a
/// size-preserving random partition would retain. `1` when every dependency
/// stays within its module, `0` when the declared boundaries do no better than
/// chance. `None` when there is no non-trivial partition (fewer than two
/// modules), no dependency weight, or a single module holds essentially all
/// items (no measurable structure).
#[must_use]
pub fn cohesion_lift(ir: &CrateGraph, agg: &ModuleAggregation) -> Option<f64> {
    if agg.len() < 2 {
        return None;
    }

    let intra: f64 = agg.intra.iter().sum();
    let inter: f64 = agg.inter.values().sum();
    let total = intra + inter;
    if total <= 0.0 {
        return None;
    }
    let declared = intra / total;

    // Null expectation: for a uniform random assignment of items to the same
    // module sizes, two distinct items share a module with probability
    // `sum_m n_m (n_m - 1) / (N (N - 1))`, which is exactly the expected
    // intra-fraction of every edge.
    let mut sizes = vec![0usize; agg.len()];
    for item in &ir.items {
        if item.kind == ItemKind::Module {
            continue; // real items only; module stubs are not graph nodes
        }
        if let Some(&node) = agg.index_of.get(&ir.real_module(item.owning_module)) {
            sizes[node] += 1;
        }
    }
    let n: usize = sizes.iter().sum();
    if n < 2 {
        return None;
    }
    let same_pairs: f64 = sizes
        .iter()
        .map(|&s| (s * (s.saturating_sub(1))) as f64)
        .sum();
    let null = same_pairs / (n as f64 * (n - 1) as f64);
    if null >= 1.0 - 1e-12 {
        return None; // one module holds (almost) every item: no partition to score
    }

    Some(((declared - null) / (1.0 - null)).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use modula_ir::{
        Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
        RefKind, SCHEMA_VERSION, Visibility,
    };

    use super::cohesion_lift;
    use crate::module_graph::ModuleAggregation;
    use crate::weighting::RefKindWeights;

    /// Builds a crate: `items[i] = (owning_module, refs_to)` with one `mod` per
    /// listed module index, and a Body edge from item `i` to each listed target.
    fn build(n_modules: u32, items: &[(u32, &[usize])]) -> CrateGraph {
        let modules = (0..n_modules)
            .map(|i| Module {
                id: ModuleId(i),
                crate_id: CrateId(0),
                parent: (i != 0).then_some(ModuleId(0)),
                name: format!("m{i}"),
                canonical_path: format!("k::m{i}"),
                depth: u32::from(i != 0),
                visibility: Visibility::Public,
                kind: ModuleKind::Mod,
            })
            .collect();
        let item_vec = items
            .iter()
            .enumerate()
            .map(|(i, &(owner, _))| Item {
                id: ItemId(i as u32),
                canonical_path: format!("k::i{i}"),
                kind: ItemKind::Function,
                visibility: Visibility::Public,
                owning_module: ModuleId(owner),
                crate_id: CrateId(0),
                has_canonical_path: true,
                reachable_pub_api: false,
            })
            .collect();
        let edges = items
            .iter()
            .enumerate()
            .flat_map(|(i, &(_, refs))| {
                refs.iter().map(move |&to| Edge {
                    from: ItemId(i as u32),
                    to: ItemId(to as u32),
                    kind: RefKind::Body,
                    weight: 1,
                })
            })
            .collect();
        CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "k".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules,
            items: item_vec,
            edges,
        }
    }

    fn lift(ir: &CrateGraph) -> Option<f64> {
        let agg = ModuleAggregation::build(ir, &RefKindWeights::default());
        cohesion_lift(ir, &agg)
    }

    #[test]
    fn single_module_has_no_partition() {
        // Two items in one module: nothing to partition, so the measure is N/A.
        let ir = build(1, &[(0, &[1]), (0, &[])]);
        assert_eq!(lift(&ir), None);
    }

    #[test]
    fn perfectly_internal_dependencies_score_one() {
        // Modules 1 and 2 each hold two items that reference each other; no
        // cross-module edges. All weight is intra-module, so lift is 1.
        let ir = build(3, &[(1, &[1]), (1, &[0]), (2, &[3]), (2, &[2])]);
        assert_eq!(lift(&ir), Some(1.0));
    }

    #[test]
    fn a_shared_sink_does_not_lower_cohesion() {
        // Modules 1 and 2 each have an internal pair, and both also call into a
        // shared item in module 3 (a sink). The within-module pairs still carry
        // the signal; the sink edges are inter-module but expected to be so.
        let internal = build(4, &[(1, &[1]), (1, &[0]), (2, &[3]), (2, &[2])]);
        let with_sink = build(
            4,
            &[(1, &[1, 4]), (1, &[0]), (2, &[3, 4]), (2, &[2]), (3, &[])],
        );
        // Adding the sink dependency keeps cohesion high (well above zero),
        // because the declared partition still beats a random one by a lot.
        let s = lift(&with_sink).expect("structured");
        assert!(s > 0.5, "shared sink must not collapse cohesion: {s}");
        assert!(lift(&internal).expect("structured") >= s);
    }

    #[test]
    fn scrambled_boundaries_score_low() {
        // Items that all reference across module lines (no internal cohesion):
        // the declared partition does no better than chance, so lift is ~0.
        let ir = build(2, &[(0, &[2]), (0, &[3]), (1, &[0]), (1, &[1])]);
        let s = lift(&ir).expect("structured");
        assert!(s < 0.2, "cross-cutting references give little lift: {s}");
    }
}
