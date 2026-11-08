//! Formant estimation by linear prediction (LPC, autocorrelation method).
//!
//! Per frame: pre-emphasis → Hamming window → autocorrelation → Levinson–Durbin →
//! polynomial roots (Durand–Kerner) → resonances. Formants are the low-bandwidth
//! resonances above 180 Hz, lowest first. Per vowel, [`vowel_nucleus`] takes the
//! median over the middle half of the vowel, which rejects transitions from the
//! surrounding consonants.

use serde::{Deserialize, Serialize};

use crate::stats;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Formants {
    pub f1: f32,
    pub f2: f32,
    pub f3: f32,
    /// Bandwidths of F1–F3 in Hz.
    pub bandwidths: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FormantConfig {
    pub sample_rate: u32,
    /// LPC order. The classic rule is 2 + fs/1000 (18 at 16 kHz).
    pub order: usize,
    /// Analysis window, seconds.
    pub window_s: f32,
    /// Resonances wider than this are not formants.
    pub max_bandwidth: f32,
    pub min_freq: f32,
}

impl Default for FormantConfig {
    fn default() -> Self {
        Self { sample_rate: crate::SAMPLE_RATE, order: 18, window_s: 0.03, max_bandwidth: 450.0, min_freq: 180.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        C::new((self.re * o.re + self.im * o.im) / d, (self.im * o.re - self.re * o.im) / d)
    }
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
}

/// LPC coefficients `[1, a1, …, ap]` by Levinson–Durbin. `None` if the frame is silent.
pub fn lpc(frame: &[f32], order: usize) -> Option<Vec<f64>> {
    let n = frame.len();
    if n <= order {
        return None;
    }
    let mut r = vec![0.0f64; order + 1];
    for (k, rk) in r.iter_mut().enumerate() {
        *rk = (0..n - k).map(|i| frame[i] as f64 * frame[i + k] as f64).sum();
    }
    if r[0] <= 1e-12 {
        return None;
    }
    r[0] *= 1.0 + 1e-9; // white-noise correction for numerical stability
    let mut a = vec![0.0f64; order + 1];
    a[0] = 1.0;
    let mut err = r[0];
    for i in 1..=order {
        let acc: f64 = r[i] + (1..i).map(|j| a[j] * r[i - j]).sum::<f64>();
        let k = -acc / err;
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        if err <= 0.0 {
            return None;
        }
    }
    Some(a)
}

/// Roots of the monic polynomial with coefficients `[1, c1, …, cp]` (highest power first).
fn roots(coeffs: &[f64]) -> Vec<C> {
    let p = coeffs.len() - 1;
    let eval = |z: C| coeffs.iter().fold(C::new(0.0, 0.0), |acc, &c| acc.mul(z).add(C::new(c, 0.0)));
    let seed = C::new(0.4, 0.9);
    let mut z: Vec<C> = Vec::with_capacity(p);
    let mut s = C::new(1.0, 0.0);
    for _ in 0..p {
        s = s.mul(seed);
        z.push(s);
    }
    for _ in 0..800 {
        let mut max_step = 0.0f64;
        for k in 0..p {
            let mut denom = C::new(1.0, 0.0);
            for j in 0..p {
                if j != k {
                    denom = denom.mul(z[k].sub(z[j]));
                }
            }
            if denom.abs() < 1e-300 {
                continue;
            }
            let step = eval(z[k]).div(denom);
            z[k] = z[k].sub(step);
            max_step = max_step.max(step.abs());
        }
        if max_step < 1e-12 {
            break;
        }
    }
    z
}

/// Resonances `(frequency Hz, bandwidth Hz)` of an LPC polynomial, sorted by frequency.
pub fn resonances(a: &[f64], sample_rate: u32) -> Vec<(f32, f32)> {
    let fs = sample_rate as f64;
    let mut out: Vec<(f32, f32)> = roots(a)
        .into_iter()
        .filter(|z| z.im > 1e-6)
        .map(|z| {
            let freq = z.arg() * fs / (2.0 * std::f64::consts::PI);
            let bw = -z.abs().max(1e-12).ln() * fs / std::f64::consts::PI;
            (freq as f32, bw as f32)
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Formants of a single frame of audio (any length ≥ the window).
pub fn frame_formants(frame: &[f32], cfg: &FormantConfig) -> Option<Formants> {
    let n = frame.len();
    let alpha = (-2.0 * std::f32::consts::PI * 50.0 / cfg.sample_rate as f32).exp();
    let windowed: Vec<f32> = (0..n)
        .map(|i| {
            let pre = if i == 0 { frame[0] } else { frame[i] - alpha * frame[i - 1] };
            let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
            pre * w
        })
        .collect();
    let a = lpc(&windowed, cfg.order)?;
    let nyquist = cfg.sample_rate as f32 / 2.0;
    let candidates: Vec<(f32, f32)> = resonances(&a, cfg.sample_rate)
        .into_iter()
        .filter(|&(f, bw)| f >= cfg.min_freq && f < nyquist - 50.0 && bw > 0.0 && bw <= cfg.max_bandwidth)
        .collect();
    if candidates.len() < 3 {
        return None;
    }
    Some(Formants {
        f1: candidates[0].0,
        f2: candidates[1].0,
        f3: candidates[2].0,
        bandwidths: [candidates[0].1, candidates[1].1, candidates[2].1],
    })
}

/// Formant track every `step_s` seconds between `start_s` and `end_s`.
pub fn track(samples: &[f32], start_s: f32, end_s: f32, step_s: f32, cfg: &FormantConfig) -> Vec<(f32, Formants)> {
    let sr = cfg.sample_rate as f32;
    let win = (cfg.window_s * sr) as usize;
    let mut out = Vec::new();
    let mut t = start_s;
    while t <= end_s {
        let centre = (t * sr) as isize;
        let lo = centre - win as isize / 2;
        if lo >= 0 && (lo as usize + win) <= samples.len() {
            if let Some(f) = frame_formants(&samples[lo as usize..lo as usize + win], cfg) {
                out.push((t, f));
            }
        }
        t += step_s;
    }
    out
}

/// Robust steady-state formants of a vowel: median over its middle 50%.
pub fn vowel_nucleus(samples: &[f32], start_s: f32, end_s: f32, cfg: &FormantConfig) -> Option<Formants> {
    let dur = end_s - start_s;
    if dur <= 0.0 {
        return None;
    }
    let frames = track(samples, start_s + dur * 0.25, end_s - dur * 0.25, 0.005, cfg);
    if frames.len() < 2 {
        return None;
    }
    let col = |f: fn(&Formants) -> f32| -> Option<f32> {
        stats::median(&frames.iter().map(|(_, x)| f(x)).collect::<Vec<_>>())
    };
    Some(Formants {
        f1: col(|x| x.f1)?,
        f2: col(|x| x.f2)?,
        f3: col(|x| x.f3)?,
        bandwidths: [col(|x| x.bandwidths[0])?, col(|x| x.bandwidths[1])?, col(|x| x.bandwidths[2])?],
    })
}

/// Onset and offset formants of a diphthong (20% and 80% points), for FACE, PRICE, GOAT, MOUTH.
pub fn diphthong(samples: &[f32], start_s: f32, end_s: f32, cfg: &FormantConfig) -> Option<(Formants, Formants)> {
    let dur = end_s - start_s;
    let seg = dur * 0.15;
    let on = vowel_nucleus(samples, start_s + dur * 0.2 - seg, start_s + dur * 0.2 + seg, cfg)?;
    let off = vowel_nucleus(samples, start_s + dur * 0.8 - seg, start_s + dur * 0.8 + seg, cfg)?;
    Some((on, off))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Klatt-style cascade synthesiser: glottal impulse train through second-order resonators.
    fn vowel(f0: f32, formants: &[(f32, f32)], seconds: f32) -> Vec<f32> {
        let sr = crate::SAMPLE_RATE as f32;
        let n = (seconds * sr) as usize;
        let period = (sr / f0) as usize;
        // Source: impulses smoothed by a two-pole low-pass (rough glottal spectral tilt).
        let mut src = vec![0.0f32; n];
        for i in (0..n).step_by(period) {
            src[i] = 1.0;
        }
        let mut y1 = 0.0f32;
        let mut y2 = 0.0f32;
        for s in src.iter_mut() {
            let y = *s + 1.6 * y1 - 0.64 * y2;
            y2 = y1;
            y1 = y;
            *s = y;
        }
        let mut sig = src;
        for &(f, bw) in formants {
            let t = 1.0 / sr;
            let c = -(-2.0 * std::f32::consts::PI * bw * t).exp();
            let b = 2.0 * (-std::f32::consts::PI * bw * t).exp() * (2.0 * std::f32::consts::PI * f * t).cos();
            let a = 1.0 - b - c;
            let (mut p1, mut p2) = (0.0f32, 0.0f32);
            for x in sig.iter_mut() {
                let y = a * *x + b * p1 + c * p2;
                p2 = p1;
                p1 = y;
                *x = y;
            }
        }
        let peak = sig.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        sig.iter().map(|v| 0.5 * v / peak).collect()
    }

    fn check(f0: f32, targets: [f32; 3]) {
        let fm: Vec<(f32, f32)> =
            [(targets[0], 80.0), (targets[1], 100.0), (targets[2], 140.0), (3500.0, 200.0), (4500.0, 250.0)].to_vec();
        let audio = vowel(f0, &fm, 0.3);
        let got = vowel_nucleus(&audio, 0.0, 0.3, &FormantConfig::default()).expect("formants");
        // LPC overestimates very low F1 slightly (harmonics pull the pole up), so allow
        // 8% or 40 Hz, whichever is larger: well under the gap between vowel categories.
        for (est, want) in [got.f1, got.f2, got.f3].iter().zip(targets) {
            let tolerance = (want * 0.08).max(40.0);
            assert!((est - want).abs() < tolerance, "f0 {f0}: wanted {targets:?}, got {got:?}");
        }
    }

    #[test]
    fn recovers_open_vowel() {
        check(110.0, [750.0, 1250.0, 2550.0]); // like "car"
    }

    #[test]
    fn recovers_close_front_vowel() {
        check(120.0, [290.0, 2250.0, 2900.0]); // like "see"
    }

    #[test]
    fn recovers_back_vowel_higher_voice() {
        check(200.0, [450.0, 850.0, 2700.0]); // like "law", female-range pitch
    }

    #[test]
    fn lpc_of_silence_is_none() {
        assert!(lpc(&[0.0; 480], 18).is_none());
    }

    #[test]
    fn roots_of_known_polynomial() {
        // (z - 0.5)(z + 0.25) = z^2 - 0.25 z - 0.125
        let mut r: Vec<f64> = roots(&[1.0, -0.25, -0.125]).iter().map(|c| c.re).collect();
        r.sort_by(|a, b| a.total_cmp(b));
        assert!((r[0] + 0.25).abs() < 1e-9 && (r[1] - 0.5).abs() < 1e-9);
    }
}
