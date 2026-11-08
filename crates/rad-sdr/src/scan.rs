//! The scanner: sweeps a band step by step (the dongle sees ~2 MHz at a time), joins the pieces
//! into one wide spectrum and finds what's transmitting.

use serde::Serialize;

/// The part of each step's spectrum that's used (the edges roll off).
pub const USABLE: f64 = 0.8;

/// One sweep of `from`–`to` Hz, a step at a time.
pub struct Sweep {
    pub from: f64,
    pub to: f64,
    /// Hz per bin.
    pub res: f64,
    pub db: Vec<f32>,
    pub floor: Vec<f32>,
    rate: f64,
    pub step: usize,
}

impl Sweep {
    pub fn new(from: f64, to: f64, rate: f64, fft: usize) -> Self {
        let res = rate / fft as f64;
        let bins = ((to - from) / res).ceil().max(1.0) as usize;
        Sweep { from, to, res, db: vec![-130.0; bins], floor: vec![-130.0; bins], rate, step: 0 }
    }

    pub fn steps(&self) -> usize {
        ((self.to - self.from) / (self.rate * USABLE)).ceil().max(1.0) as usize
    }

    /// The centre frequency for step `i`.
    pub fn center(&self, i: usize) -> f64 {
        self.from + self.rate * USABLE * (i as f64 + 0.5)
    }

    /// Puts one step's spectrum (dB, 0 Hz in the middle, from [`crate::dsp::Spectrum::take`]) in place.
    pub fn add(&mut self, center: f64, spec: &[f32]) {
        let n = spec.len();
        let mut s = spec.to_vec();
        // The DC spike in the middle.
        for k in [n / 2 - 1, n / 2, n / 2 + 1] {
            s[k] = (spec[n / 2 - 3] + spec[n / 2 + 3]) / 2.0;
        }
        let keep = (n as f64 * USABLE / 2.0) as usize;
        let part = &s[n / 2 - keep..n / 2 + keep];
        let mut sorted = part.to_vec();
        sorted.sort_by(f32::total_cmp);
        let floor = sorted[sorted.len() / 2];
        for (k, v) in part.iter().enumerate() {
            let f = center + (k as f64 - keep as f64) * self.res;
            let i = ((f - self.from) / self.res).round();
            if i >= 0.0 && (i as usize) < self.db.len() {
                self.db[i as usize] = *v;
                self.floor[i as usize] = floor;
            }
        }
    }

