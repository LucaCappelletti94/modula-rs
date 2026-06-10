//! Modules: the nodes of the ownership tree.

use serde::{Deserialize, Serialize};

use crate::{CrateId, ModuleId, Visibility};

/// A module: a node in the ownership tree.
///
/// The `parent` links form the ownership tree the tool measures the dependency
/// graph against. `depth` is precomputed (crate root is depth 0) so the metric
/// layer never walks parent links at runtime.
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
}
