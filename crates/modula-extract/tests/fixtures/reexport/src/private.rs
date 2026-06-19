//! Private module whose types are re-exported in various ways.

/// Re-exported via a simple `pub use`.
pub struct Hidden;

/// Re-exported via a nested `pub use`.
pub struct Nested1;

/// Re-exported via a nested `pub use`.
pub struct Nested2;

/// Re-exported via a renamed `pub use ... as ...`.
pub struct Renamed;

/// Re-exported only via `pub(crate) use`: not public API.
pub struct OnlyCrate;

/// Imported only via a private `use`: not public API.
pub struct OnlyPrivate;
