//! The `sweep` phase: re-run the metrics over every persisted IR, in-process and
//! in parallel. No rust-analyzer, no subprocess, so a full corpus pass is
//! seconds, which is what makes weight/metric calibration iterable.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use modula_ir::CrateGraph;
use modula_metrics::analysis::{AnalysisConfig, AnalysisResult, analyze};
use rayon::prelude::*;

use crate::db;
use crate::models::{Analysis, Extraction};

/// Options for the `sweep` phase.
pub struct SweepArgs {
    pub root: PathBuf,
    pub db_path: String,
}

/// Runs the metrics over every extracted IR and writes the `analyses` table.
pub fn run(args: &SweepArgs) -> Result<()> {
    let db_file = crate::extract::db_file(&args.root, &args.db_path);
    let mut conn = db::open(&db_file)?;
    let extractions = db::extractions_with_ir(&mut conn)?;
    println!("sweeping {} extracted crates ...", extractions.len());

    let done = AtomicUsize::new(0);
    let total = extractions.len();
    let start = Instant::now();

    let rows: Vec<Analysis> = extractions
        .par_iter()
        .map(|e| {
            let row = analyze_one(e);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(1000) || n == total {
                let rate = n as f64 / start.elapsed().as_secs_f64();
                println!("[{n}/{total}] {rate:.0}/s");
            }
            row
        })
        .collect();

    // Writes are serial (diesel SqliteConnection is single-threaded); they are
    // cheap relative to the parallel analysis above.
    let conn = Mutex::new(conn);
    let written = AtomicUsize::new(0);
    rows.iter().try_for_each(|row| -> Result<()> {
        let mut conn = conn.lock().expect("db mutex");
        db::upsert_analysis(&mut conn, row)?;
        written.fetch_add(1, Ordering::Relaxed);
        Ok(())
    })?;

    let ok = rows.iter().filter(|r| r.status == "ok").count();
    println!(
        "done: {} analyzed ({} ok, {} error) in {:.1}s",
        written.load(Ordering::Relaxed),
        ok,
        rows.len() - ok,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Reads one IR dump, runs the metrics, and builds its `analyses` row.
fn analyze_one(e: &Extraction) -> Analysis {
    let mut row = blank(e);
    let path = match &e.ir_path {
        Some(p) => p,
        None => {
            row.status = "error".to_owned();
            row.error = Some("no ir_path".to_owned());
            return row;
        }
    };
    let started = Instant::now();
    let result = std::fs::read(path)
        .with_context(|| format!("reading {path}"))
        .and_then(|bytes| serde_json::from_slice::<CrateGraph>(&bytes).context("deserializing IR"))
        .and_then(|ir| analyze(&ir, &AnalysisConfig::default()).context("analyze"));
    row.elapsed_ms = Some(started.elapsed().as_secs_f64() * 1000.0);

    match result {
        Ok(a) => fill(&mut row, &a),
        Err(err) => {
            row.status = "error".to_owned();
            row.error = Some(format!("{err:#}"));
        }
    }
    row
}

/// Populates a row from a successful analysis, flagging anomalous values.
fn fill(row: &mut Analysis, a: &AnalysisResult) {
    let c = &a.composite;
    row.status = "ok".to_owned();
    row.headline = c.headline;
    row.headline_depth_averaged = c.headline_depth_averaged;
    row.modularity_term = c.modularity_term;
    row.divergence_term = c.divergence_term;
    row.acyclicity_term = Some(c.acyclicity_term);
    row.encapsulation_term = Some(c.encapsulation_term);
    row.is_acyclic = Some(i32::from(a.tangles.is_acyclic));
    row.over_exposed_fraction = Some(a.encapsulation.over_exposed_fraction);
    row.mean_leak_cost = Some(a.encapsulation.mean_leak_cost);
    row.n_real_items = Some(a.n_real_items as i32);
    row.n_module_nodes = Some(a.n_module_nodes as i32);
    row.anomaly = anomalies(row);
}

/// Flags non-finite or out-of-`[0,1]` metric terms (the calibration bug hunt).
/// `mean_leak_cost` is exempt from the range check (it is a mean of costs).
fn anomalies(row: &Analysis) -> Option<String> {
    let terms: [(&str, Option<f64>); 7] = [
        ("headline", row.headline),
        ("modularity_term", row.modularity_term),
        ("divergence_term", row.divergence_term),
        ("acyclicity_term", row.acyclicity_term),
        ("encapsulation_term", row.encapsulation_term),
        ("over_exposed_fraction", row.over_exposed_fraction),
        ("mean_leak_cost", row.mean_leak_cost),
    ];
    let mut flags = Vec::new();
    for (name, value) in terms {
        let Some(v) = value else { continue };
        if !v.is_finite() {
            flags.push(format!("{name}=nonfinite"));
        } else if name != "mean_leak_cost" && !(-1e-9..=1.0 + 1e-9).contains(&v) {
            flags.push(format!("{name}={v:.3}_oob"));
        }
    }
    (!flags.is_empty()).then(|| flags.join(","))
}

/// A blank `ok`-less row carrying just the crate identity.
fn blank(e: &Extraction) -> Analysis {
    Analysis {
        name: e.name.clone(),
        version: e.version.clone(),
        status: "error".to_owned(),
        headline: None,
        headline_depth_averaged: None,
        modularity_term: None,
        divergence_term: None,
        acyclicity_term: None,
        encapsulation_term: None,
        is_acyclic: None,
        over_exposed_fraction: None,
        mean_leak_cost: None,
        n_real_items: None,
        n_module_nodes: None,
        anomaly: None,
        elapsed_ms: None,
        error: None,
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Analysis {
        Analysis {
            name: "x".to_owned(),
            version: "1".to_owned(),
            status: "ok".to_owned(),
            headline: None,
            headline_depth_averaged: None,
            modularity_term: None,
            divergence_term: None,
            acyclicity_term: None,
            encapsulation_term: None,
            is_acyclic: None,
            over_exposed_fraction: None,
            mean_leak_cost: None,
            n_real_items: None,
            n_module_nodes: None,
            anomaly: None,
            elapsed_ms: None,
            error: None,
            ts: 0,
        }
    }

    #[test]
    fn clean_terms_have_no_anomaly() {
        let mut r = row();
        r.headline = Some(0.5);
        r.acyclicity_term = Some(1.0);
        r.modularity_term = Some(0.0);
        assert_eq!(anomalies(&r), None);
    }

    #[test]
    fn out_of_range_and_nonfinite_are_flagged() {
        let mut r = row();
        r.modularity_term = Some(1.5);
        assert_eq!(anomalies(&r).as_deref(), Some("modularity_term=1.500_oob"));

        let mut r = row();
        r.divergence_term = Some(f64::NAN);
        assert_eq!(anomalies(&r).as_deref(), Some("divergence_term=nonfinite"));
    }

    #[test]
    fn mean_leak_cost_is_exempt_from_the_range_check() {
        let mut r = row();
        r.mean_leak_cost = Some(2.0); // a mean of costs, not a [0,1] ratio
        assert_eq!(anomalies(&r), None);
        // ...but non-finite is still flagged.
        r.mean_leak_cost = Some(f64::INFINITY);
        assert_eq!(anomalies(&r).as_deref(), Some("mean_leak_cost=nonfinite"));
    }
}
