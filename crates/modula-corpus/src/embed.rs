//! The `embed` phase: project each crate's metric feature vector to 2D with PCA
//! and t-SNE, scatter-plotted and colored by crates.io category. This is the
//! "different perspective" view: do crates of a given category occupy a
//! characteristic region of modularity-feature space?

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use diesel::prelude::*;
use nalgebra::DMatrix;
use plotters::prelude::*;
use plotters::style::RGBColor;

use crate::db;
use crate::models::{Analysis, Extraction};
use crate::schema::{analyses, extractions};

/// Options for the `embed` phase.
pub struct EmbedArgs {
    pub root: PathBuf,
    pub db_path: String,
    /// Output path prefix; the view suffixes are appended.
    pub out: PathBuf,
    pub perplexity: f64,
    /// Cap on points (0 = all); larger sets make t-SNE slower.
    pub max_points: usize,
}

/// The z-scored features, in order. Used for PCA loading readouts.
const FEATURES: [&str; 18] = [
    "modularity",
    "divergence",
    "acyclicity",
    "encapsulation",
    "over_exposed",
    "mean_leak_cost",
    "ln(cyclomatic)",
    "instability",
    "dist_main_seq",
    "cohesion",
    "ln(n_items)",
    "ln(n_modules)",
    "body_frac",
    "sig_frac",
    "trait_bound_frac",
    "impl_frac",
    "import_frac",
    "trait_frac",
];

/// A tab10-style palette for the top categories; everything else is grey.
const PALETTE: [RGBColor; 10] = [
    RGBColor(31, 119, 180),
    RGBColor(255, 127, 14),
    RGBColor(44, 160, 44),
    RGBColor(214, 39, 40),
    RGBColor(148, 103, 189),
    RGBColor(140, 86, 75),
    RGBColor(227, 119, 194),
    RGBColor(188, 189, 34),
    RGBColor(23, 190, 207),
    RGBColor(127, 127, 127),
];
const OTHER: RGBColor = RGBColor(200, 200, 200);

