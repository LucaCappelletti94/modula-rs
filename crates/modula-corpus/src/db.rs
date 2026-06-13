//! SQLite connection management via diesel, with embedded migrations.

use anyhow::{Context as _, Result};
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

use crate::models::{Analysis, Extraction};
use crate::schema::{analyses, extractions};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// Opens (creating if needed) the corpus database and applies any pending
/// migrations. WAL mode keeps concurrent readers from blocking the writer.
pub fn open(path: &str) -> Result<SqliteConnection> {
    let mut conn =
        SqliteConnection::establish(path).with_context(|| format!("opening database {path}"))?;
    diesel::sql_query("PRAGMA journal_mode=WAL;")
        .execute(&mut conn)
        .context("enabling WAL")?;
    diesel::sql_query("PRAGMA busy_timeout=30000;")
        .execute(&mut conn)
        .context("setting busy_timeout")?;
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| anyhow::anyhow!("running migrations: {e}"))?;
    Ok(conn)
}

/// Inserts or replaces one extraction row (upsert on the `(name, version)` key).
pub fn upsert_extraction(conn: &mut SqliteConnection, row: &Extraction) -> Result<()> {
    diesel::replace_into(extractions::table)
        .values(row)
        .execute(conn)
        .with_context(|| format!("writing extraction {}-{}", row.name, row.version))?;
    Ok(())
}

/// Inserts or replaces one analysis row.
pub fn upsert_analysis(conn: &mut SqliteConnection, row: &Analysis) -> Result<()> {
    diesel::replace_into(analyses::table)
        .values(row)
        .execute(conn)
        .with_context(|| format!("writing analysis {}-{}", row.name, row.version))?;
    Ok(())
}

/// The `(name, version)` keys already present in `extractions`, for resuming.
pub fn extracted_keys(conn: &mut SqliteConnection) -> Result<Vec<(String, String)>> {
    extractions::table
        .select((extractions::name, extractions::version))
        .load::<(String, String)>(conn)
        .context("loading extracted keys")
}

/// Every successful extraction with an IR dump path, for the sweep phase.
pub fn extractions_with_ir(conn: &mut SqliteConnection) -> Result<Vec<Extraction>> {
    extractions::table
        .filter(extractions::status.eq("ok"))
        .filter(extractions::ir_path.is_not_null())
        .select(Extraction::as_select())
        .load::<Extraction>(conn)
        .context("loading extractions with IR")
}
