//! Lowering test against a committed `scip-python` index.
//!
//! The index was produced once by running `scip-python index` over
//! `tests/fixtures/sample-py` in a clean, pip-capable Python environment (a
//! `node` + `python3` container, since scip-python needs pip for environment
//! detection) and committed, so the test is deterministic and needs no Python or
//! Node toolchain at test time. Regenerate it the same way if the fixture
//! changes. scip-python uses dotted module names as single `Namespace`
//! descriptors (`sample_py.mathx`), so the module tree is flat rather than nested.

use std::path::PathBuf;

use modula_extract_scip::lower_path;
use modula_ir::{ItemKind, ModuleKind, RefKind};

fn index() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "sample-py",
        "index.scip",
    ]
    .iter()
    .collect()
}

#[test]
fn lowers_python_modules_and_types() {
    let g = lower_path(&index()).expect("lower");
    assert_eq!(g.krate(g.root_crate).name, "sample-py");

    let module = |path: &str| g.modules.iter().find(|m| m.canonical_path == path);
    assert!(module("sample-py::sample_py.mathx").is_some());
    assert!(module("sample-py::sample_py.main").is_some());
    // A class becomes a type container under its module.
    assert_eq!(
        module("sample-py::sample_py.mathx::Calc").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
}

#[test]
fn lowers_python_items_and_edges() {
    let g = lower_path(&index()).expect("lower");
    let item = |suffix: &str| g.items.iter().find(|i| i.canonical_path.ends_with(suffix));

    assert_eq!(item("mathx::add").map(|i| i.kind), Some(ItemKind::Function));
    assert_eq!(
        item("mathx::double").map(|i| i.kind),
        Some(ItemKind::Function)
    );
    assert_eq!(item("Calc::run").map(|i| i.kind), Some(ItemKind::AssocFn));

    let edge = |from: &str, to: &str| {
        g.edges.iter().any(|e| {
            g.item(e.from).canonical_path.ends_with(from)
                && g.item(e.to).canonical_path.ends_with(to)
        })
    };
    // Within-module call, symbol-resolved.
    assert!(edge("mathx::add", "mathx::double"), "add -> double");
    // Cross-module call resolved through the import.
    assert!(edge("main::greet", "mathx::add"), "greet -> add");
    assert!(
        g.edges
            .iter()
            .all(|e| matches!(e.kind, RefKind::Body | RefKind::Import)),
        "SCIP edges are Body or Import"
    );
}

#[test]
fn python_graph_analyzes() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let g = lower_path(&index()).expect("lower");
    let result = analyze(&g, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample-py");
    assert!(result.n_real_items > 0);
}
