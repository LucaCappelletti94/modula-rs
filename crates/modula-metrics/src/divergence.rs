//! Partition-comparison measures: Variation of Information, Normalized and
//! Adjusted Mutual Information, and the Adjusted Rand Index.
//!
//! These are pure functions over two equal-length label slices (two partitions
//! of the same items). They power the "how far do the imports diverge from the
//! module tree" metric by comparing the declared module partition against a
//! detector-discovered partition. They are invariant to how clusters are named,
//! and the adjusted variants are corrected for chance.
//!
//! The Normalized and Adjusted Mutual Information here use the `max(H(A), H(B))`
//! normalizer (scikit-learn's `average_method="max"`), and the implementation
//! is validated against scikit-learn to 1e-9.
//!
//! References:
//! - Meila (2007), Variation of Information.
//! - Vinh, Epps, Bailey (2010), AMI and the expected-MI chance correction.
//! - Hubert, Arabie (1985), Adjusted Rand Index.

use std::collections::HashMap;
use std::f64::consts::PI;

/// The full set of partition-comparison measures, computed from one shared
/// contingency table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divergence {
    /// Variation of Information, `H(A|B) + H(B|A)`. A metric, in `[0, ln n]`.
    pub vi: f64,
    /// Variation of Information divided by `ln n`, in `[0, 1]`.
    pub vi_normalized: f64,
    /// Normalized Mutual Information with the `max` normalizer, in `[0, 1]`.
    pub nmi: f64,
    /// Adjusted (chance-corrected) Mutual Information, `~0` for independent
    /// partitions, `1` for identical.
    pub ami: f64,
    /// Adjusted Rand Index, `~0` for independent, `1` for identical.
    pub ari: f64,
    /// `H(A|B)`: information about A still missing given B (A over-split vs B).
    pub h_a_given_b: f64,
    /// `H(B|A)`: information about B still missing given A (B over-split vs A).
    pub h_b_given_a: f64,
}

impl Divergence {
    /// Computes all measures comparing partition `a` against partition `b`.
    ///
    /// # Panics
    /// Panics if `a` and `b` differ in length.
    #[must_use]
    pub fn compute(a: &[usize], b: &[usize]) -> Divergence {
        let ct = Contingency::new(a, b);
        let (ha, hb) = ct.entropies();
        let mi = ct.mutual_information();
        let emi = ct.expected_mutual_information();

        let h_a_given_b = (ha - mi).max(0.0);
        let h_b_given_a = (hb - mi).max(0.0);
        let vi = (ha + hb - 2.0 * mi).max(0.0);
        let vi_normalized = if ct.n > 1 {
            vi / (ct.n as f64).ln()
        } else {
            0.0
        };

        Divergence {
            vi,
            vi_normalized,
            nmi: ct.nmi(ha, hb, mi),
            ami: ct.ami(ha, hb, mi, emi),
            ari: ct.ari(),
            h_a_given_b,
            h_b_given_a,
        }
    }
}

/// Variation of Information between two partitions.
#[must_use]
pub fn variation_of_information(a: &[usize], b: &[usize]) -> f64 {
    let ct = Contingency::new(a, b);
    let (ha, hb) = ct.entropies();
    let mi = ct.mutual_information();
    (ha + hb - 2.0 * mi).max(0.0)
}

/// Normalized Mutual Information (`max` normalizer) between two partitions.
#[must_use]
pub fn normalized_mutual_information(a: &[usize], b: &[usize]) -> f64 {
    let ct = Contingency::new(a, b);
    let (ha, hb) = ct.entropies();
    let mi = ct.mutual_information();
    ct.nmi(ha, hb, mi)
}

/// Adjusted Mutual Information (`max` normalizer) between two partitions.
#[must_use]
pub fn adjusted_mutual_information(a: &[usize], b: &[usize]) -> f64 {
    let ct = Contingency::new(a, b);
    let (ha, hb) = ct.entropies();
    let mi = ct.mutual_information();
    let emi = ct.expected_mutual_information();
    ct.ami(ha, hb, mi, emi)
}

/// Adjusted Rand Index between two partitions.
#[must_use]
pub fn adjusted_rand_index(a: &[usize], b: &[usize]) -> f64 {
    Contingency::new(a, b).ari()
}

/// A contingency table between two partitions of the same items.
struct Contingency {
    n: u64,
    /// Row sums: sizes of A's clusters.
    rows: Vec<u64>,
    /// Column sums: sizes of B's clusters.
    cols: Vec<u64>,
    /// Cell counts `n_ij`, sparse.
    cells: HashMap<(usize, usize), u64>,
}

