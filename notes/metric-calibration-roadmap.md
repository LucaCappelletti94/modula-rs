# modula-rs metric calibration roadmap

A roadmap derived from the 19,051-crate corpus sweep. The histograms showed prominent spikes; the investigation (read-only, on `/mnt/nvme/modula-corpus/sweep.db`) traced every spike to one of **three problem clusters**, plus confirmed that the divergence term is sound (see the closing note). This document states, per problem: the **meaning** we assign the metric, the **multiphase fix**, and how we **measure** that the fix worked. We implement one problem at a time.

Shared baseline for all measurements: the current `sweep.db` (19,051 ok crates, >=100k downloads) and `corpus-v1.db` are the before snapshots on the NVMe; each fix is judged by re-running a sample (`tools/corpus/sweep.py --limit N` into a fresh DB) plus `socket2`/`smallvec`/`futures` as named probes, and re-plotting (`tools/corpus/plot.py`). Spikes that are real signal must NOT move; targeted spikes must redistribute.

---

## Problem A -- Vacuous scores for "no measurable structure" crates

### Meaning
A crate (or a single term at a given depth) has **no measurable internal structure** when it is a single module, a pure re-export facade (its only items are module-stub nodes), or otherwise yields no non-trivial partition and no detectable communities. For such inputs a quality term is **undefined**, not "perfect". Today the code invents a maximal value instead:
- `divergence_term` `.map_or(1.0, ..)` when there is no primary depth (`score.rs`).
- AMI is a genuine `1.0` when both declared and detected partitions are trivially one-item-per-community (no structure), e.g. `futures`.
- `n_items` for a facade is dominated by `ItemKind::Module` **stub nodes** (added for import-edge resolution), so the item-level metrics run on module stubs.

Result: `headline==1.0` (6.9%) and `divergence==1.0` (9.8%) are mostly crates that have nothing to score. The semantic we want: **report N/A** (and exclude such crates from calibration / distribution stats), not a vacuous 1.0.

### Spikes addressed
#1 divergence==1.0, #2 headline==1.0, the trivial-AMI 1.0, module-stub pollution.

### Multiphase fix
1. **Exclude module-stub items from the item-level metrics.** Filter `ItemKind::Module` items out of the item graph used by `modularity::profiles` / `build_item_graphs` (and the partitions), so a pure facade is correctly seen as having ~0 real items. Touchpoints: `crates/modula-metrics/src/graph.rs` (`build_item_graphs`/`directed_arcs`), `crates/modula-ir/src/lib.rs` (`partition_at_depth`), or a shared "real item" predicate on `Item`.
2. **Make divergence (and confirm modularity) return `Option<f64> = None`** when undefined: no primary depth, or `q_detected <= 0` (trivial detected partition -> AMI is meaningless). Drop the `.map_or(1.0, ..)` in `score.rs`. `weighted` already skips `None` and renormalizes.
3. **Crate-level N/A verdict.** Renormalizing is not enough (a structureless crate still has acyclicity=1 + encapsulation=1 -> headline ~1.0). Add an explicit "insufficient structure" outcome: when the crate has no detectable communities at any depth (modularity AND divergence both `None`), report the headline as **N/A** (and the JSON/report flags it), rather than a number. Decision to settle when implementing: the exact threshold (both-None vs a `>=2` real-module / `>=k` real-item gate).
4. **Corpus tooling**: add an `n_real_items` / `na` column to the sweep schema so N/A crates are excluded from distribution stats.

### Measurement
- `futures` and other facades -> reported N/A (not 1.0).
- The `headline==1.0` and `divergence==1.0` spikes shrink to ~0; a new N/A bucket appears whose size we quantify.
- Real-structure crates' headlines are unchanged (within float noise).

