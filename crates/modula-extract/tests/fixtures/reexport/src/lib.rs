//! Fixture exercising public-API detection through `pub use` re-exports.

mod private;

// Re-export from a private module: `Hidden` becomes public API.
pub use private::Hidden;
