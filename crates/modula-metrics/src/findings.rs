//! Actionable findings derived from an analysis: the report turned into a flat
//! list of "here is what to change" items, worst first.
//!
//! The action-generation lives here, not in any front end, so the web report
//! viewer and a future `cargo modula --lint` emit the same set: each finding is
//! a stable `rule` id, a severity, a location, what is wrong, the concrete change
//! to make, and the specific items it covers. Leaks and over-exposure are
//! aggregated per module (and per module pair) rather than per reference, because
//! "module A reaches into module B's internals in N places" is the actionable
//! unit, not each individual call.

use std::collections::BTreeMap;

use modula_ir::{CrateGraph, ItemId, ModuleId, Visibility};
use serde::Serialize;

use crate::analysis::AnalysisResult;

/// How much a finding ought to weigh on attention, worst first. The ordering is
/// the enum declaration order (`High < Medium < Low`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    /// A genuine architectural defect: a module dependency cycle.
    High,
    /// An interface boundary worth revisiting: references into another module's
    /// internals.
    Medium,
    /// A mechanical tidy-up: visibility wider than any consumer requires.
    Low,
}

/// One actionable item.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    /// Stable lint identifier, for filtering and a future linter.
    pub rule: &'static str,
    /// Attention level.
    pub severity: Severity,
    /// Canonical path the finding applies to.
    pub location: String,
    /// What is wrong. Paths are wrapped in backticks for rich rendering.
    pub message: String,
    /// The concrete change to make.
    pub suggestion: String,
    /// The specific items the finding aggregates (each backtick-wrapped), for an
    /// expandable breakdown. Empty when the finding is already atomic.
    pub details: Vec<String>,
}

/// Derives the actionable findings from a completed analysis, ordered worst
/// severity first and then by location.
#[must_use]
pub fn findings(result: &AnalysisResult, ir: &CrateGraph) -> Vec<Finding> {
    let mut out = Vec::new();
    let item_module = |id: ItemId| -> String {
        let module = ir.real_module(ir.item(id).owning_module);
        ir.module(module).canonical_path.clone()
    };

    // Module cycles (High): one per feedback edge that must be cut to layer the
    // module graph.
    let module_path: BTreeMap<ModuleId, &str> = result
        .modules
        .iter()
        .map(|m| (m.module, m.path.as_str()))
        .collect();
    for (from, to) in &result.tangles.feedback_edges {
        let from = module_path.get(from).copied().unwrap_or("?");
        let to = module_path.get(to).copied().unwrap_or("?");
        out.push(Finding {
            rule: "module-cycle",
            severity: Severity::High,
            location: from.to_owned(),
            message: format!("`{from}` depends on `{to}`, closing a module cycle"),
            suggestion: format!(
                "break this back-edge (move the shared code out, or invert the dependency) so `{from}` and `{to}` can be layered"
            ),
            details: Vec::new(),
        });
    }

    // Interface leaks (Medium): grouped by (consumer module, owner module). Each
    // group is one "A reaches into B's internals" finding listing the items.
    let mut leak_groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for leak in &result.encapsulation.leaks {
        let from = item_module(leak.from);
        let to = item_module(leak.to);
        leak_groups.entry((from, to)).or_default().push(format!(
            "`{}` ({})",
            leak.to_path,
            vis_str(&leak.target_visibility)
        ));
    }
    for ((from, to), mut items) in leak_groups {
        items.sort();
        items.dedup();
        let n = items.len();
        out.push(Finding {
            rule: "interface-leak",
            severity: Severity::Medium,
            location: from.clone(),
            message: format!("`{from}` reaches into {n} internal item(s) of `{to}`"),
            suggestion: format!(
                "expose the used parts of `{to}` through a published interface, or move them next to `{from}`"
            ),
            details: items,
        });
    }

    // Over-exposure (Low): grouped by the owning module. Each group is one
    // "module M has N items wider than needed" finding listing them.
    let mut over_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for over in &result.encapsulation.over_exposed {
        over_groups
            .entry(item_module(over.item))
            .or_default()
            .push(format!(
                "`{}` ({} -> {})",
                over.path,
                vis_str(&over.declared),
                vis_str(&over.required)
            ));
    }
    for (module, mut items) in over_groups {
        items.sort();
        items.dedup();
        let n = items.len();
        out.push(Finding {
            rule: "over-exposed",
            severity: Severity::Low,
            location: module.clone(),
            message: format!("`{module}` has {n} item(s) more visible than any caller needs"),
            suggestion: "narrow each to the visibility shown (most can be private)".to_owned(),
            details: items,
        });
    }

    out.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.location.cmp(&b.location))
    });
    out
}

