//! The receiver's signal processing: 8-bit I/Q in, a spectrum for the waterfall and sound out.

use rustfft::{num_complex::Complex32 as C, Fft, FftPlanner};
use serde::Deserialize;
use std::f32::consts::PI;
use std::sync::Arc;

/// The dongle's samples (unsigned I, Q pairs) as complex numbers, full scale ≈ 1.
pub fn to_complex(iq: &[u8], out: &mut Vec<C>) {
    out.clear();
    out.extend(iq.chunks_exact(2).map(|p| C::new((p[0] as f32 - 127.4) / 128.0, (p[1] as f32 - 127.4) / 128.0)));
}

/// A low-pass filter: windowed sinc (Blackman), `cutoff` in cycles per sample (0–0.5), unity gain.
pub fn lowpass(taps: usize, cutoff: f32) -> Vec<f32> {
    let m = (taps - 1) as f32;
    let mut h: Vec<f32> = (0..taps)
        .map(|i| {
            let x = i as f32 - m / 2.0;
            let sinc = if x == 0.0 { 2.0 * cutoff } else { (2.0 * PI * cutoff * x).sin() / (PI * x) };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f32 / m).cos() + 0.08 * (4.0 * PI * i as f32 / m).cos();
            sinc * w
        })
        .collect();
    let s: f32 = h.iter().sum();
    h.iter_mut().for_each(|v| *v /= s);
    h
}

/// A FIR filter that keeps one output in `decim`.
pub struct Decimator<T> {
    taps: Vec<f32>,
    hist: Vec<T>,
    decim: usize,
}

impl<T: Copy + Default + std::ops::AddAssign + std::ops::Mul<f32, Output = T>> Decimator<T> {
    pub fn new(taps: Vec<f32>, decim: usize) -> Self {
        assert!(taps.len() >= decim);
        Decimator { hist: vec![T::default(); taps.len() - 1], taps, decim }
    }

    pub fn process(&mut self, input: &[T], out: &mut Vec<T>) {
        self.hist.extend_from_slice(input);
        let n = self.taps.len();
        let mut i = 0;
        while i + n <= self.hist.len() {
            let mut acc = T::default();
            for (x, h) in self.hist[i..i + n].iter().zip(&self.taps) {
                acc += *x * *h;
            }
            out.push(acc);
            i += self.decim;
        }
        self.hist.drain(..i);
    }
}

/// Moves a signal down in frequency by `hz` (so what was at +hz ends up at 0).
pub struct Shift {
    rot: C,
    step: C,
    n: u32,
}

impl Shift {
    pub fn new(hz: f64, rate: f64) -> Self {
        let w = (-2.0 * std::f64::consts::PI * hz / rate) as f32;
        Shift { rot: C::new(1.0, 0.0), step: C::new(w.cos(), w.sin()), n: 0 }
    }

    pub fn apply(&mut self, x: &mut [C]) {
        for v in x {
            *v *= self.rot;
            self.rot *= self.step;
            self.n += 1;
            if self.n & 1023 == 0 {
                self.rot /= self.rot.norm();
            }
        }
    }
}

/// Linear-interpolation resampler (the audio is low-passed before it).
pub struct Resampler {
    ratio: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: f64, to: f64) -> Self {
        Resampler { ratio: from / to, pos: 0.0, last: 0.0 }
    }

    pub fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        let n = x.len();
        if n == 0 {
            return;
        }
        while self.pos < (n - 1) as f64 {
            let i = self.pos.floor();
            let f = (self.pos - i) as f32;
            let a = if i < 0.0 { self.last } else { x[i as usize] };
            let b = x[(i + 1.0) as usize];
            out.push(a + (b - a) * f);
            self.pos += self.ratio;
        }
        self.pos -= n as f64;
        self.last = x[n - 1];
    }
}

// ---------------------------------------------------------------- spectrum

