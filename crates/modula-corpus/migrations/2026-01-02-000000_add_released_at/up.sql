-- The publish time (unix seconds) of the extracted crate version, parsed from
-- the crates.io db-dump `versions.created_at`. Used to check whether the metrics
-- are confounded by crate age (era / deprecation) rather than modularity.
-- Backfilled onto existing rows by the `meta` subcommand.
ALTER TABLE extractions ADD COLUMN released_at BIGINT;
