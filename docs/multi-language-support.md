# Multi-language support for modula

## Context and goal

modula currently scores only Rust crates. Supporting Rust alone caps the potential user base sharply, so the goal is to let modula analyze code written in other languages. The same score, findings, report, and the planned hosted service then apply to any supported language.

This is tractable because of where the seam already sits. `modula-metrics` is language-agnostic: it only ever sees a `modula_ir::CrateGraph` (a module tree, items carrying visibility and an owning module, and directed dependency edges). Rust support is just one producer of that IR, the `RaExtractor` in `modula-extract`, which is the only crate that touches rust-analyzer. Adding a language therefore means adding a new producer of the same `CrateGraph`. Scoring, findings, the report, and the future web service come along unchanged.

## Scope: Rust-native languages only

A survey of the available tooling (recorded in the project research) showed that only three languages have mature Rust-native semantic analysis: Rust, JavaScript and TypeScript (via `oxc`), and Python (via Astral's `ruff` and `ty`). These keep the whole pipeline in Rust, with no foreign toolchain, and can compute a module-level graph from source alone with no install, which suits scale and the free upload and open-source tiers.

The other candidates (Go, Java and Kotlin, C#, C and C++) have no credible Rust-native semantic resolver. Their resolution lives only in their own toolchains, reachable through external tools such as SCIP indexers or by driving the language compiler. Since every one of those paths is a non-Rust process, they are out of scope for now and deferred. They can be revisited if the "keep it in Rust" constraint is relaxed.

## Architecture

The extraction seam is `Extractor::extract(&req) -> anyhow::Result<CrateGraph>`, with `RaExtractor` and all the `ra_ap_*` dependencies isolated inside `modula-extract`. A new language is a sibling extractor crate that emits a `CrateGraph`. The downstream stack (metrics, findings, report, corpus sweep, web viewer) does not change.

The shared abstraction lives in a new lean crate `modula-extract-api` (dependencies just `modula-ir` and `anyhow`), so language extractor crates depend on it rather than on the rust-analyzer-heavy `modula-extract`. It holds three layers.

The construction helper, `CrateGraphBuilder`, owns the IR-construction bookkeeping once so every producer gets it right: dense ids (`items[i].id == ItemId(i)` and likewise for modules and crates), `canonical_path` and `depth` derived from the module tree, module-stub items for every real `mod`, synthetic `ModuleKind::Type` containers for nominal types, edge deduplication into summed weights keyed by `(from, to, kind)` with self-edges dropped, edges by stable symbol key resolved at the end (so forward and cross-file references work, and references to unknown external symbols are dropped), and a final `compute_public_api` pass. The derivation matches the compact-container invariants, so any graph the builder produces round-trips losslessly through the binary IR.

The per-language `Frontend` trait (language, project detection, and a `populate` step that fills a `CrateGraphBuilder`) is what each new language implements. A blanket `impl<F: Frontend> Extractor for F` turns any frontend into an extractor.

The dispatch seam, an object-safe `Extractor` trait plus a `Registry`, makes adding a language "register an extractor", with uniform detection and dispatch for `cargo-modula`, the corpus, and the web flows. The existing Rust path implements `Extractor` directly (its rust-analyzer pipeline already lowers a fully resolved model), and can move onto the builder later.

## Per-language tooling

| Language | Primary tool | Resolution | Visibility source |
|---|---|---|---|
| Rust | rust-analyzer (`RaExtractor`, done) | full | the `Visibility` model directly |
| JS / TS | `oxc_semantic` + `oxc_resolver` | within-file references plus cross-file import paths (no TS type inference) | `export`, member modifiers |
| Python | `ruff_python_semantic` (+ `ty` for cross-module) | per-file bindings and imports now, cross-module via `ty` | underscore convention, `__all__` |
| any (fallback) | tree-sitter | parse-only, no cross-file or type resolution | declared keywords only |

tree-sitter is the universal parse-only fallback. It cannot resolve names, imports, or types across files, so it would yield materially weaker cohesion and tangle scores. It is a last resort, not a target.

## IR mapping rules

These conventions apply to every new extractor so the metrics behave consistently across languages.

- Module tree: map the language's real namespacing onto `ModuleKind::Mod`. For file-based languages (JS/TS, Python) directories and files are modules, and nested namespace constructs are nested modules. Represent classes, interfaces, and similar nominal types as `ModuleKind::Type` containers holding their members (the builder's `add_type` helper), mirroring the Rust extractor so per-type cohesion clustering is reused.
- Items: emit one item per function, type, method, constant, and similar declaration, attributed to its owning module.
- Visibility: map the language's export or visibility model onto `Visibility`. The signal the leak and over-exposure metrics need is exported versus module-local at the boundary, so `export` or public maps to `Public` and non-exported top-level declarations to a local `Private`. Finer member-level modifiers are refined per language. The `Visibility` enum may gain language-specific variants if a model does not map cleanly.
- Edges: import statements become `Import` edges, type annotations and signatures become `Signature` edges, references in bodies become `Body` edges, and implements or extends relationships become `Impl` or `TraitBound` edges.
- Public-API reachability: the builder runs `compute_public_api` after the tree and visibilities exist.

## Cross-cutting concerns

- Language detection and dispatch in `cargo-modula` via the `Registry`, and how the corpus and web flows select an extractor.
- Testing: each extractor ships small fixture projects with snapshotted IR and scores, mirroring how the Rust path is tested, plus at least one real-world project scored and sanity-checked.

Because all supported extractors build the `CrateGraph` in process, no language-neutral interchange format is needed. The hosted web service design (Dioxus SPA, axum, Postgres with RLS, treemap, and the rest) is unaffected by which languages are supported, since it consumes the IR and the analysis result. Its decisions are recorded separately.

## Phased plan

Each language is an independent, separately shippable PR. The IR and metrics are stable, so the PRs do not conflict.

### PR 0: the abstraction crate

`modula-extract-api` with the three layers above (`CrateGraphBuilder`, `Frontend`, `Extractor` and `Registry`), the Rust path adapted onto the `Extractor` trait, and `cargo-modula` and the corpus updated to dispatch through the registry. No new language yet, this is the groundwork PR 1 builds on.

### PR 1: TypeScript and JavaScript (oxc)

The pattern-setter. New crate `modula-extract-ts` implementing `Frontend` with `oxc_parser`, `oxc_semantic`, `oxc_ast`, `oxc_resolver`, and the builder, with `cargo-modula` dispatching to it on `tsconfig.json` or a flag.

Internal steps:

1. Walking skeleton: parse a fixture project, build the directory and file module tree with exported items via the builder, run `analyze`, and get a score.
2. Import edges via `oxc_resolver`.
3. Within-file reference edges via `oxc_semantic` resolved references.
4. Type-container modules for classes and interfaces, and the visibility refinements.
5. Fixture snapshot tests for IR and scores, plus a real repo sanity check.

### PR 2: Python (ruff and ty)

New crate `modula-extract-py` implementing `Frontend` with `ruff_python_semantic` for per-file bindings, imports, and the `__all__` and underscore visibility signals, and `ty` for cross-module resolution. Confirm whether `ty` can be embedded directly at its current pre-1.0 state or must be driven over its protocol, and pin accordingly.

### Deferred: non-Rust-native languages

Go, Java and Kotlin, C#, and C and C++ are out of scope while the Rust-only constraint holds, because each requires driving its own toolchain. If revisited, the cleanest single mechanism is lowering SCIP indexes into `CrateGraph`, which would reuse the builder and add one ingestion path rather than one per language.

## Definition of done per language PR

- The extractor emits a `CrateGraph` for its fixture projects through the builder.
- `cargo-modula` detects the language and dispatches to it.
- Snapshot tests cover the IR and the scores on the fixtures.
- At least one real-world project is scored and sanity-checked.
- Format, clippy, and tests are green, and this document is updated.

## Risks and open questions

- `oxc` has no Rust-native TypeScript type inference, so body edges through typed objects are not resolved. Acceptable at module granularity, revisit if call-graph precision is needed.
- `ty` is pre-1.0, so the Python cross-module API may shift. `ruff_python_semantic` covers per-file analysis in the meantime.
- The `Visibility` enum may need language-specific variants where a model does not map onto the current Rust-shaped set.
