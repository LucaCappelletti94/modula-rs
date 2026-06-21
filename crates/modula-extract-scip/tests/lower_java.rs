//! Lowering test against a committed `scip-java` index.
//!
//! The index was produced once by running `scip-java index` over
//! `tests/fixtures/sample-java` in a `maven`/JDK container (scip-java rides the
//! project's build tool, here Maven) and committed, so the test is deterministic
//! and needs no JVM toolchain at test time. Regenerate it the same way if the
//! fixture changes. scip-java emits each Java package segment as its own
//! `Namespace` descriptor, so the package tree nests (`com::example::mathx`).

use std::path::PathBuf;

use modula_extract_scip::lower_path;
use modula_ir::{ItemKind, ModuleKind, RefKind};

fn index() -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "sample-java",
        "index.scip",
    ]
    .iter()
    .collect()
}

#[test]
fn lowers_java_packages_and_classes() {
    let g = lower_path(&index()).expect("lower");
    assert_eq!(g.krate(g.root_crate).name, "sample-java");

    let module = |path: &str| g.modules.iter().find(|m| m.canonical_path == path);
    // Nested package namespaces become plain modules.
    assert_eq!(
        module("sample-java::com::example").map(|m| m.kind),
        Some(ModuleKind::Mod)
    );
    assert_eq!(
        module("sample-java::com::example::mathx").map(|m| m.kind),
        Some(ModuleKind::Mod)
    );
    // Classes become type containers.
    assert_eq!(
        module("sample-java::com::example::mathx::Mathx").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
    assert_eq!(
        module("sample-java::com::example::mathx::Calc").map(|m| m.kind),
        Some(ModuleKind::Type)
    );
}

#[test]
fn lowers_java_methods_and_edges() {
    let g = lower_path(&index()).expect("lower");
    let item = |suffix: &str| g.items.iter().find(|i| i.canonical_path.ends_with(suffix));

    // Methods on a class lower to associated functions.
    assert_eq!(item("Mathx::add").map(|i| i.kind), Some(ItemKind::AssocFn));
    assert_eq!(
        item("Mathx::doubleValue").map(|i| i.kind),
        Some(ItemKind::AssocFn)
    );
    assert_eq!(item("Calc::run").map(|i| i.kind), Some(ItemKind::AssocFn));
    assert_eq!(item("Main::greet").map(|i| i.kind), Some(ItemKind::AssocFn));

    let edge = |from: &str, to: &str| {
        g.edges.iter().any(|e| {
            g.item(e.from).canonical_path.ends_with(from)
                && g.item(e.to).canonical_path.ends_with(to)
        })
    };
    // Within-class call inside the mathx package.
    assert!(
        edge("Mathx::add", "Mathx::doubleValue"),
        "add -> doubleValue"
    );
    // Cross-package call: com.example.Main -> com.example.mathx.Mathx.add.
    assert!(edge("Main::greet", "Mathx::add"), "greet -> add");
    assert!(
        g.edges
            .iter()
            .all(|e| matches!(e.kind, RefKind::Body | RefKind::Import)),
        "SCIP edges are Body or Import"
    );
}

#[test]
fn java_graph_analyzes() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let g = lower_path(&index()).expect("lower");
    let result = analyze(&g, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample-java");
    assert!(result.n_real_items > 0);
}
