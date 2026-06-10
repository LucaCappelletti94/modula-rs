//! Edge weighting policy: how a reference kind and its multiplicity combine into
//! a single `f64` arc weight.

use modula_ir::{Edge, RefKind};
use serde::{Deserialize, Serialize};

/// How edge multiplicity (occurrence count) is folded into the weight.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum MultiplicityMode {
    /// Weight scales linearly with the occurrence count.
    Linear,
    /// Weight scales with `ln(1 + count)`, damping hot references.
    #[default]
    Log1p,
    /// Weight scales linearly but is capped at the given count.
    Capped(u32),
}

/// Per-reference-kind base weights plus the multiplicity policy.
///
/// Signature and trait-bound references express interface coupling (what module
/// boundaries are meant to encapsulate), so they weigh more than body references
/// by default. These defaults are a tuning knob, calibrated later on real
/// crates; every downstream metric is invariant to the choice.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefKindWeights {
    /// Weight of a `use` import or re-export edge.
    pub import: f64,
    /// Weight of a signature-type edge.
    pub signature: f64,
    /// Weight of a generic-bound / supertrait edge.
    pub trait_bound: f64,
    /// Weight of an impl-induced structural edge.
    pub impl_block: f64,
    /// Weight of a function-body edge.
    pub body: f64,
    /// How multiplicity is folded in.
    pub multiplicity: MultiplicityMode,
}

impl Default for RefKindWeights {
    fn default() -> Self {
        Self {
            import: 1.0,
            signature: 3.0,
            trait_bound: 3.0,
            impl_block: 2.0,
            body: 1.0,
            multiplicity: MultiplicityMode::default(),
        }
    }
}

impl RefKindWeights {
    /// The base weight for a reference kind, before multiplicity.
    #[must_use]
    pub fn base(&self, kind: RefKind) -> f64 {
        match kind {
            RefKind::Import => self.import,
            RefKind::Signature => self.signature,
            RefKind::TraitBound => self.trait_bound,
            RefKind::Impl => self.impl_block,
            RefKind::Body => self.body,
        }
    }

    /// The full weight of an edge: base weight times the multiplicity factor.
    #[must_use]
    pub fn edge_weight(&self, edge: &Edge) -> f64 {
        let factor = match self.multiplicity {
            MultiplicityMode::Linear => f64::from(edge.weight),
            MultiplicityMode::Log1p => (1.0 + f64::from(edge.weight)).ln(),
            MultiplicityMode::Capped(cap) => f64::from(edge.weight.min(cap)),
        };
        self.base(edge.kind) * factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modula_ir::ItemId;

    fn edge(kind: RefKind, weight: u32) -> Edge {
        Edge {
            from: ItemId(0),
            to: ItemId(1),
            kind,
            weight,
        }
    }

    #[test]
    fn log1p_default_scales_base_by_ln1p() {
        let w = RefKindWeights::default();
        let ln2 = 2.0f64.ln();
        assert!((w.edge_weight(&edge(RefKind::Body, 1)) - ln2).abs() < 1e-12);
        assert!((w.edge_weight(&edge(RefKind::Signature, 1)) - 3.0 * ln2).abs() < 1e-12);
    }

    #[test]
    fn linear_and_capped_multiplicity() {
        let linear = RefKindWeights {
            multiplicity: MultiplicityMode::Linear,
            ..Default::default()
        };
        assert!((linear.edge_weight(&edge(RefKind::Body, 5)) - 5.0).abs() < 1e-12);
        let capped = RefKindWeights {
            multiplicity: MultiplicityMode::Capped(3),
            ..Default::default()
        };
        assert!((capped.edge_weight(&edge(RefKind::Body, 5)) - 3.0).abs() < 1e-12);
    }
}
