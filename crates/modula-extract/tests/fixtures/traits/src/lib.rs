//! Fixture exercising trait associated items, impl generic bounds, and the
//! `impl Trait` type position.

/// A local type returned by a trait method.
pub struct Record;

/// A marker trait used as a bound and in an `impl Trait` position.
pub trait Marker {}

/// A type implementing `Marker`.
pub struct Token;
impl Marker for Token {}

/// A trait with a method signature and a default method body.
pub trait Store {
    /// Signature references the local `Record`.
    fn describe(&self) -> Record;

    /// Default body calls `describe` (a body edge `helper -> describe`).
    fn helper(&self) -> Record {
        self.describe()
    }
}

/// A generic wrapper.
pub struct Pair<T>(pub T);

/// The impl is bounded by `Marker` (a bound edge `Pair -> Marker`).
impl<T: Marker> Pair<T> {
    /// Constructs a pair.
    pub fn new(value: T) -> Self {
        Pair(value)
    }
}

/// Returns `impl Marker` (a signature edge `provide -> Marker`).
pub fn provide() -> impl Marker {
    Token
}
