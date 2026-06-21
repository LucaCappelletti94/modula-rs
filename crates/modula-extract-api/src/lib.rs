//! Language-neutral extraction seam for modula-rs.
//!
//! `modula-metrics` only ever sees a [`modula_ir::CrateGraph`], so supporting a
//! new language means adding a new producer of that IR. This crate is the thin,
//! rust-analyzer-free seam every producer shares, in three layers:
//!
//! - [`CrateGraphBuilder`] assembles a valid `CrateGraph` (dense ids, module
//!   stubs, type containers, derived paths and depths, deduplicated edges,
//!   public-API computation), so each extractor does not re-implement the
//!   invariant-laden bookkeeping.
//! - [`Frontend`] is what each language implements: it detects a project and
//!   populates a builder. A blanket impl turns any `Frontend` into an
//!   [`Extractor`].
//! - [`Extractor`] plus [`Registry`] are the object-safe dispatch boundary, so
//!   the CLI, corpus, and web flows select an extractor by detection or language.
//!
//! The Rust extractor (`modula-extract`) implements [`Extractor`] directly,
//! since rust-analyzer already hands it a fully resolved model. New languages use
//! the [`Frontend`] plus [`CrateGraphBuilder`] path.

#![forbid(unsafe_code)]

mod builder;

use std::path::{Path, PathBuf};

use modula_ir::CrateGraph;

pub use builder::CrateGraphBuilder;

/// A source language modula can analyze, as a type-level marker.
///
/// Each language is its own zero-sized type implementing this trait, so a
/// [`Frontend`] can be generic over its language and carry per-language
/// associated data. The object-safe [`Extractor`] surfaces [`Language::ID`] as a
/// runtime tag for detection, dispatch, and persistence. A new language may
/// define its own marker in its own crate, with no central enum to edit.
pub trait Language {
    /// A stable, lowercase identifier (for example `"typescript"`), used as the
    /// runtime tag in the registry, the IR provenance, and persisted results.
    const ID: &'static str;
}

/// Rust (rust-analyzer backed).
pub struct Rust;
impl Language for Rust {
    const ID: &'static str = "rust";
}

/// JavaScript.
pub struct JavaScript;
impl Language for JavaScript {
    const ID: &'static str = "javascript";
}

/// TypeScript.
pub struct TypeScript;
impl Language for TypeScript {
    const ID: &'static str = "typescript";
}

/// Python.
pub struct Python;
impl Language for Python {
    const ID: &'static str = "python";
}

/// Language-neutral knobs for an extraction. Language-specific configuration
/// (a `tsconfig.json`, a `Cargo.toml`) is read from the project itself.
#[derive(Clone, Debug, Default)]
pub struct ExtractOptions {
    /// Whether to include items from dependencies as analysis boundaries. Not
    /// every extractor honors this.
    pub include_dependencies: bool,
    /// Analyze a specific package or member of a multi-package project by name,
    /// rather than the default.
    pub member: Option<String>,
    /// Analyze every package or member of a multi-package project.
    pub all_members: bool,
}

/// A request to extract IR from a project on disk.
#[derive(Clone, Debug)]
pub struct ExtractRequest {
    /// The project root (a directory, or a manifest file for some languages).
    pub root: PathBuf,
    /// The language id to use (see [`Language::ID`]), or `None` to auto-detect
    /// via the registry.
    pub language: Option<String>,
    /// Language-neutral options.
    pub options: ExtractOptions,
}

impl ExtractRequest {
    /// A request for `root` with auto-detection and default options.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            language: None,
            options: ExtractOptions::default(),
        }
    }

    /// Pins the language at the type level, skipping detection.
    #[must_use]
    pub fn with_language<L: Language>(mut self) -> Self {
        self.language = Some(L::ID.to_owned());
        self
    }

    /// Pins the language by its runtime id (for example from a CLI flag).
    #[must_use]
    pub fn with_language_id(mut self, id: impl Into<String>) -> Self {
        self.language = Some(id.into());
        self
    }
}

/// A per-language extractor implemented by populating a [`CrateGraphBuilder`].
///
/// This is the ergonomic path for new languages: parse and resolve however the
/// language's tooling allows, then describe the result to the builder. The
/// blanket [`Extractor`] impl handles the rest.
pub trait Frontend {
    /// The language this frontend handles, as a type-level marker.
    type Lang: Language;

    /// A cheap check of whether `root` looks like a project this frontend can
    /// analyze (for example, the presence of a `tsconfig.json`).
    fn detect(&self, root: &Path) -> bool;

    /// The tool and version used, recorded as IR provenance. Empty by default.
    fn tool_version(&self) -> String {
        String::new()
    }

    /// Populates the builder with this project's modules, items, and edges.
    fn populate(&self, req: &ExtractRequest, builder: &mut CrateGraphBuilder)
    -> anyhow::Result<()>;
}

