//! The `extract` phase: enumerate crates.io, download each crate, and run IR
//! extraction once per crate in an isolated subprocess, persisting the IR.
//!
//! Extraction is the expensive, rust-analyzer-bound step, and it can panic, hang
//! or leak on pathological crates, so each crate runs in a `extract-one`
//! subprocess (this same binary re-invoked) under a hard wall-clock timeout that
//! kills the whole process group. The serialized `CrateGraph` is written to
//! `ir/<name>-<version>.ir.json` for the metrics-only `sweep` phase to reuse.

use std::fs;
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use modula_extract::{ExtractOptions, Extractor, RaExtractor};
use tar::Archive;

use crate::db;
use crate::dump::{self, CrateVersion};
use crate::http;
use crate::models::Extraction;

/// Options for the `extract` phase.
pub struct ExtractArgs {
    pub root: PathBuf,
    pub db_path: String,
    pub min_downloads: i64,
    pub jobs: usize,
    /// `CARGO_BUILD_JOBS` per worker; total compile fan-out is `jobs * build_jobs`.
    pub build_jobs: usize,
    pub timeout: Duration,
    pub limit: Option<usize>,
}

/// Working directories under the corpus root.
struct Dirs {
    dl: PathBuf,
    work: PathBuf,
    targets: PathBuf,
    ir: PathBuf,
    cargo_home: PathBuf,
}

impl Dirs {
    fn create(root: &Path) -> Result<Self> {
        let dirs = Dirs {
            dl: root.join("dl"),
            work: root.join("work"),
            targets: root.join("targets"),
            ir: root.join("ir"),
            cargo_home: root.join("cargo-home"),
        };
        for d in [
            &dirs.dl,
            &dirs.work,
            &dirs.targets,
            &dirs.ir,
            &dirs.cargo_home,
        ] {
            fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(dirs)
    }
}

/// The `extract-one` worker: extract IR for the crate at `crate_dir`, print the
/// serialized `CrateGraph` to stdout. Run as a subprocess, one crate per call.
pub fn extract_one(crate_dir: &Path) -> Result<()> {
    let manifest = crate_dir.join("Cargo.toml");
    let graph = RaExtractor
        .extract(&ExtractOptions {
            manifest_path: manifest,
            package: None,
            workspace: false,
        })
        .context("extraction failed")?;
    let json = serde_json::to_string(&graph).context("serializing IR")?;
    let mut out = std::io::stdout().lock();
    out.write_all(json.as_bytes())
        .context("writing IR to stdout")?;
    Ok(())
}

/// Runs the full `extract` phase.
pub fn run(args: &ExtractArgs) -> Result<()> {
    let dirs = Dirs::create(&args.root)?;
    let dump_path = args.root.join("db-dump.tar.gz");
    let agent = http::agent()?;
    ensure_dump(&agent, &dump_path)?;

    let spinner = ProgressBar::new_spinner();
    spinner.enable_steady_tick(Duration::from_millis(120));
    spinner.set_message(format!(
        "parsing db-dump (>= {} downloads) ...",
        args.min_downloads
    ));
    let mut work = dump::build_worklist(
        dump_path.to_str().context("dump path not utf-8")?,
        args.min_downloads,
    )?;
    spinner.finish_with_message(format!("work-list: {} crates", work.len()));
    if let Some(limit) = args.limit {
        work.truncate(limit);
    }

    let exe = std::env::current_exe().context("locating current exe")?;
    let mut conn = db::open(&db_file(&args.root, &args.db_path))?;
    let done: std::collections::HashSet<(String, String)> =
        db::extracted_keys(&mut conn)?.into_iter().collect();
    let todo: Vec<CrateVersion> = work
        .into_iter()
        .filter(|c| !done.contains(&(c.name.clone(), c.version.clone())))
        .collect();
    println!(
        "already done: {} | to run: {} | jobs: {}",
        done.len(),
        todo.len(),
        args.jobs
    );

    // Total compile parallelism is `jobs * build_jobs`, and each worker also
    // drives a rust-analyzer instance. `build_jobs` defaults to 1 so `--jobs N`
    // bounds CPU to roughly N: each worker's `cargo check` builds the crate's
    // dependency tree serially rather than fanning out and oversubscribing.
    let build_jobs = args.build_jobs.max(1);

    let conn = Mutex::new(conn);
    let next = AtomicUsize::new(0);
    let done_count = AtomicUsize::new(0);
    let ok_count = AtomicUsize::new(0);
    let total = todo.len();
    let pb = progress_bar(total as u64);

    std::thread::scope(|scope| {
        for slot in 0..args.jobs {
            let (dirs, agent, exe, todo, next, conn, done_count, ok_count, pb) = (
                &dirs,
                &agent,
                &exe,
                &todo,
                &next,
                &conn,
                &done_count,
                &ok_count,
                &pb,
            );
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = todo.get(i) else { break };
                    let row = process(item, slot, dirs, agent, exe, build_jobs, args.timeout);
                    if row.status == "ok" {
                        ok_count.fetch_add(1, Ordering::Relaxed);
                    }
                    {
                        let mut conn = conn.lock().expect("db mutex");
                        if let Err(e) = db::upsert_extraction(&mut conn, &row) {
                            pb.println(format!(
                                "db write failed for {}-{}: {e:#}",
                                row.name, row.version
                            ));
                        }
                    }
                    let n = done_count.fetch_add(1, Ordering::Relaxed) + 1;
                    let ok = ok_count.load(Ordering::Relaxed);
                    pb.inc(1);
                    pb.set_message(format!("ok={ok} last={} {}", item.name, row.status));
                    // Milestone lines so a non-TTY log (nohup) stays monitorable;
                    // they print above the bar in a terminal.
                    if n.is_multiple_of(250) || n == total {
                        let secs = pb.elapsed().as_secs_f64().max(1e-9);
                        let rate = n as f64 / secs;
                        let eta_h = (total - n) as f64 / rate / 3600.0;
                        pb.println(format!(
                            "[{n}/{total}] ok={ok} {:.0}/h eta={eta_h:.1}h",
                            rate * 3600.0
                        ));
                    }
                }
            });
        }
    });

    pb.finish_with_message("done");
    Ok(())
}

