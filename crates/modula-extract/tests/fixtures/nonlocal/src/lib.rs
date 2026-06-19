//! A trait implemented for a non-local / non-ADT self type. The associated
//! method has no local owning type, so it must stay owned by its `mod` (no
//! type container is created for `u32`).

/// A local trait implemented on a foreign primitive.
pub trait Doubler {
    /// Doubles the value.
    fn doubled(&self) -> u64;
}

impl Doubler for u32 {
    fn doubled(&self) -> u64 {
        u64::from(*self) * 2
    }
}
