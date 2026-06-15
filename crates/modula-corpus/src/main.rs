//! `modula-corpus`: internal tooling to enumerate crates.io, extract each
//! crate's IR once (rust-analyzer bound, isolated per crate), then re-run the
//! metric sweep in-process as often as calibration needs.
//!
//! Phases:
//! - `extract`: build the work-list from the crates.io db-dump, download each
//!   crate, run IR extraction in an isolated subprocess, persist the IR.
//! - `sweep`: re-run `modula-metrics::analyze` over every persisted IR in
//!   parallel (no rust-analyzer), writing the `analyses` table.
//! - `plot`: render the metric histograms to an SVG.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod db;
mod dump;
mod embed;
mod extract;
mod http;
mod models;
mod plot;
mod schema;
mod sweep;

const DEFAULT_ROOT: &str = "/mnt/nvme/modula-corpus";

#[derive(Parser)]
#[command(version, about = "modula-rs corpus extraction + metric sweep tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Enumerate crates.io and extract each crate's IR once (isolated, resumable).
    Extract {
        #[arg(long, default_value = DEFAULT_ROOT)]
        root: PathBuf,
        #[arg(long, default_value = "corpus.db")]
        db: String,
        #[arg(long, default_value_t = 100_000)]
        min_downloads: i64,
        #[arg(long, default_value_t = 12, help = "concurrent crates (~ cores used)")]
        jobs: usize,
        #[arg(
            long,
            default_value_t = 1,
            help = "CARGO_BUILD_JOBS per worker; total compile fan-out is jobs * build_jobs"
        )]
        build_jobs: usize,
        #[arg(long, default_value_t = 1200, value_name = "SECONDS")]
        timeout: u64,
        #[arg(long, help = "cap the work-list (for testing)")]
        limit: Option<usize>,
    },
    /// Re-run the metrics over every persisted IR (in-process, parallel).
    Sweep {
        #[arg(long, default_value = DEFAULT_ROOT)]
        root: PathBuf,
        #[arg(long, default_value = "corpus.db")]
        db: String,
    },
    /// Render the metric distributions to an SVG grid of histograms.
    Plot {
        #[arg(long, default_value = DEFAULT_ROOT)]
        root: PathBuf,
        #[arg(long, default_value = "corpus.db")]
        db: String,
        #[arg(long, default_value = "plots/metrics.svg")]
        out: PathBuf,
    },
    /// Project crate metric features to 2D (PCA + t-SNE), colored by category.
    Embed {
        #[arg(long, default_value = DEFAULT_ROOT)]
        root: PathBuf,
        #[arg(long, default_value = "corpus.db")]
        db: String,
        #[arg(
            long,
            default_value = "plots/embed",
            help = "output prefix; view suffixes are appended"
        )]
        out: PathBuf,
        #[arg(long, default_value_t = 30.0)]
        perplexity: f64,
        #[arg(long, default_value_t = 0, help = "cap on points (0 = all)")]
        max_points: usize,
    },
    /// Internal worker: extract one already-unpacked crate, print IR to stdout.
    #[command(hide = true)]
    ExtractOne { dir: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Extract {
            root,
            db,
            min_downloads,
            jobs,
            build_jobs,
            timeout,
            limit,
        } => extract::run(&extract::ExtractArgs {
            root,
            db_path: db,
            min_downloads,
            jobs,
            build_jobs,
            timeout: Duration::from_secs(timeout),
            limit,
        }),
        Command::Sweep { root, db } => sweep::run(&sweep::SweepArgs { root, db_path: db }),
        Command::Plot { root, db, out } => {
            let out = if out.is_absolute() {
                out
            } else {
                root.join(out)
            };
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            plot::run(&plot::PlotArgs {
                root,
                db_path: db,
                out,
            })
        }
        Command::Embed {
            root,
            db,
            out,
            perplexity,
            max_points,
        } => {
            let out = if out.is_absolute() {
                out
            } else {
                root.join(out)
            };
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            embed::run(&embed::EmbedArgs {
                root,
                db_path: db,
                out,
                perplexity,
                max_points,
            })
        }
        Command::ExtractOne { dir } => extract::extract_one(&dir),
    }
}