/// Per-rule explanation used in the agent prompt: each group is introduced once
/// with what the issue means and how to fix it, so the agent has the context the
/// individual findings assume. Kept separate from the report wording, which is
/// terser because the UI shows it interactively.
fn rule_briefing(rule: &str) -> (&'static str, &'static str) {
    match rule {
        "module-cycle" => (
            "Module cycles (fix first)",
            "Two or more of these modules depend on each other, so they cannot be put in a dependency order and must be understood together. Break each listed back-edge: move the code they share into a lower-level module both can depend on, or invert one direction (for example, have the lower module define a trait that the higher one implements). Each line is `consumer` depends on `provider`.",
        ),
        "interface-leak" => (
            "Interface leaks",
            "These references cross a module boundary into an item that is internal (`pub(crate)`, `pub(super)`, or `pub` inside a module that is not itself `pub`), so it is not part of the crate's published API and the two modules are coupled through private details. For each `consumer -> provider` pair, choose one: (a) if the listed items are genuinely a shared service, give the provider one small, intentional interface (a trait, or a focused and documented set of functions) that the consumer goes through, and treat that as the module's published boundary; (b) if an item logically belongs with the consumer, move it there so the reference becomes intra-module. Do not blindly add `pub` to the items: that widens the crate's public API, the opposite of the goal. A leak that reflects a deliberate internal interface is acceptable, leave it and add a one-line doc comment marking it intentional.",
        ),
        _ => (
            "Over-broad visibility",
            "Each item below is declared more visible than any caller modula observed needs; the `current -> minimum sufficient` visibility shows the tighter setting. That minimum already accounts for every in-crate caller, including the cross-module uses listed under interface leaks, so applying it will not break those calls (modula does not see callers reached only through macros, doctests, benchmarks, or examples, so let `cargo test` and `cargo build --all-targets` confirm). Only change items whose visibility you actually control: an inherent method, a free function, a module, a struct or enum, or a constant. Skip anything whose visibility is fixed elsewhere and so cannot be narrowed: trait-implementation methods (including derived ones such as `clone`, `fmt`, `eq`, `hash`), methods required by a trait, and enum variants (a variant's visibility always follows its enum). Those are listed as an artifact, not an action. The `minimum sufficient` value assumes the current module layout, so if you resolve an interface leak by moving an item, re-derive its visibility from the new location rather than applying the value listed here.",
        ),
    }
}

