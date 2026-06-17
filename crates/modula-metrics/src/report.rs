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
    let _ = writeln!(s, "Headline score: {:>5} / 1.000", opt(c.headline));
    let _ = writeln!(s, "  cohesion       : {:>5}", opt(c.cohesion_term));
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
                "  {:<28} {:>5} | {:>2} | {:>2} | {:>5}",
                m.path,
                opt(m.cohesion),
                m.ca,
                m.ce,
                opt(m.instability)
            );
        }
        let _ = writeln!(s);
    }

    // Tangle: feedback references that prevent the module graph from laying in
    // a clean dependency order. Listing them is the actionable "cut these" set.
    if result.tangles.is_acyclic {
        let _ = writeln!(s, "Tangle: none (module graph is a clean DAG)");
    } else {
        let _ = writeln!(
            s,
            "Tangle: {:.1}% of inter-module dependency weight is feedback ({} reference(s) to cut to layer it):",
            result.tangles.feedback_fraction * 100.0,
            result.tangles.feedback_edges.len(),
        );
        for (from, to) in result.tangles.feedback_edges.iter().take(5) {
            let from = module_path.get(from).unwrap_or(&"?");
            let to = module_path.get(to).unwrap_or(&"?");
            let _ = writeln!(s, "  {from} -> {to}");
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

    // Leaks: cross-module references that reach a module's non-API internals.
    if result.encapsulation.leaks.is_empty() {
        let _ = writeln!(s, "Leaks (cross-module references into internals): none");
    } else {
        let _ = writeln!(
            s,
            "Leaks (cross-module references into internals, {} of {} cross-module refs):",
            result.encapsulation.leaks.len(),
            result.encapsulation.n_cross_module_refs
        );
        for l in result.encapsulation.leaks.iter().take(5) {
            let _ = writeln!(
                s,
                "  {} -> {} ({:?})",
                l.from_path, l.to_path, l.target_visibility
            );
        }
    }
    let _ = writeln!(s);

    s
}

/// Formats an optional ratio as a 3-decimal value or `n/a`. Call sites apply
/// their own width spec (e.g. `{:>5}`) when column alignment matters.
fn opt(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.3}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coupling::ModuleCoupling;
    use crate::cycles::TangleReport;
    use crate::encapsulation::{EncapsulationReport, Leak, OverExposure};
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
            modules: Vec::new(),
            tangles: TangleReport {
                sccs: vec![vec![ModuleId(0)]; sccs],
                is_acyclic,
                largest_scc: usize::from(sccs != 0),
                cyclomatic_number: sccs,
                feedback_fraction: if is_acyclic { 0.0 } else { 0.5 },
                feedback_edges: if is_acyclic {
                    Vec::new()
                } else {
                    vec![(ModuleId(0), ModuleId(1))]
                },
            },
            encapsulation: EncapsulationReport {
                over_exposed: Vec::new(),
                over_exposed_fraction,
                leaks: Vec::new(),
                n_cross_module_refs: 0,
                mean_leak_cost: 0.0,
            },
            composite: CompositeScore {
                headline: Some(headline),
                cohesion_term: Some(0.5),
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
        assert!(text.contains("Tangle: none"));
        assert!(text.contains("Over-exposed items: none"));
        assert!(text.contains("Leaks (cross-module references into internals): none"));
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
            abstractness: Some(0.5),
            distance_main_sequence: Some(0.16),
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
        r.encapsulation.leaks = vec![Leak {
            from: ItemId(0),
            to: ItemId(1),
            from_path: "demo::a".to_owned(),
            to_path: "demo::b::Deep".to_owned(),
            target_visibility: Visibility::Crate,
        }];
        r.encapsulation.n_cross_module_refs = 3;
        let text = to_human(&r);
        assert!(text.contains("Leakiest modules"));
        assert!(text.contains("demo::a"));
        assert!(text.contains("reference(s) to cut to layer it"));
        assert!(text.contains("Over-exposed items (1)"));
        assert!(text.contains("Leaks (cross-module references into internals"));
    }

    #[test]
    fn json_serializes_with_headline() {
        let json = to_json(&bare(0.5, true, 0, 0.0)).expect("serialize");
        assert!(json.contains("\"headline\""));
    }
}
