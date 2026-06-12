//! Aggregation of the item dependency graph up to the module level.
//!
//! Each item is attributed to its owning module; an item edge within a module is
//! intra-module weight (cohesion), and an edge between modules is an inter-module
//! arc (coupling). Only modules that own at least one item become nodes.

use std::collections::{BTreeMap, HashMap};

use modula_ir::{CrateGraph, ModuleId};

use crate::weighting::RefKindWeights;

/// The module-level view of a crate graph: per-module intra weight and directed
/// inter-module arc weights, over a dense node space.
pub struct ModuleAggregation {
    /// Node index to module id.
    pub nodes: Vec<ModuleId>,
    /// Module id to node index.
    pub index_of: HashMap<ModuleId, usize>,
    /// Intra-module (within-module) edge weight per node.
    pub intra: Vec<f64>,
    /// Directed inter-module arc weights, keyed by `(src_node, dst_node)`.
    pub inter: BTreeMap<(usize, usize), f64>,
}

impl ModuleAggregation {
    /// Aggregates the item edges of `ir` to the module level under `weights`.
    #[must_use]
    pub fn build(ir: &CrateGraph, weights: &RefKindWeights) -> Self {
        let mut index_of: HashMap<ModuleId, usize> = HashMap::new();
        let mut nodes: Vec<ModuleId> = Vec::new();
        for item in &ir.items {
            let module = item.owning_module;
            index_of.entry(module).or_insert_with(|| {
                let index = nodes.len();
                nodes.push(module);
                index
            });
        }

        let mut intra = vec![0.0; nodes.len()];
        let mut inter: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        for edge in &ir.edges {
            let w = weights.edge_weight(edge);
            if w <= 0.0 {
                continue;
            }
            let src = index_of[&ir.item(edge.from).owning_module];
            let dst = index_of[&ir.item(edge.to).owning_module];
            if src == dst {
                intra[src] += w;
            } else {
                *inter.entry((src, dst)).or_insert(0.0) += w;
            }
        }

        Self {
            nodes,
            index_of,
            intra,
            inter,
        }
    }

    /// Number of module nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether there are no module nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use modula_ir::ModuleId;

    use super::ModuleAggregation;

    #[test]
    fn is_empty_reflects_node_count() {
        let empty = ModuleAggregation {
            nodes: Vec::new(),
            index_of: HashMap::new(),
            intra: Vec::new(),
            inter: BTreeMap::new(),
        };
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let one = ModuleAggregation {
            nodes: vec![ModuleId(0)],
            index_of: [(ModuleId(0), 0)].into_iter().collect(),
            intra: vec![0.0],
            inter: BTreeMap::new(),
        };
        assert!(!one.is_empty());
        assert_eq!(one.len(), 1);
    }
}