/// Renders the findings as a single, self-contained prompt a user can paste into
/// a coding agent, which then performs the fixes. Provider-agnostic. It explains
/// each issue category before listing instances, because the agent has no prior
/// knowledge of modula or its terms; the per-instance lines reuse each finding's
/// `message` / `details` so the wording matches the report.
#[must_use]
pub fn agent_prompt(result: &AnalysisResult, actions: &[Finding]) -> String {
    use std::fmt::Write as _;

    let c = &result.composite;
    let crate_name = &result.crate_name;
    let mut s = String::new();

    let _ = writeln!(
        s,
        "# Improve the modularity of the Rust crate `{crate_name}`\n"
    );

    let _ = writeln!(
        s,
        "You are working inside this crate's own repository. A static analyzer called modula measured how well the crate's declared module structure matches its actual internal dependency graph, and produced the tasks below. A well-modularized crate has cohesive modules, a module dependency graph with no cycles (so the modules can be layered), and cross-module references that go through each module's public interface rather than into its internals. Being widely depended upon is not a problem; only cycles, reaching into another module's internals, and over-broad visibility count against it.\n"
    );

    match c.headline {
        Some(h) => {
            let cohesion = c
                .cohesion_term
                .map_or_else(|| "N/A".to_owned(), |v| format!("{v:.2}"));
            let _ = writeln!(
                s,
                "modula scored `{crate_name}` at {h:.2} out of 1.00 (cohesion {cohesion}, acyclicity {:.2}, encapsulation {:.2}), across {} items in {} modules. The findings below are what hold the score back.\n",
                c.acyclicity_term, c.encapsulation_term, result.n_real_items, result.n_module_nodes
            );
        }
        None => {
            let _ = writeln!(
                s,
                "modula found no measurable module structure to score in `{crate_name}` ({} items in {} modules), but flagged the items below.\n",
                result.n_real_items, result.n_module_nodes
            );
        }
    }

    let _ = writeln!(
        s,
        "modula has already set aside the crate's genuine public API (anything reachable through an unbroken `pub` chain from the crate root), so by its reckoning nothing listed below is part of your published surface, still, sanity-check any item you deliberately export. Ground rules: keep all existing behavior and the test suite passing, make the smallest change that resolves each item, and treat these as suggestions rather than absolutes (if a finding reflects a deliberate design choice, leave the code and add a short doc comment explaining why). The two sections are independent lenses on the same code and can be done in either order; the visibility narrowing is the safer, more mechanical of the two.\n"
    );

    let groups = ["module-cycle", "interface-leak", "over-exposed"];
    let mut any = false;
    for rule in groups {
        let items: Vec<&Finding> = actions.iter().filter(|f| f.rule == rule).collect();
        if items.is_empty() {
            continue;
        }
        any = true;
        let (heading, blurb) = rule_briefing(rule);
        let _ = writeln!(s, "## {heading}\n");
        let _ = writeln!(s, "{blurb}\n");
        for f in &items {
            let _ = writeln!(s, "- {}", f.message);
            if !f.details.is_empty() {
                let _ = writeln!(s, "  - {}", f.details.join("\n  - "));
            }
        }
        let _ = writeln!(s);
    }

    if !any {
        let _ = writeln!(
            s,
            "No issues were found: the module boundaries are already clean."
        );
        return s;
    }

    let _ = writeln!(
        s,
        "When done, run `cargo build`, `cargo test`, and `cargo clippy` and make sure all three are clean. If modula is installed, re-run `cargo modula` to confirm the score went up."
    );
    s
}

/// Renders a visibility as the Rust syntax a user would write.
fn vis_str(v: &Visibility) -> String {
    match v {
        Visibility::Public => "pub".to_owned(),
        Visibility::Crate => "pub(crate)".to_owned(),
        Visibility::Super => "pub(super)".to_owned(),
        Visibility::Private => "private".to_owned(),
        Visibility::Module(path) => format!("pub(in {path})"),
    }
}

#[cfg(test)]
mod tests {
    use modula_ir::{
        Crate, CrateGraph, CrateId, Edge, Item, ItemId, ItemKind, Module, ModuleId, ModuleKind,
        RefKind, SCHEMA_VERSION, Visibility,
    };

    use super::{Severity, agent_prompt, findings};
    use crate::analysis::{AnalysisConfig, analyze};

