//! Dynamic time warping with an optional Sakoe–Chiba band.
//!
//! Generic over a cost closure, so callers align whatever feature vectors they
//! like (pitch + intensity for prosody, MFCCs later) without copying data.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    /// Monotonic path of `(i, j)` index pairs from `(0, 0)` to `(n-1, m-1)`.
    pub path: Vec<(usize, usize)>,
    /// Total accumulated cost along the path.
    pub cost: f32,
    /// `cost / path.len()`, comparable across lengths.
    pub normalized_cost: f32,
}

/// Align sequences of length `n` and `m` given `cost(i, j)`.
///
/// `band` is the Sakoe–Chiba half-width as a fraction of the longer length
/// (for example `Some(0.2)`), measured around the diagonal. `None` = full matrix.
pub fn align<F>(n: usize, m: usize, band: Option<f32>, cost: F) -> Option<Alignment>
where
    F: Fn(usize, usize) -> f32,
{
    if n == 0 || m == 0 {
        return None;
    }
    let width = band.map(|b| ((b * n.max(m) as f32).ceil() as usize).max(1));
    let in_band = |i: usize, j: usize| -> bool {
        match width {
            None => true,
            Some(w) => {
                let diag = if n > 1 { i as f32 * (m - 1) as f32 / (n - 1) as f32 } else { 0.0 };
                (j as f32 - diag).abs() <= w as f32
            }
        }
    };

    let inf = f32::INFINITY;
    let mut acc = vec![inf; n * m];
    let idx = |i: usize, j: usize| i * m + j;
    for i in 0..n {
        for j in 0..m {
            if !in_band(i, j) {
                continue;
            }
            let c = cost(i, j);
            let best_prev = if i == 0 && j == 0 {
                0.0
            } else {
                let mut b = inf;
                if i > 0 {
                    b = b.min(acc[idx(i - 1, j)]);
                }
                if j > 0 {
                    b = b.min(acc[idx(i, j - 1)]);
                }
                if i > 0 && j > 0 {
                    b = b.min(acc[idx(i - 1, j - 1)]);
                }
                b
            };
            acc[idx(i, j)] = c + best_prev;
        }
    }
    let total = acc[idx(n - 1, m - 1)];
    if !total.is_finite() {
        return None;
    }

    // Backtrack, preferring the diagonal on ties.
    let mut path = vec![(n - 1, m - 1)];
    let (mut i, mut j) = (n - 1, m - 1);
    while i > 0 || j > 0 {
        let (ni, nj) = if i == 0 {
            (0, j - 1)
        } else if j == 0 {
            (i - 1, 0)
        } else {
            let d = acc[idx(i - 1, j - 1)];
            let u = acc[idx(i - 1, j)];
            let l = acc[idx(i, j - 1)];
            if d <= u && d <= l {
                (i - 1, j - 1)
            } else if u <= l {
                (i - 1, j)
            } else {
                (i, j - 1)
            }
        };
        i = ni;
        j = nj;
        path.push((i, j));
    }
    path.reverse();
    let len = path.len() as f32;
    Some(Alignment { path, cost: total, normalized_cost: total / len })
}

/// For each index of the second sequence, the mean first-sequence index mapped to it.
/// Useful to resample one track onto the other's timeline.
pub fn map_to_second(alignment: &Alignment, m: usize) -> Vec<f32> {
    let mut sum = vec![0.0f32; m];
    let mut count = vec![0u32; m];
    for &(i, j) in &alignment.path {
        sum[j] += i as f32;
        count[j] += 1;
    }
    sum.iter().zip(&count).map(|(s, &c)| if c > 0 { s / c as f32 } else { f32::NAN }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_sequences_align_on_the_diagonal() {
        let a: Vec<f32> = (0..20).map(|i| (i as f32 * 0.3).sin()).collect();
        let al = align(a.len(), a.len(), None, |i, j| (a[i] - a[j]).abs()).unwrap();
        assert_eq!(al.cost, 0.0);
        assert!(al.path.iter().all(|(i, j)| i == j));
    }

    #[test]
    fn recovers_a_stretch() {
        // b is a played twice as slowly.
        let a: Vec<f32> = (0..30).map(|i| (i as f32 * 0.4).sin()).collect();
        let b: Vec<f32> = (0..60).map(|j| (j as f32 * 0.2).sin()).collect();
        let al = align(a.len(), b.len(), Some(0.3), |i, j| (a[i] - b[j]).abs()).unwrap();
        let mapped = map_to_second(&al, b.len());
        for (j, i) in mapped.iter().enumerate().skip(4).take(50) {
            assert!((i - j as f32 / 2.0).abs() <= 1.5, "j={j} mapped to {i}");
        }
    }

    #[test]
    fn narrow_band_still_finds_a_path() {
        let a = [0.0f32; 10];
        let b = [0.0f32; 25];
        assert!(align(10, 25, Some(0.05), |i, j| (a[i] - b[j]).abs()).is_some());
    }

    #[test]
    fn empty_is_none() {
        assert!(align(0, 5, None, |_, _| 0.0).is_none());
    }
}
