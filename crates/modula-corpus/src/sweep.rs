//! The `sweep` phase: re-run the metrics over every persisted IR, in-process and
//! in parallel. No rust-analyzer, no subprocess, so a full corpus pass is
//! seconds, which is what makes weight/metric calibration iterable.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use indicatif::ParallelProgressIterator as _;
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

    let total = extractions.len();
    let start = Instant::now();
    let pb = crate::extract::progress_bar(total as u64);

    let rows: Vec<Analysis> = extractions
        .par_iter()
        .progress_with(pb.clone())
        .map(analyze_one)
        .collect();
    pb.finish_and_clear();

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
        .and_then(|bytes| modula_ir::read_container(&bytes).context("deserializing IR"))
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
    row.cohesion_term = c.cohesion_term;
    row.acyclicity_term = Some(c.acyclicity_term);
    row.encapsulation_term = Some(c.encapsulation_term);
    row.is_acyclic = Some(i32::from(a.tangles.is_acyclic));
    row.over_exposed_fraction = Some(a.encapsulation.over_exposed_fraction);
    row.mean_leak_cost = Some(a.encapsulation.mean_leak_cost);
    row.n_real_items = Some(a.n_real_items as i32);
    row.n_module_nodes = Some(a.n_module_nodes as i32);

    // Cycle severity, beyond the is_acyclic boolean.
    let t = &a.tangles;
    row.n_sccs = Some(t.sccs.len() as i32);
    row.largest_scc = Some(t.largest_scc as i32);
    row.modules_in_cycles = Some(t.sccs.iter().map(Vec::len).sum::<usize>() as i32);
    row.cyclomatic_number = Some(t.cyclomatic_number as i32);

    // Encapsulation counts. `leaks` are the cross-module references that reach
    // into another module's internals (target not public API); `mean_leak_cost`
    // is their fraction over all cross-module references (the leak rate).
    let enc = &a.encapsulation;
    row.n_over_exposed = Some(enc.over_exposed.len() as i32);
    row.n_cross_module_edges = Some(enc.n_cross_module_refs as i32);

    // Martin package metrics, aggregated over real modules.
    row.mean_instability = mean_opt(a.modules.iter().map(|m| m.instability));
    row.median_instability = median_opt(a.modules.iter().map(|m| m.instability));
    row.mean_cohesion = mean_opt(a.modules.iter().map(|m| m.cohesion));
    row.mean_distance_main_sequence = mean_opt(a.modules.iter().map(|m| m.distance_main_sequence));

    row.anomaly = anomalies(row);
}

