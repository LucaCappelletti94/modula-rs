//! Encapsulation metrics, the Rust-specific dimension.
//!
//! Two signals:
//! - **Over-exposure**: an item whose declared visibility is wider than its
//!   actual use-set requires (for example a `pub` item used only within its own
//!   module).
//! - **Leak depth**: each cross-module dependency edge is costed by how far apart
//!   its two modules sit in the module tree, using Lin similarity. The cost is
//!   `1 - lin(m_src, m_dst)`: near 0 for closely related modules (deep shared
//!   ancestor), near 1 when the only shared ancestor is the crate root.

use geometric_traits::{
    impls::{CSR2D, SortedVec, SquareCSR2D},
    naive_structs::DiGraph,
    prelude::*,
};
use modula_ir::{CrateGraph, ItemId, ModuleId, Visibility};
use serde::Serialize;

/// An error computing encapsulation metrics.
#[derive(Debug, thiserror::Error)]
pub enum EncapsulationError {
    /// The module tree could not be built or its information content computed.
    #[error("module tree analysis failed: {0}")]
    Tree(String),
}

/// An item exposed more widely than its use-set requires.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverExposure {
    /// The over-exposed item.
    pub item: ItemId,
    /// The item's canonical path.
    pub path: String,
    /// The declared visibility.
    pub declared: Visibility,
    /// The narrowest visibility that still satisfies every consumer.
    pub required: Visibility,
    /// Whether the item is part of the intended public API (advisory only).
    pub reachable_pub_api: bool,
}

/// A cross-module dependency edge, costed by how far apart its modules are.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leak {
    /// The referring item.
    pub from: ItemId,
    /// The referenced item.
    pub to: ItemId,
    /// Canonical path of the referring item.
    pub from_path: String,
    /// Canonical path of the referenced item.
    pub to_path: String,
    /// Lin similarity of the two owning modules, in `[0, 1]`.
    pub lin: f64,
    /// Leak cost `1 - lin`, in `[0, 1]`.
    pub leak_cost: f64,
}

/// The encapsulation report for a crate graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EncapsulationReport {
    /// Over-exposed items (declared visibility wider than required), excluding
    /// intended public-API items.
    pub over_exposed: Vec<OverExposure>,
    /// Fraction of items that are over-exposed.
    pub over_exposed_fraction: f64,
    /// Cross-module leaks, ranked by descending cost.
    pub deepest_leaks: Vec<Leak>,
    /// Mean leak cost across cross-module edges (`0` when there are none).
    pub mean_leak_cost: f64,
}

/// Computes the encapsulation report.
///
/// # Errors
/// Returns [`EncapsulationError`] if the module-tree information content cannot
/// be computed.
pub fn encapsulation(ir: &CrateGraph) -> Result<EncapsulationReport, EncapsulationError> {
    let over_exposed = over_exposed(ir);
    let over_exposed_fraction = if ir.items.is_empty() {
        0.0
    } else {
        over_exposed.len() as f64 / ir.items.len() as f64
    };

    let (deepest_leaks, mean_leak_cost) = leaks(ir)?;

    Ok(EncapsulationReport {
        over_exposed,
        over_exposed_fraction,
        deepest_leaks,
        mean_leak_cost,
    })
}

/// Finds items whose declared visibility exceeds what their consumers require.
fn over_exposed(ir: &CrateGraph) -> Vec<OverExposure> {
    // Consumer sets: for each item, the items that reference it.
    let mut consumers: Vec<Vec<ItemId>> = vec![Vec::new(); ir.items.len()];
    for edge in &ir.edges {
        consumers[edge.to.index()].push(edge.from);
    }

    let mut result = Vec::new();
    for item in &ir.items {
        if item.reachable_pub_api {
            // Intended public API: over-exposure is advisory, so do not flag it.
            continue;
        }
        let required = required_visibility(ir, item.id, &consumers[item.id.index()]);
        if item.visibility.restrictiveness() > required.restrictiveness() {
            result.push(OverExposure {
                item: item.id,
                path: item.canonical_path.clone(),
                declared: item.visibility.clone(),
                required,
                reachable_pub_api: item.reachable_pub_api,
            });
        }
    }
    result
}

/// The narrowest visibility that still lets every consumer reach `item`.
fn required_visibility(ir: &CrateGraph, item: ItemId, consumers: &[ItemId]) -> Visibility {
    let owner = ir.item(item).owning_module;
    let krate = ir.item(item).crate_id;
    let mut required = Visibility::Private;
    for &consumer in consumers {
        let needed = needed_visibility(
            ir,
            owner,
            krate,
            ir.item(consumer).owning_module,
            ir.item(consumer).crate_id,
        );
        if needed.restrictiveness() > required.restrictiveness() {
            required = needed;
        }
    }
    required
}

