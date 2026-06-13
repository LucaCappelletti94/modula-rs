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
    anomaly                 TEXT,              -- comma-separated out-of-range / non-finite flags
    elapsed_ms              DOUBLE,
    error                   TEXT,
    ts                      BIGINT  NOT NULL,
    PRIMARY KEY (name, version)
);

CREATE INDEX idx_extractions_status ON extractions (status);
CREATE INDEX idx_analyses_status ON analyses (status);