/// Downloads, unpacks, and extracts one crate, returning its `extractions` row.
fn process(
    item: &CrateVersion,
    slot: usize,
    dirs: &Dirs,
    agent: &ureq::Agent,
    exe: &Path,
    build_jobs: usize,
    timeout: Duration,
) -> Extraction {
    let mut row = Extraction {
        name: item.name.clone(),
        version: item.version.clone(),
        downloads: item.downloads,
        status: "pending".to_owned(),
        ir_path: None,
        n_items: None,
        n_modules: None,
        n_edges: None,
        n_import_edges: None,
        n_signature_edges: None,
        n_trait_bound_edges: None,
        n_impl_edges: None,
        n_body_edges: None,
        n_structs: None,
        n_enums: None,
        n_traits: None,
        n_type_aliases: None,
        n_functions: None,
        n_pub_api_items: None,
        elapsed_sec: None,
        prepare_sec: None,
        peak_rss_kb: None,
        crate_bytes: None,
        error: None,
        ra_version: None,
        schema_version: None,
        categories: non_empty(&item.categories),
        keywords: non_empty(&item.keywords),
        ts: now_secs(),
    };

    let prepare_started = Instant::now();
    let (crate_dir, crate_bytes) = match download_and_unpack(item, slot, dirs, agent) {
        Ok(out) => out,
        Err(e) => {
            row.status = "download_fail".to_owned();
            row.error = Some(format!("{e:#}"));
            return row;
        }
    };
    row.prepare_sec = Some(prepare_started.elapsed().as_secs_f64());
    row.crate_bytes = Some(crate_bytes as i64);

    let target = dirs.targets.join(format!("slot{slot}"));
    let started = Instant::now();
    let (outcome, peak_rss_kb) = run_worker(
        exe,
        &crate_dir,
        &target,
        &dirs.cargo_home,
        build_jobs,
        timeout,
    );
    row.elapsed_sec = Some(started.elapsed().as_secs_f64());
    if peak_rss_kb > 0 {
        row.peak_rss_kb = Some(peak_rss_kb as i64);
    }
    let _ = fs::remove_dir_all(dirs.work.join(format!("slot{slot}")));

    match outcome {
        WorkerOutcome::Timeout => {
            row.status = "timeout".to_owned();
            row.error = Some(format!("exceeded {}s", timeout.as_secs()));
        }
        WorkerOutcome::Spawn(e) => {
            row.status = "spawn_fail".to_owned();
            row.error = Some(e);
        }
        WorkerOutcome::Failed(msg) => {
            row.status = "extract_fail".to_owned();
            row.error = Some(msg);
        }
        WorkerOutcome::Ok(ir_json) => match ir_summary(&ir_json) {
            Some(summary) => {
                let ir_path = dirs
                    .ir
                    .join(format!("{}-{}.ir.json", item.name, item.version));
                if let Err(e) = fs::write(&ir_path, &ir_json) {
                    row.status = "extract_fail".to_owned();
                    row.error = Some(format!("writing IR: {e}"));
                } else {
                    row.status = "ok".to_owned();
                    row.ir_path = Some(ir_path.to_string_lossy().into_owned());
                    row.n_items = Some(summary.n_items);
                    row.n_modules = Some(summary.n_modules);
                    row.n_edges = Some(summary.n_edges);
                    row.ra_version = summary.ra_version;
                    row.schema_version = summary.schema_version;
                    row.n_import_edges = Some(summary.n_import_edges);
                    row.n_signature_edges = Some(summary.n_signature_edges);
                    row.n_trait_bound_edges = Some(summary.n_trait_bound_edges);
                    row.n_impl_edges = Some(summary.n_impl_edges);
                    row.n_body_edges = Some(summary.n_body_edges);
                    row.n_structs = Some(summary.n_structs);
                    row.n_enums = Some(summary.n_enums);
                    row.n_traits = Some(summary.n_traits);
                    row.n_type_aliases = Some(summary.n_type_aliases);
                    row.n_functions = Some(summary.n_functions);
                    row.n_pub_api_items = Some(summary.n_pub_api_items);
                }
            }
            None => {
                row.status = "parse_fail".to_owned();
                row.error = Some("IR JSON missing expected arrays".to_owned());
            }
        },
    }
    row
}

