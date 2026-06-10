//! Crates analyzed in a single run.

use serde::{Deserialize, Serialize};

use crate::{CrateId, ModuleId};

/// A crate in the analyzed workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crate {
    /// Dense id of this crate.
    pub id: CrateId,
    /// The crate name.
    pub name: String,
    /// `true` for a workspace member, `false` for an external dependency kept as
    /// a boundary.
    pub is_local: bool,
    /// The root module of this crate.
    pub root_module: ModuleId,
}
