# GR: the Eades-Lin-Smyth feedback-arc-set heuristic

Reference notes for the planned replacement of the acyclicity term. This document collects everything needed to implement and reason about GR (the greedy linear-arrangement heuristic for the feedback arc set problem) and to turn its output into a module-level tangle metric.

## Why we need it

The current acyclicity term scores a crate by the fraction of module nodes that sit in any non-trivial strongly connected component (SCC). That measure is brittle: a single back-edge from one dual-role node (typically the crate root, which both defines shared types and runs the top-level facade or dispatcher) collapses the entire crate into one giant SCC, so a clean, layerable crate reads as maximally cyclic.

Empirically, the item-level dependency graphs of real crates are essentially always acyclic (verified on `imagesize`, `pathfinder_simd`, and even `gl`, all zero item-level cycles). The module-level cycles are therefore aggregation artifacts, not genuine mutual dependence. What we actually want to measure is not "are there cycles" but "how far is the module dependency graph from being layerable", that is, how much dependency weight you would have to remove to make it a directed acyclic graph (DAG). A pure sink (a shared `util` everyone calls but that calls nobody) contributes nothing to that distance. A genuine mutual tangle contributes a lot. That distance is the feedback arc set.

## The feedback arc set problem

Given a directed graph `G = (V, E)`, a feedback arc set (FAS) is a subset `F` of `E` whose removal makes `G` acyclic. Equivalently, the kept edges `E \ F` form a DAG, so there exists a linear ordering of `V` in which every kept edge points forward. The minimum FAS is the smallest such `F` (by edge count, or by total weight in the weighted case).

Computing the minimum FAS is NP-hard, and it stays NP-hard however it is formulated (an integer-program formulation does not change the complexity class, a solver only prunes the search). On large inputs (for example a peripheral-access crate with on the order of a thousand module nodes) an exact solver can blow up. For a 0-to-1 metric we do not need the true minimum. We need a fast, stable, monotone estimate of how tangled the graph is. That is exactly what GR provides.

Key point: GR is a heuristic, not an exact minimizer. It runs in linear time and never blows up. It does not return the smallest possible feedback set, only a good one. The metric is defined on GR's output ("feedback weight under a good linear arrangement"), not on the minimum, so NP-hardness never enters.

## The GR algorithm

Source: P. Eades, X. Lin, W. F. Smyth, "A fast and effective heuristic for the feedback arc set problem", Information Processing Letters 47 (1993), pages 319 to 323.

GR does not pick edges to cut directly. It builds a linear ordering (a vertex sequence) of `V`, and then the feedback arcs are simply the edges that point backward in that ordering. The ordering is grown from both ends:

- A **sink** (out-degree 0 in the remaining graph) can be placed at the far right with no cost, because none of its edges can point backward from the rightmost position. Peel sinks to the right.
- A **source** (in-degree 0 in the remaining graph) can be placed at the far left with no cost, because none of its edges can point backward into it. Peel sources to the left.
- When neither a source nor a sink remains, the graph still has a tangled core. Pick the vertex `u` that maximizes `delta(u) = outdeg(u) - indeg(u)` in the remaining graph and place it next at the left end. The intuition is that this keeps as many of `u`'s edges pointing forward as possible (it has more out-edges than in-edges, so forward placement saves more than it costs).

Repeat until the graph is empty. The final order is the left sequence followed by the right sequence.

### Pseudocode (unweighted)

```
GR(G):
    s_left  <- empty list   # filled left to right
    s_right <- empty list   # filled right to left (prepended)
    while G is not empty:
        while G has a sink u:                 # outdeg(u) == 0
            prepend u to s_right
            remove u from G
        while G has a source u:               # indeg(u) == 0
            append u to s_left
            remove u from G
        if G is not empty:
            u <- argmax over remaining v of (outdeg(v) - indeg(v))
            append u to s_left
            remove u from G
    order <- s_left ++ s_right                # concatenation
    return order

feedback_edges(G, order):
    pos <- position of each vertex in order
    return { (u, v) in E : pos[u] > pos[v] }  # edges pointing backward
```

