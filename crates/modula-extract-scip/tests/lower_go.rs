//! Lowering test against a committed `scip-go` index.
//!
//! The index was produced once by running `scip-go index` over
//! `tests/fixtures/sample-go` in a `golang` container (scip-go needs the Go
//! toolchain) and committed, so the test is deterministic and needs no Go
//! toolchain at test time. Regenerate it the same way if the fixture changes.
//! scip-go emits a package's import path as a single `Namespace` descriptor
//! (`sample-go/mathx`), so the module tree is flat.

use std::path::PathBuf;

use modula_extract_scip::lower_path;
use modula_ir::{ItemKind, ModuleKind, RefKind};

fn index() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "sample-go",
        "index.scip",
    ]
    .iter()
    .collect()
}

#[test]
fn lowers_go_packages_and_types() {
    let g = lower_path(&index()).expect("lower");
    assert_eq!(g.krate(g.root_crate).name, "sample-go");

    let module = |path: &str| g.modules.iter().find(|m| m.canonical_path == path);
    assert!(module("sample-go::sample-go").is_some(), "main package");
    assert!(
        module("sample-go::sample-go/mathx").is_some(),
        "mathx package"
    );
    assert_eq!(
        module("sample-go::sample-go/mathx::Calc").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
}

#[test]
fn lowers_go_items_and_edges() {
    let g = lower_path(&index()).expect("lower");
    let item = |suffix: &str| g.items.iter().find(|i| i.canonical_path.ends_with(suffix));

    assert_eq!(item("mathx::Add").map(|i| i.kind), Some(ItemKind::Function));
    assert_eq!(
        item("mathx::Double").map(|i| i.kind),
        Some(ItemKind::Function)
    );
    assert_eq!(item("Calc::Run").map(|i| i.kind), Some(ItemKind::AssocFn));

    let edge = |from: &str, to: &str| {
        g.edges.iter().any(|e| {
            g.item(e.from).canonical_path.ends_with(from)
                && g.item(e.to).canonical_path.ends_with(to)
        })
    };
    // Within-package call.
    assert!(edge("mathx::Add", "mathx::Double"), "Add -> Double");
    // Cross-package call.
    assert!(edge("::Greet", "mathx::Add"), "Greet -> Add");
    assert!(
        g.edges
            .iter()
            .all(|e| matches!(e.kind, RefKind::Body | RefKind::Import)),
        "SCIP edges are Body or Import"
    );
}

#[test]
fn go_graph_analyzes() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let g = lower_path(&index()).expect("lower");
    let result = analyze(&g, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample-go");
    assert!(result.n_real_items > 0);
}
