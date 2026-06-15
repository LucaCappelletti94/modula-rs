//! Parsing the crates.io database dump into a download-ranked work-list.
//!
//! The dump is a single `.tar.gz` of CSVs. We stream it once (the machine has
//! ample RAM, so every needed column is held in memory) and join: the version
//! tables (`crate_downloads` for counts, `crates` for id -> name,
//! `default_versions` + `versions` for the version cargo would pick) and the
//! metadata tables (`categories` + `crates_categories` for the standardized
//! taxonomy, `keywords` + `crates_keywords` for free-form tags).

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
    /// Comma-joined crates.io category slugs (the standardized taxonomy), or
    /// empty when the crate has none.
    pub categories: String,
    /// Comma-joined crates.io keyword slugs (free-form), or empty.
    pub keywords: String,
    /// Publish time of this version (unix seconds), from `versions.created_at`.
    pub released_at: Option<i64>,
}

/// Streams `dump_path` once and returns every crate whose download count is at
/// least `min_downloads`, most-downloaded first.
pub fn build_worklist(dump_path: &str, min_downloads: i64) -> Result<Vec<CrateVersion>> {
    let mut downloads: HashMap<i64, i64> = HashMap::new();
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut default_vid: HashMap<i64, i64> = HashMap::new();
    let mut vid_num: HashMap<i64, String> = HashMap::new();
    let mut vid_created: HashMap<i64, i64> = HashMap::new();
    // Metadata: id -> slug, plus crate_id -> [id]. Joined to slug strings below.
    let mut cat_slug: HashMap<i64, String> = HashMap::new();
    let mut crate_cats: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut kw_slug: HashMap<i64, String> = HashMap::new();
    let mut crate_kws: HashMap<i64, Vec<i64>> = HashMap::new();

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
            "versions.csv" => {
                three_col(&mut entry, "id", "num", "created_at", |id, num, created| {
                    if let Ok(id) = id.parse::<i64>() {
                        vid_num.insert(id, num.to_owned());
                        if let Some(ts) = parse_created_at(created) {
                            vid_created.insert(id, ts);
                        }
                    }
                })?
            }
            "categories.csv" => two_col(&mut entry, "id", "slug", |id, slug| {
                if let Ok(id) = id.parse::<i64>() {
                    cat_slug.insert(id, slug.to_owned());
                }
            })?,
            "crates_categories.csv" => {
                two_col(&mut entry, "crate_id", "category_id", |cid, catid| {
                    if let (Ok(cid), Ok(catid)) = (cid.parse::<i64>(), catid.parse::<i64>()) {
                        crate_cats.entry(cid).or_default().push(catid);
                    }
                })?
            }
            "keywords.csv" => two_col(&mut entry, "id", "keyword", |id, kw| {
                if let Ok(id) = id.parse::<i64>() {
                    kw_slug.insert(id, kw.to_owned());
                }
            })?,
            "crates_keywords.csv" => two_col(&mut entry, "crate_id", "keyword_id", |cid, kwid| {
                if let (Ok(cid), Ok(kwid)) = (cid.parse::<i64>(), kwid.parse::<i64>()) {
                    crate_kws.entry(cid).or_default().push(kwid);
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
                categories: join_slugs(crate_cats.get(&cid), &cat_slug),
                keywords: join_slugs(crate_kws.get(&cid), &kw_slug),
                released_at: vid_created.get(vid).copied(),
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

/// Resolves a crate's metadata ids to their slugs, sorted and comma-joined
/// (sorted so the string is deterministic across runs).
fn join_slugs(ids: Option<&Vec<i64>>, slugs: &HashMap<i64, String>) -> String {
    let Some(ids) = ids else {
        return String::new();
    };
    let mut out: Vec<&str> = ids
        .iter()
        .filter_map(|id| slugs.get(id).map(String::as_str))
        .collect();
    out.sort_unstable();
    out.join(",")
}

/// Parses a CSV stream, invoking `f(col_a, col_b, col_c)` for every row.
fn three_col<R: Read>(
    reader: R,
    a: &str,
    b: &str,
    c: &str,
    mut f: impl FnMut(&str, &str, &str),
) -> Result<()> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(reader);
    let headers = rdr.headers().context("reading CSV header")?.clone();
    let col = |name: &str| {
        headers
            .iter()
            .position(|h| h == name)
            .with_context(|| format!("no column `{name}`"))
    };
    let (ia, ib, ic) = (col(a)?, col(b)?, col(c)?);
    for rec in rdr.records() {
        let rec = rec.context("reading CSV row")?;
        if let (Some(va), Some(vb), Some(vc)) = (rec.get(ia), rec.get(ib), rec.get(ic)) {
            f(va, vb, vc);
        }
    }
    Ok(())
}

/// Parses a crates.io `created_at` timestamp (`YYYY-MM-DD HH:MM:SS[.ffffff][+00]`)
/// to unix seconds. Returns `None` if the leading date does not parse. Uses
/// Howard Hinnant's `days_from_civil` so it needs no date dependency.
fn parse_created_at(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Time portion is optional; default to midnight when absent or malformed.
    let (hh, mm, ss) = if bytes.len() >= 19 && bytes[10] == b' ' {
        (
            num(11..13).unwrap_or(0),
            num(14..16).unwrap_or(0),
            num(17..19).unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };
    let yy = y - i64::from(m <= 2);
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = yy - era * 400;
    let doy = (153 * (m + (if m > 2 { -3 } else { 9 })) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
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

#[cfg(test)]
mod tests {
    use super::parse_created_at;

    #[test]
    fn parses_dump_timestamp_to_unix_seconds() {
        // Reference values from `date -u -d '...' +%s`.
        // 2024-07-19 17:09:31 UTC -> 1721408971.
        assert_eq!(
            parse_created_at("2024-07-19 17:09:31.683638+00"),
            Some(1_721_408_971)
        );
        // Epoch and a date-only string (midnight).
        assert_eq!(parse_created_at("1970-01-01 00:00:00"), Some(0));
        assert_eq!(parse_created_at("2015-12-09"), Some(1_449_619_200));
    }

    #[test]
    fn rejects_malformed_dates() {
        assert_eq!(parse_created_at(""), None);
        assert_eq!(parse_created_at("not-a-date"), None);
        assert_eq!(parse_created_at("2024-13-01 00:00:00"), None);
    }
}