    /// What stands `threshold` dB above the noise.
    pub fn signals(&self, threshold: f32) -> Vec<Hit> {
        let n = self.db.len();
        let above = |k: usize| self.db[k] >= self.floor[k] + threshold;
        // Runs of bins above the line.
        let mut runs: Vec<(usize, usize)> = vec![];
        let mut i = 0;
        while i < n {
            if !above(i) {
                i += 1;
                continue;
            }
            let start = i;
            while i < n && above(i) {
                i += 1;
            }
            runs.push((start, i - 1));
        }
        // One transmitter can show as several pieces (broadcast FM's spectrum has dips): pieces
        // no further apart than their width are one signal. Airband channels 8.33/25 kHz apart,
        // each a few kHz wide, stay apart.
        let mut merged: Vec<(usize, usize)> = vec![];
        for r in runs {
            if let Some(last) = merged.last_mut() {
                let gap = r.0 - last.1 - 1;
                let width = (last.1 - last.0 + 1).max(r.1 - r.0 + 1);
                if gap <= 3.max(width) {
                    last.1 = r.1;
                    continue;
                }
            }
            merged.push(r);
        }
        merged
            .into_iter()
            .filter(|(s, e)| e - s + 1 >= 3)
            .map(|(start, end)| {
                let (mut w, mut wf, mut peak) = (0f64, 0f64, f32::MIN);
                for k in start..=end {
                    let p = 10f64.powf(self.db[k] as f64 / 10.0);
                    w += p;
                    wf += p * k as f64;
                    peak = peak.max(self.db[k]);
                }
                let floor = self.floor[(start + end) / 2];
                Hit { freq: self.from + wf / w * self.res, bw: (end - start + 1) as f64 * self.res, db: peak, snr: peak - floor }
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Hit {
    pub freq: f64,
    pub bw: f64,
    pub db: f32,
    pub snr: f32,
}

/// A frequency the scanner has heard, over all its sweeps.
#[derive(Clone, Debug, Serialize)]
pub struct Signal {
    pub freq: f64,
    pub bw: f64,
    /// Strongest seen, dBFS, and how far above the noise.
    pub db: f32,
    pub snr: f32,
    pub hits: u32,
    pub first: u64,
    pub last: u64,
    /// Heard in the latest sweep.
    pub active: bool,
}

#[derive(Default)]
pub struct Book {
    pub signals: Vec<Signal>,
    pub sweeps: u32,
}

impl Book {
    /// Adds a sweep's hits: the same transmitter (within half its width, or 5 kHz) adds up.
    pub fn add(&mut self, hits: &[Hit], now: u64) {
        self.sweeps += 1;
        for s in &mut self.signals {
            s.active = false;
        }
        for h in hits {
            let near = self.signals.iter_mut().find(|s| (s.freq - h.freq).abs() <= (s.bw.max(h.bw) / 2.0).max(5e3));
            match near {
                Some(s) => {
                    s.hits += 1;
                    s.last = now;
                    s.active = true;
                    if h.db > s.db {
                        s.db = h.db;
                        s.freq = h.freq;
                        s.bw = h.bw;
                    }
                    s.snr = s.snr.max(h.snr);
                }
                None => self.signals.push(Signal { freq: h.freq, bw: h.bw, db: h.db, snr: h.snr, hits: 1, first: now, last: now, active: true }),
            }
        }
        self.signals.sort_by(|a, b| a.freq.total_cmp(&b.freq));
        // Entries whose occupied bandwidths overlap are the same transmitter, seen in pieces on
        // some sweeps.
        let mut out: Vec<Signal> = Vec::with_capacity(self.signals.len());
        for s in self.signals.drain(..) {
            if let Some(c) = out.last_mut() {
                let (c_lo, c_hi) = (c.freq - c.bw / 2.0, c.freq + c.bw / 2.0);
                let (s_lo, s_hi) = (s.freq - s.bw / 2.0, s.freq + s.bw / 2.0);
                if s_lo <= c_hi + 2e3 {
                    let (lo, hi) = (c_lo.min(s_lo), c_hi.max(s_hi));
                    if s.db > c.db {
                        c.freq = s.freq;
                        c.db = s.db;
                    }
                    c.bw = (hi - lo).max(c.bw);
                    c.snr = c.snr.max(s.snr);
                    c.hits = c.hits.max(s.hits);
                    c.first = c.first.min(s.first);
                    c.last = c.last.max(s.last);
                    c.active |= s.active;
                    continue;
                }
            }
            out.push(s);
        }
        self.signals = out;
    }

    pub fn clear(&mut self) {
        self.signals.clear();
        self.sweeps = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Spectrum;
    use rustfft::num_complex::Complex32 as C;

    #[test]
    fn finds_transmitters_across_steps() {
        let rate = 2.048e6;
        let mut sw = Sweep::new(88e6, 92e6, rate, 2048);
        let tx = [89.1e6, 90.7e6, 91.5e6];
        for i in 0..sw.steps() {
            let c = sw.center(i);
            // Noise plus a 100 kHz-wide signal at each transmitter in view.
            let mut seed = 1u32 + i as u32;
            let mut rnd = || {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / 16777216.0 - 0.5
            };
            let mut ph = vec![0f64; tx.len()];
            let x: Vec<C> = (0..32768)
                .map(|n| {
                    let mut v = C::new(rnd() * 0.01, rnd() * 0.01);
                    for (k, f) in tx.iter().enumerate() {
                        let off = f - c;
                        if off.abs() < rate / 2.0 {
                            ph[k] += 2.0 * std::f64::consts::PI * (off + 50e3 * ((n as f64) * 0.01).sin()) / rate;
                            v += C::new(ph[k].cos() as f32 * 0.1, ph[k].sin() as f32 * 0.1);
                        }
                    }
                    v
                })
                .collect();
            let mut s = Spectrum::new(2048);
            s.feed(&x, 1);
            sw.add(c, &s.take());
        }
        let hits = sw.signals(10.0);
        for f in tx {
            assert!(hits.iter().any(|h| (h.freq - f).abs() < 30e3), "{f} not in {hits:?}");
        }
        assert_eq!(hits.len(), 3, "{hits:?}");
        // A broadcast station in pieces is one signal; airband channels 25 kHz apart are two.
        let mut sw2 = Sweep::new(100e6, 101e6, rate, 2048);
        let floor = -100.0;
        sw2.floor.iter_mut().for_each(|f| *f = floor);
        sw2.db.iter_mut().for_each(|d| *d = floor);
        for k in (300..340).chain(370..410) {
            sw2.db[k] = -60.0;
        }
        for k in (600..606).chain(625..631) {
            sw2.db[k] = -70.0;
        }
        let h2 = sw2.signals(10.0);
        assert_eq!(h2.len(), 3, "{h2:?}");
        let mut b = Book::default();
        b.add(&hits, 1);
        b.add(&hits, 2);
        assert_eq!(b.signals.len(), 3);
        assert!(b.signals.iter().all(|s| s.hits == 2 && s.active));
    }
}