### Result (implemented)
Shipped all four phases: module-stub items excluded from the item graph/partition (`graph_item_ids`/`partition_of_nodes`/`n_real_items` in the IR; compact node space in `graph.rs`); `divergence_term`, `headline`, and `headline_depth_averaged` are now `Option` (`None` = N/A when no module-tree depth yields more than one declared community); CI `min_headline` gate passes vacuously on N/A; `n_real_items` added to the JSON and the sweep schema. Validated on a 235-crate sample (`sweep-a.db`, import=0 + Problem A) against `sweep-b.db` (import=0, no A):
- **N/A bucket = 10%** (23/235): 11 pure re-export facades (`n_real_items == 0`, e.g. `futures` reports N/A instead of a vacuous ~1.0) plus 12 crates with real items but no non-trivial declared partition at any depth.
- **Vacuous spikes fully collapsed**: `headline >= 0.999` 8% -> **0%**, `divergence >= 0.999` 8% -> **0%**. Every crate in those spikes (16 of each) is now N/A, i.e. the entire high-end was structureless crates.
- **Real-structure crates unchanged**: of 212 crates defined in both, **median |delta headline| = 0.0038** (float noise); only 16 moved by >0.02, all small, legitimate refinements from dropping stub nodes (e.g. `either` 0.244 -> 0.331). Median headline 0.450 -> 0.444 (only because the vacuous high-end left the distribution).

Final corpus-wide re-sweep and re-plot deferred until after Problem C, to avoid re-running the 19k sweep between every problem.

---

## Problem B -- `Import`/re-export edges manufacture false coupling and cycles

### Meaning
An `Import` edge (`use`, and especially `pub use` re-exports) is an **API-surface/namespace** relationship, not implementation coupling. Real usage is already captured by `Signature`/`Body`/`Impl`/`TraitBound` edges. Counting `Import` at weight 1.0 makes the idiomatic re-export facade (`lib.rs` re-exports its submodules while they reference crate-level types) collapse a crate into one SCC, so well-architected crates like `socket2` read as maximally cyclic.

### Spikes addressed
#3 acyclicity low cluster (0.0 + the `1 - k/n` fractions); also helps the module-stub pollution from Problem A.

### Multiphase fix
1. **Zero the default `Import` weight** (`RefKindWeights::default().import = 0.0`) in `crates/modula-metrics/src/weighting.rs`. `ModuleAggregation::build` already drops `w <= 0` edges, so this removes `Import` from the coupling + cycle graph (and the item modularity graph) in one change.
2. **Validate the coupling/cycle graph still reflects real dependencies**: confirm genuinely mutually-recursive modules (real `Body`/`Signature` cycles) are still flagged; only facade-induced cycles disappear.
3. **Decide the final policy** once measured: hard zero, a small residual weight, or distinguishing re-export Import edges from plain imports (we already track re-exports separately in the extractor). Start with hard zero.

### Measurement
- `socket2` becomes acyclic (acyclicity 1.0).
- The acyclicity histogram de-bimodalizes: the 0.0/`1-k/n` low cluster shrinks, the empty 0.9-1.0 middle fills in; the genuine-cycle tail remains.
- Spot-check a handful of crates that SHOULD have real module cycles (mutual non-import references) still score < 1.0.

### Result (implemented: hard zero)
Shipped phase 1 (`import = 0.0`). Phase 3 decision: **hard zero** (the data did not justify a residual). Validated on a 498-crate sample (`sweep-b.db` vs `sweep.db`, same crates):
- Acyclicity de-bimodalized exactly as predicted: `<0.5` cluster halved (38% -> 18%), the empty middle filled (`0.5-1.0` 19% -> 29%), fully-acyclic rose (43% -> 53%), `==0` shrank (7% -> 3%), median 0.667 -> 1.000.
- **Zero regressions**: no crate's acyclicity decreased. Fully-facade crates (`utf-8`, `rustc-hash`, `heck`, `num-integer`, `radium`, `ucd-trie`, `hashlink`, `plotters-svg`) went 0.0 -> 1.0; genuinely-coupled crates kept a real residual (`socket2` 0.0 -> 0.20, retaining a true 4-module submodule cycle).
- No collateral damage: headline median 0.418 -> 0.454 (facade crates correctly score higher), modularity 0.402 -> 0.414 (import noise removed), divergence 0.310 -> 0.299 (negligible), encapsulation unchanged.