/// Mean of the present values, or `None` when there are none.
fn mean_opt(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let xs: Vec<f64> = values.flatten().collect();
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

/// Median of the present values, or `None` when there are none.
fn median_opt(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let mut xs: Vec<f64> = values.flatten().collect();
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(xs[xs.len() / 2])
}

/// Flags non-finite or out-of-`[0,1]` metric terms (the calibration bug hunt).
fn anomalies(row: &Analysis) -> Option<String> {
    let terms: [(&str, Option<f64>); 6] = [
        ("headline", row.headline),
        ("cohesion_term", row.cohesion_term),
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
        } else if !(-1e-9..=1.0 + 1e-9).contains(&v) {
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
        cohesion_term: None,
        acyclicity_term: None,
        encapsulation_term: None,
        is_acyclic: None,
        over_exposed_fraction: None,
        mean_leak_cost: None,
        n_real_items: None,
        n_module_nodes: None,
        n_sccs: None,
        largest_scc: None,
        modules_in_cycles: None,
        cyclomatic_number: None,
        n_over_exposed: None,
        n_cross_module_edges: None,
        mean_instability: None,
        median_instability: None,
        mean_cohesion: None,
        mean_distance_main_sequence: None,
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
            cohesion_term: None,
            acyclicity_term: None,
            encapsulation_term: None,
            is_acyclic: None,
            over_exposed_fraction: None,
            mean_leak_cost: None,
            n_real_items: None,
            n_module_nodes: None,
            n_sccs: None,
            largest_scc: None,
            modules_in_cycles: None,
            cyclomatic_number: None,
            n_over_exposed: None,
            n_cross_module_edges: None,
            mean_instability: None,
            median_instability: None,
            mean_cohesion: None,
            mean_distance_main_sequence: None,
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
        r.cohesion_term = Some(0.0);
        assert_eq!(anomalies(&r), None);
    }

    #[test]
    fn out_of_range_and_nonfinite_are_flagged() {
        let mut r = row();
        r.cohesion_term = Some(1.5);
        assert_eq!(anomalies(&r).as_deref(), Some("cohesion_term=1.500_oob"));

        let mut r = row();
        r.acyclicity_term = Some(f64::NAN);
        assert_eq!(anomalies(&r).as_deref(), Some("acyclicity_term=nonfinite"));
    }

    #[test]
    fn mean_leak_cost_is_range_checked_as_a_rate() {
        // mean_leak_cost is now the leak rate (1 - MII), a fraction in [0,1], so
        // it is range-checked like every other term.
        let mut r = row();
        r.mean_leak_cost = Some(2.0);
        assert_eq!(anomalies(&r).as_deref(), Some("mean_leak_cost=2.000_oob"));
        r.mean_leak_cost = Some(f64::INFINITY);
        assert_eq!(anomalies(&r).as_deref(), Some("mean_leak_cost=nonfinite"));
        // A legitimate rate is clean.
        r.mean_leak_cost = Some(0.5);
        assert_eq!(anomalies(&r), None);
    }

    #[test]
    fn mean_and_median_skip_none_and_handle_empty() {
        let near = |got: Option<f64>, want: f64| matches!(got, Some(x) if (x - want).abs() < 1e-12);
        assert!(near(
            mean_opt([Some(0.2), None, Some(0.4)].into_iter()),
            0.3
        ));
        assert_eq!(mean_opt([None, None].into_iter()), None);
        assert_eq!(mean_opt(std::iter::empty::<Option<f64>>()), None);
        // Sorted [0.2, 0.4, 0.8] -> middle element is 0.4.
        assert_eq!(
            median_opt([Some(0.4), Some(0.2), Some(0.8)].into_iter()),
            Some(0.4)
        );
        // Even count [0.2, 0.8] -> upper-middle (index len/2 = 1) is 0.8.
        assert_eq!(median_opt([Some(0.8), Some(0.2)].into_iter()), Some(0.8));
        assert_eq!(median_opt(std::iter::empty::<Option<f64>>()), None);
    }

    /// A two-module crate whose modules mutually depend, so the analysis has a
    /// real cycle, real cross-module leaks, and defined instability.
    fn cyclic_ir() -> modula_ir::CrateGraph {
        use modula_ir::{
            Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
            RefKind, SCHEMA_VERSION, Visibility,
        };
        let m = |id: u32, parent: Option<u32>, depth: u32| Module {
            id: ModuleId(id),
            crate_id: CrateId(0),
            parent: parent.map(ModuleId),
            name: format!("m{id}"),
            canonical_path: format!("c::m{id}"),
            depth,
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        };
        let it = |id: u32, owner: u32| Item {
            id: ItemId(id),
            canonical_path: format!("c::i{id}"),
            kind: ItemKind::Struct,
            visibility: Visibility::Public,
            owning_module: ModuleId(owner),
            crate_id: CrateId(0),
            has_canonical_path: true,
            reachable_pub_api: false,
        };
        let body = |from: u32, to: u32| Edge {
            from: ItemId(from),
            to: ItemId(to),
            kind: RefKind::Body,
            weight: 1,
        };
        CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: CrateId(0),
            crates: vec![Crate {
                id: CrateId(0),
                name: "c".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules: vec![m(0, None, 0), m(1, Some(0), 1), m(2, Some(0), 1)],
            items: vec![it(0, 1), it(1, 2)],
            edges: vec![body(0, 1), body(1, 0)],
        }
    }

    #[test]
    fn fill_mirrors_the_analysis_result() {
        use modula_metrics::analysis::{AnalysisConfig, analyze};
        let ir = cyclic_ir();
        let a = analyze(&ir, &AnalysisConfig::default()).expect("analysis");
        // The fixture must genuinely be cyclic, or the cycle-severity mapping is
        // exercised only on empty data.
        assert!(!a.tangles.is_acyclic);
        assert!(a.tangles.largest_scc >= 2);

        let mut r = row();
        fill(&mut r, &a);

        assert_eq!(r.status, "ok");
        assert_eq!(r.is_acyclic, Some(0));
        assert_eq!(r.n_sccs, Some(a.tangles.sccs.len() as i32));
        assert_eq!(r.largest_scc, Some(a.tangles.largest_scc as i32));
        assert_eq!(
            r.modules_in_cycles,
            Some(a.tangles.sccs.iter().map(Vec::len).sum::<usize>() as i32)
        );
        assert_eq!(
            r.n_cross_module_edges,
            Some(a.encapsulation.n_cross_module_refs as i32)
        );
        assert_eq!(
            r.mean_instability,
            mean_opt(a.modules.iter().map(|m| m.instability))
        );
        // Mutually-dependent modules each have Ca = Ce = 1, so instability is
        // defined and non-trivial.
        assert_eq!(r.mean_instability, Some(0.5));
    }
}
