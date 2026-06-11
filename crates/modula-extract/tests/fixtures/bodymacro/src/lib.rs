//! Fixture exercising reference resolution inside macro-call expansions.
//!
//! The only textual reference to `make_marker` lives inside the body of the
//! `marker!` macro, so the body edge `build -> make_marker` is captured only if
//! the body walk descends into the macro expansion.

/// A local type produced through a macro expansion.
pub struct Marker;

/// Referenced only from within a macro expansion.
pub fn make_marker() -> Marker {
    Marker
}

/// Expands to a call to `make_marker()`.
macro_rules! marker {
    () => {
        make_marker()
    };
}

/// `build`'s body contains no direct reference to `make_marker`; it appears only
/// after expanding `marker!()`.
pub fn build() -> Marker {
    marker!()
}