/// Runs PCA + t-SNE over the corpus and writes the two scatter SVGs.
pub fn run(args: &EmbedArgs) -> Result<()> {
    let db_file = crate::extract::db_file(&args.root, &args.db_path);
    let mut conn = db::open(&db_file)?;

    // Join the metric (analyses) and structural (extractions) rows in memory.
    let exts: HashMap<(String, String), Extraction> = extractions::table
        .select(Extraction::as_select())
        .load::<Extraction>(&mut conn)
        .context("loading extractions")?
        .into_iter()
        .map(|e| ((e.name.clone(), e.version.clone()), e))
        .collect();
    let rows: Vec<Analysis> = analyses::table
        .filter(analyses::status.eq("ok"))
        .select(Analysis::as_select())
        .load::<Analysis>(&mut conn)
        .context("loading analyses")?;

    // Keep crates with measurable structure (a defined headline) that also have
    // their structural row.
    let mut raw: Vec<Vec<f64>> = Vec::new();
    let mut cat: Vec<Option<String>> = Vec::new();
    let mut kw: Vec<Option<String>> = Vec::new();
    for a in &rows {
        if a.headline.is_none() {
            continue;
        }
        let Some(e) = exts.get(&(a.name.clone(), a.version.clone())) else {
            continue;
        };
        raw.push(features(a, e));
        cat.push(label_of(e, "category"));
        kw.push(label_of(e, "keyword"));
    }
    if raw.len() < 3 {
        anyhow::bail!("only {} usable crates; nothing to embed", raw.len());
    }

    // Optional subsample (deterministic stride) to keep t-SNE responsive.
    if args.max_points > 0 && raw.len() > args.max_points {
        let step = raw.len().div_ceil(args.max_points);
        let pick = |v: &[Vec<f64>]| v.iter().step_by(step).cloned().collect();
        let pick_l = |v: &[Option<String>]| v.iter().step_by(step).cloned().collect();
        raw = pick(&raw);
        cat = pick_l(&cat);
        kw = pick_l(&kw);
    }
    let n = raw.len();
    println!("embedding {n} crates over {} features", FEATURES.len());
    standardize(&mut raw);

    // Compute both projections once; every view below reuses them.
    let pca = pca_2d(&raw);
    println!(
        "PCA variance explained: PC1 {:.1}%, PC2 {:.1}%",
        pca.var.0 * 100.0,
        pca.var.1 * 100.0
    );
    println!("  PC1 top loadings: {}", pca.loadings.0);
    println!("  PC2 top loadings: {}", pca.loadings.1);
    println!("running t-SNE on {n} points (this takes a few minutes) ...");
    let tsne = tsne_2d(&raw, args.perplexity);

    // Categorical scatters (PCA + t-SNE) colored by category, and t-SNE by keyword.
    let (cat_groups, cat_of) = color_groups(&cat);
    let (kw_groups, kw_of) = color_groups(&kw);
    let write = |path: PathBuf,
                 title,
                 xl,
                 yl,
                 pts: &[(f64, f64)],
                 g: &[usize],
                 groups: &[(String, RGBColor)]|
     -> Result<()> {
        scatter(&path, title, xl, yl, pts, g, groups)
            .map_err(|e| anyhow::anyhow!("rendering {}: {e}", path.display()))?;
        println!("wrote {}", path.display());
        Ok(())
    };
    write(
        with_suffix(&args.out, "-pca.svg"),
        "PCA (color: category)",
        "PC1",
        "PC2",
        &pca.coords,
        &cat_of,
        &cat_groups,
    )?;
    write(
        with_suffix(&args.out, "-tsne.svg"),
        "t-SNE (color: category)",
        "t-SNE 1",
        "t-SNE 2",
        &tsne,
        &cat_of,
        &cat_groups,
    )?;
    write(
        with_suffix(&args.out, "-tsne-keywords.svg"),
        "t-SNE (color: keyword)",
        "t-SNE 1",
        "t-SNE 2",
        &tsne,
        &kw_of,
        &kw_groups,
    )?;

    // Per-feature heatmaps over the t-SNE layout: where each input feature is
    // high/low, which shows what actually drives the embedding.
    let feat_path = with_suffix(&args.out, "-features.svg");
    feature_heatmaps(&feat_path, &tsne, &raw)
        .map_err(|e| anyhow::anyhow!("rendering heatmaps: {e}"))?;
    println!("wrote {}", feat_path.display());
    Ok(())
}

/// The raw (un-standardized) feature vector for one crate; missing values are
/// `NaN` and imputed to the column mean during standardization.
fn features(a: &Analysis, e: &Extraction) -> Vec<f64> {
    let nan = f64::NAN;
    let ln1p = |x: Option<i32>| x.map_or(nan, |v| f64::from(v.max(0)).ln_1p());
    let edges = e.n_edges.unwrap_or(0).max(0) as f64;
    let frac = |x: Option<i32>| {
        if edges > 0.0 {
            f64::from(x.unwrap_or(0)) / edges
        } else {
            nan
        }
    };
    let types = (e.n_structs.unwrap_or(0)
        + e.n_enums.unwrap_or(0)
        + e.n_traits.unwrap_or(0)
        + e.n_type_aliases.unwrap_or(0)) as f64;
    let trait_frac = if types > 0.0 {
        f64::from(e.n_traits.unwrap_or(0)) / types
    } else {
        nan
    };
    vec![
        a.modularity_term.unwrap_or(nan),
        a.divergence_term.unwrap_or(nan),
        a.acyclicity_term.unwrap_or(nan),
        a.encapsulation_term.unwrap_or(nan),
        a.over_exposed_fraction.unwrap_or(nan),
        a.mean_leak_cost.unwrap_or(nan),
        ln1p(a.cyclomatic_number),
        a.mean_instability.unwrap_or(nan),
        a.mean_distance_main_sequence.unwrap_or(nan),
        a.mean_cohesion.unwrap_or(nan),
        ln1p(a.n_real_items),
        ln1p(a.n_module_nodes),
        frac(e.n_body_edges),
        frac(e.n_signature_edges),
        frac(e.n_trait_bound_edges),
        frac(e.n_impl_edges),
        frac(e.n_import_edges),
        trait_frac,
    ]
}

