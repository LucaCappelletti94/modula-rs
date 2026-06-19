//! Dense identifiers for crates, modules, and items.
//!
//! These are assigned during extraction and are dense indices into the
//! [`CrateGraph`](crate::CrateGraph) vectors. They are an artifact of a
//! particular extraction and are NOT stable across runs. The stable, semantic
//! key for an item is its `canonical_path`; snapshots and hand-written fixtures
//! key on the path, never on these numeric ids.

use serde::{Deserialize, Serialize};

macro_rules! dense_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl $name {
            /// Returns the underlying index as a `usize`.
            #[inline]
            #[must_use]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl From<$name> for usize {
            #[inline]
            fn from(id: $name) -> usize {
                id.index()
            }
        }
    };
}

dense_id!(
    /// Identifier of a crate within a [`CrateGraph`](crate::CrateGraph).
    CrateId
);
dense_id!(
    /// Identifier of a module within a [`CrateGraph`](crate::CrateGraph).
    ModuleId
);
dense_id!(
    /// Identifier of an item within a [`CrateGraph`](crate::CrateGraph).
    ItemId
);
