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
    elapsed_sec  DOUBLE,
    error        TEXT,
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
