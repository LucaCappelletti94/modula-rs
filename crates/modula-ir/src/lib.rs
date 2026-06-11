//! Intermediate representation (IR) shared between the modula-rs extraction and
//! metrics layers.
//!
//! This crate is the firewall between the unstable rust-analyzer based
//! extraction (`modula-extract`) and the pure metric layer (`modula-metrics`).
//! It depends only on `serde`, so the metric layer can be built and tested on
//! hand-written IR with no rust-analyzer involved.
//!
//! The central type is [`CrateGraph`]: a flat, serializable bundle of crates,
//! modules, items, and directed dependency edges. The `parent` links on
//! [`Module`] form the ownership tree the tool measures against; the [`Edge`]
//! list is the dependency graph.

#![forbid(unsafe_code)]

mod edge;
mod ids;
mod item;
mod krate;
mod module;
mod visibility;

pub use edge::{Edge, RefKind};
pub use ids::{CrateId, ItemId, ModuleId};
pub use item::{Item, ItemKind};
pub use krate::Crate;
pub use module::Module;
pub use visibility::Visibility;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// The IR schema version. Bumped when the shape of [`CrateGraph`] changes so
/// that stale caches and snapshots self-invalidate.
pub const SCHEMA_VERSION: u32 = 1;

/// The complete extracted representation of a workspace.
///
/// All ids are dense indices into the corresponding vectors: `items[i].id` is
/// `ItemId(i as u32)`, and likewise for modules and crates. The metric layer
/// relies on this density to map ids to graph node indices cheaply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateGraph {
    /// The IR schema version (see [`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The rust-analyzer snapshot version used for extraction, for provenance
    /// and cache invalidation. Empty for hand-written fixtures.
    pub ra_version: String,
    /// The crate the user asked about (the root of the analysis).
    pub root_crate: CrateId,
    /// All crates touched by the analysis (locals plus optional boundaries).
    pub crates: Vec<Crate>,
    /// All modules, indexed by [`ModuleId`].
    pub modules: Vec<Module>,
    /// All items, indexed by [`ItemId`].
    pub items: Vec<Item>,
    /// All directed dependency edges, one per `(from, to, kind)` triple.
    pub edges: Vec<Edge>,
}

