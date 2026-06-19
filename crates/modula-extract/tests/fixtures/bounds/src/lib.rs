//! Fixture exercising trait-bound edges.

/// A trait used as a bound.
pub trait LocalTrait {}

/// A supertrait.
pub trait Super {}

/// A trait with a supertrait (`Sub -> Super` bound edge).
pub trait Sub: Super {}

/// Generic function bounded by `LocalTrait` (`f -> LocalTrait` bound edge).
pub fn f<T: LocalTrait>(_t: T) {}

/// Generic struct bounded by `LocalTrait` (`S -> LocalTrait` bound edge), with a
/// generic field so there is no concrete signature edge.
pub struct S<T: LocalTrait> {
    _t: T,
}
