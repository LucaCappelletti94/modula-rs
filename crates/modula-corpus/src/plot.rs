//! Renders the metric distributions from the `analyses` table to a single SVG
//! grid of histograms (SVG so there is no font-rasterization / system dep).

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use diesel::prelude::*;
use plotters::prelude::*;

use crate::db;
use crate::models::Analysis;
use crate::schema::analyses;

/// Options for the `plot` phase.
pub struct PlotArgs {
    pub root: PathBuf,
    pub db_path: String,
    pub out: PathBuf,
}

type Field = fn(&Analysis) -> Option<f64>;

const METRICS: [(&str, Field); 6] = [
    ("headline", |a| a.headline),
    ("cohesion_term", |a| a.cohesion_term),
    ("acyclicity_term", |a| a.acyclicity_term),
    ("encapsulation_term", |a| a.encapsulation_term),
    ("over_exposed_fraction", |a| a.over_exposed_fraction),
    ("mean_leak_cost", |a| a.mean_leak_cost),
];

/// Loads the `ok` analyses and writes the histogram grid SVG.
pub fn run(args: &PlotArgs) -> Result<()> {
    let db_file = crate::extract::db_file(&args.root, &args.db_path);
    let mut conn = db::open(&db_file)?;
    let rows: Vec<Analysis> = analyses::table
        .filter(analyses::status.eq("ok"))
        .select(Analysis::as_select())
        .load(&mut conn)
        .context("loading analyses")?;
    let n_a = rows.iter().filter(|r| r.headline.is_none()).count();
    println!(
        "plotting {} crates ({} N/A headline) -> {}",
        rows.len(),
        n_a,
        args.out.display()
    );

    draw(&rows, &args.out).map_err(|e| anyhow::anyhow!("rendering SVG: {e}"))?;
    Ok(())
}

/// Draws the 2x4 grid (last cell unused) to `out`.
fn draw(rows: &[Analysis], out: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = SVGBackend::new(out, (1600, 800)).into_drawing_area();
    root.fill(&WHITE)?;
    let panels = root.split_evenly((2, 4));
    for (panel, (title, field)) in panels.iter().zip(METRICS) {
        let values: Vec<f64> = rows
            .iter()
            .filter_map(field)
            .filter(|v| v.is_finite())
            .collect();
        draw_hist(panel, title, &values)?;
    }
    root.present()?;
    Ok(())
}

/// Draws one histogram of `values` over `[0, 1]` into `area`.
fn draw_hist(
    area: &DrawingArea<SVGBackend, plotters::coord::Shift>,
    title: &str,
    values: &[f64],
) -> Result<(), Box<dyn std::error::Error>> {
    const BINS: usize = 50;
    let mut counts = [0u32; BINS];
    for &v in values {
        let b = ((v.clamp(0.0, 1.0) * BINS as f64) as usize).min(BINS - 1);
        counts[b] += 1;
    }
    let y_max = counts.iter().copied().max().unwrap_or(1).max(1);

    let mut median = f64::NAN;
    if !values.is_empty() {
        let mut sorted: Vec<f64> = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        median = sorted[sorted.len() / 2];
    }

    let mut chart = ChartBuilder::on(area)
        .caption(
            format!("{title}  (n={}, med={median:.3})", values.len()),
            ("sans-serif", 16),
        )
        .margin(10)
        .x_label_area_size(26)
        .y_label_area_size(44)
        .build_cartesian_2d(0f64..1f64, 0u32..y_max)?;
    chart.configure_mesh().light_line_style(WHITE).draw()?;
    chart.draw_series(counts.iter().enumerate().map(|(i, &c)| {
        let x0 = i as f64 / BINS as f64;
        let x1 = (i + 1) as f64 / BINS as f64;
        Rectangle::new([(x0, 0u32), (x1, c)], BLUE.mix(0.7).filled())
    }))?;
    Ok(())
}
