#!/usr/bin/env python3
"""Build a modularity-metrics corpus by running cargo-modula over the most-
downloaded crates on crates.io.

Pure standard library (urllib, tarfile, sqlite3, subprocess) so it runs with
`uv run` or plain python3 and adds no dependency to the Rust workspace.

Pipeline, per crate:
  1. list top-N crates by downloads from the crates.io API,
  2. download the .crate tarball from the static.crates.io CDN,
  3. extract it,
  4. run `cargo-modula modula <dir> --json` in a subprocess with a hard timeout,
  5. store the metrics (or the failure reason) in SQLite,
  6. delete the extracted source (the shared target dir is kept for dep reuse).

Resumable: crates already recorded in the DB are skipped.
"""

import argparse
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request

API = "https://crates.io/api/v1/crates"
CDN = "https://static.crates.io/crates/{name}/{name}-{version}.crate"
UA = "modula-rs-corpus (https://github.com/LucaCappelletti94/modula-rs; cappelletti.luca94@gmail.com)"

SCHEMA = """
CREATE TABLE IF NOT EXISTS results (
    name                  TEXT NOT NULL,
    version               TEXT NOT NULL,
    rank                  INTEGER,
    downloads             INTEGER,
    status                TEXT NOT NULL,   -- ok | timeout | extract_fail | download_fail | parse_fail
    elapsed_sec           REAL,
    n_items               INTEGER,
    n_modules             INTEGER,
    n_module_nodes        INTEGER,
    headline              REAL,
    modularity_term       REAL,
    divergence_term       REAL,
    acyclicity_term       REAL,
    encapsulation_term    REAL,
    is_acyclic            INTEGER,
    over_exposed_fraction REAL,
    mean_leak_cost        REAL,
    error                 TEXT,
    ts                    REAL,
    PRIMARY KEY (name, version)
);
"""


def http_get(url, accept_json):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=60) as resp:
        data = resp.read()
    return json.loads(data) if accept_json else data


def top_crates(limit):
    """Yield (rank, name, version, downloads) for the top `limit` crates by downloads."""
    fetched = 0
    page = 1
    while fetched < limit:
        per_page = min(100, limit - fetched)
        url = f"{API}?page={page}&per_page={per_page}&sort=downloads"
        payload = http_get(url, accept_json=True)
        crates = payload.get("crates", [])
        if not crates:
            break
        for c in crates:
            fetched += 1
            yield fetched, c["name"], c["max_stable_version"] or c["max_version"], c.get("downloads", 0)
        page += 1
        time.sleep(1.0)  # crates.io crawler policy: <= 1 request/sec


def download_crate(name, version, dest):
    url = CDN.format(name=name, version=version)
    data = http_get(url, accept_json=False)
    with open(dest, "wb") as f:
        f.write(data)


def extract_crate(tarball, workdir):
    """Extract the .crate (gzip tar) and return the top-level crate directory."""
    with tarfile.open(tarball, "r:gz") as tar:
        members = tar.getnames()
        tar.extractall(workdir, filter="data")
    # A published .crate always has a single top-level `<name>-<version>/` dir.
    top = members[0].split("/", 1)[0]
    return os.path.join(workdir, top)


