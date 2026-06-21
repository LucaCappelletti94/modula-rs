//! Walking-skeleton tests: the TypeScript extractor builds a module tree with
//! exported and module-local items, and the result analyzes.

use std::path::PathBuf;

use modula_extract_api::{ExtractRequest, Extractor};
use modula_extract_ts::TsExtractor;

fn fixture() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "sample"]
        .iter()
        .collect()
}

#[test]
fn builds_module_tree_and_items() {
    let graph = TsExtractor
        .extract(&ExtractRequest::new(fixture()))
        .expect("extract");

    assert_eq!(graph.krate(graph.root_crate).name, "sample");

    let module_paths: Vec<&str> = graph
        .modules
        .iter()
        .map(|m| m.canonical_path.as_str())
        .collect();
    assert!(module_paths.contains(&"sample"), "{module_paths:?}");
    assert!(
        module_paths.iter().any(|p| p.ends_with("::index")),
        "{module_paths:?}"
    );
    assert!(
        module_paths.iter().any(|p| p.ends_with("sample::util")),
        "{module_paths:?}"
    );
    assert!(
        module_paths.iter().any(|p| p.ends_with("::math")),
        "{module_paths:?}"
    );

    let item = |suffix: &str| {
        graph
            .items
            .iter()
            .find(|i| i.canonical_path.ends_with(suffix))
            .unwrap_or_else(|| panic!("missing item {suffix}"))
    };
    assert!(
        item("::greet").reachable_pub_api,
        "exported greet is public"
    );
    assert!(
        !item("::Internal").reachable_pub_api,
        "non-exported class is module-local"
    );
    assert!(item("::add").reachable_pub_api);
    assert!(item("::Calc").reachable_pub_api);
}

#[test]
fn records_import_edges_between_modules() {
    use modula_ir::{ItemId, RefKind};

    let graph = TsExtractor
        .extract(&ExtractRequest::new(fixture()))
        .expect("extract");
    let module_path = |item: ItemId| {
        graph
            .module(graph.item(item).owning_module)
            .canonical_path
            .clone()
    };
    let has_edge = graph.edges.iter().any(|e| {
        e.kind == RefKind::Import
            && module_path(e.from).ends_with("::index")
            && module_path(e.to).ends_with("::math")
    });
    assert!(has_edge, "expected an import edge from index to util/math");
}

#[test]
fn records_within_file_reference_edges() {
    use modula_ir::{ItemId, RefKind};

    let graph = TsExtractor
        .extract(&ExtractRequest::new(fixture()))
        .expect("extract");
    let path = |item: ItemId| graph.item(item).canonical_path.clone();
    let has_edge = graph.edges.iter().any(|e| {
        e.kind == RefKind::Body
            && path(e.from).ends_with("::add")
            && path(e.to).ends_with("::double")
    });
    assert!(
        has_edge,
        "expected a body edge add -> double within util/math"
    );
}

#[test]
fn analyzes_without_panicking() {
    use modula_metrics::analysis::{AnalysisConfig, analyze};

    let graph = TsExtractor
        .extract(&ExtractRequest::new(fixture()))
        .expect("extract");
    let result = analyze(&graph, &AnalysisConfig::default()).expect("analyze");
    assert_eq!(result.crate_name, "sample");
}

#[test]
fn detects_a_typescript_project() {
    assert!(TsExtractor.detect(&fixture()));
}