impl CrateGraph {
    /// Returns the module with the given id.
    #[inline]
    #[must_use]
    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.index()]
    }

    /// Returns the item with the given id.
    #[inline]
    #[must_use]
    pub fn item(&self, id: ItemId) -> &Item {
        &self.items[id.index()]
    }

    /// Returns the crate with the given id.
    #[inline]
    #[must_use]
    pub fn krate(&self, id: CrateId) -> &Crate {
        &self.crates[id.index()]
    }

    /// The deepest module depth in the tree (crate root is depth 0).
    #[must_use]
    pub fn max_depth(&self) -> u32 {
        self.modules.iter().map(|m| m.depth).max().unwrap_or(0)
    }

    /// The ancestor of `module` at the given `depth`, climbing parent links.
    ///
    /// If `module` is shallower than `depth` (its own depth is less than
    /// `depth`), `module` itself is returned, clamping to the deepest ancestor
    /// available along this path.
    #[must_use]
    pub fn ancestor_module_at_depth(&self, module: ModuleId, depth: u32) -> ModuleId {
        let mut current = module;
        while self.module(current).depth > depth {
            match self.module(current).parent {
                Some(parent) => current = parent,
                None => break,
            }
        }
        current
    }

    /// The partition of items induced by the module tree at the given `depth`:
    /// each item is assigned the community id of its ancestor module at `depth`.
    ///
    /// Community ids are contiguous `0..c` (first-appearance order over items),
    /// and the returned vector is aligned with item ids (index `i` is item `i`).
    /// At `depth` 0 a single-crate graph collapses to one community; at
    /// [`max_depth`](Self::max_depth) each item maps to its own leaf module.
    #[must_use]
    pub fn partition_at_depth(&self, depth: u32) -> Vec<usize> {
        let mut community_of: HashMap<ModuleId, usize> = HashMap::new();
        let mut partition = Vec::with_capacity(self.items.len());
        for item in &self.items {
            let ancestor = self.ancestor_module_at_depth(item.owning_module, depth);
            let next = community_of.len();
            partition.push(*community_of.entry(ancestor).or_insert(next));
        }
        partition
    }

    /// Whether `module` is reachable through an unbroken `pub` chain from its
    /// crate root: every module from `module` up to (but not including) the root
    /// must be [`Visibility::Public`]. The root itself imposes no requirement.
    #[must_use]
    pub fn module_public_reachable(&self, module: ModuleId) -> bool {
        let mut current = module;
        loop {
            let node = self.module(current);
            match node.parent {
                None => return true,
                Some(parent) => {
                    if node.visibility != Visibility::Public {
                        return false;
                    }
                    current = parent;
                }
            }
        }
    }

    /// Sets [`Item::reachable_pub_api`] on every item: an item is intended public
    /// API iff it is `pub` and its owning module is publicly reachable from the
    /// crate root.
    pub fn compute_public_api(&mut self) {
        self.compute_public_api_with_reexports(&HashSet::new());
    }

    /// Like [`compute_public_api`](Self::compute_public_api), but also marks any
    /// item in `reexported` as public API. The extractor supplies the items that
    /// are `pub use`-re-exported through a publicly reachable module, which makes
    /// them public even when their defining module is private.
    pub fn compute_public_api_with_reexports(&mut self, reexported: &HashSet<ItemId>) {
        for index in 0..self.items.len() {
            let item = &self.items[index];
            let public = reexported.contains(&item.id)
                || (item.visibility == Visibility::Public
                    && self.module_public_reachable(item.owning_module));
            self.items[index].reachable_pub_api = public;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        Crate, CrateGraph, CrateId, Item, ItemId, ItemKind, Module, ModuleId, SCHEMA_VERSION,
        Visibility,
    };

    fn module(id: u32, parent: Option<u32>, depth: u32, visibility: Visibility) -> Module {
        Module {
            id: ModuleId(id),
            crate_id: CrateId(0),
            parent: parent.map(ModuleId),
            name: format!("m{id}"),
            canonical_path: format!("c::m{id}"),
            depth,
            visibility,
        }
    }

    fn item(id: u32, owning: u32, visibility: Visibility) -> Item {
        Item {
            id: ItemId(id),
            canonical_path: format!("c::item{id}"),
            kind: ItemKind::Function,
            visibility,
            owning_module: ModuleId(owning),
            crate_id: CrateId(0),
            has_canonical_path: true,
            reachable_pub_api: false,
        }
    }

    /// Tree: root(0,d0) <- a(1,d1,pub) <- b(2,d2,priv); c(3,d1,priv).
    /// Items: 0 pub in a, 1 pub in b, 2 priv in a, 3 pub in c.
    fn sample() -> CrateGraph {
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
            modules: vec![
                module(0, None, 0, Visibility::Public),
                module(1, Some(0), 1, Visibility::Public),
                module(2, Some(1), 2, Visibility::Private),
                module(3, Some(0), 1, Visibility::Private),
            ],
            items: vec![
                item(0, 1, Visibility::Public),
                item(1, 2, Visibility::Public),
                item(2, 1, Visibility::Private),
                item(3, 3, Visibility::Public),
            ],
            edges: Vec::new(),
        }
    }

    #[test]
    fn id_index_and_into_usize_roundtrip() {
        assert_eq!(ItemId(7).index(), 7);
        assert_eq!(usize::from(ModuleId(3)), 3);
        assert_eq!(usize::from(CrateId(0)), 0);
    }

    #[test]
    fn restrictiveness_orders_every_visibility() {
        assert!(Visibility::Private.restrictiveness() < Visibility::Super.restrictiveness());
        assert!(
            Visibility::Super.restrictiveness()
                < Visibility::Module(String::new()).restrictiveness()
        );
        assert!(
            Visibility::Module(String::new()).restrictiveness()
                < Visibility::Crate.restrictiveness()
        );
        assert!(Visibility::Crate.restrictiveness() < Visibility::Public.restrictiveness());
    }

    #[test]
    fn max_depth_and_ancestor_climb() {
        let g = sample();
        assert_eq!(g.max_depth(), 2);
        assert_eq!(g.ancestor_module_at_depth(ModuleId(2), 1), ModuleId(1));
        assert_eq!(g.ancestor_module_at_depth(ModuleId(2), 0), ModuleId(0));
        // Already shallower than the target depth: returns itself.
        assert_eq!(g.ancestor_module_at_depth(ModuleId(1), 5), ModuleId(1));
    }

    #[test]
    fn ancestor_breaks_on_orphan_module() {
        // A malformed module: positive depth but no parent. Climbing must break
        // and return it rather than loop forever.
        let mut g = sample();
        g.modules.push(module(4, None, 1, Visibility::Public));
        assert_eq!(g.ancestor_module_at_depth(ModuleId(4), 0), ModuleId(4));
    }

    #[test]
    fn partition_groups_items_by_ancestor() {
        let g = sample();
        let p0 = g.partition_at_depth(0);
        assert!(p0.iter().all(|&c| c == p0[0]), "depth 0 is one community");
        let p1 = g.partition_at_depth(1);
        assert_eq!(p1[0], p1[2], "items 0 and 2 share module a");
        assert_eq!(p1[0], p1[1], "item 1 (in b) rolls up to a at depth 1");
        assert_ne!(p1[0], p1[3], "c is a separate community from a");
    }

    #[test]
    fn module_public_reachable_requires_pub_chain() {
        let g = sample();
        assert!(g.module_public_reachable(ModuleId(0)));
        assert!(g.module_public_reachable(ModuleId(1)));
        assert!(!g.module_public_reachable(ModuleId(2)));
        assert!(!g.module_public_reachable(ModuleId(3)));
    }

    #[test]
    fn compute_public_api_marks_only_reachable_pub_items() {
        let mut g = sample();
        g.compute_public_api();
        assert!(g.item(ItemId(0)).reachable_pub_api, "pub in reachable a");
        assert!(!g.item(ItemId(1)).reachable_pub_api, "pub in private b");
        assert!(!g.item(ItemId(2)).reachable_pub_api, "private in a");
        assert!(!g.item(ItemId(3)).reachable_pub_api, "pub in private c");
    }

    #[test]
    fn reexports_force_public_api_in_private_module() {
        let mut g = sample();
        let reexported: HashSet<ItemId> = [ItemId(1)].into_iter().collect();
        g.compute_public_api_with_reexports(&reexported);
        assert!(
            g.item(ItemId(1)).reachable_pub_api,
            "re-exported item is public even from a private module"
        );
    }
}