/// The minimum visibility `item` (owned by `owner` in `krate`) must declare to
/// be reachable from a consumer owned by `consumer_owner` in `consumer_crate`.
fn needed_visibility(
    ir: &CrateGraph,
    owner: ModuleId,
    krate: modula_ir::CrateId,
    consumer_owner: ModuleId,
    consumer_crate: modula_ir::CrateId,
) -> Visibility {
    if consumer_owner == owner {
        Visibility::Private
    } else if consumer_crate == krate {
        match ir.module(owner).parent {
            Some(parent) if is_within_subtree(ir, consumer_owner, parent) => Visibility::Super,
            _ => Visibility::Crate,
        }
    } else {
        Visibility::Public
    }
}

/// Whether `module` is `ancestor` or a descendant of it.
fn is_within_subtree(ir: &CrateGraph, module: ModuleId, ancestor: ModuleId) -> bool {
    let mut current = Some(module);
    while let Some(m) = current {
        if m == ancestor {
            return true;
        }
        current = ir.module(m).parent;
    }
    false
}

/// Computes per-edge leak costs (ranked) and their mean.
fn leaks(ir: &CrateGraph) -> Result<(Vec<Leak>, f64), EncapsulationError> {
    let n = ir.modules.len();
    let vocabulary: SortedVec<usize> = GenericVocabularyBuilder::default()
        .expected_number_of_symbols(n)
        .symbols((0..n).enumerate())
        .build()
        .map_err(|e| EncapsulationError::Tree(format!("{e:?}")))?;

    // Edges point parent -> child so the crate root is the source (least
    // specific, information content 0) and leaf modules are sinks. The edge
    // builder requires row-major sorted coordinates.
    let mut dag_edges: Vec<(usize, usize)> = ir
        .modules
        .iter()
        .filter_map(|m| m.parent.map(|parent| (parent.index(), m.id.index())))
        .collect();
    dag_edges.sort_unstable();
    let edges: SquareCSR2D<CSR2D<usize, usize, usize>> = DiEdgesBuilder::default()
        .expected_number_of_edges(dag_edges.len())
        .expected_shape(n)
        .edges(dag_edges.iter().copied())
        .build()
        .map_err(|e| EncapsulationError::Tree(format!("{e:?}")))?;

    let graph: DiGraph<usize> = DiGraph::from((vocabulary, edges));
    // Uniform occurrences: information content reflects tree position, and every
    // leaf module has a non-zero occurrence.
    let occurrences = vec![1usize; n];
    let lin = graph
        .lin(&occurrences)
        .map_err(|e| EncapsulationError::Tree(format!("{e:?}")))?;

    // One leak per unique cross-module item edge.
    let mut seen = std::collections::HashSet::new();
    let mut leaks = Vec::new();
    for edge in &ir.edges {
        let src_module = ir.item(edge.from).owning_module;
        let dst_module = ir.item(edge.to).owning_module;
        if src_module == dst_module || !seen.insert((edge.from, edge.to)) {
            continue;
        }
        let similarity = lin.similarity(&src_module.index(), &dst_module.index());
        leaks.push(Leak {
            from: edge.from,
            to: edge.to,
            from_path: ir.item(edge.from).canonical_path.clone(),
            to_path: ir.item(edge.to).canonical_path.clone(),
            lin: similarity,
            leak_cost: 1.0 - similarity,
        });
    }

    let mean_leak_cost = if leaks.is_empty() {
        0.0
    } else {
        leaks.iter().map(|l| l.leak_cost).sum::<f64>() / leaks.len() as f64
    };
    leaks.sort_by(|a, b| {
        b.leak_cost
            .partial_cmp(&a.leak_cost)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    Ok((leaks, mean_leak_cost))
}

#[cfg(test)]
mod tests {
    use modula_ir::{Crate, CrateGraph, CrateId, Module, ModuleId, SCHEMA_VERSION, Visibility};

    use super::is_within_subtree;

    /// Tree: 0 root; 1 child of 0; 2 child of 1; 3 child of 0.
    fn tree() -> CrateGraph {
        let parents = [None, Some(0u32), Some(1), Some(0)];
        let modules = (0..4u32)
            .map(|i| Module {
                id: ModuleId(i),
                crate_id: CrateId(0),
                parent: parents[i as usize].map(ModuleId),
                name: format!("m{i}"),
                canonical_path: format!("c::m{i}"),
                depth: u32::from(i != 0),
                visibility: Visibility::Public,
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
    fn within_subtree_walks_ancestors() {
        let g = tree();
        assert!(is_within_subtree(&g, ModuleId(2), ModuleId(1)));
        assert!(is_within_subtree(&g, ModuleId(2), ModuleId(0)));
        assert!(is_within_subtree(&g, ModuleId(1), ModuleId(1)));
        // Sibling branch: 3 is not under 1.
        assert!(!is_within_subtree(&g, ModuleId(3), ModuleId(1)));
    }
}
