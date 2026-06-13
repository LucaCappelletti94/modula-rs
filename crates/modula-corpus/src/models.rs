//! Row structs for the `extractions` and `analyses` tables.

use diesel::prelude::*;

use crate::schema::{analyses, extractions};

/// A row of the `extractions` table: the outcome of one crate's IR extraction.
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = extractions)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Extraction {
    pub name: String,
    pub version: String,
    pub downloads: i64,
    pub status: String,
    pub ir_path: Option<String>,
    pub n_items: Option<i32>,
    pub n_modules: Option<i32>,
    pub n_edges: Option<i32>,
    pub n_import_edges: Option<i32>,
    pub n_signature_edges: Option<i32>,
    pub n_trait_bound_edges: Option<i32>,
    pub n_impl_edges: Option<i32>,
    pub n_body_edges: Option<i32>,
    pub n_structs: Option<i32>,
    pub n_enums: Option<i32>,
    pub n_traits: Option<i32>,
    pub n_type_aliases: Option<i32>,
    pub n_functions: Option<i32>,
    pub n_pub_api_items: Option<i32>,
    pub elapsed_sec: Option<f64>,
    /// Download + unpack wall time preceding extraction.
    pub prepare_sec: Option<f64>,
    /// Peak resident memory of the extractor process, in KiB (from /proc).
    pub peak_rss_kb: Option<i64>,
    /// Size of the downloaded `.crate` tarball, in bytes.
    pub crate_bytes: Option<i64>,
    pub error: Option<String>,
    /// rust-analyzer version that produced the IR (from the IR file).
    pub ra_version: Option<String>,
    /// IR schema version that produced the IR (from the IR file).
    pub schema_version: Option<i32>,
    /// Comma-joined crates.io category slugs (the standardized taxonomy).
    pub categories: Option<String>,
    /// Comma-joined crates.io keyword slugs (free-form author tags).
    pub keywords: Option<String>,
    pub ts: i64,
}

/// A row of the `analyses` table: the metric outcome of one crate's sweep.
#[derive(Debug, Clone, Queryable, Selectable, Insertable)]
#[diesel(table_name = analyses)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub struct Analysis {
    pub name: String,
    pub version: String,
    pub status: String,
    pub headline: Option<f64>,
    pub headline_depth_averaged: Option<f64>,
    pub modularity_term: Option<f64>,
    pub divergence_term: Option<f64>,
    pub acyclicity_term: Option<f64>,
    pub encapsulation_term: Option<f64>,
    pub is_acyclic: Option<i32>,
    pub over_exposed_fraction: Option<f64>,
    pub mean_leak_cost: Option<f64>,
    pub n_real_items: Option<i32>,
    pub n_module_nodes: Option<i32>,
    pub n_sccs: Option<i32>,
    pub largest_scc: Option<i32>,
    pub modules_in_cycles: Option<i32>,
    pub circuits_truncated: Option<i32>,
    pub max_leak_cost: Option<f64>,
    pub n_over_exposed: Option<i32>,
    pub n_cross_module_edges: Option<i32>,
    pub mean_instability: Option<f64>,
    pub median_instability: Option<f64>,
    pub mean_cohesion: Option<f64>,
    pub mean_distance_main_sequence: Option<f64>,
    pub anomaly: Option<String>,
    pub elapsed_ms: Option<f64>,
    pub error: Option<String>,
    pub ts: i64,
}
