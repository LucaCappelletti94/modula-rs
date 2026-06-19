//! rust-analyzer based extraction of the modula-rs intermediate representation.
//!
//! This is the only crate in the workspace that depends on the `ra_ap_*`
//! (rust-analyzer) crates. Everything rust-analyzer specific is sealed behind
//! the [`Extractor`] trait so the rest of the tool sees only [`modula_ir`].

#![forbid(unsafe_code)]

use std::path::PathBuf;

mod ra;

pub use ra::RaExtractor;

/// Options controlling a single extraction.
#[derive(Clone, Debug, Default)]
pub struct ExtractOptions {
    /// Path to the `Cargo.toml` of the workspace or package to analyze.
    pub manifest_path: PathBuf,
    /// Analyze a specific workspace member by name (overrides the default of
    /// selecting the package at `manifest_path`).
    pub package: Option<String>,
    /// Analyze every workspace member rather than a single package.
    pub workspace: bool,
}

/// Produces the [`modula_ir::CrateGraph`] for a workspace.
///
/// The trait exists so the binary and tests can pick an implementation
/// (rust-analyzer backed, or a fixture loader). All rust-analyzer richness lives
/// behind the returned IR, not behind a wide trait.
pub trait Extractor {
    /// Extracts the IR for the workspace described by `opts`.
    fn extract(&self, opts: &ExtractOptions) -> anyhow::Result<modula_ir::CrateGraph>;
}
