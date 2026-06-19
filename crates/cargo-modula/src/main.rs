//! `cargo modula`: score how well a Rust crate's module tree matches its actual
//! internal dependency graph.
//!
//! Invoked as a cargo subcommand (`cargo modula [PATH]`) or directly
//! (`cargo-modula modula [PATH]`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser;
use modula_extract::{ExtractOptions, Extractor, RaExtractor};
use modula_metrics::analysis::{AnalysisConfig, analyze};
use modula_metrics::report::{Gates, evaluate_gates, to_human, to_json};

/// Cargo passes the subcommand name (`modula`) as the first argument, so the
/// top-level parser is an enum with a single `Modula` variant.
#[derive(Parser)]
#[command(bin_name = "cargo")]
enum Cargo {
    /// Score a crate's modularity.
    Modula(Args),
}

#[derive(clap::Args)]
#[command(version, about)]
struct Args {
    /// Path to the crate or workspace (a directory or a `Cargo.toml`).
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Analyze a specific workspace member by name.
    #[arg(long, value_name = "NAME")]
    package: Option<String>,

    /// Analyze every workspace member rather than the package at PATH.
    #[arg(long)]
    workspace: bool,

    /// Emit the machine-readable JSON report instead of the human report.
    #[arg(long)]
    json: bool,

    /// Fail (non-zero exit) if the headline score is below this threshold.
    #[arg(long, value_name = "SCORE")]
    min_headline: Option<f64>,

    /// Fail if the module dependency graph has any cycle.
    #[arg(long)]
    require_acyclic: bool,

    /// Fail if the over-exposed fraction exceeds this threshold.
    #[arg(long, value_name = "FRACTION")]
    max_overexposed: Option<f64>,
}

fn main() -> ExitCode {
    let Cargo::Modula(args) = Cargo::parse();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the analysis and prints the report. Returns whether the gates passed.
fn run(args: &Args) -> anyhow::Result<bool> {
    let manifest_path = resolve_manifest(&args.path)?;

    let graph = RaExtractor
        .extract(&ExtractOptions {
            manifest_path,
            package: args.package.clone(),
            workspace: args.workspace,
        })
        .context("extraction failed")?;
    let result = analyze(&graph, &AnalysisConfig::default()).context("analysis failed")?;

    if args.json {
        println!("{}", to_json(&result).context("serializing report")?);
    } else {
        print!("{}", to_human(&result));
    }

    let gates = Gates {
        min_headline: args.min_headline,
        require_acyclic: args.require_acyclic,
        max_overexposed_fraction: args.max_overexposed,
    };
    let outcome = evaluate_gates(&result, &gates);
    if !outcome.results.is_empty() && !args.json {
        eprintln!();
        eprintln!("Gates: {}", if outcome.passed { "PASS" } else { "FAIL" });
        for gate in &outcome.results {
            eprintln!(
                "  [{}] {}: {}",
                if gate.passed { "ok" } else { "!!" },
                gate.name,
                gate.detail
            );
        }
    }
    Ok(outcome.passed)
}

/// Resolves a user-supplied path to a `Cargo.toml`.
fn resolve_manifest(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_dir() {
        let manifest = path.join("Cargo.toml");
        anyhow::ensure!(
            manifest.is_file(),
            "no Cargo.toml found in {}",
            path.display()
        );
        Ok(manifest)
    } else if path.is_file() {
        Ok(path.to_path_buf())
    } else {
        anyhow::bail!("path does not exist: {}", path.display())
    }
}
