-- One row per crate version we attempted to extract. Written by `extract`;
-- `ir_path` points at the serialized CrateGraph the `sweep` phase reads.
CREATE TABLE extractions (
    name         TEXT    NOT NULL,
    version      TEXT    NOT NULL,
    downloads    BIGINT  NOT NULL,
    status       TEXT    NOT NULL,  -- ok | download_fail | extract_fail | timeout | spawn_fail | parse_fail
    ir_path      TEXT,
    n_items      INTEGER,
    n_modules    INTEGER,
    n_edges      INTEGER,
    -- Edge-kind composition (the structural fingerprint the weights act on) and
    -- item-kind composition, tallied from the IR. `n_pub_api_items` is the count
    -- of items reachable through an unbroken pub chain from the crate root.
    n_import_edges      INTEGER,
    n_signature_edges   INTEGER,
    n_trait_bound_edges INTEGER,
    n_impl_edges        INTEGER,
    n_body_edges        INTEGER,
    n_structs           INTEGER,
    n_enums             INTEGER,
    n_traits            INTEGER,
    n_type_aliases      INTEGER,
    n_functions         INTEGER,
    n_pub_api_items     INTEGER,
    -- Resource cost of extraction. `elapsed_sec` is the extractor subprocess
    -- wall time; `prepare_sec` is the preceding download + unpack; `peak_rss_kb`
    -- is the peak resident memory of the extractor process (the rust-analyzer
    -- database dominates it), sampled from /proc; `crate_bytes` is the .crate
    -- download size.
    elapsed_sec  DOUBLE,
    prepare_sec  DOUBLE,
    peak_rss_kb  BIGINT,
    crate_bytes  BIGINT,
    error        TEXT,
    -- Provenance: the rust-analyzer version and IR schema version that produced
    -- the dump, so stale IR can be re-extracted after a toolchain/schema bump
    -- without opening every file.
    ra_version     TEXT,
    schema_version INTEGER,
    -- Comma-joined crates.io metadata captured from the db-dump: `categories`
    -- is the curated/standardized taxonomy (slugs like `parsing`,
    -- `command-line-utilities`); `keywords` are free-form author tag slugs.
    categories   TEXT,
    keywords     TEXT,
    ts           BIGINT  NOT NULL,  -- unix seconds
    PRIMARY KEY (name, version)
);

-- One row per crate version per sweep. Regenerated cheaply (metrics-only,
-- in-process) every time the metric code or weights change.
CREATE TABLE analyses (
    name                    TEXT    NOT NULL,
    version                 TEXT    NOT NULL,
    status                  TEXT    NOT NULL,  -- ok | error
    headline                DOUBLE,            -- NULL = N/A (no measurable structure)
    headline_depth_averaged DOUBLE,
    modularity_term         DOUBLE,
    divergence_term         DOUBLE,
    acyclicity_term         DOUBLE,
    encapsulation_term      DOUBLE,
    is_acyclic              INTEGER,           -- 0 | 1
    over_exposed_fraction   DOUBLE,
    mean_leak_cost          DOUBLE,
    n_real_items            INTEGER,
    n_module_nodes          INTEGER,
    -- Cycle severity (beyond the is_acyclic boolean): number of non-trivial
    -- SCCs, size of the largest, total module nodes in any cycle, and the
    -- cyclomatic number (independent cycles = E - V + 1 per SCC) measuring how
    -- densely interwoven the tangles are.
    n_sccs                  INTEGER,
    largest_scc             INTEGER,
    modules_in_cycles       INTEGER,
    cyclomatic_number       INTEGER,
    -- Encapsulation counts: over-exposed items and the number of cross-module
    -- references (the denominator of mean_leak_cost, the leak rate = 1 - MII).
    n_over_exposed          INTEGER,
    n_cross_module_edges    INTEGER,
    -- Martin package metrics, aggregated over real modules.
    mean_instability             DOUBLE,
    median_instability           DOUBLE,
    mean_cohesion                DOUBLE,
    mean_distance_main_sequence  DOUBLE,
    anomaly                 TEXT,              -- comma-separated out-of-range / non-finite flags
    elapsed_ms              DOUBLE,
    error                   TEXT,
    ts                      BIGINT  NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE INDEX idx_extractions_status ON extractions (status);
CREATE INDEX idx_analyses_status ON analyses (status);
