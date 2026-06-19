//! Shared error type for lowering the IR into `geometric-traits` weighted
//! graphs. The module-level graph is built directly where it is consumed (see
//! [`crate::cycles`]); this module only carries the build error.

/// An error building a `geometric-traits` graph from arcs.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// The underlying edge builder rejected the arcs.
    #[error("failed to build graph: {0}")]
    Build(String),
}
