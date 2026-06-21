//! Lowering test against a committed `scip-clang` index.
//!
//! The index was produced once by running `scip-clang` over
//! `tests/fixtures/sample-cpp` in a container (CMake configured with
//! `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`, then `scip-clang --compdb-path=
//! build/compile_commands.json`) and committed, so the test is deterministic and
//! needs no C++ toolchain at test time. Regenerate it the same way if the fixture
//! changes.
//!
//! Two `scip-clang` traits the assertions account for. It omits definition
//! `enclosing_range`s, so edges are recovered through the lowering's
//! nearest-definition fallback. And it emits a synthetic file-scope namespace
//! (named like `<file>/mathx/mathx.cpp`) per translation unit, which lowers to
//! extra empty modules that carry no items, so the assertions target the real C++
//! namespaces (`mathx`, `app`) rather than the module count.

use std::path::PathBuf;

use modula_extract_scip::lower_path;
use modula_ir::{ItemKind, ModuleKind, RefKind};

fn index() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "sample-cpp",
        "index.scip",
    ]
    .iter()
    .collect()
}

#[test]
fn lowers_cpp_namespaces_and_classes() {
    let g = lower_path(&index()).expect("lower");
    assert_eq!(g.krate(g.root_crate).name, "sample-cpp");

    let module = |path: &str| g.modules.iter().find(|m| m.canonical_path == path);
    assert_eq!(
        module("sample-cpp::mathx").map(|m| m.kind),
        Some(ModuleKind::Mod)
    );
    assert_eq!(
        module("sample-cpp::app").map(|m| m.kind),
        Some(ModuleKind::Mod)
    );
    // A C++ class becomes a type container.
    assert_eq!(
        module("sample-cpp::mathx::Calc").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
}

#[test]
fn lowers_cpp_functions_and_edges() {
    let g = lower_path(&index()).expect("lower");
    let item = |suffix: &str| g.items.iter().find(|i| i.canonical_path.ends_with(suffix));

    // Free functions in a namespace lower to plain functions.
    assert_eq!(item("mathx::add").map(|i| i.kind), Some(ItemKind::Function));
    assert_eq!(
        item("mathx::doubleValue").map(|i| i.kind),
        Some(ItemKind::Function)
    );
    assert_eq!(item("app::greet").map(|i| i.kind), Some(ItemKind::Function));
    // A method on a class lowers to an associated function.
    assert_eq!(item("Calc::run").map(|i| i.kind), Some(ItemKind::AssocFn));

    let edge = |from: &str, to: &str| {
        g.edges.iter().any(|e| {
            g.item(e.from).canonical_path.ends_with(from)
                && g.item(e.to).canonical_path.ends_with(to)
        })
    };
    // Within-namespace call (recovered through the no-enclosing-range fallback).
    assert!(
        edge("mathx::add", "mathx::doubleValue"),
        "add -> doubleValue"
    );
    // Cross-namespace call: app::greet -> mathx::add.
    assert!(edge("app::greet", "mathx::add"), "greet -> add");
    // Global main calls into the app namespace.
    assert!(edge("::main", "app::greet"), "main -> greet");
    assert!(
        g.edges
            .iter()
            .all(|e| matches!(e.kind, RefKind::Body | RefKind::Import)),
        "SCIP edges are Body or Import"
    );
}

#[test]
fn cpp_graph_analyzes() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let g = lower_path(&index()).expect("lower");
    let result = analyze(&g, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample-cpp");
    assert!(result.n_real_items > 0);
}
