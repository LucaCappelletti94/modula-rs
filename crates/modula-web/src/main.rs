//! `modula-web`: a Dioxus web report viewer for modula-rs.
//!
//! The whole scoring core (`modula-ir` plus `modula-metrics`) compiles to
//! `wasm32-unknown-unknown`, so this app runs the actual metric in the browser:
//! the user uploads a crate's IR container (produced by `cargo modula
//! --emit-ir`), and the report is computed client-side. Extraction stays
//! host-side because it needs a Rust toolchain, which is why the input is the IR
//! rather than a path.

use dioxus::prelude::*;
use dioxus_free_icons::Icon;
use dioxus_free_icons::icons::fa_solid_icons::{
    FaCircleCheck, FaCircleExclamation, FaCircleInfo, FaClipboard, FaDiagramProject, FaFileLines,
    FaRobot, FaTriangleExclamation, FaUpload,
};
use modula_ir::{CrateGraph, read_container};
use modula_metrics::analysis::{AnalysisConfig, AnalysisResult, analyze};
use modula_metrics::findings::{Finding, agent_prompt, findings};
use modula_metrics::report::to_human;

fn main() {
    dioxus::launch(App);
}

/// A successfully scored crate: the report plus its actionable findings.
#[derive(Clone, PartialEq)]
struct Loaded {
    result: AnalysisResult,
    actions: Vec<Finding>,
}

/// Outcome of scoring an uploaded file: either a loaded report or a message.
type Scored = Result<Loaded, String>;

#[component]
fn App() -> Element {
    let mut report: Signal<Option<Scored>> = use_signal(|| None);

    rsx! {
        style { "{STYLE}" }
        main { class: "wrap",
            header {
                h1 {
                    Icon { icon: FaDiagramProject, width: 26, height: 26 }
                    " modula"
                }
                p { class: "tag",
                    "How well does a Rust crate's module tree match its internal dependency graph?"
                }
            }

            section { class: "upload",
                label { class: "filebtn",
                    Icon { icon: FaUpload, width: 15, height: 15 }
                    " Choose a crate IR file"
                    input {
                        r#type: "file",
                        accept: ".bin,.zst",
                        onchange: move |evt| async move {
                            if let Some(file) = evt.files().into_iter().next() {
                                let scored = match file.read_bytes().await {
                                    Ok(bytes) => score(&bytes),
                                    Err(err) => Err(format!("could not read the file: {err}")),
                                };
                                report.set(Some(scored));
                            }
                        },
                    }
                }
                p { class: "hint",
                    "Produce it with "
                    code { "cargo modula --emit-ir > crate.bin.zst" }
                }
            }

            {match report() {
                None => rsx! {},
                Some(Err(message)) => rsx! { p { class: "err", "{message}" } },
                Some(Ok(loaded)) => rsx! { Report { loaded } },
            }}
        }
    }
}

/// The scored report for one crate.
#[component]
fn Report(loaded: Loaded) -> Element {
    let Loaded { result, actions } = loaded;
    let composite = result.composite;
    let headline = composite
        .headline
        .map_or_else(|| "N/A".to_owned(), |h| format!("{h:.3}"));
    let prompt = agent_prompt(&result, &actions);

    rsx! {
        section { class: "report",
            div { class: "headline",
                div { class: "score", "{headline}" }
                div { class: "meta",
                    h2 { "{result.crate_name}" }
                    p { "{result.n_real_items} items in {result.n_module_nodes} modules" }
                }
            }

            div { class: "terms",
                Term { label: "cohesion", value: composite.cohesion_term }
                Term { label: "acyclicity", value: Some(composite.acyclicity_term) }
                Term { label: "encapsulation", value: Some(composite.encapsulation_term) }
            }

            Actions { actions, prompt }

            details { class: "rawwrap",
                summary {
                    Icon { icon: FaFileLines, width: 13, height: 13 }
                    " Full text report"
                }
                pre { class: "raw", "{to_human(&result)}" }
            }
        }
    }
}