/// Ensures the crate tarball is cached, then unpacks it, returning the crate
/// source directory and the `.crate` tarball size in bytes.
fn download_and_unpack(
    item: &CrateVersion,
    slot: usize,
    dirs: &Dirs,
    agent: &ureq::Agent,
) -> Result<(PathBuf, u64)> {
    let tarball = dirs
        .dl
        .join(format!("{}-{}.crate", item.name, item.version));
    if !tarball.exists() {
        let url = format!(
            "https://static.crates.io/crates/{name}/{name}-{ver}.crate",
            name = item.name,
            ver = item.version
        );
        let bytes = http::get_bytes(agent, &url)?;
        fs::write(&tarball, &bytes).with_context(|| format!("writing {}", tarball.display()))?;
    }
    let crate_bytes = fs::metadata(&tarball).map(|m| m.len()).unwrap_or(0);
    let work = dirs.work.join(format!("slot{slot}"));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let file = fs::File::open(&tarball)?;
    Archive::new(GzDecoder::new(file))
        .unpack(&work)
        .with_context(|| format!("unpacking {}", tarball.display()))?;
    Ok((
        work.join(format!("{}-{}", item.name, item.version)),
        crate_bytes,
    ))
}

/// The result of running the extraction subprocess.
enum WorkerOutcome {
    Ok(Vec<u8>),
    Failed(String),
    Timeout,
    Spawn(String),
}

