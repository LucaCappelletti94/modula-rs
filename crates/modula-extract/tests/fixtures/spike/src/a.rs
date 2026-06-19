//! Module `a`.

/// A crate-visible function that module `b` reaches into.
pub(crate) fn f() -> u32 {
    42
}
