//! rust-analyzer based extraction of the modula-rs intermediate representation.
//!
//! This is the only crate in the workspace that depends on the `ra_ap_*`
//! (rust-analyzer) crates. It implements the language-neutral
//! [`modula_extract_api::Extractor`] for Rust, so the rest of the tool sees only
//! [`modula_ir`] behind the shared extraction seam.

#![forbid(unsafe_code)]

mod ra;

pub use ra::RaExtractor;