/// Averages FFTs of the samples into one spectrum (dB full scale, lowest frequency first).
pub struct Spectrum {
    pub size: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    norm: f32,
    acc: Vec<f32>,
    count: usize,
    buf: Vec<C>,
    pending: Vec<C>,
}

impl Spectrum {
    pub fn new(size: usize) -> Self {
        let window: Vec<f32> = (0..size).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / size as f32).cos()).collect();
        let s: f32 = window.iter().sum();
        Spectrum { size, fft: FftPlanner::new().plan_fft_forward(size), norm: 1.0 / (s * s), window, acc: vec![0.0; size], count: 0, buf: vec![C::default(); size], pending: Vec::new() }
    }

    /// Adds the samples; with `every` > 1 only one block in `every` is transformed (to save time).
    pub fn feed(&mut self, x: &[C], every: usize) {
        self.pending.extend_from_slice(x);
        let n = self.size;
        let mut at = 0;
        let mut k = 0;
        while at + n <= self.pending.len() {
            if k % every.max(1) == 0 {
                for (i, v) in self.buf.iter_mut().enumerate() {
                    *v = self.pending[at + i] * self.window[i];
                }
                self.fft.process(&mut self.buf);
                for (a, v) in self.acc.iter_mut().zip(&self.buf) {
                    *a += v.norm_sqr();
                }
                self.count += 1;
            }
            k += 1;
            at += n;
        }
        self.pending.drain(..at);
    }

    pub fn blocks(&self) -> usize {
        self.count
    }

    /// The average so far, in dBFS, with 0 Hz in the middle; then starts again.
    pub fn take(&mut self) -> Vec<f32> {
        let n = self.size;
        let c = self.count.max(1) as f32;
        let out = (0..n).map(|i| 10.0 * (self.acc[(i + n / 2) % n] / c * self.norm + 1e-20).log10()).collect();
        self.acc.iter_mut().for_each(|a| *a = 0.0);
        self.count = 0;
        out
    }

    pub fn reset(&mut self) {
        self.acc.iter_mut().for_each(|a| *a = 0.0);
        self.count = 0;
        self.pending.clear();
    }
}

/// Fewer bins for the screen: the strongest of each group.
pub fn shrink(db: &[f32], bins: usize) -> Vec<f32> {
    if db.len() <= bins {
        return db.to_vec();
    }
    (0..bins)
        .map(|i| {
            let a = i * db.len() / bins;
            let b = ((i + 1) * db.len() / bins).max(a + 1);
            db[a..b].iter().cloned().fold(f32::MIN, f32::max)
        })
        .collect()
}

/// dBFS as bytes for the waterfall: 0.5 dB steps from −130 dB.
pub fn to_bytes(db: &[f32]) -> Vec<u8> {
    db.iter().map(|d| ((d + 130.0) * 2.0).clamp(0.0, 255.0) as u8).collect()
}

// ---------------------------------------------------------------- listening

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Demod {
    /// Broadcast FM (200 kHz channels).
    Wfm,
    /// Narrow FM (two-way radio, 12.5/25 kHz).
    Nfm,
    /// AM (airband, shortwave, medium wave).
    Am,
}

/// Turns the samples around one frequency into sound.
pub struct Receiver {
    pub demod: Demod,
    shift: Shift,
    stage1: Decimator<C>,
    stage2: Option<Decimator<C>>,
    audio: Decimator<f32>,
    resample: Resampler,
    rate: f32,
    last: C,
    deemph: f32,
    deemph_a: f32,
    dc: f32,
    agc: f32,
    hp: f32,
    /// Channel power, dBFS (smoothed).
    pub level: f32,
    /// Squelch in dBFS; `None` = always open.
    pub squelch: Option<f32>,
    pub open: bool,
    hang: u32,
    b1: Vec<C>,
    b2: Vec<C>,
    b3: Vec<f32>,
    b4: Vec<f32>,
}