/// The color label of a crate: the top-level of its first category (or first
/// keyword), e.g. `development-tools::macros` -> `development-tools`.
fn label_of(e: &Extraction, color_by: &str) -> Option<String> {
    let field = if color_by == "keyword" {
        &e.keywords
    } else {
        &e.categories
    };
    field
        .as_deref()
        .and_then(|s| s.split(',').next())
        .map(|first| first.split("::").next().unwrap_or(first).to_owned())
        .filter(|s| !s.is_empty())
}

/// Robustly scales each column in place: winsorize to the 1st-99th percentile,
/// impute missing to the median, then center on the median and scale by the IQR.
/// Median/IQR (rather than mean/std) keeps the heavy tails here (e.g. cyclomatic
/// up to 12k) from stretching an axis and compressing everything else. A column
/// with no inter-quartile spread contributes nothing (scaled to 0).
fn standardize(data: &mut [Vec<f64>]) {
    let d = data[0].len();
    for j in 0..d {
        let mut col: Vec<f64> = data
            .iter()
            .filter_map(|r| r[j].is_finite().then_some(r[j]))
            .collect();
        if col.is_empty() {
            for row in data.iter_mut() {
                row[j] = 0.0;
            }
            continue;
        }
        col.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = percentile(&col, 0.5);
        let iqr = percentile(&col, 0.75) - percentile(&col, 0.25);
        let (wlo, whi) = (percentile(&col, 0.01), percentile(&col, 0.99));
        let scale = if iqr > 1e-12 { 1.0 / iqr } else { 0.0 };
        for row in data.iter_mut() {
            let v = if row[j].is_finite() { row[j] } else { median };
            row[j] = (v.clamp(wlo, whi) - median) * scale;
        }
    }
}

/// A 2D PCA projection plus interpretive context.
struct Pca {
    /// 2D coordinates per point.
    coords: Vec<(f64, f64)>,
    /// Variance fraction explained by PC1 and PC2.
    var: (f64, f64),
    /// Largest-magnitude feature loadings of PC1 and PC2 (textual readout).
    loadings: (String, String),
}

/// PCA via the eigendecomposition of the (standardized) covariance matrix.
fn pca_2d(data: &[Vec<f64>]) -> Pca {
    let n = data.len();
    let d = data[0].len();
    let x = DMatrix::from_fn(n, d, |i, j| data[i][j]);
    let cov = (x.transpose() * &x) / (n as f64 - 1.0);
    let eig = cov.symmetric_eigen();

    let mut idx: Vec<usize> = (0..d).collect();
    idx.sort_by(|&a, &b| eig.eigenvalues[b].partial_cmp(&eig.eigenvalues[a]).unwrap());
    let (i0, i1) = (idx[0], idx[1]);
    let total: f64 = eig
        .eigenvalues
        .iter()
        .map(|v| v.max(0.0))
        .sum::<f64>()
        .max(1e-12);

    let basis = DMatrix::from_fn(d, 2, |r, c| {
        eig.eigenvectors[(r, if c == 0 { i0 } else { i1 })]
    });
    let proj = &x * basis;
    let coords = (0..n).map(|i| (proj[(i, 0)], proj[(i, 1)])).collect();

    let var = (eig.eigenvalues[i0] / total, eig.eigenvalues[i1] / total);
    let loadings = (
        top_loadings(&eig.eigenvectors.column(i0)),
        top_loadings(&eig.eigenvectors.column(i1)),
    );
    Pca {
        coords,
        var,
        loadings,
    }
}