Removing a vertex means deleting it and its incident edges from the working graph, which updates the in- and out-degrees of its neighbors (possibly turning them into new sources or sinks).

### Weighted variant

Our module graph carries weights (the aggregated `RefKind` edge weights from `ModuleAggregation`). The weighted generalization is the standard one used in graph drawing: use weighted degrees in the selection step,

```
delta(u) = (sum of weights of u's out-edges) - (sum of weights of u's in-edges)
```

and pick the vertex of maximum weighted `delta` in the dense core. Sources and sinks are still defined by zero out- or in-degree (zero total out- or in-weight). The feedback measure is then the total weight of backward edges, not their count.

### Determinism

Ties (multiple sinks, multiple sources, or several vertices sharing the maximum `delta`) must be broken by a fixed rule, for example smallest node id, so the ordering and therefore the metric are reproducible across runs. Without a fixed tie-break the feedback set can wobble between equally good orderings.

## Complexity and quality

- **Running time**: `O(V + E)`. The linear bound needs the right data structures (see below). A naive implementation that rescans for the max `delta` each step is `O(V * (V + E))`, which is still fine at our module-graph sizes (hundreds to low thousands of nodes), but the linear version is not much more code.
- **Quality bound**: for a connected graph with `n` vertices and `m` edges, GR leaves at most `m/2 - n/6` backward edges, equivalently it keeps an acyclic subgraph of at least `m/2 + n/6` edges. So it never removes more than about half the edges, and it does measurably better than half on graphs with many vertices. This is the unweighted guarantee from the paper. The weighted variant has no such clean closed-form bound but behaves well in practice.
- **Optimality**: none. GR is a heuristic. Two good orderings give nearly the same feedback fraction, which is all the metric needs.

### Efficient implementation (linear time)

The paper achieves `O(m + n)` by bucketing vertices by their `delta` value:

- Keep the working graph as adjacency lists with mutable in- and out-degree counters per vertex.
- Maintain bins: one bin for current sinks, one for current sources, and an array of bins indexed by `delta` value (which ranges over `-(n-1) .. (n-1)`, weighted variants need a different indexing or a priority structure) holding the remaining "core" vertices, stored in doubly linked lists so a vertex can be moved between bins in `O(1)`.
- Removing a vertex updates each neighbor's degree by one and moves it to the adjacent `delta` bin in `O(1)`, so the whole run is linear in the number of edge endpoints touched.

For the weighted variant, `delta` is real-valued, so the exact bin array does not apply. Options: bucket by rounded weighted `delta`, or use a simple max-structure. At our sizes a straightforward `O(V * (V + E))` scan or an `O(E log V)` heap is entirely acceptable, the linear structure is an optimization to reach for only if profiling demands it.

## From GR to the metric

```
1. Build the directed module graph: nodes are real modules (type-container
   modules climbed to their owning `mod`), edges are inter-module references
   aggregated and weighted by RefKind, self-loops (intra-module) dropped.
2. order <- GR(module_graph)
3. feedback_weight <- sum of weights of edges (u, v) with pos[u] > pos[v]
4. total_weight    <- sum of weights of all inter-module edges
5. tangle_fraction <- feedback_weight / total_weight        # 0 when no edges
6. acyclicity_term <- 1 - tangle_fraction                   # in [0, 1]
```

Properties this gives us:

- A clean DAG has zero backward weight under some ordering, and GR finds one, so `acyclicity_term == 1`.
- A pure sink or pure source never contributes backward weight, so widely-used foundation modules are free. This is the "penalize tangle, not usage" property we want.
- A dual-role hub (the crate root) contributes only its few back-edges (for example the dispatcher calls), so a facade crate gets a small, graded penalty instead of collapsing to zero.
- A genuine mutual tangle requires cutting heavy edges to layer, so its backward weight is large and the term is low.
- A crate with no inter-module edges (single module, or fully intra-module) has `total_weight == 0` and the term is defined as `1` (vacuously layerable), matching the current convention for crates with no module nodes.

### Diagnostic bonus

