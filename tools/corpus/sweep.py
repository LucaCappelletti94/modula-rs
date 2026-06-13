#!/usr/bin/env python3
"""Large-scale modularity-metrics sweep over crates.io.

Unlike `run.py` (which hits the API for a small top-N run), this enumerates the
whole registry from the crates.io database dump, filters by download count, and
runs cargo-modula over the result in parallel, building a SQLite corpus for
weight calibration and metric-bug hunting.

Design notes for scale:
- Enumeration: the daily db-dump (`db-dump.tar.gz`, CSV) is parsed offline; we
  pick each crate's highest non-yanked, non-prerelease version.
- Parallelism: a worker pool. Each worker owns its own CARGO_TARGET_DIR (cargo
  locks a target dir, so a shared one would serialize), while CARGO_HOME (the
  downloaded-crate registry cache) is shared. Per-crate CARGO_BUILD_JOBS is
  capped so N concurrent cargo runs do not oversubscribe the machine.
- Robustness: every crate is a subprocess with a hard timeout; all failure modes
  are recorded. Resumable: crates already in the DB are skipped.
- Anomaly capture: out-of-range / non-finite terms are flagged for bug hunting.

Pure standard library.
"""

import argparse
import csv
import json
import math
import os
import queue
import shutil
import signal
import sqlite3
import subprocess
import sys
import tarfile
import threading
import time
import urllib.error
import urllib.request

DUMP_URL = "https://static.crates.io/db-dump.tar.gz"
CDN = "https://static.crates.io/crates/{name}/{name}-{version}.crate"
UA = "modula-rs-corpus (https://github.com/LucaCappelletti94/modula-rs; cappelletti.luca94@gmail.com)"

csv.field_size_limit(1 << 30)

SCHEMA = """
CREATE TABLE IF NOT EXISTS results (
    name TEXT NOT NULL, version TEXT NOT NULL, downloads INTEGER,
    status TEXT NOT NULL, elapsed_sec REAL,
    n_items INTEGER, n_real_items INTEGER, n_modules INTEGER, n_module_nodes INTEGER,
    headline REAL, modularity_term REAL, divergence_term REAL,
    acyclicity_term REAL, encapsulation_term REAL,
    is_acyclic INTEGER, over_exposed_fraction REAL, mean_leak_cost REAL,
    anomaly TEXT, error TEXT, ts REAL,
    PRIMARY KEY (name, version)
);
"""


def http_get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=120) as resp:
        return resp.read()


def ensure_dump(path):
    if not os.path.exists(path):
        print(f"downloading db-dump -> {path} (this is large) ...", flush=True)
        data = http_get(DUMP_URL)
        with open(path, "wb") as f:
            f.write(data)
    return path


def build_worklist(dump_path, min_downloads):
    """Parse the db-dump and return [(name, version, downloads)] above the cutoff.

    Downloads live in `crate_downloads.csv`; the canonical version per crate is
    `default_versions.csv` (crate_id -> version_id) resolved through
    `versions.csv` (id -> num), exactly the version cargo would pick.
    """
    with tarfile.open(dump_path, "r:gz") as tar:
        members = {
            m.name.split("/data/")[1]: m
            for m in tar.getmembers()
            if "/data/" in m.name and m.name.endswith(".csv")
        }

        def rows(fname):
            with tar.extractfile(members[fname]) as fh:
                yield from csv.DictReader((line.decode("utf-8", "replace") for line in fh))

        downloads = {
            r["crate_id"]: int(r["downloads"])
            for r in rows("crate_downloads.csv")
            if int(r["downloads"]) >= min_downloads
        }
        names = {r["id"]: r["name"] for r in rows("crates.csv") if r["id"] in downloads}
        default_vid = {
            r["crate_id"]: r["version_id"]
            for r in rows("default_versions.csv")
            if r["crate_id"] in downloads
        }
        wanted = set(default_vid.values())
        vid_num = {r["id"]: r["num"] for r in rows("versions.csv") if r["id"] in wanted}

    work = []
    for cid, name in names.items():
        num = vid_num.get(default_vid.get(cid, ""))
        if num:
            work.append((name, num, downloads[cid]))
    work.sort(key=lambda w: -w[2])  # most-downloaded first
    return work