/// The actionable findings, grouped by rule and worst first, with a text filter.
/// This is the same set a future `cargo modula --lint` would emit.
#[component]
fn Actions(actions: Vec<Finding>, prompt: String) -> Element {
    let mut query = use_signal(String::new);
    let mut copied = use_signal(|| false);
    let mut show_prompt = use_signal(|| false);

    if actions.is_empty() {
        return rsx! {
            section { class: "actions",
                p { class: "clean",
                    Icon { icon: FaCircleCheck, width: 16, height: 16 }
                    " No actions: nothing over-exposed, no leaks, no cycles."
                }
            }
        };
    }

    let needle = query().to_lowercase();
    let matches = |f: &Finding| {
        needle.is_empty()
            || f.message.to_lowercase().contains(&needle)
            || f.location.to_lowercase().contains(&needle)
            || f.details.iter().any(|d| d.to_lowercase().contains(&needle))
    };
    let visible: Vec<Finding> = actions.iter().filter(|f| matches(f)).cloned().collect();
    // A clone the copy-button closure can own and re-clone on each click, so the
    // original `prompt` is still available for the fallback textarea.
    let prompt_copy = prompt.clone();

    // (rule id, heading, severity css class, open by default).
    let groups = [
        ("module-cycle", "Module cycles", "high", true),
        ("interface-leak", "Interface leaks", "medium", true),
        ("over-exposed", "Tighten visibility", "low", false),
    ];

    rsx! {
        section { class: "actions",
            div { class: "actions-head",
                h3 { "Suggested actions ({visible.len()})" }
                div { class: "head-controls",
                    input {
                        class: "filter",
                        r#type: "search",
                        placeholder: "filter by name...",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                    button {
                        class: "copybtn",
                        onclick: move |_| {
                            let text = prompt_copy.clone();
                            spawn(async move {
                                let eval = document::eval(
                                    "const t = await dioxus.recv(); await navigator.clipboard.writeText(t);",
                                );
                                let _ = eval.send(text);
                            });
                            copied.set(true);
                            show_prompt.set(true);
                        },
                        if copied() {
                            Icon { icon: FaCircleCheck, width: 14, height: 14 }
                            " Copied!"
                        } else {
                            Icon { icon: FaClipboard, width: 14, height: 14 }
                            " Copy agent prompt"
                        }
                    }
                }
            }
            if show_prompt() {
                details { class: "promptpanel", open: true,
                    summary {
                        Icon { icon: FaRobot, width: 14, height: 14 }
                        " Agent prompt (paste into your coding agent)"
                    }
                    textarea { class: "prompttext", readonly: true, rows: "18", "{prompt}" }
                }
            }
            for (rule, heading, sev, open) in groups {
                {
                    let items: Vec<Finding> =
                        visible.iter().filter(|f| f.rule == rule).cloned().collect();
                    if items.is_empty() {
                        rsx! {}
                    } else {
                        rsx! {
                            details { class: "agroup", open,
                                summary {
                                    span { class: "sev sev-{sev}",
                                        SevIcon { sev: sev.to_string() }
                                        "{sev}"
                                    }
                                    span { class: "ghead", "{heading}" }
                                    span { class: "gcount", "{items.len()}" }
                                }
                                for f in items {
                                    div { class: "action",
                                        div { class: "amsg", Rich { text: f.message.clone() } }
                                        div { class: "afix", Rich { text: f.suggestion.clone() } }
                                        if !f.details.is_empty() {
                                            details { class: "adetails",
                                                summary { "{f.details.len()} item(s)" }
                                                for d in f.details.clone() {
                                                    div { class: "ditem", Rich { text: d } }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The severity-appropriate Font Awesome icon for an action group. Each branch
/// renders a distinct icon type, so they cannot be selected by a single value.
#[component]
fn SevIcon(sev: String) -> Element {
    match sev.as_str() {
        "high" => rsx! { Icon { icon: FaTriangleExclamation, width: 12, height: 12 } },
        "medium" => rsx! { Icon { icon: FaCircleExclamation, width: 12, height: 12 } },
        _ => rsx! { Icon { icon: FaCircleInfo, width: 12, height: 12 } },
    }
}

/// Renders text with `backtick`-wrapped spans as inline `code` elements.
#[component]
fn Rich(text: String) -> Element {
    rsx! {
        for (i, part) in text.split('`').enumerate() {
            if i % 2 == 1 {
                code { "{part}" }
            } else {
                span { "{part}" }
            }
        }
    }
}

/// One labelled `[0, 1]` term shown as a bar.
#[component]
fn Term(label: String, value: Option<f64>) -> Element {
    let (text, pct) = match value {
        Some(v) => (format!("{v:.3}"), (v.clamp(0.0, 1.0) * 100.0)),
        None => ("N/A".to_owned(), 0.0),
    };
    rsx! {
        div { class: "term",
            div { class: "term-head",
                span { class: "term-label", "{label}" }
                span { class: "term-value", "{text}" }
            }
            div { class: "bar",
                div { class: "fill", style: "width: {pct}%" }
            }
        }
    }
}

/// Reads an IR container, scores it, and derives its actionable findings.
fn score(ir_bytes: &[u8]) -> Scored {
    let graph: CrateGraph =
        read_container(ir_bytes).map_err(|e| format!("not a valid crate IR: {e}"))?;
    let result =
        analyze(&graph, &AnalysisConfig::default()).map_err(|e| format!("analysis failed: {e}"))?;
    let actions = findings(&result, &graph);
    Ok(Loaded { result, actions })
}

const STYLE: &str = r#"
* { box-sizing: border-box; }
body { margin: 0; background: #0f1115; color: #e6e8eb;
  font: 16px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif; }
svg { vertical-align: -0.125em; flex: none; }
h1 svg { color: #4ade80; vertical-align: -0.1em; }
.sev { display: inline-flex; align-items: center; gap: 0.3rem; }
.wrap { max-width: 760px; margin: 0 auto; padding: 2.5rem 1.25rem 4rem; }
h1 { margin: 0; font-size: 2rem; letter-spacing: -0.02em; }
.tag { color: #9aa3ad; margin: 0.25rem 0 2rem; }
.upload { border: 1px dashed #2b313b; border-radius: 12px; padding: 1.5rem; text-align: center; }
.filebtn { display: inline-block; cursor: pointer; background: #2f6feb; color: #fff;
  padding: 0.6rem 1rem; border-radius: 8px; font-weight: 600; }
.filebtn input { display: none; }
.hint { color: #9aa3ad; font-size: 0.9rem; margin: 0.9rem 0 0; }
code { background: #1b1f27; padding: 0.1rem 0.4rem; border-radius: 5px; font-size: 0.85em; }
.err { color: #ff6b6b; background: #2a1416; border: 1px solid #4a1f22;
  padding: 0.75rem 1rem; border-radius: 8px; margin-top: 1.5rem; }
.report { margin-top: 2rem; }
.headline { display: flex; align-items: center; gap: 1.25rem; }
.score { font-size: 3rem; font-weight: 700; color: #4ade80; min-width: 5rem; }
.meta h2 { margin: 0; font-size: 1.25rem; }
.meta p { margin: 0.2rem 0 0; color: #9aa3ad; font-size: 0.9rem; }
.terms { margin: 1.75rem 0; display: grid; gap: 1rem; }
.term-head { display: flex; justify-content: space-between; font-size: 0.95rem; margin-bottom: 0.3rem; }
.term-label { color: #c3c9d1; }
.term-value { color: #e6e8eb; font-variant-numeric: tabular-nums; }
.bar { height: 8px; background: #1b1f27; border-radius: 999px; overflow: hidden; }
.fill { height: 100%; background: linear-gradient(90deg, #2f6feb, #4ade80); }
.actions { margin-top: 2rem; }
.actions-head { display: flex; align-items: center; justify-content: space-between;
  gap: 1rem; margin-bottom: 0.75rem; }
.actions-head h3 { font-size: 1.05rem; margin: 0; }
.head-controls { display: flex; align-items: center; gap: 0.6rem; }
.filter { background: #1b1f27; border: 1px solid #2b313b; color: #e6e8eb;
  border-radius: 8px; padding: 0.4rem 0.7rem; font-size: 0.9rem; min-width: 10rem; }
.copybtn { background: #2f6feb; color: #fff; border: 0; border-radius: 8px;
  padding: 0.45rem 0.8rem; font-size: 0.9rem; font-weight: 600; cursor: pointer; white-space: nowrap; }
.copybtn:hover { background: #4079f0; }
.promptpanel { background: #161a21; border: 1px solid #232934; border-radius: 10px;
  padding: 0.6rem 0.9rem; margin-bottom: 1rem; }
.prompttext { width: 100%; margin-top: 0.6rem; background: #0f1115; color: #e6e8eb;
  border: 1px solid #2b313b; border-radius: 8px; padding: 0.75rem; resize: vertical;
  font: 12.5px/1.5 ui-monospace, monospace; }
.clean { color: #4ade80; }
.agroup { background: #161a21; border: 1px solid #232934; border-radius: 10px;
  margin-bottom: 0.75rem; padding: 0.5rem 0.9rem; }
.agroup summary { display: flex; align-items: center; gap: 0.6rem; cursor: pointer; }
.sev { font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.04em;
  padding: 0.1rem 0.45rem; border-radius: 999px; font-weight: 700; }
.sev-high { background: #4a1f22; color: #ff8b8b; }
.sev-medium { background: #4a3a1a; color: #ffd27d; }
.sev-low { background: #25303f; color: #8fb6ff; }
.ghead { font-weight: 600; }
.gcount { margin-left: auto; color: #9aa3ad; font-variant-numeric: tabular-nums; }
.action { border-top: 1px solid #232934; padding: 0.6rem 0 0.5rem; }
.amsg { color: #e6e8eb; }
.afix { color: #9aa3ad; font-size: 0.9rem; margin-top: 0.15rem; }
.afix::before { content: "fix: "; color: #4ade80; }
.adetails { margin-top: 0.4rem; }
.adetails summary { font-size: 0.85rem; color: #8fb6ff; }
.ditem { color: #c3c9d1; font-size: 0.85rem; padding: 0.15rem 0 0.15rem 0.9rem; }
.rawwrap { margin-top: 1.5rem; }
summary { cursor: pointer; color: #9aa3ad; }
.raw { background: #1b1f27; padding: 1rem; border-radius: 10px; overflow-x: auto;
  font: 13px/1.5 ui-monospace, monospace; }
"#;
