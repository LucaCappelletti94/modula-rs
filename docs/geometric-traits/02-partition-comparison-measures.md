# Tier 2: partition-comparison measures (VI, NMI, AMI, ARI)

## What these are (and what they are not)

These are **not** graph algorithms. They are small, pure, deterministic functions. Each one takes two partitions of the same set of N items (two integer arrays of equal length, where the value at position i is the cluster id of item i) and returns a single number that measures how much the two groupings agree.

There is:

- no graph involved (they never look at edges),
- no iteration to convergence and no optimization (unlike Louvain, Leiden, Leicht-Newman),
- no randomness.

The right mental model is "compute a correlation coefficient between two labelings," not "run a community detection algorithm." A useful comparison: Louvain takes a graph and **produces** a partition. These functions take two **already produced** partitions and score their similarity. They sit at the very end of the pipeline, after detection has already happened.

The one important property they all share: they are **invariant to how clusters are named**. If one partition calls a group "7" and the other calls the same group "parser", that does not matter. They only look at which items are grouped together, never at the label values. The adjusted variants (AMI, ARI) are additionally invariant to the **number** of clusters, by correcting for what agreement you would expect from two random partitions.

## Why modula-rs needs them

They power the divergence metric: the comparison between the declared module partition A (from the module tree) and the detected community partition B (from a detector run on the dependency graph). That comparison is the literal "how far do the imports diverge from the module tree" number. We compute it at each module-tree depth, producing a divergence profile rather than a single value.

## How they are computed

All of them are built from one shared structure, the contingency table between A and B:

```
n_ij = number of items that are in cluster i of A AND cluster j of B
a_i  = size of cluster i in A      (row sums)
b_j  = size of cluster j in B      (col sums)
N    = total number of items
```

Building the table is O(N). Everything else is a small sum over the table cells, O(R * C) where R and C are the cluster counts. From the table:

```
entropy            H(A)    = - sum_i (a_i/N) log(a_i/N)
mutual information I(A,B)  = sum_ij (n_ij/N) log( (n_ij/N) / ((a_i/N)(b_j/N)) )
```

The four measures:

| Measure | Definition | Range | Notes |
|---------|-----------|-------|-------|
| Variation of Information (VI) | `H(A) + H(B) - 2 I(A,B)` | `[0, log N]` | True metric. 0 = identical. Decomposes into `H(A|B)` and `H(B|A)`, which say whether A is over- or under-split relative to B. |
| Normalized MI (NMI) | `I(A,B) / normalizer`, normalizer in {max, mean, geometric mean of H(A), H(B)} | `[0, 1]` | Simple, but not corrected for chance, so biased when cluster counts differ. |
| Adjusted MI (AMI) | `(I - E[I]) / (max(H(A),H(B)) - E[I])` | ~`[0, 1]` | Chance-corrected. ~0 for random partitions, 1 for identical. Preferred when cluster counts differ, which is our case. |
| Adjusted Rand Index (ARI) | pair-counting agreement, chance-corrected | ~`[-1, 1]` | A different (pair-based) lens than the information-theoretic ones. Good as a corroborating third number. |

`E[I]` for AMI is the expected mutual information of two random partitions with the same cluster sizes, computed from the row and column sums under a hypergeometric model. It is a standard closed-form sum (see Vinh et al. 2010 for the exact expression).

## Proposed signatures (Rust)

Pure free functions over two label slices:

```rust
pub fn variation_of_information(a: &[usize], b: &[usize]) -> f64;
pub fn normalized_mutual_information(a: &[usize], b: &[usize]) -> f64;
pub fn adjusted_mutual_information(a: &[usize], b: &[usize]) -> f64;
pub fn adjusted_rand_index(a: &[usize], b: &[usize]) -> f64;
```

Preconditions: `a.len() == b.len()`. Labels do not need to be contiguous or sorted, the contingency table handles arbitrary ids. A shared internal helper builds the contingency table and the entropies once.

## Where they should live

Because they depend on nothing from `geometric-traits` (no graph types, no traits), the default plan is to implement them directly in `modula-rs`. They are a few hundred lines including tests. They would only belong in `geometric-traits` if you want them as reusable clustering-comparison utilities for other projects, which is a fine option but not required. This document exists mainly to explain what they are, since the earlier list made them sound heavier than they are.

## References

- Meila, M. (2007). "Comparing clusterings, an information based distance." J. Multivariate Analysis 98(5), 873-895. https://doi.org/10.1016/j.jmva.2006.11.013
- Vinh, N. X., Epps, J., Bailey, J. (2010). "Information Theoretic Measures for Clusterings Comparison." JMLR 11, 2837-2854. https://www.jmlr.org/papers/v11/vinh10a.html
- Hubert, L., Arabie, P. (1985). "Comparing partitions." Journal of Classification 2, 193-218. https://doi.org/10.1007/BF01908075

## Acceptance criteria

1. Identical partitions give VI = 0, NMI = 1, AMI = 1, ARI = 1.
2. Independent (random) partitions give AMI and ARI near 0.
3. Values match a reference implementation (for example scikit-learn `adjusted_mutual_info_score`, `adjusted_rand_score`, `normalized_mutual_info_score`) on a handful of small fixtures.
4. Label renaming (permuting cluster ids in either input) does not change any output.
