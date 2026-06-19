//! Module `b`.

/// Calls `a::f` from inside its body using a fully qualified path and no `use`
/// import. This is the body-level cross-module reference a signature-only
/// analyzer (such as cargo-modules) does not capture, and the edge the spike
/// must prove it can extract.
pub fn g() -> u32 {
    crate::a::f() + 1
}
