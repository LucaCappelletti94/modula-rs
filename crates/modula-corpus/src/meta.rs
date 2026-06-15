//! The `meta` phase: backfill crates.io metadata onto already-extracted rows
//! without re-running extraction. Currently fills `released_at` (the publish
//! time) by reparsing the db-dump and updating existing `extractions` rows in
//! place. This exists to add confounder columns (crate age) to an extant corpus.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use diesel::prelude::*;

use crate::db;
use crate::dump;
use crate::schema::extractions::dsl;

/// Options for the `meta` phase.
pub struct MetaArgs {
    pub root: PathBuf,
    pub db_path: String,
    /// Minimum downloads filter for the dump parse; match the original extract
    /// run so the worklist covers (a superset of) the extracted rows.
    pub min_downloads: i64,
}

/// Reparses the db-dump and backfills `released_at` for existing extraction rows.
pub fn run(args: &MetaArgs) -> Result<()> {
    let db_file = crate::extract::db_file(&args.root, &args.db_path);
    let mut conn = db::open(&db_file)?; // also applies the released_at migration
    let keys: HashSet<(String, String)> = db::extracted_keys(&mut conn)?.into_iter().collect();
    println!("{} extracted rows to backfill", keys.len());

    let dump_path = args.root.join("db-dump.tar.gz");
    println!("parsing db-dump (>= {} downloads) ...", args.min_downloads);
    let work = dump::build_worklist(
        dump_path.to_str().context("dump path not utf-8")?,
        args.min_downloads,
    )?;

    let mut updated = 0usize;
    let mut missing_date = 0usize;
    conn.transaction::<_, anyhow::Error, _>(|conn| {
        for cv in &work {
            if !keys.contains(&(cv.name.clone(), cv.version.clone())) {
                continue;
            }
            let Some(rel) = cv.released_at else {
                missing_date += 1;
                continue;
            };
            updated += diesel::update(
                dsl::extractions
                    .filter(dsl::name.eq(&cv.name))
                    .filter(dsl::version.eq(&cv.version)),
            )
            .set(dsl::released_at.eq(rel))
            .execute(conn)
            .with_context(|| format!("updating {}-{}", cv.name, cv.version))?;
        }
        Ok(())
    })?;

    println!("backfilled released_at for {updated} rows ({missing_date} had no parseable date)");
    Ok(())
}
