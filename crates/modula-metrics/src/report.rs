//! Rendering of an [`AnalysisResult`]: machine-readable JSON, a plain-text human
//! report, and CI gate evaluation.

use std::collections::HashMap;
use std::fmt::Write as _;

use modula_ir::ModuleId;
use serde::Serialize;

use crate::analysis::AnalysisResult;

/// Serializes the analysis result to pretty JSON.
///
/// # Errors
/// Returns a [`serde_json::Error`] if serialization fails (it should not).
pub fn to_json(result: &AnalysisResult) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(result)
}

/// CI gate thresholds. A `None` field is not checked.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Gates {
    /// Minimum acceptable headline score.
    pub min_headline: Option<f64>,
    /// Require the module graph to be acyclic.
    pub require_acyclic: bool,
    /// Maximum acceptable over-exposed fraction.
    pub max_overexposed_fraction: Option<f64>,
}

/// The result of one gate.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GateResult {
    /// Gate name.
    pub name: String,
    /// Whether the gate passed.
    pub passed: bool,
    /// Human-readable detail.
    pub detail: String,
}

/// The combined outcome of all gates.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GateOutcome {
    /// Whether every gate passed.
    pub passed: bool,
    /// Per-gate results.
    pub results: Vec<GateResult>,
}

/// Evaluates the CI gates against an analysis result.
#[must_use]
pub fn evaluate_gates(result: &AnalysisResult, gates: &Gates) -> GateOutcome {
    let mut results = Vec::new();
    if let Some(min) = gates.min_headline {
        let value = result.composite.headline;
        results.push(GateResult {
            name: "min_headline".to_owned(),
            passed: value >= min,
            detail: format!("headline {value:.3} >= {min:.3}"),
        });
    }
    if gates.require_acyclic {
        let acyclic = result.tangles.is_acyclic;
        results.push(GateResult {
            name: "require_acyclic".to_owned(),
            passed: acyclic,
            detail: format!("{} non-trivial tangle(s)", result.tangles.sccs.len()),
        });
    }
    if let Some(max) = gates.max_overexposed_fraction {
        let value = result.encapsulation.over_exposed_fraction;
        results.push(GateResult {
            name: "max_overexposed_fraction".to_owned(),
            passed: value <= max,
            detail: format!("over-exposed {value:.3} <= {max:.3}"),
        });
    }
    GateOutcome {
        passed: results.iter().all(|r| r.passed),
        results,
    }
}

/// Renders a plain-text human report (ASCII only).
#[must_use]
pub fn to_human(result: &AnalysisResult) -> String {
    let mut s = String::new();
    let module_path: HashMap<ModuleId, &str> = result
        .modules
        .iter()
        .map(|m| (m.module, m.path.as_str()))
        .collect();

    let _ = writeln!(s, "Modularity report for crate `{}`", result.crate_name);
    let _ = writeln!(
        s,
        "  {} items, {} modules ({} in the dependency graph)",
        result.n_items, result.n_modules, result.n_module_nodes
    );
    let _ = writeln!(s);

    let c = &result.composite;
    let _ = writeln!(s, "Headline score: {:.3} / 1.000", c.headline);
    let _ = writeln!(s, "  depth-averaged : {:.3}", c.headline_depth_averaged);
    let _ = writeln!(s, "  modularity     : {}", opt(c.modularity_term));
    let _ = writeln!(s, "  divergence     : {:.3}", c.divergence_term);
    let _ = writeln!(s, "  acyclicity     : {:.3}", c.acyclicity_term);
    let _ = writeln!(s, "  encapsulation  : {:.3}", c.encapsulation_term);
    let _ = writeln!(s);

    // Leakiest modules: lowest cohesion first.
    let mut leakiest: Vec<_> = result
        .modules
        .iter()
        .filter(|m| m.cohesion.is_some())
        .collect();
    leakiest.sort_by(|a, b| {
        a.cohesion
            .partial_cmp(&b.cohesion)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    if !leakiest.is_empty() {
        let _ = writeln!(s, "Leakiest modules (cohesion | Ca | Ce | instability):");
        for m in leakiest.iter().take(5) {
            let _ = writeln!(
                s,
                "  {:<28} {:>5} | {:>2} | {:>2} | {}",
                m.path,
                fmt_opt(m.cohesion),
                m.ca,
                m.ce,
                opt(m.instability)
            );
        }
        let _ = writeln!(s);
    }

    // Tangles.
    if result.tangles.is_acyclic {
        let _ = writeln!(s, "Cycles: none (module graph is acyclic)");
    } else {
        let _ = writeln!(
            s,
            "Cycles: {} tangle(s), largest {} modules",
            result.tangles.sccs.len(),
            result.tangles.largest_scc
        );
        for scc in result.tangles.sccs.iter().take(5) {
            let names: Vec<&str> = scc
                .iter()
                .map(|m| *module_path.get(m).unwrap_or(&"?"))
                .collect();
            let _ = writeln!(s, "  {}", names.join(" <-> "));
        }
    }
    let _ = writeln!(s);

    // Over-exposed items.
    if result.encapsulation.over_exposed.is_empty() {
        let _ = writeln!(s, "Over-exposed items: none");
    } else {
        let _ = writeln!(
            s,
            "Over-exposed items ({}):",
            result.encapsulation.over_exposed.len()
        );
        for e in result.encapsulation.over_exposed.iter().take(5) {
            let _ = writeln!(s, "  {:<32} {:?} -> {:?}", e.path, e.declared, e.required);
        }
    }
    let _ = writeln!(s);

    // Deepest leaks.
    if result.encapsulation.deepest_leaks.is_empty() {
        let _ = writeln!(s, "Deepest leaks: none");
    } else {
        let _ = writeln!(s, "Deepest leaks (cost):");
        for l in result.encapsulation.deepest_leaks.iter().take(5) {
            let _ = writeln!(s, "  {:.3}  {} -> {}", l.leak_cost, l.from_path, l.to_path);
        }
    }
    let _ = writeln!(s);

    // Modularity profile.
    let _ = writeln!(
        s,
        "Modularity profile (depth: declared/detected Q, efficiency):"
    );
    for r in &result.modularity_profile {
        let _ = writeln!(
            s,
            "  d{} c{:<3} U {:>7.3}/{:>7.3} e {}  D {:>7.3}/{:>7.3} e {}",
            r.depth,
            r.communities_declared,
            r.q_declared_undirected,
            r.q_detected_undirected,
            opt(r.efficiency_undirected),
            r.q_declared_directed,
            r.q_detected_directed,
            opt(r.efficiency_directed),
        );
    }
    let _ = writeln!(s);

    // Divergence profile.
    let _ = writeln!(s, "Divergence profile (depth: VI, AMI, ARI):");
    for r in &result.divergence_profile {
        let _ = writeln!(
            s,
            "  d{} VI {:>7.3} AMI {:>7.3} ARI {:>7.3}",
            r.depth, r.vi, r.ami, r.ari
        );
    }

    s
}

/// Formats an optional ratio as a fixed-width value or `n/a`.
fn opt(value: Option<f64>) -> String {
    value.map_or_else(|| "  n/a".to_owned(), |v| format!("{v:.3}"))
}

/// Formats an optional cohesion value (right-aligned).
fn fmt_opt(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.3}"))
}
