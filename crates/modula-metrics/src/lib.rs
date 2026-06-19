//! Modularity metrics for modula-rs.
//!
//! Consumes the [`modula_ir`] intermediate representation and the
//! `geometric-traits` graph backend to produce the modularity score and the
//! diagnostic report. This crate never depends on rust-analyzer, so it builds
//! and tests fast against hand-written IR fixtures.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod cohesion;
pub mod coupling;
pub mod cycles;
pub mod encapsulation;
pub mod graph;
pub mod module_graph;
pub mod report;
pub mod score;
pub mod weighting;