def run_modula(binary, crate_dir, target_dir, cargo_home, timeout):
    """Run cargo-modula on a crate. Returns (status, json_or_None, error_or_None, elapsed)."""
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = target_dir
    env["CARGO_HOME"] = cargo_home
    start = time.monotonic()
    try:
        proc = subprocess.Popen(
            [binary, "modula", crate_dir, "--json"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            start_new_session=True,  # own process group so we can kill cargo's children
        )
        try:
            out, err = proc.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            proc.communicate()
            return "timeout", None, f"exceeded {timeout}s", time.monotonic() - start
    except OSError as exc:
        return "extract_fail", None, f"spawn failed: {exc}", time.monotonic() - start

    elapsed = time.monotonic() - start
    if proc.returncode != 0:
        msg = err.decode("utf-8", "replace").strip().splitlines()
        return "extract_fail", None, (msg[-1] if msg else f"exit {proc.returncode}"), elapsed
    try:
        return "ok", json.loads(out), None, elapsed
    except json.JSONDecodeError as exc:
        return "parse_fail", None, str(exc), elapsed


def row_from_json(j):
    c = j.get("composite", {})
    enc = j.get("encapsulation", {})
    tan = j.get("tangles", {})
    return {
        "n_items": j.get("n_items"),
        "n_modules": j.get("n_modules"),
        "n_module_nodes": j.get("n_module_nodes"),
        "headline": c.get("headline"),
        "modularity_term": c.get("modularity_term"),
        "divergence_term": c.get("divergence_term"),
        "acyclicity_term": c.get("acyclicity_term"),
        "encapsulation_term": c.get("encapsulation_term"),
        "is_acyclic": 1 if tan.get("is_acyclic") else 0,
        "over_exposed_fraction": enc.get("over_exposed_fraction"),
        "mean_leak_cost": enc.get("mean_leak_cost"),
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--root", default="/mnt/nvme/modula-corpus", help="corpus working directory (use a fast, roomy disk)")
    ap.add_argument("--limit", type=int, default=100, help="number of top crates to analyze")
    ap.add_argument("--timeout", type=int, default=900, help="per-crate hard timeout in seconds")
    ap.add_argument("--bin", default=None, help="path to cargo-modula (default: ../../target/release/cargo-modula)")
    ap.add_argument("--keep-sources", action="store_true", help="do not delete extracted crate sources")
    args = ap.parse_args()

    root = os.path.abspath(args.root)
    binary = args.bin or os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "target", "release", "cargo-modula"))
    if not os.path.exists(binary):
        sys.exit(f"cargo-modula not found at {binary}; build it with `cargo build --release -p cargo-modula`")

    dl_dir = os.path.join(root, "dl")
    work_dir = os.path.join(root, "work")
    target_dir = os.path.join(root, "target")
    cargo_home = os.path.join(root, "cargo-home")
    for d in (dl_dir, work_dir, target_dir, cargo_home):
        os.makedirs(d, exist_ok=True)

    db = sqlite3.connect(os.path.join(root, "corpus.db"))
    db.executescript(SCHEMA)
    done = {name for (name,) in db.execute("SELECT name FROM results")}

    print(f"corpus root: {root}")
    print(f"binary:      {binary}")
    print(f"already done: {len(done)} | target: top {args.limit} by downloads\n")

    for rank, name, version, downloads in top_crates(args.limit):
        if name in done:
            print(f"[{rank:>4}] {name} {version}  (skip, already recorded)")
            continue
        label = f"[{rank:>4}] {name} {version}"
        row = {
            "name": name, "version": version, "rank": rank, "downloads": downloads,
            "status": None, "elapsed_sec": None, "error": None, "ts": time.time(),
            "n_items": None, "n_modules": None, "n_module_nodes": None, "headline": None,
            "modularity_term": None, "divergence_term": None, "acyclicity_term": None,
            "encapsulation_term": None, "is_acyclic": None, "over_exposed_fraction": None,
            "mean_leak_cost": None,
        }
        tarball = os.path.join(dl_dir, f"{name}-{version}.crate")
        crate_dir = None
        try:
            if not os.path.exists(tarball):
                download_crate(name, version, tarball)
            crate_dir = extract_crate(tarball, work_dir)
        except (urllib.error.URLError, OSError, tarfile.TarError) as exc:
            row["status"] = "download_fail"
            row["error"] = str(exc)
        else:
            status, j, err, elapsed = run_modula(binary, crate_dir, target_dir, cargo_home, args.timeout)
            row["status"] = status
            row["elapsed_sec"] = round(elapsed, 1)
            row["error"] = err
            if status == "ok":
                row.update(row_from_json(j))

        cols = ", ".join(row.keys())
        marks = ", ".join("?" for _ in row)
        db.execute(f"INSERT OR REPLACE INTO results ({cols}) VALUES ({marks})", list(row.values()))
        db.commit()

        if not args.keep_sources and crate_dir and os.path.isdir(crate_dir):
            shutil.rmtree(crate_dir, ignore_errors=True)

        if row["status"] == "ok":
            print(f"{label}  ok  headline={row['headline']:.3f}  items={row['n_items']}  {row['elapsed_sec']}s")
        else:
            print(f"{label}  {row['status']}  ({row['error']})  {row.get('elapsed_sec')}s")

    db.close()


if __name__ == "__main__":
    main()
