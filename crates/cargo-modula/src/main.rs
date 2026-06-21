//! `cargo modula`: score how well a Rust crate's module tree matches its actual
//! internal dependency graph.
//!
//! Invoked as a cargo subcommand (`cargo modula [PATH]`) or directly
//! (`cargo-modula modula [PATH]`).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser;
use modula_extract::RaExtractor;
use modula_extract_api::{ExtractOptions, ExtractRequest, Registry};
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

    /// Score a prebuilt SCIP index (`.scip`) instead of running extraction. This
    /// is how non-Rust languages are analyzed: a SCIP indexer (run in CI or
    /// locally) produces the index, and the IR is lowered from it, so no language
    /// toolchain is needed here.
    #[arg(long, value_name = "FILE")]
    scip: Option<PathBuf>,

    /// Emit the machine-readable JSON report instead of the human report.
    #[arg(long)]
    json: bool,

    /// Emit the extracted IR (the `CrateGraph`) as a compact binary container and
    /// exit, without scoring. This is the input the `modula-web` report viewer
    /// consumes. The bytes go to stdout, so redirect them to a file.
    #[arg(long)]
    emit_ir: bool,

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
        // Exit 0 when the gates pass, 1 when a gate fails, and 2 when the tool
        // itself errors. The distinct error code lets callers (the CI action)
        // tell a low score from a failure to analyze at all.
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// Runs the analysis and prints the report. Returns whether the gates passed.
fn run(args: &Args) -> anyhow::Result<bool> {
    let graph = if let Some(scip) = &args.scip {
        modula_extract_scip::lower_path(scip).context("lowering SCIP index")?
    } else {
        anyhow::ensure!(
            args.path.exists(),
            "path does not exist: {}",
            args.path.display()
        );
        let mut registry = Registry::new();
        registry.register(Box::new(RaExtractor));
        if registry.detect(&args.path).is_some() {
            let request = ExtractRequest {
                root: args.path.clone(),
                language: None,
                options: ExtractOptions {
                    include_dependencies: false,
                    member: args.package.clone(),
                    all_members: args.workspace,
                },
            };
            registry.extract(&request).context("extraction failed")?
        } else if let Some(indexer) = modula_extract_scip::indexer_for(&args.path) {
            modula_extract_scip::run_indexer(indexer.as_ref(), &args.path)
                .context("indexing failed")?
        } else {
            anyhow::bail!(
                "could not detect a supported project at {}",
                args.path.display()
            )
        }
    };

    if args.emit_ir {
        use std::io::Write as _;
        let payload = modula_ir::encode_compact(&graph).context("encoding IR")?;
        let compressed = zstd::encode_all(payload.as_slice(), 19).context("compressing IR")?;
        let bytes = modula_ir::wrap_container(
            &compressed,
            modula_ir::Codec::PostcardCompact,
            modula_ir::Compression::Zstd,
        );
        std::io::stdout()
            .write_all(&bytes)
            .context("writing IR container")?;
        return Ok(true);
    }

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