impl Contingency {
    /// # Panics
    /// Panics if `a` and `b` differ in length.
    fn new(a: &[usize], b: &[usize]) -> Self {
        assert_eq!(a.len(), b.len(), "partitions must have equal length");
        let (ra, r) = relabel(a);
        let (rb, c) = relabel(b);
        let mut rows = vec![0u64; r];
        let mut cols = vec![0u64; c];
        let mut cells: HashMap<(usize, usize), u64> = HashMap::new();
        for (&i, &j) in ra.iter().zip(rb.iter()) {
            rows[i] += 1;
            cols[j] += 1;
            *cells.entry((i, j)).or_insert(0) += 1;
        }
        Contingency {
            n: a.len() as u64,
            rows,
            cols,
            cells,
        }
    }

    /// `(H(A), H(B))` in nats.
    fn entropies(&self) -> (f64, f64) {
        (entropy(&self.rows, self.n), entropy(&self.cols, self.n))
    }

    /// Mutual information `I(A, B)` in nats.
    fn mutual_information(&self) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        let n = self.n as f64;
        self.cells
            .iter()
            .map(|(&(i, j), &nij)| {
                let nij = nij as f64;
                let ai = self.rows[i] as f64;
                let bj = self.cols[j] as f64;
                (nij / n) * ((nij * n) / (ai * bj)).ln()
            })
            .sum()
    }

    /// Expected mutual information under the permutation model (Vinh 2010),
    /// computed in log space to avoid factorial overflow.
    fn expected_mutual_information(&self) -> f64 {
        if self.n < 2 {
            return 0.0;
        }
        let n = self.n;
        let nf = n as f64;
        let mut emi = 0.0;
        for &ai in &self.rows {
            for &bj in &self.cols {
                let lo = (ai + bj).saturating_sub(n).max(1);
                let hi = ai.min(bj);
                let (aif, bjf) = (ai as f64, bj as f64);
                for nij in lo..=hi {
                    let nijf = nij as f64;
                    let term = (nijf / nf) * ((nf * nijf) / (aif * bjf)).ln();
                    let log_weight = ln_factorial(ai)
                        + ln_factorial(bj)
                        + ln_factorial(n - ai)
                        + ln_factorial(n - bj)
                        - ln_factorial(n)
                        - ln_factorial(nij)
                        - ln_factorial(ai - nij)
                        - ln_factorial(bj - nij)
                        // Group as `(n + nij) - ai - bj` so the unsigned
                        // intermediates never underflow when `ai + bj > n`.
                        - ln_factorial(n + nij - ai - bj);
                    emi += term * log_weight.exp();
                }
            }
        }
        emi
    }

    /// `true` when both partitions are a single cluster (or both empty): a
    /// perfect, if degenerate, match.
    fn both_single(&self) -> bool {
        (self.rows.len() == 1 && self.cols.len() == 1) || self.n == 0
    }

    /// `true` when both partitions are all singletons: also a perfect match.
    fn both_singletons(&self) -> bool {
        self.n > 0 && self.rows.len() as u64 == self.n && self.cols.len() as u64 == self.n
    }

    /// Normalized Mutual Information with the `max` normalizer.
    fn nmi(&self, ha: f64, hb: f64, mi: f64) -> f64 {
        // scikit-learn returns 1.0 only for the both-single (or both-empty)
        // degenerate case; all-singletons falls out to 1.0 naturally.
        if self.both_single() {
            return 1.0;
        }
        let norm = ha.max(hb);
        if norm <= 0.0 { 0.0 } else { mi / norm }
    }

    /// Adjusted Mutual Information with the `max` normalizer.
    fn ami(&self, ha: f64, hb: f64, mi: f64, emi: f64) -> f64 {
        if self.both_single() || self.both_singletons() {
            return 1.0;
        }
        let denom = ha.max(hb) - emi;
        if denom.abs() < 1e-12 {
            0.0
        } else {
            (mi - emi) / denom
        }
    }

    /// Adjusted Rand Index (Hubert-Arabie).
    fn ari(&self) -> f64 {
        if self.both_single() || self.both_singletons() {
            return 1.0;
        }
        let sum_comb_c: f64 = self.cells.values().map(|&nij| comb2(nij)).sum();
        let sum_comb_a: f64 = self.rows.iter().map(|&ai| comb2(ai)).sum();
        let sum_comb_b: f64 = self.cols.iter().map(|&bj| comb2(bj)).sum();
        let comb_n = comb2(self.n);
        let expected = sum_comb_a * sum_comb_b / comb_n;
        let max_index = 0.5 * (sum_comb_a + sum_comb_b);
        if (max_index - expected).abs() < 1e-12 {
            0.0
        } else {
            (sum_comb_c - expected) / (max_index - expected)
        }
    }
}

/// Relabels arbitrary cluster ids to a contiguous `0..k` range (first-appearance
/// order) and returns the relabeled vector and the cluster count.
fn relabel(labels: &[usize]) -> (Vec<usize>, usize) {
    let mut map: HashMap<usize, usize> = HashMap::new();
    let mut out = Vec::with_capacity(labels.len());
    for &label in labels {
        let next = map.len();
        let id = *map.entry(label).or_insert(next);
        out.push(id);
    }
    (out, map.len())
}

