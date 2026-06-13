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