impl Receiver {
    /// `offset`: how far the wanted frequency is from the dongle's centre; `out_rate`: the sound
    /// card's rate.
    pub fn new(demod: Demod, offset: f64, in_rate: f64, out_rate: f64) -> Self {
        let r1 = in_rate / 8.0; // 256 kHz
        let (stage1, stage2, rate, audio, audio_rate) = match demod {
            Demod::Wfm => {
                let s1 = Decimator::new(lowpass(95, (100e3 / in_rate) as f32), 8);
                let audio = Decimator::new(lowpass(63, (15e3 / r1) as f32), 4);
                (s1, None, r1, audio, r1 / 4.0)
            }
            Demod::Nfm | Demod::Am => {
                let s1 = Decimator::new(lowpass(31, (40e3 / in_rate) as f32), 8);
                let cut = if demod == Demod::Nfm { 8e3 } else { 5e3 };
                let s2 = Decimator::new(lowpass(127, (cut / r1) as f32), 8);
                let r2 = r1 / 8.0; // 32 kHz
                let audio = Decimator::new(lowpass(47, (3.4e3 / r2) as f32), 1);
                (s1, Some(s2), r2, audio, r2)
            }
        };
        // 50 µs de-emphasis (Europe, Asia, Australia).
        let deemph_a = 1.0 - (-1.0 / (rate * 50e-6)).exp() as f32;
        Receiver {
            demod,
            shift: Shift::new(offset, in_rate),
            stage1,
            stage2,
            audio,
            resample: Resampler::new(audio_rate, out_rate),
            rate: rate as f32,
            last: C::new(1.0, 0.0),
            deemph: 0.0,
            deemph_a,
            dc: 0.0,
            agc: 0.05,
            hp: 0.0,
            level: -120.0,
            squelch: None,
            open: true,
            hang: 0,
            b1: Vec::new(),
            b2: Vec::new(),
            b3: Vec::new(),
            b4: Vec::new(),
        }
    }

    pub fn retune(&mut self, offset: f64, in_rate: f64) {
        self.shift = Shift::new(offset, in_rate);
    }