/// Spawns `extract-one` as an isolated, group-led subprocess and waits with a
/// hard timeout, killing the whole group on expiry. Returns the outcome and the
/// peak resident memory (KiB) of the extractor process, sampled while it runs.
fn run_worker(
    exe: &Path,
    crate_dir: &Path,
    target: &Path,
    cargo_home: &Path,
    build_jobs: usize,
    timeout: Duration,
) -> (WorkerOutcome, u64) {
    let mut cmd = Command::new(exe);
    cmd.arg("extract-one")
        .arg(crate_dir)
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_BUILD_JOBS", build_jobs.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0); // own process group, so we can signal the whole tree

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (WorkerOutcome::Spawn(e.to_string()), 0),
    };
    let pid = child.id() as i32;

    let killed = std::sync::Arc::new(AtomicBool::new(false));
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let peak_rss_kb = std::sync::Arc::new(AtomicU64::new(0));
    // The watchdog both enforces the timeout (group SIGKILL) and samples peak
    // resident memory from /proc while the extractor runs.
    let watchdog = {
        let (killed, finished, peak) = (killed.clone(), finished.clone(), peak_rss_kb.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(kb) = read_peak_rss_kb(pid) {
                    peak.fetch_max(kb, Ordering::Relaxed);
                }
                if finished.load(Ordering::Acquire) {
                    return;
                }
                if Instant::now() >= deadline {
                    killed.store(true, Ordering::Release);
                    // SAFETY: sending SIGKILL to the child's process group.
                    // `-pid` targets the group led by the child
                    // (process_group(0) above).
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let output = child.wait_with_output();
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();
    let peak = peak_rss_kb.load(Ordering::Relaxed);

    if killed.load(Ordering::Acquire) {
        return (WorkerOutcome::Timeout, peak);
    }
    let outcome = match output {
        Ok(out) if out.status.success() => WorkerOutcome::Ok(out.stdout),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
            WorkerOutcome::Failed(
                last.map(str::to_owned)
                    .unwrap_or_else(|| format!("exit {:?}", out.status.code())),
            )
        }
        Err(e) => WorkerOutcome::Spawn(e.to_string()),
    };
    (outcome, peak)
}

/// Reads a process's peak resident set size (`VmHWM`) from `/proc/<pid>/status`,
/// in KiB. `None` once the process has exited (its `/proc` entry is gone).
fn read_peak_rss_kb(pid: i32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// A lightweight summary of a serialized `CrateGraph`: node/edge counts, the
/// edge-kind and item-kind composition, the public-API item count, and the
/// provenance fields, all read without fully modelling the IR.
struct IrSummary {
    n_items: i32,
    n_modules: i32,
    n_edges: i32,
    ra_version: Option<String>,
    schema_version: Option<i32>,
    n_import_edges: i32,
    n_signature_edges: i32,
    n_trait_bound_edges: i32,
    n_impl_edges: i32,
    n_body_edges: i32,
    n_structs: i32,
    n_enums: i32,
    n_traits: i32,
    n_type_aliases: i32,
    n_functions: i32,
    n_pub_api_items: i32,
}

fn ir_summary(ir_json: &[u8]) -> Option<IrSummary> {
    let v: serde_json::Value = serde_json::from_slice(ir_json).ok()?;
    let items = v.get("items")?.as_array()?;
    let edges = v.get("edges")?.as_array()?;
    let modules = v.get("modules")?.as_array()?;

    // Tally edge kinds and item kinds by their serde tag.
    let tag = |val: &serde_json::Value| val.get("kind").and_then(|x| x.as_str()).map(str::to_owned);
    let edge_kind = |k: &str| {
        edges
            .iter()
            .filter(|e| tag(e).as_deref() == Some(k))
            .count() as i32
    };
    let item_kind = |k: &str| {
        items
            .iter()
            .filter(|i| tag(i).as_deref() == Some(k))
            .count() as i32
    };
    let n_pub_api_items = items
        .iter()
        .filter(|i| i.get("reachable_pub_api").and_then(|x| x.as_bool()) == Some(true))
        .count() as i32;

    Some(IrSummary {
        n_items: items.len() as i32,
        n_modules: modules.len() as i32,
        n_edges: edges.len() as i32,
        ra_version: v
            .get("ra_version")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        schema_version: v
            .get("schema_version")
            .and_then(|x| x.as_i64())
            .map(|x| x as i32),
        n_import_edges: edge_kind("Import"),
        n_signature_edges: edge_kind("Signature"),
        n_trait_bound_edges: edge_kind("TraitBound"),
        n_impl_edges: edge_kind("Impl"),
        n_body_edges: edge_kind("Body"),
        n_structs: item_kind("Struct"),
        n_enums: item_kind("Enum"),
        n_traits: item_kind("Trait"),
        n_type_aliases: item_kind("TypeAlias"),
        n_functions: item_kind("Function"),
        n_pub_api_items,
    })
}

/// Downloads the db-dump if it is not already present.
fn ensure_dump(agent: &ureq::Agent, path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    println!("downloading db-dump -> {} (large) ...", path.display());
    let bytes = http::get_bytes(agent, "https://static.crates.io/db-dump.tar.gz")?;
    fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Resolves the database file path (absolute, or under the corpus root).
pub fn db_file(root: &Path, db_path: &str) -> String {
    let p = Path::new(db_path);
    if p.is_absolute() {
        db_path.to_owned()
    } else {
        root.join(db_path).to_string_lossy().into_owned()
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A progress bar for a per-crate loop. On a non-TTY (a redirected log) the live
/// bar hides itself, so callers also emit periodic milestone `println`s.
pub(crate) fn progress_bar(len: u64) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} ({percent}%) {per_sec} eta {eta} {msg}",
        )
        .expect("valid progress template")
        .progress_chars("=>-"),
    );
    pb
}

/// `None` for an empty metadata string, so absent tags are stored as SQL NULL.
fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::ir_summary;

    #[test]
    fn ir_summary_reads_counts_and_provenance() {
        let json = br#"{"items":[1,2,3],"modules":[1],"edges":[],
            "ra_version":"0.0.336","schema_version":2,"extra":9}"#;
        let s = ir_summary(json).expect("valid IR");
        assert_eq!((s.n_items, s.n_modules, s.n_edges), (3, 1, 0));
        assert_eq!(s.ra_version.as_deref(), Some("0.0.336"));
        assert_eq!(s.schema_version, Some(2));
    }

    #[test]
    fn ir_summary_tallies_edge_and_item_composition() {
        let json = br#"{
            "items":[{"kind":"Struct","reachable_pub_api":true},
                     {"kind":"Trait","reachable_pub_api":false},
                     {"kind":"Function","reachable_pub_api":true},
                     {"kind":"TypeAlias","reachable_pub_api":false}],
            "modules":[{}],
            "edges":[{"kind":"Body"},{"kind":"Body"},{"kind":"Signature"},
                     {"kind":"Impl"},{"kind":"Import"}]
        }"#;
        let s = ir_summary(json).expect("valid IR");
        assert_eq!(
            (
                s.n_structs,
                s.n_enums,
                s.n_traits,
                s.n_type_aliases,
                s.n_functions
            ),
            (1, 0, 1, 1, 1)
        );
        assert_eq!(
            (
                s.n_body_edges,
                s.n_signature_edges,
                s.n_import_edges,
                s.n_impl_edges,
                s.n_trait_bound_edges
            ),
            (2, 1, 1, 1, 0)
        );
        assert_eq!(s.n_pub_api_items, 2);
    }

    #[test]
    fn ir_summary_tolerates_missing_provenance() {
        let s = ir_summary(br#"{"items":[],"modules":[],"edges":[],"ra_version":""}"#)
            .expect("valid IR");
        assert_eq!((s.n_items, s.n_modules, s.n_edges), (0, 0, 0));
        assert_eq!(s.ra_version, None); // empty string normalized to None
        assert_eq!(s.schema_version, None);
    }

    #[test]
    fn ir_summary_rejects_malformed_or_incomplete_ir() {
        assert!(ir_summary(b"{}").is_none());
        assert!(ir_summary(b"not json").is_none());
        // Missing the `edges` array.
        assert!(ir_summary(br#"{"items":[],"modules":[]}"#).is_none());
    }
}
