//! Induced modularity profile and the per-depth divergence profile.
//!
//! For each module-tree depth `k`, the declared partition (each item to its
//! ancestor module at depth `k`) is scored with both undirected and directed
//! modularity, then compared against a depth-matched detector optimum. The ratio
//! `Q_declared / Q_detected` is the efficiency. The same depth-matched detector
//! partition is also compared against the declared partition with the divergence
//! measures, so both profiles come from a single detector run.

use geometric_traits::prelude::*;
use geometric_traits::traits::algorithms::ModularityError;
use modula_ir::CrateGraph;
use serde::Serialize;

use crate::divergence::Divergence;
use crate::graph::{ItemGraphs, WeightedGraph};

/// Configuration for the modularity profile.
#[derive(Clone, Copy, Debug)]
pub struct ModularityConfig {
    /// Resolution parameter (gamma) for modularity and the detectors.
    pub resolution: f64,
}

impl Default for ModularityConfig {
    fn default() -> Self {
        Self { resolution: 1.0 }
    }
}

/// One row of the modularity profile, for a single module-tree depth.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DepthRecord {
    /// The module-tree depth this row describes.
    pub depth: u32,
    /// Number of communities in the declared partition at this depth.
    pub communities_declared: usize,
    /// Undirected modularity of the declared partition.
    pub q_declared_undirected: f64,
    /// Directed modularity of the declared partition.
    pub q_declared_directed: f64,
    /// Undirected modularity of the depth-matched detector partition.
    pub q_detected_undirected: f64,
    /// Directed modularity of the depth-matched detector partition.
    pub q_detected_directed: f64,
    /// `q_declared_undirected / q_detected_undirected`, clamped to `[0, 1]`.
    /// `None` when the detector found no positive community structure.
    pub efficiency_undirected: Option<f64>,
    /// `q_declared_directed / q_detected_directed`, clamped to `[0, 1]`.
    pub efficiency_directed: Option<f64>,
}

/// One row of the divergence profile, for a single module-tree depth.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DivergenceRecord {
    /// The module-tree depth this row describes.
    pub depth: u32,
    /// Variation of Information between declared and detected partitions.
    pub vi: f64,
    /// Variation of Information normalized to `[0, 1]`.
    pub vi_normalized: f64,
    /// Normalized Mutual Information.
    pub nmi: f64,
    /// Adjusted Mutual Information (chance-corrected agreement).
    pub ami: f64,
    /// Adjusted Rand Index.
    pub ari: f64,
    /// `H(declared | detected)`: how much the declared partition over-splits.
    pub h_declared_given_detected: f64,
    /// `H(detected | declared)`: how much the detected partition over-splits
    /// (declared under-modularization).
    pub h_detected_given_declared: f64,
}

/// Both profiles, produced from a single detector run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Profiles {
    /// The modularity efficiency profile over depth.
    pub modularity: Vec<DepthRecord>,
    /// The divergence profile over depth.
    pub divergence: Vec<DivergenceRecord>,
}

/// Community count and partition of one detector level.
struct Level {
    communities: usize,
    modularity: f64,
    partition: Vec<usize>,
}

/// Computes the modularity and divergence profiles across all tree depths.
///
/// # Errors
/// Returns [`ModularityError`] if scoring or detection rejects an input.
pub fn profiles(
    ir: &CrateGraph,
    graphs: &ItemGraphs,
    config: &ModularityConfig,
) -> Result<Profiles, ModularityError> {
    // Each detector runs once; its level hierarchy is reused for every depth.
    // With no edges there is nothing to detect, so skip it (the detectors expect
    // a non-empty graph).
    let detect = !ir.edges.is_empty();
    let undirected = if detect {
        undirected_levels(&graphs.undirected, config.resolution)?
    } else {
        Vec::new()
    };
    let directed = if detect {
        directed_levels(&graphs.directed, config.resolution)?
    } else {
        Vec::new()
    };

    let mut modularity = Vec::new();
    let mut divergence = Vec::new();
    for depth in 0..=ir.max_depth() {
        let declared = ir.partition_at_depth(depth);
        let communities_declared = community_count(&declared);

        let q_declared_undirected = graphs
            .undirected
            .undirected_modularity(&declared, config.resolution)?;
        let q_declared_directed = graphs
            .directed
            .directed_modularity(&declared, config.resolution)?;

        let matched_undirected = matched(&undirected, communities_declared);
        let matched_directed = matched(&directed, communities_declared);
        let q_detected_undirected = matched_undirected.map_or(0.0, |l| l.modularity);
        let q_detected_directed = matched_directed.map_or(0.0, |l| l.modularity);

        modularity.push(DepthRecord {
            depth,
            communities_declared,
            q_declared_undirected,
            q_declared_directed,
            q_detected_undirected,
            q_detected_directed,
            efficiency_undirected: efficiency(q_declared_undirected, q_detected_undirected),
            efficiency_directed: efficiency(q_declared_directed, q_detected_directed),
        });

        // Compare the declared partition against the depth-matched undirected
        // detector partition. With no detected level (empty graph), the detected
        // partition is all-singletons so divergence is still well defined.
        let detected_partition = matched_undirected
            .map(|l| l.partition.clone())
            .unwrap_or_else(|| (0..declared.len()).collect());
        let d = Divergence::compute(&declared, &detected_partition);
        divergence.push(DivergenceRecord {
            depth,
            vi: d.vi,
            vi_normalized: d.vi_normalized,
            nmi: d.nmi,
            ami: d.ami,
            ari: d.ari,
            h_declared_given_detected: d.h_a_given_b,
            h_detected_given_declared: d.h_b_given_a,
        });
    }
    Ok(Profiles {
        modularity,
        divergence,
    })
}

