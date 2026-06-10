# Tier 3: directed community detection (directed Louvain / Leiden)

## Status: optional upgrade, not a blocker

The directed pipeline already runs today. `LeichtNewman` (leading-eigenvector spectral bisection) produces a partition that optimizes directed modularity, and `DirectedModularity::directed_modularity` scores any partition. This document describes an upgrade: directed variants of Louvain and Leiden that optimize the same Leicht-Newman directed modularity.

## Why bother, given LeichtNewman exists

1. **Scalability and quality.** The leading-eigenvector method is divisive and spectral. It is fine on the small-to-medium graphs Rust crates produce, but Louvain and Leiden generally give better partitions and scale better on larger workspaces (many crates, tens of thousands of items).
2. **Multi-level hierarchy.** Louvain and Leiden naturally produce a sequence of coarser and coarser levels. modula-rs analyzes modularity at multiple module-tree depths, so a detector that emits a level hierarchy pairs cleanly with that: we can compare each declared depth against a corresponding detector level, instead of against one flat partition.
3. **Single-objective cleanliness.** If both the detected partition B and the declared-partition score use directed Louvain/Leiden modularity, the headline ratio `Q(declared) / Q(detected)` is computed under one consistent objective end to end.

## What actually changes versus the existing undirected detectors

This is the key point: a directed detector is **the same algorithm skeleton** as the undirected one, with one substitution. The local-moving phase, the aggregation (community-collapsing) phase, and for Leiden the refinement phase and the well-connectedness guarantee, are all unchanged. Only the modularity null model changes:

```
undirected gain uses   k_i * k_j / (2m)
directed   gain uses   k_i^out * k_j^in / m
```

so every node carries an in-strength and an out-strength separately, and the aggregation step must preserve directed edges (keep in and out weights distinct when collapsing a community into a super-node). `m` is the total arc weight. The resolution parameter `gamma` carries over unchanged onto the null term.

### Directed modularity gain (Dugue-Perez form)

When moving an isolated node `i` into community `C`, with resolution `gamma`:

```
delta Q = ( k_{i,in}^C + k_{i,out}^C ) / m
          - gamma * ( k_i^out * Sigma_in^C + k_i^in * Sigma_out^C ) / m^2
```

where:

- `k_{i,out}^C` = total weight of arcs from `i` to nodes of `C`,
- `k_{i,in}^C`  = total weight of arcs from nodes of `C` to `i`,
- `k_i^out`, `k_i^in` = out-strength and in-strength of `i`,
- `Sigma_out^C`, `Sigma_in^C` = total out-strength and in-strength of community `C`,
- `m` = total arc weight.

This formula should be validated against the reference C++ implementation rather than trusted verbatim.

## Proposed API

Mirror the existing `Louvain` / `Leiden` trait shapes, returning a multi-level result with per-level partition and directed modularity. Reuse `LeichtNewmanConfig` style for configuration.

```rust
pub trait DirectedLouvain<Marker = usize> {
    fn directed_louvain(&self, config: &DirectedLouvainConfig)
        -> Result<DirectedLouvainResult<Marker>, ModularityError>;
}

pub trait DirectedLeiden<Marker = usize> {
    fn directed_leiden(&self, config: &DirectedLeidenConfig)
        -> Result<DirectedLeidenResult<Marker>, ModularityError>;
}
```

`DirectedLouvainConfig`: `resolution`, `modularity_threshold`. `DirectedLeidenConfig`: additionally `seed` and `theta` for the refinement phase, as the undirected `LeidenConfig` already has. Results expose levels, each with `partition()` and `modularity()`, plus `final_partition()` and `final_modularity()`, matching the undirected results.

## Suggested phasing

1. **Directed Louvain first.** It is the simpler of the two (local moving plus aggregation, no refinement) and gives most of the benefit. It is also the one with a published reference implementation to validate against.
2. **Directed Leiden second.** Adds the refinement phase and the well-connectedness guarantee on top. The guarantee is independent of the null model, so the existing refinement logic carries over with the directed gain substituted in.

## Edge cases specific to dependency graphs

Dependency graphs are not generic directed graphs, they have structure worth calling out in tests:

- **Sources and sinks are common.** Crate entry points have in-strength 0, leaf utilities have out-strength 0. The directed null model multiplies `k_i^out` and `k_j^in`, so nodes with a zero on one side contribute zero expected weight on that side. Confirm this behaves sensibly and does not divide by zero.
- **Self-loops.** Already handled in the `directed_modularity` tests, keep that behavior.
- **Strongly asymmetric coupling.** A and B may be connected by many arcs in one direction and none in the other. This is exactly where directed detection should differ from symmetrizing, so it makes a good discriminating test.

## References

- Leicht, E. A., Newman, M. E. J. (2008). "Community Structure in Directed Networks." Phys. Rev. Lett. 100, 118703. https://doi.org/10.1103/PhysRevLett.100.118703 (the directed modularity objective)
- Dugue, N., Perez, A. (2015). "Directed Louvain: maximizing modularity in directed networks." Research report, Universite d'Orleans, hal-01231784. https://hal.science/hal-01231784v1
- Dugue, N., Perez, A. (2022). "Direction matters in complex networks: A theoretical and applied study for greedy modularity optimization." Physica A 603, 127798. https://doi.org/10.1016/j.physa.2022.127798 (the published version, preferred citation)
- Reference implementation to validate against: https://github.com/nicolasdugue/DirectedLouvain
- Blondel, V. D., et al. (2008). Louvain skeleton. https://doi.org/10.1088/1742-5468/2008/10/P10008
- Traag, V. A., Waltman, L., van Eck, N. J. (2019). Leiden skeleton and well-connectedness. https://doi.org/10.1038/s41598-019-41695-z

## Expectation and acceptance criteria

1. Final modularity returned by the detector equals `directed_modularity(final_partition)` on the same graph.
2. Modularity is non-decreasing across levels.
3. On small fixtures (for example two disjoint directed cycles, as in the existing directed modularity tests), the detected partition matches the known optimum.
4. On a set of directed fixtures, partitions agree with the reference DirectedLouvain implementation.
5. For directed Leiden, all detected communities are well-connected (the Leiden guarantee).
6. Sources, sinks, and self-loops are handled without panics or division by zero.
