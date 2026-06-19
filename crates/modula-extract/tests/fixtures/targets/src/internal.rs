//! Private module (declared `mod internal;`, not `pub`).

/// `pub` but inside a private module, so not reachable as public API.
pub fn hidden() -> u32 {
    0
}