/// Produces a [`CrateGraph`] for a project. The object-safe dispatch boundary
/// used by the registry, the CLI, the corpus, and the web flows.
pub trait Extractor {
    /// The runtime id of the language this extractor handles (see
    /// [`Language::ID`]).
    fn language_id(&self) -> &'static str;
    /// A cheap check of whether `root` is a project this extractor can analyze.
    fn detect(&self, root: &Path) -> bool;
    /// Extracts the IR for the project described by `req`.
    fn extract(&self, req: &ExtractRequest) -> anyhow::Result<CrateGraph>;
}

impl<F: Frontend> Extractor for F {
    fn language_id(&self) -> &'static str {
        <F::Lang as Language>::ID
    }

    fn detect(&self, root: &Path) -> bool {
        Frontend::detect(self, root)
    }

    fn extract(&self, req: &ExtractRequest) -> anyhow::Result<CrateGraph> {
        let mut builder = CrateGraphBuilder::new(self.tool_version());
        self.populate(req, &mut builder)?;
        builder.finish()
    }
}

/// A set of available extractors, for detection and dispatch.
#[derive(Default)]
pub struct Registry {
    extractors: Vec<Box<dyn Extractor>>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an extractor.
    pub fn register(&mut self, extractor: Box<dyn Extractor>) -> &mut Self {
        self.extractors.push(extractor);
        self
    }

    /// The registered extractor for a language id (see [`Language::ID`]), if any.
    #[must_use]
    pub fn for_language(&self, id: &str) -> Option<&dyn Extractor> {
        self.extractors
            .iter()
            .map(AsRef::as_ref)
            .find(|e| e.language_id() == id)
    }

    /// The first registered extractor that recognizes `root`, if any.
    #[must_use]
    pub fn detect(&self, root: &Path) -> Option<&dyn Extractor> {
        self.extractors
            .iter()
            .map(AsRef::as_ref)
            .find(|e| e.detect(root))
    }

    /// Extracts `req`, using its pinned language or, failing that, detection.
    pub fn extract(&self, req: &ExtractRequest) -> anyhow::Result<CrateGraph> {
        let extractor = match &req.language {
            Some(id) => self
                .for_language(id)
                .ok_or_else(|| anyhow::anyhow!("no extractor registered for language {id:?}"))?,
            None => self.detect(&req.root).ok_or_else(|| {
                anyhow::anyhow!(
                    "could not detect a supported language at {}",
                    req.root.display()
                )
            })?,
        };
        extractor.extract(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modula_ir::{ItemKind, Visibility};

    /// A trivial frontend that ignores the filesystem and emits a fixed graph,
    /// to exercise the blanket impl and the registry without real tooling.
    struct DummyFrontend;

    impl Frontend for DummyFrontend {
        type Lang = TypeScript;
        fn detect(&self, root: &Path) -> bool {
            root.ends_with("ts-project")
        }
        fn tool_version(&self) -> String {
            "dummy 0".to_owned()
        }
        fn populate(&self, _req: &ExtractRequest, b: &mut CrateGraphBuilder) -> anyhow::Result<()> {
            let (_c, root) = b.add_crate("app", true, "app");
            let m = b.add_module(root, "app::util", "util", Visibility::Public);
            b.add_item(
                m,
                "app::util::helper",
                "helper",
                ItemKind::Function,
                Visibility::Public,
            );
            Ok(())
        }
    }

    #[test]
    fn frontend_extracts_via_blanket_impl_and_analyzes() {
        let req = ExtractRequest::new("/tmp/ts-project").with_language::<TypeScript>();
        let graph = DummyFrontend.extract(&req).expect("extract");
        assert_eq!(graph.ra_version, "dummy 0");
        // The metric layer must accept a builder-produced graph.
        let result = modula_metrics::analysis::analyze(
            &graph,
            &modula_metrics::analysis::AnalysisConfig::default(),
        )
        .expect("analyze");
        assert_eq!(result.crate_name, "app");
    }

    #[test]
    fn registry_dispatches_by_language_and_detection() {
        let mut registry = Registry::new();
        registry.register(Box::new(DummyFrontend));

        let by_language = registry
            .extract(&ExtractRequest::new("/anywhere").with_language::<TypeScript>())
            .expect("by language");
        assert_eq!(by_language.krate(by_language.root_crate).name, "app");

        let by_detection = registry
            .extract(&ExtractRequest::new("/tmp/ts-project"))
            .expect("by detection");
        assert_eq!(by_detection.krate(by_detection.root_crate).name, "app");

        assert!(registry.for_language(Python::ID).is_none());
        assert!(registry.detect(Path::new("/tmp/other")).is_none());
    }
}
