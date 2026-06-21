//! Lowering test against a committed `.scip` index.
//!
//! The index was produced once with `npx @sourcegraph/scip-typescript index`
//! inside `tests/fixtures/sample-ts` and committed, so the test is deterministic
//! and needs no Node toolchain or network. Regenerate it if the fixture changes.

use std::path::PathBuf;

use modula_extract_scip::lower_path;
use modula_ir::{ItemKind, ModuleKind, RefKind};

fn index() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "sample-ts",
        "index.scip",
    ]
    .iter()
    .collect()
}

#[test]
fn lowers_the_typescript_module_tree() {
    let g = lower_path(&index()).expect("lower");
    assert_eq!(g.krate(g.root_crate).name, "sample-ts");

    let module = |path: &str| g.modules.iter().find(|m| m.canonical_path == path);
    // The file path becomes a nested module chain, including intermediate
    // namespaces the indexer did not emit on their own.
    for path in [
        "sample-ts::src",
        "sample-ts::src::util",
        "sample-ts::src::util::math.ts",
        "sample-ts::src::index.ts",
    ] {
        assert!(module(path).is_some(), "missing module {path}");
    }
    // Classes and interfaces become type containers.
    assert_eq!(
        module("sample-ts::src::util::math.ts::Calc").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
    assert_eq!(
        module("sample-ts::src::index.ts::Internal").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
}

#[test]
fn lowers_items_and_resolved_edges() {
    let g = lower_path(&index()).expect("lower");
    let item = |suffix: &str| g.items.iter().find(|i| i.canonical_path.ends_with(suffix));

    assert_eq!(
        item("math.ts::add").map(|i| i.kind),
        Some(ItemKind::Function)
    );
    assert_eq!(
        item("math.ts::double").map(|i| i.kind),
        Some(ItemKind::Function)
    );
    assert_eq!(
        item("Calc::run").map(|i| i.kind),
        Some(ItemKind::AssocFn),
        "a method on a type is an associated function"
    );

    let edge = |from: &str, to: &str| {
        g.edges.iter().any(|e| {
            g.item(e.from).canonical_path.ends_with(from)
                && g.item(e.to).canonical_path.ends_with(to)
        })
    };
    // Within-file, symbol-resolved.
    assert!(edge("math.ts::add", "math.ts::double"), "add -> double");
    // Cross-file: greet's body call resolves to add in another file.
    assert!(edge("index.ts::greet", "math.ts::add"), "greet -> add");
    assert!(
        g.edges
            .iter()
            .all(|e| matches!(e.kind, RefKind::Body | RefKind::Import)),
        "SCIP edges are Body or Import"
    );
}

#[test]
fn the_lowered_graph_analyzes() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let g = lower_path(&index()).expect("lower");
    let result = analyze(&g, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample-ts");
    assert!(result.n_real_items > 0);
}
