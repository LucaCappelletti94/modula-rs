//! Fixture exercising public-API detection through `pub use` re-exports:
//! simple, nested, glob, rename, plus the negative cases that must NOT be
//! treated as public API.
#![allow(unused_imports)]

mod globbed;
mod private;

// Simple re-export.
pub use private::Hidden;
// Nested re-export.
pub use private::{Nested1, Nested2};
// Glob re-export of a private module's public items.
pub use globbed::*;
// Renamed re-export: the target item is still public API.
pub use private::Renamed as PublicName;

// Negative cases: neither makes its target public API.
pub(crate) use private::OnlyCrate;
use private::OnlyPrivate;