    /// Sound for these samples (mono, at the sound card's rate). Silence while squelched.
    pub fn process(&mut self, x: &[C], out: &mut Vec<f32>) {
        let mut x = x.to_vec();
        self.shift.apply(&mut x);
        self.b1.clear();
        self.stage1.process(&x, &mut self.b1);
        let ch = if let Some(s2) = self.stage2.as_mut() {
            self.b2.clear();
            s2.process(&self.b1, &mut self.b2);
            &self.b2
        } else {
            &self.b1
        };
        if ch.is_empty() {
            return;
        }
        let p = ch.iter().map(|v| v.norm_sqr()).sum::<f32>() / ch.len() as f32;
        let db = 10.0 * (p + 1e-20).log10();
        self.level = self.level * 0.8 + db * 0.2;
        let open = self.squelch.map(|s| self.level >= s).unwrap_or(true);
        if open {
            self.hang = (self.rate * 0.3) as u32;
        } else {
            self.hang = self.hang.saturating_sub(ch.len() as u32);
        }
        self.open = open || self.hang > 0;

        self.b3.clear();
        match self.demod {
            Demod::Wfm | Demod::Nfm => {
                let dev = if self.demod == Demod::Wfm { 75e3 } else { 5e3 };
                let k = self.rate / (2.0 * PI * dev);
                for v in ch.iter() {
                    let d = (*v * self.last.conj()).arg() * k;
                    self.last = *v;
                    let s = if self.demod == Demod::Wfm {
                        self.deemph += self.deemph_a * (d - self.deemph);
                        self.deemph
                    } else {
                        // Takes out sub-audible tones (CTCSS) and the DC of an off-centre carrier.
                        self.hp += 0.01 * (d - self.hp);
                        d - self.hp
                    };
                    self.b3.push(s * 0.7);
                }
            }
            Demod::Am => {
                for v in ch.iter() {
                    let e = v.norm();
                    self.dc += 0.0005 * (e - self.dc);
                    self.agc += if e > self.agc { 0.01 } else { 0.0002 } * (e - self.agc);
                    self.b3.push((e - self.dc) / self.agc.max(1e-6) * 0.5);
                }
            }
        }
        self.b4.clear();
        self.audio.process(&self.b3, &mut self.b4);
        let before = out.len();
        self.resample.process(&self.b4, out);
        let silent = !self.open;
        for s in &mut out[before..] {
            *s = if silent { 0.0 } else { s.clamp(-1.0, 1.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples of a carrier at `f` (Hz from centre), FM- or AM-modulated by a tone.
    fn signal(n: usize, rate: f64, f: f64, tone: f64, fm_dev: Option<f64>) -> Vec<C> {
        let mut ph = 0.0f64;
        (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                let m = (2.0 * std::f64::consts::PI * tone * t).sin();
                match fm_dev {
                    Some(d) => {
                        ph += 2.0 * std::f64::consts::PI * (f + d * m) / rate;
                        C::new(ph.cos() as f32 * 0.3, ph.sin() as f32 * 0.3)
                    }
                    None => {
                        ph += 2.0 * std::f64::consts::PI * f / rate;
                        let a = 0.2 * (1.0 + 0.6 * m);
                        C::new((a * ph.cos()) as f32, (a * ph.sin()) as f32)
                    }
                }
            })
            .collect()
    }

    /// The strongest frequency in the sound (by counting zero crossings).
    fn tone_of(x: &[f32], rate: f64) -> f64 {
        let x = &x[x.len() / 4..];
        let z = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        z as f64 / (x.len() as f64 / rate)
    }

    #[test]
    fn spectrum_finds_a_carrier() {
        let rate = 2.048e6;
        let x = signal(8192, rate, 300e3, 0.0, Some(0.0));
        let mut s = Spectrum::new(2048);
        s.feed(&x, 1);
        let db = s.take();
        let peak = db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        let f = (peak as f64 - 1024.0) * rate / 2048.0;
        assert!((f - 300e3).abs() < 2e3, "{f}");
        assert!(db[peak] > -20.0 && db[peak] < 0.0, "{}", db[peak]);
    }

    #[test]
    fn wide_fm_gives_the_tone_back() {
        let rate = 2.048e6;
        let x = signal(2_048_000 / 2, rate, 400e3, 1000.0, Some(50e3));
        let mut r = Receiver::new(Demod::Wfm, 400e3, rate, 48e3);
        let mut out = vec![];
        for c in x.chunks(16384) {
            r.process(c, &mut out);
        }
        assert!((out.len() as f64 - 24_000.0).abs() < 200.0, "{}", out.len());
        let f = tone_of(&out, 48e3);
        assert!((f - 1000.0).abs() < 30.0, "{f}");
    }

    #[test]
    fn narrow_fm_and_am_give_the_tone_back() {
        let rate = 2.048e6;
        for (d, dev) in [(Demod::Nfm, Some(3e3)), (Demod::Am, None)] {
            let x = signal(2_048_000 / 2, rate, -250e3, 800.0, dev);
            let mut r = Receiver::new(d, -250e3, rate, 44.1e3);
            let mut out = vec![];
            for c in x.chunks(16384) {
                r.process(c, &mut out);
            }
            let f = tone_of(&out, 44.1e3);
            assert!((f - 800.0).abs() < 30.0, "{d:?} {f}");
        }
    }

    #[test]
    fn squelch_closes_on_noise() {
        let rate = 2.048e6;
        let x: Vec<C> = (0..200_000u64).map(|i| C::new(((i * 7919) % 13) as f32 * 0.0005, ((i * 104729) % 11) as f32 * 0.0005)).collect();
        let mut r = Receiver::new(Demod::Am, 0.0, rate, 48e3);
        r.squelch = Some(-40.0);
        let mut out = vec![];
        for c in x.chunks(16384) {
            r.process(c, &mut out);
        }
        assert!(!r.open);
        assert!(out[out.len() / 2..].iter().all(|s| *s == 0.0));
    }
}
