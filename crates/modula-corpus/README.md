# modula-corpus

Internal tooling (not shipped) to calibrate the modularity metrics over a large
slice of crates.io. It separates the two costs cleanly:

- **extraction** is rust-analyzer bound (~15-30s per crate) and the crate code
  never changes between experiments, so it runs **once** per crate and the IR is
  persisted;
- **analysis** is sub-100ms and is what we iterate on, so the sweep re-runs it
  **in-process** over the persisted IR, a full corpus pass in seconds.

All Rust, under the workspace's locked + `cargo deny` / `cargo audit` governed
dependency set. No Python, no npm.

## Phases

```sh
# 1. Enumerate crates.io (db-dump), download each crate, extract IR once.
#    Isolated per crate (subprocess + process-group timeout), resumable.
cargo run -p modula-corpus --release -- extract --limit 19266 --jobs 12

# 2. Re-run the metrics over every persisted IR, in-process and in parallel.
#    Cheap: re-run after every metric/weight change.
cargo run -p modula-corpus --release -- sweep

# 3. Render the metric distributions to an SVG grid of histograms.
cargo run -p modula-corpus --release -- plot --out plots/metrics.svg
```

Default corpus root is `/mnt/nvme/modula-corpus`; override with `--root`. State
lives in a SQLite database (`--db`, default `corpus.db`) with two tables:
`extractions` (one row per crate, written by `extract`) and `analyses` (one row
per crate per sweep). The schema is defined by diesel migrations under
`migrations/`.

`extract` also records, per crate:

- **resource cost**: `elapsed_sec` (extraction), `prepare_sec` (download + unpack),
  `peak_rss_kb` (peak RSS of the extractor process, sampled from `/proc`),
  `crate_bytes` (download size);
- **provenance**: `ra_version` and `schema_version` from the IR, so stale dumps
  can be re-extracted after a rust-analyzer or schema bump without opening every
  file;
- **crates.io metadata**: `categories` (the standardized taxonomy) and `keywords`
  (free-form tags) from the db-dump, comma-joined, so a later embedding step
  (z-scored metric features -> PCA -> t-SNE) can be colored by category or keyword;
- **structural composition**: edge-kind counts (import/signature/trait_bound/impl/
  body), type/item-kind counts (struct/enum/trait/type_alias/function), and the
  public-API item count.

The `sweep` phase records, beyond the four headline terms: **cycle severity**
(`n_sccs`, `largest_scc`, `modules_in_cycles`, `circuits_truncated`),
**encapsulation tails** (`max_leak_cost`, `n_over_exposed`, `n_cross_module_edges`),
and the **Martin package metrics** aggregated over real modules (`mean_instability`,
`median_instability`, `mean_cohesion`, `mean_distance_main_sequence`). These give
the embedding a far richer per-crate feature vector than the headline alone.
