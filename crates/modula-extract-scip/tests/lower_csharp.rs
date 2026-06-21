//! Lowering test against a committed `scip-dotnet` index.
//!
//! The index was produced once by running `scip-dotnet index` over
//! `tests/fixtures/sample-csharp` in a `dotnet/sdk` container and committed, so
//! the test is deterministic and needs no .NET toolchain at test time.
//! Regenerate it the same way (`scip-dotnet index --output index.scip --exclude
//! "**/obj/**"`) if the fixture changes.
//!
//! Two `scip-dotnet` quirks the assertions account for. It emits only the
//! innermost segment of a nested namespace, so the C# namespace
//! `SampleCsharp.Mathx` lowers to a `Mathx` module that sits beside `SampleCsharp`
//! rather than nesting under it. And it omits definition `enclosing_range`s, so
//! the lowering attributes each reference to the nearest preceding item (its
//! no-enclosing fallback) rather than to a containing range.

use std::path::PathBuf;

use modula_extract_scip::lower_path;
use modula_ir::{ItemKind, ModuleKind, RefKind};

fn index() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "sample-csharp",
        "index.scip",
    ]
    .iter()
    .collect()
}

#[test]
fn lowers_csharp_namespaces_and_classes() {
    let g = lower_path(&index()).expect("lower");
    assert_eq!(g.krate(g.root_crate).name, "sample-csharp");

    let module = |path: &str| g.modules.iter().find(|m| m.canonical_path == path);
    // Namespaces become plain modules (only the innermost segment is emitted).
    assert_eq!(
        module("sample-csharp::Mathx").map(|m| m.kind),
        Some(ModuleKind::Mod)
    );
    assert_eq!(
        module("sample-csharp::SampleCsharp").map(|m| m.kind),
        Some(ModuleKind::Mod)
    );
    // Classes become type containers.
    assert_eq!(
        module("sample-csharp::Mathx::Mathx").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
    assert_eq!(
        module("sample-csharp::Mathx::Calc").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
}

#[test]
fn lowers_csharp_methods_and_edges() {
    let g = lower_path(&index()).expect("lower");
    let item = |suffix: &str| g.items.iter().find(|i| i.canonical_path.ends_with(suffix));

    // Methods on a class lower to associated functions.
    assert_eq!(item("Mathx::Add").map(|i| i.kind), Some(ItemKind::AssocFn));
    assert_eq!(
        item("Mathx::DoubleValue").map(|i| i.kind),
        Some(ItemKind::AssocFn)
    );
    assert_eq!(item("Calc::Run").map(|i| i.kind), Some(ItemKind::AssocFn));
    assert_eq!(
        item("Program::Greet").map(|i| i.kind),
        Some(ItemKind::AssocFn)
    );

    let edge = |from: &str, to: &str| {
        g.edges.iter().any(|e| {
            g.item(e.from).canonical_path.ends_with(from)
                && g.item(e.to).canonical_path.ends_with(to)
        })
    };
    // Within-class call (recovered through the no-enclosing-range fallback).
    assert!(
        edge("Mathx::Add", "Mathx::DoubleValue"),
        "Add -> DoubleValue"
    );
    // Cross-namespace call: SampleCsharp.Program -> SampleCsharp.Mathx.Mathx.Add.
    assert!(edge("Program::Greet", "Mathx::Add"), "Greet -> Add");
    assert!(
        g.edges
            .iter()
            .all(|e| matches!(e.kind, RefKind::Body | RefKind::Import)),
        "SCIP edges are Body or Import"
    );
}

#[test]
fn csharp_graph_analyzes() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let g = lower_path(&index()).expect("lower");
    let result = analyze(&g, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample-csharp");
    assert!(result.n_real_items > 0);
}
