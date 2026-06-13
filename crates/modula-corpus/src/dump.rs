//! Parsing the crates.io database dump into a download-ranked work-list.
//!
//! The dump is a single `.tar.gz` of CSVs. We stream it once (the machine has
//! ample RAM, so every needed column is held in memory) and join four tables:
//! `crate_downloads` (download counts), `crates` (id -> name),
//! `default_versions` (crate_id -> the version cargo would pick), and `versions`
//! (version_id -> the version string).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};

use anyhow::{Context as _, Result};
use flate2::read::GzDecoder;
use tar::Archive;

/// One crate version to analyze.
#[derive(Debug, Clone)]
pub struct CrateVersion {
    pub name: String,
    pub version: String,
    pub downloads: i64,
}

/// Streams `dump_path` once and returns every crate whose download count is at
/// least `min_downloads`, most-downloaded first.
pub fn build_worklist(dump_path: &str, min_downloads: i64) -> Result<Vec<CrateVersion>> {
    let mut downloads: HashMap<i64, i64> = HashMap::new();
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut default_vid: HashMap<i64, i64> = HashMap::new();
    let mut vid_num: HashMap<i64, String> = HashMap::new();

    let file = File::open(dump_path).with_context(|| format!("opening dump {dump_path}"))?;
    let mut archive = Archive::new(GzDecoder::new(BufReader::new(file)));
    for entry in archive.entries().context("reading tar")? {
        let mut entry = entry.context("reading tar entry")?;
        let path = entry
            .path()
            .context("entry path")?
            .to_string_lossy()
            .into_owned();
        // db-dump paths look like `<stamp>/data/<table>.csv`.
        let Some(table) = path.split("/data/").nth(1) else {
            continue;
        };
        match table {
            "crate_downloads.csv" => two_col(&mut entry, "crate_id", "downloads", |id, dl| {
                if let (Ok(id), Ok(dl)) = (id.parse::<i64>(), dl.parse::<i64>())
                    && dl >= min_downloads
                {
                    downloads.insert(id, dl);
                }
            })?,
            "crates.csv" => two_col(&mut entry, "id", "name", |id, name| {
                if let Ok(id) = id.parse::<i64>() {
                    names.insert(id, name.to_owned());
                }
            })?,
            "default_versions.csv" => two_col(&mut entry, "crate_id", "version_id", |cid, vid| {
                if let (Ok(cid), Ok(vid)) = (cid.parse::<i64>(), vid.parse::<i64>()) {
                    default_vid.insert(cid, vid);
                }
            })?,
            "versions.csv" => two_col(&mut entry, "id", "num", |id, num| {
                if let Ok(id) = id.parse::<i64>() {
                    vid_num.insert(id, num.to_owned());
                }
            })?,
            _ => continue,
        }
    }

    let mut work: Vec<CrateVersion> = downloads
        .iter()
        .filter_map(|(&cid, &dl)| {
            let name = names.get(&cid)?;
            let vid = default_vid.get(&cid)?;
            let num = vid_num.get(vid)?;
            Some(CrateVersion {
                name: name.clone(),
                version: num.clone(),
                downloads: dl,
            })
        })
        .collect();
    work.sort_by(|a, b| {
        b.downloads
            .cmp(&a.downloads)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(work)
}

/// Parses a CSV stream, invoking `f(col_a, col_b)` for every row.
fn two_col<R: Read>(reader: R, a: &str, b: &str, mut f: impl FnMut(&str, &str)) -> Result<()> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(reader);
    let headers = rdr.headers().context("reading CSV header")?.clone();
    let ia = headers
        .iter()
        .position(|h| h == a)
        .with_context(|| format!("no column `{a}`"))?;
    let ib = headers
        .iter()
        .position(|h| h == b)
        .with_context(|| format!("no column `{b}`"))?;
    for rec in rdr.records() {
        let rec = rec.context("reading CSV row")?;
        if let (Some(va), Some(vb)) = (rec.get(ia), rec.get(ib)) {
            f(va, vb);
        }
    }
    Ok(())
}
