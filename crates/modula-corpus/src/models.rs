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
    pub elapsed_sec: Option<f64>,
    pub error: Option<String>,
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
    pub anomaly: Option<String>,
    pub elapsed_ms: Option<f64>,
    pub error: Option<String>,
    pub ts: i64,
}
