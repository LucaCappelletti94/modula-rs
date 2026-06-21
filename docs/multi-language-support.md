# Multi-language support for modula

## Context and goal

modula scores only Rust today, which caps the audience sharply. The goal is to analyze other languages by producing the same `modula_ir::CrateGraph`, so the metrics, findings, report, and the planned web service all apply unchanged. `modula-metrics` is language-agnostic: it only ever sees a `CrateGraph` (a module tree, items carrying visibility and an owning module, and directed dependency edges). Rust support is just one producer of that IR, the `RaExtractor` in `modula-extract`. Adding a language means adding another producer of the same IR.

## Strategy: SCIP for non-Rust languages, rust-analyzer for Rust

A survey of the tooling concluded that the only scalable way to get type-resolved graphs across many languages is SCIP (Sourcegraph's protobuf code-index format): write one SCIP-to-`CrateGraph` lowering in Rust, and rely on each language's existing indexer, which rides that language's real compiler, to produce the index. One lowering then covers every language that has an indexer (`scip-typescript`, `scip-python`, `scip-go`, `scip-java`, a Roslyn-based C# indexer, `scip-clang`).

Rust stays on its bespoke rust-analyzer extractor, because SCIP is verifiably lossy for Rust. We confirmed on real `rust-analyzer scip` output that SCIP encodes the module and type hierarchy in symbol descriptors and attributes references to a containing definition via `enclosing_range`, but the SCIP schema has no visibility field (only `is_global_symbol` versus `is_local_symbol`) and no reference-kind taxonomy. For Rust that drops declared visibility (which the encapsulation term depends on) and the `RefKind` distinction the bespoke extractor reads straight from the HIR, and gains nothing. For languages with no bespoke extractor, SCIP is the best available and is type-resolved.

An earlier idea, in-Rust extractors via `oxc` (TypeScript) and `ruff` (Python), is parked. Those tools do not compute types, so they cannot resolve member access (`obj.method()`), which is where type-resolved graphs differ from parse-level ones. The parked `oxc` walking skeleton remains at `crates/modula-extract-ts` as superseded, not developed further.

## Deployment model: client computes, server stores

All extraction and scoring run in the user's environment. `cargo-modula`, in CI or locally, produces the IR: rust-analyzer for Rust, and for other languages a SCIP indexer (run where the project is already built) whose index is lowered into the IR. The user uploads the finished IR. The server only stores it and serves the UI, badge, and share link, and the web viewer scores an uploaded IR in the browser via wasm. There is no server-side indexing, building, or analysis. The build-free case is simply uploading a pre-made IR to the viewer, not handing source to the server.

`cargo-modula` obtains indexers without ad-hoc global installs and never bootstraps base toolchains. The intended priority order is: use an indexer already on `PATH`, else fetch-and-run via the ecosystem runner (`npx`, `uvx`, or a pinned downloaded binary), else accept a prebuilt `--scip` index. This mirrors how Codecov ships a thin uploader plus an official CI action, and how pre-commit pins isolated tool runs.

## The SCIP lowering (shipped)

`crates/modula-extract-scip` lowers a `.scip` index into a `CrateGraph` through the shared `CrateGraphBuilder`. The mapping, validated against real `scip-typescript` output:

- Each SCIP symbol carries descriptors encoding its hierarchy. A `Namespace` (or `Package`) descriptor becomes a `ModuleKind::Mod` module, a `Type` becomes a `ModuleKind::Type` container, and `Method` / `Term` / `Macro` become items. Intermediate namespaces the indexer does not emit on their own (for example `src` and `util` when only the full file path `src/util/math.ts` is emitted) are created on demand by `ensure_module`. SCIP symbol strings are the stable keys the builder expects, and the builder's duplicate-key guard catches a malformed lowering.
- A definition occurrence carries an `enclosing_range`, so each reference occurrence is attributed to the innermost definition whose range contains it (the edge `from`), with the referenced symbol as the `to`. References to parameters, locals, or external symbols resolve to keys that were never added as items and are dropped at `finish`, as are self-edges.

Two honest caveats versus a bespoke extractor: SCIP has no visibility field, so everything lowers as `Public` (the encapsulation term is therefore weaker on SCIP languages), and SCIP has no reference-kind taxonomy, so edges collapse to `Import` (when the occurrence has the import role) or `Body`.

`cargo modula --scip <file>` lowers, scores, and reports from a prebuilt index, which is exactly what the upload-and-view flow needs.

## Phased deliverables

1. Done. Pure-Rust SCIP lowering plus `--scip` ingest, proven on TypeScript (`crates/modula-extract-scip`, validated by a committed `scip-typescript` index in `tests/fixtures/sample-ts`).
2. The `ScipIndexer` descriptor and fetch-and-run orchestration in `cargo-modula` for TypeScript: detect-on-`PATH`, else `npx @sourcegraph/scip-typescript@<pinned>`, run it on the project, then lower. Local `cargo modula <ts-project>` works end to end when Node is present.
3. Python via `scip-python` (`uvx` or pipx run), reusing the lowering and the `ScipIndexer` plus dialect traits factored out of the TypeScript case.
4. Go, then JVM, then C# and C/C++, each a thin `ScipIndexer` plus dialect on the shared lowering (`scip-go`, `scip-java`, a Roslyn-based indexer, `scip-clang`).
5. An official GitHub Action per language (Codecov/CodeQL style) that installs and runs the indexer and uploads the result, so CI users do not assemble the steps by hand.

The `ScipIndexer` descriptor (tool name, detect-on-`PATH`, the pinned fetch-and-run command, install hint) and a per-indexer dialect (how the global-versus-local flag maps to visibility for that indexer, and any descriptor quirks) are factored out of the first concrete TypeScript case rather than designed up front.

## Critical files

- `crates/modula-extract-scip/` (shipped): the SCIP lowering, the fixture, and the tests.
- `crates/modula-extract-api/`: the reused `CrateGraphBuilder` (key-based `add_crate`/`add_module`/`add_type`/`add_item`/`add_edge`/`finish`), the `Extractor`/`Registry` seam, and the `Language` markers.
- `crates/cargo-modula/src/main.rs`: the `--scip` ingest (shipped), and the indexer orchestration in phase 2.
- `crates/modula-extract/src/ra.rs`: unchanged, Rust stays bespoke.
- `crates/modula-extract-ts/`: parked.
- The `scip` crate (`scip::types::Index`, `scip::symbol::{parse_symbol, is_global_symbol}`), and `rust-analyzer scip` only as a validation aid, not a shipping path.

## Verification

- Lowering correctness: the committed `.scip` tests assert the nested module chain (including the synthesized intermediates), the `Type` containers, the item kinds, and both the within-file (`add -> double`) and cross-file (`greet -> add`) type-resolved edges, plus that `analyze` succeeds on the lowered graph. The fixture index is regenerated with `npx @sourcegraph/scip-typescript index` in `tests/fixtures/sample-ts` if the source changes, so tests stay network-free.
- Optional quantified Rust loss: run `rust-analyzer scip` over a corpus sample, lower it, and diff modules, items, edges, and scores against the bespoke extractor to put numbers on what SCIP drops for Rust (expected: visibility and reference-kind detail). This confirms the keep-Rust-bespoke decision rather than changing it.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace` stay green at each phase.

## Risks and open questions

- SCIP carries no visibility, so the encapsulation term is degraded on SCIP languages. The per-indexer dialect can refine this where an indexer's global-versus-local flag tracks export-ness, though `scip-typescript` 0.3 marks non-exported symbols global, so it gives no usable signal there.
- SCIP indexing needs a buildable, dependency-installed project, so it runs in CI or locally, never on the server.
- C# and C/C++ indexers were not yet exercised; confirm their output and visibility behavior when those phases land.
