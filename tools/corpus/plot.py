#!/usr/bin/env python3
"""Plot histograms of each modularity metric over the corpus (sweep.db / corpus.db).

Usage:
    uv run --with matplotlib python3 tools/corpus/plot.py [--db PATH] [--out PATH]
"""

import argparse
import sqlite3
import statistics as st

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

PANELS = [
    ("headline", "Headline score", (0, 1)),
    ("modularity_term", "Modularity term", (0, 1)),
    ("divergence_term", "Divergence term", (0, 1)),
    ("acyclicity_term", "Acyclicity term", (0, 1)),
    ("encapsulation_term", "Encapsulation term", (0, 1)),
    ("over_exposed_fraction", "Over-exposed fraction", (0, 1)),
    ("mean_leak_cost", "Mean leak cost", (0, 1)),
    ("n_items", "Items per crate (log)", None),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", default="/mnt/nvme/modula-corpus/sweep.db")
    ap.add_argument("--out", default="/mnt/nvme/modula-corpus/plots/metrics_histograms.png")
    args = ap.parse_args()

    db = sqlite3.connect(args.db)
    ok = db.execute("SELECT COUNT(*) FROM results WHERE status='ok'").fetchone()[0]

    import os

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    fig, axes = plt.subplots(2, 4, figsize=(20, 9))
    fig.suptitle(f"modula-rs metric distributions over {ok} crates (>=100k downloads)", fontsize=15)

    for ax, (col, title, rng) in zip(axes.flat, PANELS):
        xs = [r[0] for r in db.execute(f"SELECT {col} FROM results WHERE status='ok' AND {col} IS NOT NULL")]
        n = len(xs)
        if col == "n_items":
            xs = [x for x in xs if x > 0]
            bins = np.logspace(0, np.log10(max(xs)), 50)
            ax.hist(xs, bins=bins, color="#4477aa", edgecolor="white", linewidth=0.3)
            ax.set_xscale("log")
        else:
            ax.hist(xs, bins=np.linspace(rng[0], rng[1], 51), color="#4477aa", edgecolor="white", linewidth=0.3)
            med = st.median(xs)
            ax.axvline(med, color="#cc3311", linestyle="--", linewidth=1.5, label=f"median {med:.3f}")
            ax.legend(loc="upper right", fontsize=9)
        ax.set_title(f"{title}  (n={n})", fontsize=11)
        ax.set_ylabel("crates")
        ax.grid(axis="y", alpha=0.25)

    fig.tight_layout(rect=(0, 0, 1, 0.97))
    fig.savefig(args.out, dpi=110)
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
