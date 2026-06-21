//! One-time migration: convert legacy `<name>-<ver>.ir.json` IR dumps to the
//! compact binary container `<name>-<ver>.bin.zst`, update the database
//! `ir_path`, and remove the JSON. No rust-analyzer is involved (it only decodes
//! and re-encodes), so a full corpus pass is cheap.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use indicatif::ParallelProgressIterator as _;
use modula_ir::{CrateGraph, read_container};
use rayon::prelude::*;

use crate::db;
use crate::extract::encode_ir_container;
use crate::models::Extraction;

/// Options for the `convert` phase.
pub struct ConvertArgs {
    pub root: PathBuf,
    pub db_path: String,
    /// Convert a single standalone `.ir.json` file instead of the whole corpus,
    /// without touching the database.
    pub file: Option<PathBuf>,
}

/// The outcome for one extraction row.
enum Outcome {
    /// Converted to binary; the updated row to persist.
    Converted(Box<Extraction>),
    /// Already binary (or no JSON path); nothing to do.
    Skipped,
    /// Conversion failed; the JSON is left in place.
    Failed(String),
}

/// Converts every legacy JSON IR dump to the binary container, or a single file
/// when `--file` is given.
pub fn run(args: &ConvertArgs) -> Result<()> {
    if let Some(file) = &args.file {
        let new_path = convert_to_container(file)?;
        println!("converted {} -> {new_path}", file.display());
        return Ok(());
    }

    let db_file = crate::extract::db_file(&args.root, &args.db_path);
    let mut conn = db::open(&db_file)?;
    let rows = db::extractions_with_ir(&mut conn)?;
    println!("converting {} IR dumps to binary ...", rows.len());

    let pb = crate::extract::progress_bar(rows.len() as u64);
    let outcomes: Vec<Outcome> = rows
        .par_iter()
        .progress_with(pb.clone())
        .map(convert_one)
        .collect();
    pb.finish_and_clear();

    // Database writes are serial (diesel SqliteConnection is single-threaded).
    let conn = Mutex::new(conn);
    let (mut converted, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    for outcome in outcomes {
        match outcome {
            Outcome::Converted(row) => {
                let mut conn = conn.lock().expect("db mutex");
                db::upsert_extraction(&mut conn, &row)?;
                converted += 1;
            }
            Outcome::Skipped => skipped += 1,
            Outcome::Failed(msg) => {
                failed += 1;
                eprintln!("convert failed: {msg}");
            }
        }
    }
    println!("done: {converted} converted, {skipped} already binary, {failed} failed");
    Ok(())
}

fn convert_one(e: &Extraction) -> Outcome {
    let Some(path) = e.ir_path.clone() else {
        return Outcome::Skipped;
    };
    if !path.ends_with(".ir.json") {
        return Outcome::Skipped;
    }
    match convert_file(e, &path) {
        Ok(row) => Outcome::Converted(Box::new(row)),
        Err(err) => Outcome::Failed(format!("{path}: {err:#}")),
    }
}

fn convert_file(e: &Extraction, path: &str) -> Result<Extraction> {
    let new_path = convert_to_container(Path::new(path))?;
    let mut row = e.clone();
    row.ir_path = Some(new_path);
    Ok(row)
}

/// Reads a legacy JSON IR file, writes the binary container beside it (swapping
/// the `.ir.json` suffix for `.bin.zst`), removes the JSON, and returns the new
/// path. The container is decoded back and compared to the original before the
/// JSON is deleted, so a faulty conversion can never destroy the only copy.
fn convert_to_container(path: &Path) -> Result<String> {
    let path_str = path.to_str().context("non-UTF-8 path")?;
    let bytes = std::fs::read(path).with_context(|| format!("reading {path_str}"))?;
    let graph: CrateGraph = serde_json::from_slice(&bytes).context("parsing legacy JSON IR")?;
    let container = encode_ir_container(&graph)?;

    let decoded = read_container(&container).context("verifying container")?;
    anyhow::ensure!(decoded == graph, "container round-trip changed the graph");

    let new_path = match path_str.strip_suffix(".ir.json") {
        Some(stem) => format!("{stem}.bin.zst"),
        None => format!("{path_str}.bin.zst"),
    };
    std::fs::write(&new_path, &container).with_context(|| format!("writing {new_path}"))?;
    if new_path != path_str {
        std::fs::remove_file(path).with_context(|| format!("removing {path_str}"))?;
    }
    Ok(new_path)
}