---

## Problem C -- The type level distorts leak-depth and modularity for flat crates

### Meaning
The type level (per-type container modules) was added to give single-`mod` crates a non-trivial partition for the divergence/efficiency **depth sweep** (it fixed the single-module vacuous-high). But two metrics were not designed for type-container scopes:
- **Leak-depth (#4):** type containers share only the crate root, whose Lin information content is 0, so any cross-type edge in a flat crate reads as a **max-cost leak**. 86% of flat-but-typed crates hit `mean_leak_cost==1.0` (vs 0% of flat crates with no types). The type level turned former intra-module references into max-cost "leaks".
- **Primary modularity (#5):** the per-type partition is **anti-modular** for type-dense flat crates (26% have `modularity_term==0` vs 5% of real multi-module crates).

Meaning we assign: the type level should **inform the depth-sweep only** (where it earned its keep), and **must not** make leak-depth / the primary-modularity term treat two types in one module as maximally distant or anti-modular. Concretely we saw the type level swing tiny crates from vacuous-HIGH (e.g. `smallvec` 0.886) to artifact-LOW (`smallvec` 0.388 with encapsulation 0.13, `scopeguard` modularity 0.02) -- a different wrong answer, not a right one.

### Spikes addressed
#4 mean_leak_cost==1.0, #5 modularity_term==0.

### Multiphase fix (options to test, in order of preference)
1. **Leak-depth on the real-module tree.** Change `encapsulation::leaks` to map `owning_module` through `ir.real_module(..)` before computing `lin(src, dst)`. Then two types in one module roll up to the same real module -> intra -> not a leak; only genuine cross-`mod` leaks count. Touchpoint: `crates/modula-metrics/src/encapsulation.rs`.
2. **Primary modularity term granularity.** Decide whether the primary depth for the modularity *term* should prefer the real-module depth, so an anti-modular per-type partition does not become the headline's modularity. Options: (i) compute the term at the deepest *real-module* depth; (ii) keep the type depth but report `modularity==0` honestly. Needs a small study (re-check after Problems A+B land, since some of #5 may be import/stub pollution A+B remove).
3. **Reassess net value of the type level** after 1-2: confirm flat crates now get *meaningful* component terms, not artifact-low ones, and that the original single-module fix still holds.

### Measurement
- `mean_leak_cost==1.0` spike collapses toward the real-module-tree rate (~9%).
- `smallvec`/`scopeguard`/`strsim` encapsulation and modularity terms become defensible; their headlines stay sensible and do NOT revert to vacuous 1.0.
- Re-run the corpus; the flat-but-typed cohort's term distributions normalize.

---

## Sequencing and interactions

Recommended order, because of dependencies:
1. **Problem B (drop `Import` weight)** -- smallest change, clear win, and it makes module-stub nodes edgeless, which simplifies Problem A.
2. **Problem A (N/A for no structure + stub exclusion)** -- highest impact on the distribution; best done right after B since they share the stub/import modeling.
3. **Problem C (type level vs leak/modularity)** -- subtlest; re-measure #5 *after* A+B (some of the anti-modular signal may be import/stub pollution that A and B remove), then decide phase 2.

Each problem is independently shippable and measurable; we re-plot and re-run a corpus sample between problems so effects don't get conflated.

---

## Closing note: divergence is sound, do not "fix" it

On the 10,070 crates with genuine structure, divergence has a healthy spread (median 0.35), is monotonic with modularity (median modularity 0.14 -> 0.33 -> 0.45 -> 0.60 across divergence buckets), and is independent of acyclicity/encapsulation. Its `==0`/`==1` spikes are the no-structure artifacts handled by Problem A, not a flaw in the term. The only open calibration nuance is its mild overlap with modularity (r=+0.49); revisit weights after A/B/C, not before.
