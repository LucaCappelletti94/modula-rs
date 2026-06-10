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
