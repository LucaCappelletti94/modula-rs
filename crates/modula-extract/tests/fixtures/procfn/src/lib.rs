//! Fixture exercising function-like and attribute proc macros that reference a
//! local item only after expansion.

use procfn_macro::{call_local, wrap};

/// The local function both macros reach.
pub fn local_target() -> u32 {
    0
}

/// A function-like proc macro in the body expands to `local_target()`, so the
/// edge `via_function_like -> local_target` exists only via macro descent.
pub fn via_function_like() -> u32 {
    call_local!()
}

/// An attribute proc macro rewrites this body to `local_target()`, so the edge
/// `via_attribute -> local_target` exists only if the expanded body is walked.
#[wrap]
pub fn via_attribute() -> u32 {
    0
}
