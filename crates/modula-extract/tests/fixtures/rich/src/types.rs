//! Types module.

/// A leaf type.
pub struct Inner;

/// A type with a field of a local type, producing a signature edge to `Inner`.
pub struct Outer {
    pub inner: Inner,
}

/// A local trait.
pub trait Greet {
    /// Returns a number.
    fn greet(&self) -> u32;
}
