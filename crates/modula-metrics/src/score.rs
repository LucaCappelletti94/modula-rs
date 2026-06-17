//! The composite 0-1 modularity score: a weighted blend of three components,
//! each individually reported so the headline is never a black box.

use serde::Serialize;

use crate::cycles::TangleReport;
use crate::encapsulation::EncapsulationReport;

/// Weights for the three composite components (need not sum to one; the score
/// renormalizes over whichever components are available).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CompositeWeights {
    /// Weight of the cohesion-lift term.
    pub cohesion: f64,
    /// Weight of the acyclicity term.
    pub acyclicity: f64,
    /// Weight of the encapsulation term.
    pub encapsulation: f64,
}

impl Default for CompositeWeights {
    fn default() -> Self {
        Self {
            cohesion: 0.6,
            acyclicity: 0.2,
            encapsulation: 0.2,
        }
    }
}

/// The composite score and its components.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CompositeScore {
    /// Headline score in `[0, 1]`. `None` (N/A) when the crate has no measurable
    /// internal structure: fewer than two real modules, or no dependency weight
    /// to partition (a single module, or a pure re-export facade). Reporting a
    /// number there would invent a vacuous score, so the headline is undefined.
    pub headline: Option<f64>,
    /// Cohesion-lift term: how much better than chance the declared module
    /// boundaries contain the dependency weight. `None` with no structure.
    pub cohesion_term: Option<f64>,
    /// Acyclicity term in `[0, 1]`: `1 - 2 * feedback_fraction` (the feedback
    /// fraction is bounded at `0.5`, so this rescaling uses the full range). `1`
    /// for a clean layerable DAG, `0` for a maximally tangled module graph.
    pub acyclicity_term: f64,
    /// Encapsulation term: blend of over-exposure and interface-leak rate.
    pub encapsulation_term: f64,
    /// The weights used.
    pub weights: CompositeWeights,
}

/// Computes the composite score from the assembled metric pieces.
#[must_use]
pub fn composite_score(
    cohesion: Option<f64>,
    tangles: &TangleReport,
    encapsulation: &EncapsulationReport,
    weights: &CompositeWeights,
) -> CompositeScore {
    let acyclicity_term = acyclicity(tangles);
    let encapsulation_term = encapsulation_term(encapsulation);

    // Cohesion lift is defined exactly when the crate has a non-trivial module
    // partition with dependency weight to measure. Absent that, the crate has no
    // measurable structure and the headline is N/A, not a vacuous blend of the
    // structure-free terms (which would read ~1.0 for any small, clean crate).
    let has_structure = cohesion.is_some();

    let headline = has_structure.then(|| {
        weighted(&[
            (weights.cohesion, cohesion),
            (weights.acyclicity, Some(acyclicity_term)),
            (weights.encapsulation, Some(encapsulation_term)),
        ])
    });

    CompositeScore {
        headline,
        cohesion_term: cohesion,
        acyclicity_term,
        encapsulation_term,
        weights: *weights,
    }
}

/// Distance-from-DAG acyclicity, in `[0, 1]`. A clean (layerable) module graph
/// scores `1`; a maximally tangled one scores `0`. Shared sinks contribute no
/// feedback, so a widely-used foundation module is not penalized.
///
/// The feedback fraction is bounded at `0.5` (any vertex order or its reverse
/// keeps at least half the edge weight forward, so the minimum feedback set is
/// at most half), so we rescale by that bound to use the whole `[0, 1]` range.
fn acyclicity(tangles: &TangleReport) -> f64 {
    (1.0 - 2.0 * tangles.feedback_fraction).clamp(0.0, 1.0)
}

/// Blend of over-exposure and interface-leak rate, both in `[0, 1]`.
fn encapsulation_term(report: &EncapsulationReport) -> f64 {
    let exposure = 1.0 - report.over_exposed_fraction.clamp(0.0, 1.0);
    let leak = 1.0 - report.mean_leak_cost.clamp(0.0, 1.0);
    0.5 * exposure + 0.5 * leak
}

/// Weighted mean over the available `(weight, Some(value))` terms.
fn weighted(terms: &[(f64, Option<f64>)]) -> f64 {
    let mut weight_sum = 0.0;
    let mut acc = 0.0;
    for &(w, term) in terms {
        if let Some(v) = term {
            weight_sum += w;
            acc += w * v;
        }
    }
    if weight_sum > 0.0 {
        acc / weight_sum
    } else {
        0.0
    }
}

#[cfg(test)]
mod unit_tests {
    use super::{CompositeWeights, acyclicity, composite_score, encapsulation_term, weighted};
    use crate::cycles::TangleReport;
    use crate::encapsulation::EncapsulationReport;

    fn tangles(feedback_fraction: f64) -> TangleReport {
        TangleReport {
            sccs: Vec::new(),
            is_acyclic: feedback_fraction == 0.0,
            largest_scc: 0,
            cyclomatic_number: 0,
            feedback_fraction,
            feedback_edges: Vec::new(),
        }
    }

    fn encapsulation(over: f64, leak: f64) -> EncapsulationReport {
        EncapsulationReport {
            over_exposed: Vec::new(),
            over_exposed_fraction: over,
            leaks: Vec::new(),
            n_cross_module_refs: 0,
            mean_leak_cost: leak,
        }
    }

    #[test]
    fn weighted_average_over_present_terms() {
        // (1*0.2 + 3*0.6) / (1 + 3) = 2.0 / 4 = 0.5; the None term is skipped.
        let w = weighted(&[(1.0, Some(0.2)), (3.0, Some(0.6)), (2.0, None)]);
        assert!((w - 0.5).abs() < 1e-12);
        // No present terms -> zero weight -> 0.0 (never divides by zero).
        assert_eq!(weighted(&[(1.0, None)]), 0.0);
    }

    #[test]
    fn acyclicity_rescales_feedback_fraction() {
        // 25% feedback -> 1 - 2*0.25 = 0.5; clean DAG -> 1.0; the 0.5 cap -> 0.0.
        assert!((acyclicity(&tangles(0.25)) - 0.5).abs() < 1e-12);
        assert_eq!(acyclicity(&tangles(0.0)), 1.0);
        assert_eq!(acyclicity(&tangles(0.5)), 0.0);
    }

    #[test]
    fn encapsulation_term_blends_exposure_and_leak() {
        // exposure = 1 - 0.2 = 0.8, leak = 1 - 0.4 = 0.6 -> 0.5*0.8 + 0.5*0.6 = 0.7.
        assert!((encapsulation_term(&encapsulation(0.2, 0.4)) - 0.7).abs() < 1e-12);
    }

    #[test]
    fn no_cohesion_means_na_headline() {
        // Without a measurable partition the headline is N/A even when the other
        // terms are perfect, rather than a vacuous ~1.0.
        let score = composite_score(
            None,
            &tangles(0.0),
            &encapsulation(0.0, 0.0),
            &CompositeWeights::default(),
        );
        assert_eq!(score.headline, None);
        assert_eq!(score.cohesion_term, None);
        assert_eq!(score.acyclicity_term, 1.0);
    }

    #[test]
    fn headline_blends_present_terms() {
        // cohesion 0.5 (w 0.6), acyclicity 1.0 (w 0.2), encapsulation 1.0 (w 0.2):
        // (0.6*0.5 + 0.2*1 + 0.2*1) / 1.0 = 0.7.
        let score = composite_score(
            Some(0.5),
            &tangles(0.0),
            &encapsulation(0.0, 0.0),
            &CompositeWeights::default(),
        );
        assert!((score.headline.unwrap() - 0.7).abs() < 1e-12);
    }
}
