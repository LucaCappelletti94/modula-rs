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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use flate2::read::GzDecoder;
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

    println!("parsing db-dump (>= {} downloads) ...", args.min_downloads);
    let mut work = dump::build_worklist(
        dump_path.to_str().context("dump path not utf-8")?,
        args.min_downloads,
    )?;
    println!("work-list: {} crates", work.len());
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

    // Each concurrent crate gets ~ (cores / jobs) build threads so N parallel
    // cargo invocations do not oversubscribe the machine.
    let build_jobs = (num_cpus().max(2) / args.jobs.max(1)).max(2);

    let conn = Mutex::new(conn);
    let next = AtomicUsize::new(0);
    let done_count = AtomicUsize::new(0);
    let ok_count = AtomicUsize::new(0);
    let start = Instant::now();
    let total = todo.len();

    std::thread::scope(|scope| {
        for slot in 0..args.jobs {
            let (dirs, agent, exe, todo, next, conn, done_count, ok_count) = (
                &dirs,
                &agent,
                &exe,
                &todo,
                &next,
                &conn,
                &done_count,
                &ok_count,
            );
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = todo.get(i) else { break };
                    let row = process(item, slot, dirs, agent, exe, build_jobs, args.timeout);
                    let ok = row.status == "ok";
                    {
                        let mut conn = conn.lock().expect("db mutex");
                        if let Err(e) = db::upsert_extraction(&mut conn, &row) {
                            eprintln!("db write failed for {}-{}: {e:#}", row.name, row.version);
                        }
                    }
                    let n = done_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if ok {
                        ok_count.fetch_add(1, Ordering::Relaxed);
                    }
                    if n % 25 == 0 || n == total {
                        let rate = n as f64 / start.elapsed().as_secs_f64();
                        let eta_h = if rate > 0.0 {
                            (total - n) as f64 / rate / 3600.0
                        } else {
                            0.0
                        };
                        println!(
                            "[{}/{}] ok={} {:.0}/h eta={:.1}h last={} {}",
                            n,
                            total,
                            ok_count.load(Ordering::Relaxed),
                            rate * 3600.0,
                            eta_h,
                            item.name,
                            row.status,
                        );
                    }
                }
            });
        }
    });

    println!("done.");
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
        elapsed_sec: None,
        error: None,
        ts: now_secs(),
    };

    let crate_dir = match download_and_unpack(item, slot, dirs, agent) {
        Ok(dir) => dir,
        Err(e) => {
            row.status = "download_fail".to_owned();
            row.error = Some(format!("{e:#}"));
            return row;
        }
    };

    let target = dirs.targets.join(format!("slot{slot}"));
    let started = Instant::now();
    let outcome = run_worker(
        exe,
        &crate_dir,
        &target,
        &dirs.cargo_home,
        build_jobs,
        timeout,
    );
    row.elapsed_sec = Some(started.elapsed().as_secs_f64());
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
        WorkerOutcome::Ok(ir_json) => match counts(&ir_json) {
            Some((items, modules, edges)) => {
                let ir_path = dirs
                    .ir
                    .join(format!("{}-{}.ir.json", item.name, item.version));
                if let Err(e) = fs::write(&ir_path, &ir_json) {
                    row.status = "extract_fail".to_owned();
                    row.error = Some(format!("writing IR: {e}"));
                } else {
                    row.status = "ok".to_owned();
                    row.ir_path = Some(ir_path.to_string_lossy().into_owned());
                    row.n_items = Some(items);
                    row.n_modules = Some(modules);
                    row.n_edges = Some(edges);
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
/// source directory.
fn download_and_unpack(
    item: &CrateVersion,
    slot: usize,
    dirs: &Dirs,
    agent: &ureq::Agent,
) -> Result<PathBuf> {
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
    let work = dirs.work.join(format!("slot{slot}"));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let file = fs::File::open(&tarball)?;
    Archive::new(GzDecoder::new(file))
        .unpack(&work)
        .with_context(|| format!("unpacking {}", tarball.display()))?;
    Ok(work.join(format!("{}-{}", item.name, item.version)))
}

/// The result of running the extraction subprocess.
enum WorkerOutcome {
    Ok(Vec<u8>),
    Failed(String),
    Timeout,
    Spawn(String),
}

/// Spawns `extract-one` as an isolated, group-led subprocess and waits with a
/// hard timeout, killing the whole group on expiry.
fn run_worker(
    exe: &Path,
    crate_dir: &Path,
    target: &Path,
    cargo_home: &Path,
    build_jobs: usize,
    timeout: Duration,
) -> WorkerOutcome {
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
        Err(e) => return WorkerOutcome::Spawn(e.to_string()),
    };
    let pid = child.id() as i32;

    let killed = std::sync::Arc::new(AtomicBool::new(false));
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (killed, finished) = (killed.clone(), finished.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if finished.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if !finished.load(Ordering::Acquire) {
                killed.store(true, Ordering::Release);
                // SAFETY: sending SIGKILL to the child's process group. `-pid`
                // targets the group led by the child (process_group(0) above).
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        })
    };

    let output = child.wait_with_output();
    finished.store(true, Ordering::Release);
    let _ = watchdog.join();

    if killed.load(Ordering::Acquire) {
        return WorkerOutcome::Timeout;
    }
    match output {
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
    }
}

/// Counts `(items, modules, edges)` in a serialized `CrateGraph` without fully
/// modelling it.
fn counts(ir_json: &[u8]) -> Option<(i32, i32, i32)> {
    let v: serde_json::Value = serde_json::from_slice(ir_json).ok()?;
    let len = |k: &str| v.get(k)?.as_array().map(|a| a.len() as i32);
    Some((len("items")?, len("modules")?, len("edges")?))
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

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(16)
}

#[cfg(test)]
mod tests {
    use super::counts;

    #[test]
    fn counts_reads_array_lengths() {
        let json = br#"{"items":[1,2,3],"modules":[1],"edges":[],"extra":9}"#;
        assert_eq!(counts(json), Some((3, 1, 0)));
    }

    #[test]
    fn counts_rejects_malformed_or_incomplete_ir() {
        assert_eq!(counts(b"{}"), None);
        assert_eq!(counts(b"not json"), None);
        // Missing the `edges` array.
        assert_eq!(counts(br#"{"items":[],"modules":[]}"#), None);
    }
}
