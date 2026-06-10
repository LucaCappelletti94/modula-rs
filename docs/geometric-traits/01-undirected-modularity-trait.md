# Tier 1: public `UndirectedModularity` trait

## Summary

Expose, as a public trait, the ability to compute undirected Newman-Girvan modularity Q of an **externally supplied** partition on a weighted undirected graph. The computation already exists in `geometric-traits` but is private. This is the exact undirected twin of the `DirectedModularity` trait that already landed.

## Why modula-rs needs it

The core metric of modula-rs is "how good is the declared module decomposition," measured as the modularity of a partition we hand in (each item mapped to the module that owns it at a given tree depth). The existing `Louvain` and `Leiden` traits only report the modularity of the partition **they themselves discover**, there is no public way to score a partition that comes from the outside. We need to score the declared module partition, which we build from the module tree, not from any detector.

We already get this on the directed side via `DirectedModularity::directed_modularity`. We want the same on the undirected side because:

1. Undirected Newman Q is the canonical, widely understood modularity number, so it is the most interpretable headline score and the easiest to compare against the wider literature.
2. Reporting undirected Q next to directed Q is a useful cross-check. A large gap between the two is itself a signal that the coupling is strongly asymmetric.

## What already exists

- `UndirectedView::modularity(&self, partition: &[usize], resolution: f64) -> f64` in `src/traits/algorithms/modularity.rs` (around line 407), currently `pub(crate)`.
- `DirectedModularity` in `src/traits/algorithms/directed_modularity.rs` as the template to mirror (public trait, validates inputs, converts the `Marker` partition to `usize`, delegates to the private view method).

## Proposed API

Mirror `DirectedModularity` one to one:

```rust
pub trait UndirectedModularity<Marker: AsPrimitive<usize> = usize>:
    /* same bounds as DirectedModularity, but for the undirected/symmetric matrix input */
{
    /// Returns the undirected (Newman-Girvan) modularity of `partition`,
    /// where `partition[i]` is the community id of node `i`.
    ///
    /// # Errors
    /// - resolution not finite or not strictly positive
    /// - `partition.len()` does not equal the number of nodes
    /// - an edge weight is non-finite, non-positive, or unrepresentable as f64
    /// - the matrix is not square or not symmetric (not an undirected graph)
    fn undirected_modularity(
        &self,
        partition: &[Marker],
        resolution: f64,
    ) -> Result<f64, ModularityError>;
}
```

Method name `undirected_modularity` keeps it parallel with `directed_modularity`. A plain `modularity` would also be fine if you prefer, as long as it does not collide with the existing inherent methods.

## Definition

Standard resolution-parametrized Newman-Girvan modularity, which is what the private method already computes:

```
Q = (1 / 2m) * sum_ij [ A_ij - gamma * (k_i * k_j) / (2m) ] * delta(c_i, c_j)
```

Equivalently, summing over communities c:

```
Q = sum_c [ Sigma_in(c) / (2m) - gamma * (Sigma_tot(c) / (2m))^2 ]
```

where `m` is the total edge weight, `A_ij` the symmetric weight, `k_i` the weighted degree, `Sigma_in(c)` the total weight of edges inside community c (counted in both directions, as the existing code does), and `Sigma_tot(c)` the sum of degrees of nodes in c. `gamma` is the resolution parameter.

## Validation and error behavior

Identical to `DirectedModularity`: reject non-finite or non-positive resolution, require `partition.len() == number_of_nodes`, and require the matrix to be a valid undirected (square, symmetric, positive-weight) graph. Reuse `ModularityError`.

## Reference

- Newman, M. E. J., Girvan, M. (2004). "Finding and evaluating community structure in networks." Phys. Rev. E 69, 026113. https://doi.org/10.1103/PhysRevE.69.026113
- Newman, M. E. J. (2006). "Modularity and community structure in networks." PNAS 103(23), 8577-8582. https://doi.org/10.1073/pnas.0601602103

## Acceptance criteria

1. A new public trait method scores an arbitrary partition and matches the value the private `UndirectedView::modularity` returns on the same input.
2. For a partition that `Louvain` or `Leiden` discovered, `undirected_modularity` on that partition equals the modularity those detectors report for it (consistency check).
3. Reuses the existing undirected ground-truth fixtures (`tests/fixtures/modularity_ground_truth.json`) where applicable.
4. Rejects invalid resolution, mismatched partition length, and non-symmetric input with the existing `ModularityError` variants.

## Effort

Small. This is a symmetric copy of the `DirectedModularity` trait wired to the undirected view instead of the directed view, with the validation and tests following the same shape.
