# /// script
# requires-python = ">=3.10"
# dependencies = ["scikit-learn", "numpy"]
# ///
"""Generate scikit-learn ground truth for the divergence measures.

Run with: uv run crates/modula-metrics/tests/fixtures/gen_divergence.py

VI is not provided by scikit-learn, so it is derived from mutual_info_score and
the per-partition entropies (all in nats). NMI and AMI use average_method="max"
to match the Rust implementation's normalizer.
"""

import json
from pathlib import Path

import numpy as np
from sklearn.metrics import (
    adjusted_mutual_info_score,
    adjusted_rand_score,
    mutual_info_score,
    normalized_mutual_info_score,
)


def entropy(labels):
    _, counts = np.unique(labels, return_counts=True)
    p = counts / counts.sum()
    return float(-(p * np.log(p)).sum())


def case(name, a, b):
    mi = mutual_info_score(a, b)
    ha, hb = entropy(a), entropy(b)
    vi = ha + hb - 2.0 * mi
    return {
        "name": name,
        "a": list(a),
        "b": list(b),
        "vi": float(vi),
        "nmi": float(normalized_mutual_info_score(a, b, average_method="max")),
        "ami": float(adjusted_mutual_info_score(a, b, average_method="max")),
        "ari": float(adjusted_rand_score(a, b)),
    }


CASES = [
    ("identical", [0, 0, 1, 1], [0, 0, 1, 1]),
    ("relabeled_identical", [0, 0, 1, 1, 2], [2, 2, 0, 0, 1]),
    ("crossing_independent", [0, 0, 1, 1], [0, 1, 0, 1]),
    ("one_merged", [0, 0, 1, 1], [0, 0, 0, 0]),
    ("nested_refinement", [0, 0, 0, 0], [0, 0, 1, 1]),
    ("all_singletons", [0, 1, 2, 3], [0, 1, 2, 3]),
    ("both_single_cluster", [0, 0, 0], [0, 0, 0]),
    ("partial_overlap", [0, 0, 0, 1, 1, 1], [0, 0, 1, 1, 2, 2]),
    (
        "twelve_mixed",
        [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2],
        [0, 0, 1, 1, 1, 1, 2, 2, 2, 0, 0, 0],
    ),
    (
        "twelve_skewed",
        [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 3],
        [0, 1, 1, 1, 2, 2, 2, 2, 2, 2, 0, 0],
    ),
]


def main():
    data = [case(name, a, b) for name, a, b in CASES]
    out = Path(__file__).with_name("divergence_sklearn.json")
    out.write_text(json.dumps(data, indent=2) + "\n")
    print(f"wrote {len(data)} cases to {out}")


if __name__ == "__main__":
    main()
