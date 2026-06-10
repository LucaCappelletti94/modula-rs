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
    /// depth.
    pub headline: f64,
    /// Headline averaged over all non-trivial depths (more stable, less
    /// interpretable).
    pub headline_depth_averaged: f64,
    /// Modularity-efficiency term; `None` when no positive structure exists.
    pub modularity_term: Option<f64>,
    /// Divergence term (AMI at the primary depth).
    pub divergence_term: f64,
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
        .map_or(1.0, |d| d.ami.clamp(0.0, 1.0));

    let headline = weighted(&[
        (weights.modularity, modularity_term),
        (weights.divergence, Some(divergence_term)),
        (weights.acyclicity, Some(acyclicity_term)),
        (weights.encapsulation, Some(encapsulation_term)),
    ]);

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
    }))
    .unwrap_or(1.0);
    let headline_depth_averaged = weighted(&[
        (weights.modularity, avg_efficiency),
        (weights.divergence, Some(avg_ami)),
        (weights.acyclicity, Some(acyclicity_term)),
        (weights.encapsulation, Some(encapsulation_term)),
    ]);

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
