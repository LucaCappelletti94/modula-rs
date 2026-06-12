//! Modules: the nodes of the ownership tree.

use serde::{Deserialize, Serialize};

use crate::{CrateId, ModuleId, Visibility};

/// What kind of containment node a [`Module`] is.
///
/// The ownership tree carries two kinds of node: real Rust `mod` namespaces, and
/// synthetic per-type containers inserted one level below their `mod` so a type
/// and its associated items form a cohesion cluster (the "type level").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModuleKind {
    /// A real Rust `mod` namespace (including the crate root).
    Mod,
    /// A synthetic container for a struct/enum/union/trait and its associated
    /// items. Its `parent` is always a [`ModuleKind::Mod`] node.
    Type,
}

/// A module: a node in the ownership tree.
///
/// The `parent` links form the ownership tree the tool measures the dependency
/// graph against. `depth` is precomputed (crate root is depth 0) so the metric
/// layer never walks parent links at runtime. The tree mixes real `mod`s and
/// synthetic per-type containers; see [`ModuleKind`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    /// Dense id of this module.
    pub id: ModuleId,
    /// The crate this module belongs to.
    pub crate_id: CrateId,
    /// The parent module, or `None` for a crate root.
    pub parent: Option<ModuleId>,
    /// The module name (empty string for a crate root).
    pub name: String,
    /// The canonical path (for example `my_crate::parser`).
    pub canonical_path: String,
    /// Depth in the ownership tree; the crate root is `0`.
    pub depth: u32,
    /// The declared visibility of the module.
    pub visibility: Visibility,
    /// Whether this is a real `mod` or a synthetic per-type container.
    pub kind: ModuleKind,
}