GR yields not just a number but the actual backward edge set, the specific references that prevent layering. The report can name them ("these references from `imagesize::formats` into the leaves close the cycle, move the shared types into a leaf module to break it"). That is far more actionable than reporting an opaque SCC, and it fits the score-plus-report goal of the project.

## Where it lives

- **geometric-traits**: the GR algorithm itself. Input is an abstract weighted directed graph, output is a linear vertex ordering (and, derived from it, the backward edge set). It has no knowledge of modules, items, visibility, or anything Rust-specific, so it is a generic graph algorithm that sits next to the existing SCC and modularity code and is publishable on its own.
- **modula-metrics**: builds the module digraph from the IR, calls GR, computes `tangle_fraction` and `acyclicity_term`, and renders the backward edges as the human-facing tangle diagnostic. All Rust-specific interpretation stays here.

This is the same boundary used elsewhere: generic graph algorithms (Lin similarity, information content, SCC, modularity, and now GR) live in geometric-traits, while metrics that carry Rust domain semantics (the leak metric, over-exposure) live in modula-metrics.

## Worked example: a facade crate

Consider a crate whose root `lib.rs` defines the shared types (`Error`, `Output`) and a dispatcher function that calls each feature module, while every feature module references the shared root types. The module graph has:

- feature modules -> root (each feature uses the shared types): forward if root is placed left,
- root -> feature modules (the dispatcher calls them): these become the backward edges.

GR places the leaf modules and the shared-type role naturally, leaving only the dispatcher edges pointing backward. So `feedback_weight` is just the weight of the dispatcher's calls, a small fraction of the total. The crate scores near 1 (a minor, real, levelization smell: the root and leaves mutually depend, fixable by moving types to a leaf), instead of the current near 0. The genuine monolith case (every leaf cycles through shared mutable state) keeps heavy backward weight and stays low.

## Relationship to existing diagnostics

The current `TangleReport` (SCC list, largest SCC, cyclomatic number `E - V + 1` summed over non-trivial SCCs) stays useful as raw structure reporting and as the `is_acyclic` boolean. GR replaces only the scalar acyclicity term fed into the composite score. SCC detection is also a natural precondition check: if the condensation is already acyclic, the feedback fraction is exactly zero and GR can be skipped.

## Alternatives considered

- **Exact minimum FAS** (by any method): NP-hard, can blow up, unnecessary for a metric. Rejected.
- **DFS back-edge count**: even simpler than GR, but the back-edge set depends on the DFS start and visitation order, so it is noisier and less stable. GR's degree-driven arrangement is barely more code and much more reproducible. Rejected in favor of GR.
- **Local search on top of GR** (sifting, vertex moves to reduce backward weight): a polynomial way to tighten GR's estimate if ever needed. Not worth it unless a ranking turns out to be sensitive to GR's suboptimality, which we do not expect.

## Validation plan

Before wiring GR into the Rust pipeline, prototype the feedback fraction in throwaway Python over the sample crates and confirm the re-ranking matches expectations:

- `imagesize`, `gl`, `alacritty` (today artifact-low) should rise sharply toward 1.
- `pathfinder_simd`, `postgresql_commands` (clean DAGs) should stay near 1.
- A crate with genuine cross-module mutual recursion (for example a parser whose expression and statement modules call each other) should retain a non-trivial feedback fraction. Find one in the corpus as a positive control.

Then re-sweep the full corpus and inspect how the acyclicity histogram and the headline distribution move, the artifact low cluster should drain while any genuine-tangle tail remains.

## References

- P. Eades, X. Lin, W. F. Smyth. "A fast and effective heuristic for the feedback arc set problem." Information Processing Letters 47 (1993), 319 to 323. The GR algorithm, the linear-time bucket implementation, and the `m/2 - n/6` bound.
- R. M. Karp. "Reducibility among combinatorial problems." (1972). FAS is among the original NP-hard problems.
- J. Lakos. "Large-Scale C++ Software Design." Levelization and the acyclic physical dependency principle, the architectural rationale for treating cross-module cycles as the defect.