/// Shannon entropy (nats) of a partition given its cluster sizes.
fn entropy(counts: &[u64], n: u64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let n = n as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.ln()
        })
        .sum()
}

/// `C(k, 2) = k(k-1)/2`.
fn comb2(k: u64) -> f64 {
    let k = k as f64;
    k * (k - 1.0) / 2.0
}

/// `ln(k!)`.
fn ln_factorial(k: u64) -> f64 {
    ln_gamma(k as f64 + 1.0)
}

/// Natural log of the gamma function via the Lanczos approximation (g = 7),
/// accurate to roughly 1e-15 relative error for the arguments used here.
fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const COEFFICIENTS: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];

    if x < 0.5 {
        // Reflection formula for the left half-plane.
        PI.ln() - (PI * x).sin().ln() - ln_gamma(1.0 - x)
    } else {
        let x = x - 1.0;
        let mut a = COEFFICIENTS[0];
        let t = x + G + 0.5;
        for (i, &coef) in COEFFICIENTS.iter().enumerate().skip(1) {
            a += coef / (x + i as f64);
        }
        0.5 * (2.0 * PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
    }
}

#[cfg(test)]
mod unit_tests {
    use super::{Divergence, comb2, entropy, ln_factorial, ln_gamma};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn comb2_matches_closed_form() {
        assert_eq!(comb2(0), 0.0);
        assert_eq!(comb2(1), 0.0);
        assert_eq!(comb2(2), 1.0);
        assert_eq!(comb2(4), 6.0); // 4*3/2
        assert_eq!(comb2(5), 10.0);
    }

    #[test]
    fn ln_gamma_matches_known_values() {
        assert!(close(ln_gamma(1.0), 0.0)); // gamma(1) = 1
        assert!(close(ln_gamma(2.0), 0.0)); // gamma(2) = 1
        assert!(close(ln_gamma(5.0), 24.0_f64.ln())); // gamma(5) = 4! = 24
        assert!(close(ln_gamma(0.5), std::f64::consts::PI.sqrt().ln())); // gamma(1/2) = sqrt(pi)
        assert!(close(ln_factorial(5), 120.0_f64.ln())); // 5! = 120
    }

    #[test]
    fn ln_gamma_reflection_branch() {
        use std::f64::consts::PI;
        // x < 0.5 takes the reflection formula. Verify it satisfies Euler's
        // reflection identity gamma(x) gamma(1-x) = pi / sin(pi x), i.e.
        // ln_gamma(x) + ln_gamma(1-x) == ln(pi) - ln(sin(pi x)). Only ln_gamma(x)
        // uses the mutated branch here (1-x = 0.75 takes the Lanczos branch).
        let x = 0.25_f64;
        let lhs = ln_gamma(x) + ln_gamma(1.0 - x);
        let rhs = PI.ln() - (PI * x).sin().ln();
        assert!(close(lhs, rhs));
    }

    #[test]
    fn entropy_known_value_and_zero_count_guard() {
        // Two equal clusters of 2 over n = 4: H = ln 2.
        assert!(close(entropy(&[2, 2], 4), 2.0_f64.ln()));
        // A zero count must be skipped, not produce NaN.
        let h = entropy(&[2, 0, 2], 4);
        assert!(h.is_finite() && close(h, 2.0_f64.ln()));
        // n == 0 short-circuits.
        assert_eq!(entropy(&[], 0), 0.0);
    }

    #[test]
    fn identical_partitions_have_zero_divergence() {
        let a = [0usize, 0, 1, 1];
        let d = Divergence::compute(&a, &a);
        assert!(d.vi.abs() < 1e-12, "VI of identical partitions is 0");
        assert!(close(d.ami, 1.0), "AMI of identical partitions is 1");
        assert!(close(d.ari, 1.0), "ARI of identical partitions is 1");
        assert!(d.h_a_given_b.abs() < 1e-12);
        assert!(d.h_b_given_a.abs() < 1e-12);
    }

    #[test]
    fn orthogonal_partitions_diverge_with_normalized_vi() {
        let a = [0usize, 0, 1, 1];
        let b = [0usize, 1, 0, 1];
        let d = Divergence::compute(&a, &b);
        assert!(d.vi > 0.0, "orthogonal partitions have positive VI");
        // Normalized VI stays in (0, 1].
        assert!(d.vi_normalized > 0.0 && d.vi_normalized <= 1.0 + 1e-12);
    }

    #[test]
    fn normalized_vi_is_zero_for_a_singleton() {
        // n == 1: the `ct.n > 1` guard must return 0, never divide by ln(1) = 0.
        let d = Divergence::compute(&[0usize], &[0usize]);
        assert_eq!(d.vi_normalized, 0.0);
    }
}