    /// A two-module crate: `a::caller` uses a same-module `pub(crate)` helper
    /// (over-exposed), the public `b::api` (legitimate), and `b::internal`
    /// (`pub(crate)`, a leak into b's internals).
    fn ir() -> CrateGraph {
        let krate = CrateId(0);
        let module = |id: u32, path: &str, parent: Option<u32>| Module {
            id: ModuleId(id),
            crate_id: krate,
            parent: parent.map(ModuleId),
            name: path.rsplit("::").next().unwrap_or(path).to_owned(),
            canonical_path: path.to_owned(),
            depth: u32::from(parent.is_some()),
            visibility: Visibility::Public,
            kind: ModuleKind::Mod,
        };
        let item = |id: u32, path: &str, owner: u32, vis: Visibility| Item {
            id: ItemId(id),
            canonical_path: path.to_owned(),
            kind: ItemKind::Function,
            visibility: vis,
            owning_module: ModuleId(owner),
            crate_id: krate,
            has_canonical_path: true,
            reachable_pub_api: false,
        };
        let edge = |from: u32, to: u32| Edge {
            from: ItemId(from),
            to: ItemId(to),
            kind: RefKind::Body,
            weight: 1,
        };
        let mut graph = CrateGraph {
            schema_version: SCHEMA_VERSION,
            ra_version: String::new(),
            root_crate: krate,
            crates: vec![Crate {
                id: krate,
                name: "k".to_owned(),
                is_local: true,
                root_module: ModuleId(0),
            }],
            modules: vec![
                module(0, "k", None),
                module(1, "k::a", Some(0)),
                module(2, "k::b", Some(0)),
            ],
            items: vec![
                item(0, "k::a::caller", 1, Visibility::Private),
                item(1, "k::a::helper", 1, Visibility::Crate),
                item(2, "k::b::api", 2, Visibility::Public),
                item(3, "k::b::internal", 2, Visibility::Crate),
            ],
            edges: vec![edge(0, 1), edge(0, 2), edge(0, 3)],
        };
        graph.compute_public_api();
        graph
    }

    #[test]
    fn findings_group_the_leak_and_the_over_exposure_worst_first() {
        let graph = ir();
        let result = analyze(&graph, &AnalysisConfig::default()).expect("analysis");
        let found = findings(&result, &graph);

        let leak = found
            .iter()
            .find(|f| f.rule == "interface-leak")
            .expect("a leak finding");
        assert_eq!(leak.severity, Severity::Medium);
        assert_eq!(leak.location, "k::a");
        assert!(leak.message.contains("k::b"));
        assert!(leak.details.iter().any(|d| d.contains("k::b::internal")));

        let over = found
            .iter()
            .find(|f| f.rule == "over-exposed")
            .expect("an over-exposure finding");
        assert_eq!(over.severity, Severity::Low);
        assert_eq!(over.location, "k::a");
        assert!(over.details.iter().any(|d| d.contains("k::a::helper")));

        // Acyclic crate, so no cycle findings; the leak (Medium) sorts before the
        // over-exposure (Low).
        assert!(found.iter().all(|f| f.rule != "module-cycle"));
        let leak_pos = found.iter().position(|f| f.rule == "interface-leak");
        let over_pos = found.iter().position(|f| f.rule == "over-exposed");
        assert!(leak_pos < over_pos, "worse findings come first");
    }

    #[test]
    fn agent_prompt_is_self_contained() {
        let graph = ir();
        let result = analyze(&graph, &AnalysisConfig::default()).expect("analysis");
        let actions = findings(&result, &graph);
        let prompt = agent_prompt(&result, &actions);

        // Names the crate and the score, explains and lists the groups present,
        // includes the non-narrowable caveat, and tells the agent how to verify.
        assert!(prompt.contains("`k`"), "names the crate");
        assert!(prompt.contains("out of 1.00"), "states the score");
        assert!(prompt.contains("## Interface leaks"));
        assert!(
            prompt.contains("not part of the crate's published API"),
            "explains what a leak is"
        );
        assert!(prompt.contains("## Over-broad visibility"));
        assert!(
            prompt.contains("a variant's visibility always follows its enum"),
            "warns about non-narrowable items"
        );
        assert!(!prompt.contains("Module cycles"), "acyclic: no cycle group");
        assert!(prompt.contains("k::b::internal"), "lists the leaked item");
        assert!(prompt.contains("cargo test"), "tells the agent to verify");
    }
}
