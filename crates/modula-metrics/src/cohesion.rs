//! Cohesion lift: a hub-tolerant measure of how well the declared module
//! boundaries match the dependency graph.
//!
//! Cohesion is scored per module and then combined, rather than as one global
//! intra-fraction. For each module `m`, `observed` is the share of the edge
//! weight leaving `m`'s items that stays inside `m`, and `expected` is the share
//! that would stay inside if those items pointed at random targets,
//! `(n_m - 1) / (N - 1)`. The per-module lift `(observed - expected) /
//! (1 - expected)` is how much more `m` keeps to itself than chance. The crate
//! score is the average of those lifts, weighted by `log(1 + activity)` and
//! raised to a strictness exponent.
//!
//! Only outgoing edges count, so being depended upon (high fan-in) is free,
//! which is what makes the measure hub-tolerant and is why it replaces the
//! Newman-modularity efficiency and AMI divergence, whose configuration-model
//! null treated a shared, widely-used module as having no community structure.
//! The `log` weight stops one enormous module (often generated) from dominating
//! the verdict and squeezing out the others, and the exponent discounts modules
//! that only barely beat chance so an arbitrary partition with marginal local
//! cohesion does not read as well-modularized. The measure is analytic (no
//! sampling), deterministic, and `O(V + E)`.

use modula_ir::{CrateGraph, ItemKind};

use crate::module_graph::ModuleAggregation;

/// Strictness exponent applied to each per-module lift before averaging. `2`
/// discounts marginal cohesion hard while preserving strong cohesion; raising it
/// is stricter, lowering it toward `1` is more lenient.
const STRICTNESS_EXPONENT: f64 = 2.0;

/// Cohesion lift in `[0, 1]`: the `log`-activity-weighted, strictness-raised
/// average of each module's lift over its size-based chance expectation. `1`
/// when modules keep all their outgoing edges to themselves, `0` when the
/// declared boundaries do no better than chance. `None` when there is no
/// non-trivial partition (fewer than two modules), fewer than two real items, or
/// no outgoing dependency weight to measure.
#[must_use]
pub fn cohesion_lift(ir: &CrateGraph, agg: &ModuleAggregation) -> Option<f64> {
    let k = agg.len();
    if k < 2 {
        return None;
    }

    // Real-item count per module node (module stubs are not graph nodes).
    let mut sizes = vec![0usize; k];
    for item in &ir.items {
        if item.kind == ItemKind::Module {
            continue;
        }
        if let Some(&node) = agg.index_of.get(&ir.real_module(item.owning_module)) {
            sizes[node] += 1;
        }
    }
    let n: usize = sizes.iter().sum();
    if n < 2 {
        return None;
    }

    // Efferent (outgoing cross-module) weight per module node.
    let mut eff = vec![0.0f64; k];
    for (&(src, _dst), &w) in &agg.inter {
        eff[src] += w;
    }

    let mut num = 0.0;
    let mut den = 0.0;
    for node in 0..k {
        // Outgoing edge weight from this module: its internal edges plus the
        // edges it sends to other modules. Incoming (afferent) weight is ignored
        // so a depended-upon module is not penalized for its fan-in.
        let activity = agg.intra[node] + eff[node];
        if activity <= 0.0 {
            continue; // a pure sink / edgeless module carries no cohesion signal
        }
        let observed = agg.intra[node] / activity;
        let expected = (sizes[node] as f64 - 1.0) / (n as f64 - 1.0);
        if expected >= 1.0 {
            continue; // this module already holds every item: no partition to score
        }
        let lift = ((observed - expected) / (1.0 - expected)).clamp(0.0, 1.0);
        let weight = (1.0 + activity).ln();
        num += weight * lift.powf(STRICTNESS_EXPONENT);
        den += weight;
    }

    (den > 0.0).then_some(num / den)
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
    fn a_pure_sink_is_excluded_not_penalized() {
        // Modules 1 and 2 each have an internal pair; both also depend on a
        // shared item in module 3, which depends on nothing (a pure sink). The
        // sink has no outgoing edges, so it is excluded from the average rather
        // than scored as zero-cohesion: cohesion stays clearly positive, driven
        // by modules 1 and 2. It is lower than the no-dependency case only
        // because the consumers genuinely send some weight outward.
        let internal = build(4, &[(1, &[1]), (1, &[0]), (2, &[3]), (2, &[2])]);
        let with_sink = build(
            4,
            &[(1, &[1, 4]), (1, &[0]), (2, &[3, 4]), (2, &[2]), (3, &[])],
        );
        let s = lift(&with_sink).expect("structured");
        assert!(s > 0.25, "the sink must not collapse cohesion to ~0: {s}");
        assert!(
            lift(&internal).expect("structured") > s,
            "depending outward on the sink lowers the consumers' cohesion"
        );
    }

    #[test]
    fn modules_without_dependencies_have_no_cohesion() {
        // Two modules holding items but with no edges at all: no module has any
        // outgoing activity, so there is no cohesion signal and the result is
        // N/A, not a spurious score (and never `Some(NaN)` from a 0/0 average).
        let ir = build(3, &[(1, &[]), (1, &[]), (2, &[]), (2, &[])]);
        assert_eq!(lift(&ir), None);
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