/// The three features with the largest absolute loading on an eigenvector.
fn top_loadings(v: &nalgebra::DVectorView<f64>) -> String {
    let mut pairs: Vec<(usize, f64)> = (0..v.len()).map(|i| (i, v[i])).collect();
    pairs.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap());
    pairs
        .iter()
        .take(3)
        .map(|&(i, w)| {
            format!(
                "{}{:.2} {}",
                if w >= 0.0 { "+" } else { "-" },
                w.abs(),
                FEATURES[i]
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Barnes-Hut t-SNE to 2D over the standardized features (Euclidean metric).
fn tsne_2d(data: &[Vec<f64>], perplexity: f64) -> Vec<(f64, f64)> {
    let samples: Vec<Vec<f32>> = data
        .iter()
        .map(|r| r.iter().map(|&v| v as f32).collect())
        .collect();
    let mut tsne = bhtsne::tSNE::new(&samples);
    tsne.embedding_dim(2)
        .perplexity(perplexity as f32)
        .epochs(1000)
        .barnes_hut(0.5, |a, b| {
            a.iter()
                .zip(b)
                .map(|(x, y)| (x - y) * (x - y))
                .sum::<f32>()
                .sqrt()
        });
    let flat = tsne.embedding();
    (0..data.len())
        .map(|i| (f64::from(flat[2 * i]), f64::from(flat[2 * i + 1])))
        .collect()
}

/// Assigns each point a color group: the top-9 most common labels get a palette
/// color, everything else (and unlabeled) falls into "other".
fn color_groups(labels: &[Option<String>]) -> (Vec<(String, RGBColor)>, Vec<usize>) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for l in labels.iter().flatten() {
        *counts.entry(l.as_str()).or_insert(0) += 1;
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let top: Vec<String> = ranked
        .iter()
        .take(PALETTE.len() - 1)
        .map(|(s, _)| (*s).to_owned())
        .collect();

    let index: HashMap<&str, usize> = top
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let other = top.len();
    let group_of: Vec<usize> = labels
        .iter()
        .map(|l| {
            l.as_deref()
                .and_then(|s| index.get(s).copied())
                .unwrap_or(other)
        })
        .collect();

    let mut groups: Vec<(String, RGBColor)> = top
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), PALETTE[i]))
        .collect();
    groups.push(("other".to_owned(), OTHER));
    (groups, group_of)
}

/// Draws a colored 2D scatter with a legend.
fn scatter(
    path: &std::path::Path,
    title: &str,
    xlab: &str,
    ylab: &str,
    pts: &[(f64, f64)],
    group_of: &[usize],
    groups: &[(String, RGBColor)],
) -> std::result::Result<(), Box<dyn Error>> {
    let root = SVGBackend::new(path, (1200, 950)).into_drawing_area();
    root.fill(&WHITE)?;
    let pad = |lo: f64, hi: f64| {
        let m = (hi - lo).abs().max(1.0) * 0.05;
        (lo - m, hi + m)
    };
    let (xlo, xhi) = pad(
        pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
        pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
    );
    let (ylo, yhi) = pad(
        pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
        pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
    );
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 20))
        .margin(12)
        .x_label_area_size(36)
        .y_label_area_size(48)
        .build_cartesian_2d(xlo..xhi, ylo..yhi)?;
    chart
        .configure_mesh()
        .x_desc(xlab)
        .y_desc(ylab)
        .light_line_style(WHITE)
        .draw()?;

    // "other" first (underneath), then the colored categories on top.
    let order = std::iter::once(groups.len() - 1).chain(0..groups.len() - 1);
    for gi in order {
        let (name, color) = &groups[gi];
        let series: Vec<(f64, f64)> = (0..pts.len())
            .filter(|&i| group_of[i] == gi)
            .map(|i| pts[i])
            .collect();
        if series.is_empty() {
            continue;
        }
        let c = *color;
        let alpha = if gi == groups.len() - 1 { 0.25 } else { 0.55 };
        chart
            .draw_series(
                series
                    .iter()
                    .map(|&p| Circle::new(p, 2, c.mix(alpha).filled())),
            )?
            .label(format!("{name} ({})", series.len()))
            .legend(move |(x, y)| Circle::new((x, y), 4, c.filled()));
    }
    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.85))
        .border_style(BLACK)
        .position(SeriesLabelPosition::UpperRight)
        .draw()?;
    root.present()?;
    Ok(())
}

