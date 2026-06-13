//! The composite 0-1 modularity score: a weighted blend of four components,
//! each individually reported so the headline is never a black box.

use serde::Serialize;

use crate::cycles::TangleReport;
use crate::encapsulation::EncapsulationReport;
use crate::modularity::{DepthRecord, DivergenceRecord};

/// Weights for the four composite components (need not sum to one; the score
/// renormalizes over whichever components are available).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CompositeWeights {
    /// Weight of the modularity-efficiency term.
    pub modularity: f64,
    /// Weight of the divergence (AMI) term.
    pub divergence: f64,
    /// Weight of the acyclicity term.
    pub acyclicity: f64,
    /// Weight of the encapsulation term.
    pub encapsulation: f64,
}

impl Default for CompositeWeights {
    fn default() -> Self {
        Self {
            modularity: 0.35,
            divergence: 0.25,
            acyclicity: 0.20,
            encapsulation: 0.20,
        }
    }
}

/// The composite score and its components.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CompositeScore {
    /// Headline score in `[0, 1]`, taken at the primary (finest non-trivial)
    /// depth. `None` (N/A) when the crate has no measurable internal structure:
    /// no module-tree depth yields more than one declared community (a single
    /// module, or a pure re-export facade). Reporting a number there would
    /// invent a vacuous score, so the headline is undefined instead.
    pub headline: Option<f64>,
    /// Headline averaged over all non-trivial depths (more stable, less
    /// interpretable). `None` under the same no-structure condition as
    /// [`headline`](Self::headline).
    pub headline_depth_averaged: Option<f64>,
    /// Modularity-efficiency term; `None` when no positive structure exists.
    pub modularity_term: Option<f64>,
    /// Divergence term (AMI at the primary depth); `None` when there is no
    /// primary depth to measure against.
    pub divergence_term: Option<f64>,
    /// Acyclicity term: fraction of module nodes not in any tangle.
    pub acyclicity_term: f64,
    /// Encapsulation term: blend of over-exposure and leak depth.
    pub encapsulation_term: f64,
    /// The weights used.
    pub weights: CompositeWeights,
}

/// Computes the composite score from the assembled metric pieces.
#[must_use]
pub fn composite_score(
    modularity: &[DepthRecord],
    divergence: &[DivergenceRecord],
    tangles: &TangleReport,
    n_module_nodes: usize,
    encapsulation: &EncapsulationReport,
    weights: &CompositeWeights,
) -> CompositeScore {
    let acyclicity_term = acyclicity(tangles, n_module_nodes);
    let encapsulation_term = encapsulation_term(encapsulation);

    // Primary depth: the finest declared layer that still has more than one
    // community.
    let primary = modularity
        .iter()
        .filter(|r| r.communities_declared > 1)
        .max_by_key(|r| r.communities_declared);

    let modularity_term = primary.and_then(mean_efficiency);
    let divergence_term = primary
        .and_then(|p| divergence.iter().find(|d| d.depth == p.depth))
        .map(|d| d.ami.clamp(0.0, 1.0));

    // A crate with no non-trivial declared partition at any depth has no
    // measurable structure: the headline is N/A, not a vacuous blend of the
    // structure-free terms (which would read ~1.0 for any small, clean crate).
    let has_structure = primary.is_some();

    let headline = has_structure.then(|| {
        weighted(&[
            (weights.modularity, modularity_term),
            (weights.divergence, divergence_term),
            (weights.acyclicity, Some(acyclicity_term)),
            (weights.encapsulation, Some(encapsulation_term)),
        ])
    });

    // Depth-averaged variant over all non-trivial depths.
    let nontrivial: Vec<&DepthRecord> = modularity
        .iter()
        .filter(|r| r.communities_declared > 1)
        .collect();
    let avg_efficiency = mean(nontrivial.iter().filter_map(|r| mean_efficiency(r)));
    let avg_ami = mean(nontrivial.iter().filter_map(|r| {
        divergence
            .iter()
            .find(|d| d.depth == r.depth)
            .map(|d| d.ami.clamp(0.0, 1.0))
    }));
    let headline_depth_averaged = has_structure.then(|| {
        weighted(&[
            (weights.modularity, avg_efficiency),
            (weights.divergence, avg_ami),
            (weights.acyclicity, Some(acyclicity_term)),
            (weights.encapsulation, Some(encapsulation_term)),
        ])
    });

    CompositeScore {
        headline,
        headline_depth_averaged,
        modularity_term,
        divergence_term,
        acyclicity_term,
        encapsulation_term,
        weights: *weights,
    }
}