def extract_crate(tarball, workdir):
    with tarfile.open(tarball, "r:gz") as tar:
        members = tar.getnames()
        tar.extractall(workdir, filter="data")
    return os.path.join(workdir, members[0].split("/", 1)[0])


def run_modula(binary, crate_dir, target_dir, cargo_home, build_jobs, timeout):
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = target_dir
    env["CARGO_HOME"] = cargo_home
    env["CARGO_BUILD_JOBS"] = str(build_jobs)
    start = time.monotonic()
    try:
        proc = subprocess.Popen(
            [binary, "modula", crate_dir, "--json"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, start_new_session=True,
        )
        try:
            out, err = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            proc.communicate()
            return "timeout", None, f"exceeded {timeout}s", time.monotonic() - start
    except OSError as exc:
        return "spawn_fail", None, str(exc), time.monotonic() - start
    elapsed = time.monotonic() - start
    if proc.returncode != 0:
        msg = err.decode("utf-8", "replace").strip().splitlines()
        return "extract_fail", None, (msg[-1] if msg else f"exit {proc.returncode}"), elapsed
    try:
        return "ok", json.loads(out), None, elapsed
    except json.JSONDecodeError as exc:
        return "parse_fail", None, str(exc), elapsed


def anomalies(row):
    """Flags pathological metric values (the point of the bug hunt)."""
    flags = []
    terms = {
        "headline": row["headline"], "modularity_term": row["modularity_term"],
        "divergence_term": row["divergence_term"], "acyclicity_term": row["acyclicity_term"],
        "encapsulation_term": row["encapsulation_term"],
        "over_exposed_fraction": row["over_exposed_fraction"], "mean_leak_cost": row["mean_leak_cost"],
    }
    for name, v in terms.items():
        if v is None:
            continue
        if not math.isfinite(v):
            flags.append(f"{name}=nonfinite")
        elif name != "mean_leak_cost" and not (-1e-9 <= v <= 1 + 1e-9):
            flags.append(f"{name}={v:.3f}_oob")
    return ",".join(flags) or None


def row_from_json(j):
    c, enc, tan = j.get("composite", {}), j.get("encapsulation", {}), j.get("tangles", {})
    return {
        "n_items": j.get("n_items"), "n_real_items": j.get("n_real_items"),
        "n_modules": j.get("n_modules"),
        "n_module_nodes": j.get("n_module_nodes"),
        "headline": c.get("headline"), "modularity_term": c.get("modularity_term"),
        "divergence_term": c.get("divergence_term"), "acyclicity_term": c.get("acyclicity_term"),
        "encapsulation_term": c.get("encapsulation_term"),
        "is_acyclic": 1 if tan.get("is_acyclic") else 0,
        "over_exposed_fraction": enc.get("over_exposed_fraction"), "mean_leak_cost": enc.get("mean_leak_cost"),
    }


def process(name, version, downloads, dirs, binary, build_jobs, timeout, slot):
    row = {
        "name": name, "version": version, "downloads": downloads,
        "status": None, "elapsed_sec": None, "anomaly": None, "error": None, "ts": time.time(),
        "n_items": None, "n_real_items": None, "n_modules": None, "n_module_nodes": None, "headline": None,
        "modularity_term": None, "divergence_term": None, "acyclicity_term": None,
        "encapsulation_term": None, "is_acyclic": None, "over_exposed_fraction": None, "mean_leak_cost": None,
    }
    tarball = os.path.join(dirs["dl"], f"{name}-{version}.crate")
    work = os.path.join(dirs["work"], f"{slot}-{name}-{version}")
    crate_dir = None
    try:
        if not os.path.exists(tarball):
            with open(tarball, "wb") as f:
                f.write(http_get(CDN.format(name=name, version=version)))
        shutil.rmtree(work, ignore_errors=True)
        crate_dir = extract_crate(tarball, work)
    except (urllib.error.URLError, OSError, tarfile.TarError, StopIteration) as exc:
        row["status"], row["error"] = "download_fail", str(exc)
    else:
        target = os.path.join(dirs["targets"], f"slot{slot}")
        status, j, err, elapsed = run_modula(
            binary, crate_dir, target, dirs["cargo_home"], build_jobs, timeout
        )
        row["status"], row["elapsed_sec"], row["error"] = status, round(elapsed, 1), err
        if status == "ok":
            row.update(row_from_json(j))
            row["anomaly"] = anomalies(row)
    shutil.rmtree(work, ignore_errors=True)
    return row


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--root", default="/mnt/nvme/modula-corpus")
    ap.add_argument("--min-downloads", type=int, default=100_000)
    ap.add_argument("--jobs", type=int, default=12, help="concurrent crates")
    ap.add_argument("--timeout", type=int, default=1200, help="per-crate seconds")
    ap.add_argument("--limit", type=int, default=None, help="cap (for testing)")
    ap.add_argument("--bin", default=None)
    ap.add_argument("--db", default="sweep.db")
    ap.add_argument("--prepare-only", action="store_true", help="build the work-list and report, do not run")
    args = ap.parse_args()

    root = os.path.abspath(args.root)
    binary = args.bin or os.path.abspath(
        os.path.join(os.path.dirname(__file__), "..", "..", "target", "release", "cargo-modula")
    )
    if not args.prepare_only and not os.path.exists(binary):
        sys.exit(f"cargo-modula not found at {binary}")

    dirs = {
        "dl": os.path.join(root, "dl"), "work": os.path.join(root, "work"),
        "targets": os.path.join(root, "targets"), "cargo_home": os.path.join(root, "cargo-home"),
    }
    for d in dirs.values():
        os.makedirs(d, exist_ok=True)

    dump = ensure_dump(os.path.join(root, "db-dump.tar.gz"))
    print(f"parsing db-dump (>= {args.min_downloads} downloads) ...", flush=True)
    work = build_worklist(dump, args.min_downloads)
    print(f"work-list: {len(work)} crates", flush=True)
    if args.limit:
        work = work[: args.limit]
    if args.prepare_only:
        for name, ver, dl in work[:20]:
            print(f"  {name} {ver}  ({dl} downloads)")
        print(f"... ({len(work)} total)")
        return

    db = sqlite3.connect(os.path.join(root, args.db), check_same_thread=False)
    db.executescript("PRAGMA journal_mode=WAL;\n" + SCHEMA)
    done = {n for (n,) in db.execute("SELECT name FROM results")}
    todo = [w for w in work if w[0] not in done]
    print(f"already done: {len(done)} | to run: {len(todo)} | jobs: {args.jobs}", flush=True)

    build_jobs = max(2, (os.cpu_count() or 16) // args.jobs)
    slots = queue.Queue()
    for s in range(args.jobs):
        slots.put(s)
    db_lock = threading.Lock()
    counter = {"done": 0, "ok": 0}
    cstart = time.monotonic()

    def worker(item):
        name, version, downloads = item
        slot = slots.get()
        try:
            row = process(name, version, downloads, dirs, binary, build_jobs, args.timeout, slot)
        finally:
            slots.put(slot)
        with db_lock:
            cols = ", ".join(row.keys())
            marks = ", ".join("?" for _ in row)
            db.execute(f"INSERT OR REPLACE INTO results ({cols}) VALUES ({marks})", list(row.values()))
            db.commit()
            counter["done"] += 1
            if row["status"] == "ok":
                counter["ok"] += 1
            n = counter["done"]
            if n % 25 == 0 or n == len(todo):
                rate = n / (time.monotonic() - cstart)
                eta = (len(todo) - n) / rate / 3600 if rate else 0
                print(
                    f"[{n}/{len(todo)}] ok={counter['ok']} {rate*3600:.0f}/h eta={eta:.1f}h "
                    f"last={name} {row['status']}"
                    + (f" !{row['anomaly']}" if row.get("anomaly") else ""),
                    flush=True,
                )

    from concurrent.futures import ThreadPoolExecutor

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        list(pool.map(worker, todo))
    db.close()
    print("done.", flush=True)


if __name__ == "__main__":
    main()
