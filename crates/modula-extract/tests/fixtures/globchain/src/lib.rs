//! Fixture exercising glob re-export CHAINS: a glob whose source module itself
//! re-exports (here, through a second glob), so the chain must be followed for
//! the item to be recognized as public API.

/// A private module holding a public item reachable only through the chain.
mod inner {
    /// Public within `inner`, but `inner` is private, so this is public API only
    /// if a public re-export chain reaches it.
    pub struct Chained;
}

/// A private relay module that glob-re-exports `inner`. Being private, its own
/// re-export does not expose `Chained` as public API.
mod relay {
    pub use crate::inner::*;
}

/// The crate root glob-re-exports `relay`, so the only path that exposes
/// `Chained` as public API is the glob-through-glob chain root -> relay -> inner.
pub use relay::*;
