# Panic: `renumber_partition` index out of bounds in the Leiden detector

Found while sweeping modula-rs over ~19,000 crates.io crates (>=100k downloads):
**7 crates** crash the analyzer with an identical panic inside the
`geometric-traits` dependency. Every other crate (18,599 ok, plus environmental
failures) is fine.

## Symptom

```text
thread 'main' panicked at geometric-traits/.../src/traits/algorithms/modularity.rs:567:19:
index out of bounds: the len is 3517 but the index is 3517
  2: core::panicking::panic_bounds_check
  3: geometric_traits::traits::algorithms::modularity::renumber_partition
  4: geometric_traits::traits::algorithms::leiden::leiden_levels
  5: modula_metrics::modularity::profiles
  6: modula_metrics::analysis::analyze
```

The panic is in `geometric-traits` (git rev `dba79ba`, branch `main`), reached
only through its **Leiden** detector, never through the partition modula-rs
passes in. The `len == index` (here `3517 == 3517`) is the classic off-by-one
tell: a community **label equal to the node count** reaches `renumber_partition`.

Affected crates (all the same panic): `async-stripe-shared`, `bellman_ce`,
`bgpkit-parser`, `datadog-api-client`, `multiversx-sc`, `nrf52810-pac`,
`openai-protocol`. They are large, densely-connected graphs (e.g. generated API
clients and peripheral-access crates), which is what drives Leiden into the
community-splitting path below.

## Reproduction

```console
cargo modula <a-crate-with-a-large-dense-item-graph>
# or directly:
RUST_BACKTRACE=1 cargo run -p cargo-modula -- modula path/to/openai-protocol
```

## Root cause

The bug is entirely within `geometric-traits`; modula-rs passes a valid,
dense partition. The mismatch is between two functions in
`src/traits/algorithms/modularity.rs`.

`renumber_partition` is the canonical "compact arbitrary community labels into
`0..k`" helper. It must therefore accept labels that are sparse and/or larger
than the number of nodes. But it sizes its lookup table by the **node count**:

```rust
pub(crate) fn renumber_partition(partition: &mut [usize]) -> usize {
    let mut mapping = vec![usize::MAX; partition.len()]; // <-- sized by node count
    let mut next_community_id = 0usize;
    for community in partition {
        if mapping[*community] == usize::MAX {            // <-- indexed by label
            mapping[*community] = next_community_id;
            next_community_id += 1;
        }
        *community = mapping[*community];
    }
    next_community_id
}
```

`mapping[*community]` is safe only if every label is `< partition.len()`. That
invariant is violated by the function called immediately before it in
`leiden_levels` (`leiden.rs`, lines 280-281):

```rust
graph.split_disconnected_communities(&mut refined_partition);
let number_of_communities = renumber_partition(&mut refined_partition); // panics here
```

`split_disconnected_communities` hands out **fresh** labels starting at the
current community count and counting up:

```rust
let number_of_communities = partition.iter().copied().max().map_or(0, |m| m + 1);
let mut next_community_id = number_of_communities;     // starts past every existing label
// ... for each extra connected component: assign next_community_id; next_community_id += 1;
```

So after a split, `refined_partition` can contain labels `>= partition.len()`
(it allocates new ids without bounding them by the node count; sparse input
labels make this worse). The next line feeds exactly that partition into
`renumber_partition`, which indexes a `len`-sized array with a `>= len` label and
panics. Note the inconsistency: `split_disconnected_communities` itself sizes its
own `nodes_per_community` buffer by `max(label) + 1`, the correct rule;
`renumber_partition` does not.

## Fix

### Primary (geometric-traits): make `renumber_partition` robust

Size the lookup by the largest label, not the node count, exactly the rule the
sibling function already uses:

```rust
pub(crate) fn renumber_partition(partition: &mut [usize]) -> usize {
    // Labels are not guaranteed dense or < len: split_disconnected_communities
    // allocates fresh ids beyond the node count. Size by the largest label.
    let capacity = partition.iter().copied().max().map_or(0, |m| m + 1);
    let mut mapping = vec![usize::MAX; capacity];
    let mut next_community_id = 0usize;
    for community in partition {
        if mapping[*community] == usize::MAX {
            mapping[*community] = next_community_id;
            next_community_id += 1;
        }
        *community = mapping[*community];
    }
    next_community_id
}
```

This is minimal and preserves behavior for already-dense partitions (empty input
-> capacity 0 -> returns 0). A regression test: a partition such as
`[0, 5]` (label `5` with only two nodes) must renumber to `[0, 1]` and return
`2`, rather than panicking. Worth auditing every other `vec![_; partition.len()]`
in the Leiden/Louvain path for the same node-count-vs-label confusion.

### Defensive (modula-rs): degrade instead of crashing

Independent of the upstream fix, a panic in a dependency should not take down the
whole analysis of one crate. `modula_metrics::analysis::analyze` (or just the
`profiles` call) can wrap the modularity/divergence computation in
`std::panic::catch_unwind` and, on panic, return empty modularity/divergence
profiles plus a recorded diagnostic. The crate then still gets a coupling,
cycles, and encapsulation score, and `cargo modula` exits cleanly with a warning
instead of a backtrace. This also future-proofs the corpus sweep against other
detector edge cases.

## Validation

1. Apply the `renumber_partition` fix in geometric-traits; bump the modula-rs git
   pin.
2. Re-run the 7 crates above; each should analyze to completion.
3. (If the defensive guard is added) confirm a forced panic in `profiles`
   surfaces as a clean per-crate error, not a process abort.