/// Fraction of module nodes not involved in any non-trivial cycle.
fn acyclicity(tangles: &TangleReport, n_module_nodes: usize) -> f64 {
    if n_module_nodes == 0 {
        return 1.0;
    }
    let in_cycles: usize = tangles.sccs.iter().map(Vec::len).sum();
    1.0 - in_cycles as f64 / n_module_nodes as f64
}

/// Blend of over-exposure and leak depth, both in `[0, 1]`.
fn encapsulation_term(report: &EncapsulationReport) -> f64 {
    let exposure = 1.0 - report.over_exposed_fraction.clamp(0.0, 1.0);
    let leak = 1.0 - report.mean_leak_cost.clamp(0.0, 1.0);
    0.5 * exposure + 0.5 * leak
}

/// Mean of the available efficiencies (undirected and directed) of a record.
fn mean_efficiency(record: &DepthRecord) -> Option<f64> {
    mean(
        [record.efficiency_undirected, record.efficiency_directed]
            .into_iter()
            .flatten(),
    )
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

/// Mean of an iterator of values, or `None` if empty.
fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0usize;
    for v in values {
        sum += v;
        count += 1;
    }
    (count > 0).then(|| sum / count as f64)
}

#[cfg(test)]
mod unit_tests {
    use super::{acyclicity, encapsulation_term, mean, mean_efficiency, weighted};
    use crate::cycles::TangleReport;
    use crate::encapsulation::EncapsulationReport;
    use crate::modularity::DepthRecord;
    use modula_ir::ModuleId;

    fn depth_record(eff_u: Option<f64>, eff_d: Option<f64>) -> DepthRecord {
        DepthRecord {
            depth: 1,
            communities_declared: 2,
            q_declared_undirected: 0.0,
            q_declared_directed: 0.0,
            q_detected_undirected: 0.0,
            q_detected_directed: 0.0,
            efficiency_undirected: eff_u,
            efficiency_directed: eff_d,
        }
    }

    fn encapsulation(over: f64, leak: f64) -> EncapsulationReport {
        EncapsulationReport {
            over_exposed: Vec::new(),
            over_exposed_fraction: over,
            deepest_leaks: Vec::new(),
            mean_leak_cost: leak,
        }
    }

    #[test]
    fn mean_of_values_and_empty() {
        assert!((mean([0.2, 0.4].into_iter()).unwrap() - 0.3).abs() < 1e-12);
        assert_eq!(mean(std::iter::empty()), None);
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
    fn acyclicity_is_fraction_of_nodes_outside_cycles() {
        let tangles = TangleReport {
            sccs: vec![vec![ModuleId(0), ModuleId(1)]],
            circuits: Vec::new(),
            is_acyclic: false,
            largest_scc: 2,
            circuits_truncated: false,
        };
        // 2 of 4 nodes in cycles -> 1 - 2/4 = 0.5.
        assert!((acyclicity(&tangles, 4) - 0.5).abs() < 1e-12);
        // No module nodes -> perfectly acyclic.
        assert_eq!(acyclicity(&tangles, 0), 1.0);
    }

    #[test]
    fn encapsulation_term_blends_exposure_and_leak() {
        // exposure = 1 - 0.2 = 0.8, leak = 1 - 0.4 = 0.6 -> 0.5*0.8 + 0.5*0.6 = 0.7.
        assert!((encapsulation_term(&encapsulation(0.2, 0.4)) - 0.7).abs() < 1e-12);
    }

    #[test]
    fn mean_efficiency_averages_present_efficiencies() {
        assert!(
            (mean_efficiency(&depth_record(Some(0.4), Some(0.8))).unwrap() - 0.6).abs() < 1e-12
        );
        assert!((mean_efficiency(&depth_record(Some(0.4), None)).unwrap() - 0.4).abs() < 1e-12);
        assert_eq!(mean_efficiency(&depth_record(None, None)), None);
    }
}
