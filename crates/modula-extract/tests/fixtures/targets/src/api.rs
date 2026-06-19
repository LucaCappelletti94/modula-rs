//! Public module.

/// Public API: `pub` in a `pub` module, reachable from the crate root.
pub fn visible() -> u32 {
    crate::internal::hidden()
}