/// Computes only the modularity profile (convenience wrapper over [`profiles`]).
///
/// # Errors
/// Returns [`ModularityError`] if scoring or detection rejects an input.
pub fn modularity_profile(
    ir: &CrateGraph,
    graphs: &ItemGraphs,
    config: &ModularityConfig,
) -> Result<Vec<DepthRecord>, ModularityError> {
    Ok(profiles(ir, graphs, config)?.modularity)
}

/// Number of communities in a contiguous partition.
fn community_count(partition: &[usize]) -> usize {
    partition.iter().copied().max().map_or(0, |m| m + 1)
}

/// Efficiency `q_declared / q_detected`, clamped to `[0, 1]`; `None` when the
/// detector found no positive structure to compare against.
fn efficiency(q_declared: f64, q_detected: f64) -> Option<f64> {
    if q_detected <= 0.0 {
        None
    } else {
        Some((q_declared / q_detected).clamp(0.0, 1.0))
    }
}

/// The detector level whose community count is closest to `target` (ties broken
/// toward higher modularity).
fn matched(levels: &[Level], target: usize) -> Option<&Level> {
    levels.iter().min_by(|a, b| {
        a.communities
            .abs_diff(target)
            .cmp(&b.communities.abs_diff(target))
            .then(
                b.modularity
                    .partial_cmp(&a.modularity)
                    .unwrap_or(core::cmp::Ordering::Equal),
            )
    })
}

// Skipped for mutation testing: the only generated mutant deletes the
// `resolution` field, which is equivalent because the detector's default
// resolution equals the value passed (1.0) in every real configuration.
#[cfg_attr(test, mutants::skip)]
fn undirected_levels(
    graph: &WeightedGraph,
    resolution: f64,
) -> Result<Vec<Level>, ModularityError> {
    let config = LeidenConfig {
        resolution,
        ..Default::default()
    };
    let result = Leiden::<usize>::leiden(graph, &config)?;
    Ok(result.levels().iter().map(level_from).collect())
}

// Skipped for mutation testing: see `undirected_levels`.
#[cfg_attr(test, mutants::skip)]
fn directed_levels(graph: &WeightedGraph, resolution: f64) -> Result<Vec<Level>, ModularityError> {
    let config = LouvainConfig {
        resolution,
        ..Default::default()
    };
    let result = DirectedLouvain::<usize>::directed_louvain(graph, &config)?;
    Ok(result.levels().iter().map(level_from).collect())
}

/// Builds a [`Level`] from any detector level exposing a partition and
/// modularity.
fn level_from<L>(level: &L) -> Level
where
    L: DetectorLevel,
{
    let partition = level.partition().to_vec();
    Level {
        communities: community_count(&partition),
        modularity: level.modularity(),
        partition,
    }
}

/// A detector level that exposes its partition and modularity. Implemented for
/// the Louvain and Leiden level types.
trait DetectorLevel {
    fn partition(&self) -> &[usize];
    fn modularity(&self) -> f64;
}

impl DetectorLevel for LouvainLevel<usize> {
    fn partition(&self) -> &[usize] {
        LouvainLevel::partition(self)
    }
    fn modularity(&self) -> f64 {
        LouvainLevel::modularity(self)
    }
}

impl DetectorLevel for LeidenLevel<usize> {
    fn partition(&self) -> &[usize] {
        LeidenLevel::partition(self)
    }
    fn modularity(&self) -> f64 {
        LeidenLevel::modularity(self)
    }
}
