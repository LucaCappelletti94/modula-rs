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
        // An N/A headline (no measurable structure) cannot fail the gate: there
        // is nothing to score, so the gate passes vacuously.
        let value = result.composite.headline;
        results.push(GateResult {
            name: "min_headline".to_owned(),
            passed: value.is_none_or(|v| v >= min),
            detail: value.map_or_else(
                || "headline n/a (insufficient structure)".to_owned(),
                |v| format!("headline {v:.3} >= {min:.3}"),
            ),
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
    let _ = writeln!(s, "Headline score: {} / 1.000", opt(c.headline));
    let _ = writeln!(s, "  depth-averaged : {}", opt(c.headline_depth_averaged));
    let _ = writeln!(s, "  modularity     : {}", opt(c.modularity_term));
    let _ = writeln!(s, "  divergence     : {}", opt(c.divergence_term));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coupling::ModuleCoupling;
    use crate::cycles::TangleReport;
    use crate::encapsulation::{EncapsulationReport, Leak, OverExposure};
    use crate::modularity::{DepthRecord, DivergenceRecord};
    use crate::score::{CompositeScore, CompositeWeights};
    use modula_ir::{ItemId, ModuleId, Visibility};

    /// A minimal result with the gate-relevant fields set and everything else
    /// empty, so gate and render branches can be driven deterministically.
    fn bare(
        headline: f64,
        is_acyclic: bool,
        sccs: usize,
        over_exposed_fraction: f64,
    ) -> AnalysisResult {
        AnalysisResult {
            crate_name: "demo".to_owned(),
            n_items: 1,
            n_real_items: 1,
            n_modules: 1,
            n_module_nodes: 1,
            modularity_profile: Vec::new(),
            divergence_profile: Vec::new(),
            modules: Vec::new(),
            tangles: TangleReport {
                sccs: vec![vec![ModuleId(0)]; sccs],
                circuits: Vec::new(),
                is_acyclic,
                largest_scc: usize::from(sccs != 0),
                circuits_truncated: false,
            },
            encapsulation: EncapsulationReport {
                over_exposed: Vec::new(),
                over_exposed_fraction,
                deepest_leaks: Vec::new(),
                mean_leak_cost: 0.0,
            },
            composite: CompositeScore {
                headline: Some(headline),
                headline_depth_averaged: Some(headline),
                modularity_term: None,
                divergence_term: Some(0.0),
                acyclicity_term: 0.0,
                encapsulation_term: 0.0,
                weights: CompositeWeights::default(),
            },
        }
    }

    #[test]
    fn all_gates_pass() {
        let r = bare(0.8, true, 0, 0.1);
        let gates = Gates {
            min_headline: Some(0.5),
            require_acyclic: true,
            max_overexposed_fraction: Some(0.2),
        };
        let outcome = evaluate_gates(&r, &gates);
        assert!(outcome.passed);
        assert_eq!(outcome.results.len(), 3);
        assert!(outcome.results.iter().all(|g| g.passed));
    }

    #[test]
    fn each_gate_can_fail_independently() {
        let r = bare(0.3, false, 2, 0.9);
        let headline = evaluate_gates(
            &r,
            &Gates {
                min_headline: Some(0.5),
                ..Default::default()
            },
        );
        assert!(!headline.passed && !headline.results[0].passed);

        let acyclic = evaluate_gates(
            &r,
            &Gates {
                require_acyclic: true,
                ..Default::default()
            },
        );
        assert!(!acyclic.passed);
        assert!(acyclic.results[0].detail.contains("tangle"));

        let exposed = evaluate_gates(
            &r,
            &Gates {
                max_overexposed_fraction: Some(0.2),
                ..Default::default()
            },
        );
        assert!(!exposed.passed);
    }

    #[test]
    fn no_gates_passes_vacuously() {
        let outcome = evaluate_gates(&bare(0.0, false, 1, 1.0), &Gates::default());
        assert!(outcome.passed);
        assert!(outcome.results.is_empty());
    }

    #[test]
    fn human_report_clean_graph_says_acyclic_and_none() {
        let text = to_human(&bare(0.7, true, 0, 0.0));
        assert!(text.contains("Modularity report for crate `demo`"));
        assert!(text.contains("Cycles: none"));
        assert!(text.contains("Over-exposed items: none"));
        assert!(text.contains("Deepest leaks: none"));
    }

    #[test]
    fn human_report_renders_cycles_leaks_and_profiles() {
        let mut r = bare(0.4, false, 1, 0.5);
        r.modules = vec![ModuleCoupling {
            module: ModuleId(0),
            path: "demo::a".to_owned(),
            intra_weight: 1.0,
            inter_weight_out: 2.0,
            inter_weight_in: 0.0,
            cohesion: Some(0.25),
            ca: 1,
            ce: 2,
            instability: Some(0.66),
        }];
        r.tangles.sccs = vec![vec![ModuleId(0), ModuleId(1)]];
        r.tangles.largest_scc = 2;
        r.encapsulation.over_exposed = vec![OverExposure {
            item: ItemId(0),
            path: "demo::a::Thing".to_owned(),
            declared: Visibility::Public,
            required: Visibility::Crate,
            reachable_pub_api: false,
        }];
        r.encapsulation.deepest_leaks = vec![Leak {
            from: ItemId(0),
            to: ItemId(1),
            from_path: "demo::a".to_owned(),
            to_path: "demo::b::Deep".to_owned(),
            lin: 0.1,
            leak_cost: 0.9,
        }];
        r.modularity_profile = vec![DepthRecord {
            depth: 1,
            communities_declared: 2,
            q_declared_undirected: 0.1,
            q_declared_directed: 0.2,
            q_detected_undirected: 0.3,
            q_detected_directed: 0.4,
            efficiency_undirected: Some(0.5),
            efficiency_directed: None,
        }];
        r.divergence_profile = vec![DivergenceRecord {
            depth: 1,
            vi: 1.0,
            vi_normalized: 0.5,
            nmi: 0.5,
            ami: 0.4,
            ari: 0.3,
            h_declared_given_detected: 0.2,
            h_detected_given_declared: 0.1,
        }];
        let text = to_human(&r);
        assert!(text.contains("Leakiest modules"));
        assert!(text.contains("demo::a"));
        assert!(text.contains("tangle(s)"));
        assert!(text.contains("Over-exposed items (1)"));
        assert!(text.contains("Deepest leaks (cost)"));
        // `efficiency_directed: None` exercises the `n/a` branch of `opt`.
        assert!(text.contains("n/a"));
    }

    #[test]
    fn json_serializes_with_headline() {
        let json = to_json(&bare(0.5, true, 0, 0.0)).expect("serialize");
        assert!(json.contains("\"headline\""));
    }
}