/// Renders one small heatmap panel per feature over the t-SNE layout, each point
/// colored by that (standardized) feature's value, so it is clear which input
/// features the embedding is laid out by. Points are strided to a cap to keep
/// the SVG a sane size.
fn feature_heatmaps(
    path: &std::path::Path,
    coords: &[(f64, f64)],
    feats: &[Vec<f64>],
) -> std::result::Result<(), Box<dyn Error>> {
    let step = coords.len().div_ceil(6000).max(1);
    let idx: Vec<usize> = (0..coords.len()).step_by(step).collect();
    let xs: Vec<f64> = idx.iter().map(|&i| coords[i].0).collect();
    let ys: Vec<f64> = idx.iter().map(|&i| coords[i].1).collect();
    let (xlo, xhi) = (
        xs.iter().cloned().fold(f64::INFINITY, f64::min),
        xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let (ylo, yhi) = (
        ys.iter().cloned().fold(f64::INFINITY, f64::min),
        ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );

    let cols = 5;
    let rows = FEATURES.len().div_ceil(cols);
    let root = SVGBackend::new(path, (1800, 360 * rows as u32)).into_drawing_area();
    root.fill(&WHITE)?;
    let panels = root.split_evenly((rows, cols));
    for (j, panel) in (0..FEATURES.len()).zip(panels.iter()) {
        // Robust color range: 2nd-98th percentile of this feature.
        let mut vals: Vec<f64> = idx.iter().map(|&i| feats[i][j]).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let (lo, hi) = (percentile(&vals, 0.02), percentile(&vals, 0.98));
        let span = if hi > lo { hi - lo } else { 1.0 };
        let mut chart = ChartBuilder::on(panel)
            .caption(FEATURES[j], ("sans-serif", 15))
            .margin(4)
            .build_cartesian_2d(xlo..xhi, ylo..yhi)?;
        chart
            .configure_mesh()
            .disable_mesh()
            .disable_axes()
            .draw()?;
        chart.draw_series(idx.iter().map(|&i| {
            let t = ((feats[i][j] - lo) / span).clamp(0.0, 1.0);
            Circle::new(coords[i], 1, viridis(t).filled())
        }))?;
    }
    root.present()?;
    Ok(())
}

/// The value at percentile `p` of an ascending-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Viridis colormap: maps `t in [0,1]` to a perceptually-uniform color.
fn viridis(t: f64) -> RGBColor {
    const STOPS: [(f64, (u8, u8, u8)); 5] = [
        (0.0, (68, 1, 84)),
        (0.25, (59, 82, 139)),
        (0.5, (33, 145, 140)),
        (0.75, (94, 201, 98)),
        (1.0, (253, 231, 37)),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            let lerp = |a: u8, b: u8| (f64::from(a) + f * (f64::from(b) - f64::from(a))) as u8;
            return RGBColor(lerp(c0.0, c1.0), lerp(c0.1, c1.1), lerp(c0.2, c1.2));
        }
    }
    let (_, c) = STOPS[STOPS.len() - 1];
    RGBColor(c.0, c.1, c.2)
}

/// Appends a suffix to a path's file name (`plots/embed` + `-pca.svg`).
fn with_suffix(prefix: &std::path::Path, suffix: &str) -> PathBuf {
    let mut s = prefix.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}
