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
        // Aggregate at the real-module level: a type container's items roll up to
        // the `mod` that declares the type, so coupling and cycles stay
        // module-level rather than dropping to the finer type level.
        for item in &ir.items {
            let module = ir.real_module(item.owning_module);
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
            let src = index_of[&ir.real_module(ir.item(edge.from).owning_module)];
            let dst = index_of[&ir.real_module(ir.item(edge.to).owning_module)];
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

    use modula_ir::{
        Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
        RefKind, SCHEMA_VERSION, Visibility,
    };

    use super::ModuleAggregation;
    use crate::weighting::RefKindWeights;

    #[test]
    fn type_containers_roll_up_to_their_real_module() {
        // mod a(0) owns two type containers Foo(1) and Bar(2). An edge between an
        // item in Foo and an item in Bar must aggregate as INTRA mod a (not as
        // two separate type nodes).
        let m = |id: u32, parent: Option<u32>, depth: u32, kind: ModuleKind| Module {
            id: ModuleId(id),
            crate_id: CrateId(0),
            parent: parent.map(ModuleId),
            name: format!("m{id}"),
            canonical_path: format!("c::m{id}"),
            depth,
            visibility: Visibility::Public,
            kind,
        };
        let it = |id: u32, owning: u32| Item {
            id: ItemId(id),
            canonical_path: format!("c::i{id}"),
            kind: ItemKind::Function,
            visibility: Visibility::Public,
            owning_module: ModuleId(owning),
            crate_id: CrateId(0),
            has_canonical_path: true,
            reachable_pub_api: false,
        };
        let ir = CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "c".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules: vec![
                m(0, None, 0, ModuleKind::Mod),
                m(1, Some(0), 1, ModuleKind::Type),
                m(2, Some(0), 1, ModuleKind::Type),
            ],
            items: vec![it(0, 1), it(1, 2)],
            edges: vec![Edge {
                from: ItemId(0),
                to: ItemId(1),
                kind: RefKind::Body,
                weight: 1,
            }],
        };
        let agg = ModuleAggregation::build(&ir, &RefKindWeights::default());
        assert_eq!(agg.len(), 1, "both type containers roll up to mod a");
        assert_eq!(agg.nodes, vec![ModuleId(0)]);
        assert!(
            agg.inter.is_empty(),
            "the cross-type edge is intra-module a"
        );
        assert!(agg.intra[0] > 0.0);
    }

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
