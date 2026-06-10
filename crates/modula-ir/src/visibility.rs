//! Item and module visibility.

use serde::{Deserialize, Serialize};

/// The declared visibility of an item or module.
///
/// The ordering exposed by [`Visibility::restrictiveness`] (private = most
/// restrictive, public = least) is the backbone of the over-exposure metric: an
/// item is over-exposed when its declared visibility is less restrictive than
/// the narrowest visibility its actual use-set would require.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    /// `pub`.
    Public,
    /// `pub(crate)`.
    Crate,
    /// `pub(super)`.
    Super,
    /// Private (`pub(self)` or no modifier).
    Private,
    /// `pub(in path)`, carrying the canonical path of the visibility module.
    Module(String),
}

impl Visibility {
    /// A coarse restrictiveness rank where a larger value is *less* restrictive
    /// (more visible). `Private` is the most restrictive, `Public` the least.
    ///
    /// `pub(in path)` is ranked just above `pub(super)`: it is a bounded, named
    /// scope, so it is treated as more restrictive than crate-wide visibility.
    #[must_use]
    pub fn restrictiveness(&self) -> u8 {
        match self {
            Visibility::Private => 0,
            Visibility::Super => 1,
            Visibility::Module(_) => 2,
            Visibility::Crate => 3,
            Visibility::Public => 4,
        }
    }
}
