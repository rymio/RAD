//! Short transmissions in the ~2 MHz the dongle sees (ISM 433/868/915 MHz and the rest): found
//! as they happen and identified by how they're modulated. OOK (remotes, weather stations: on/off
//! pulses, whose timings and bits are kept), FSK (sensors, meters, telemetry and RC links: baud
//! and deviation), LoRa chirps (Meshtastic, LoRaWAN, ExpressLRS: spreading factor and bandwidth),
//! plain carriers and wideband signals.
//!
//! Found with short FFTs (256 points: 125 µs and 8 kHz at 2.048 MS/s) against a noise floor kept
//! for each bin; a burst's samples are then shifted to baseband, filtered to its width and looked
//! at sample by sample (amplitude and instantaneous frequency).

use crate::dsp::{lowpass, Decimator, Shift};
use rustfft::{num_complex::Complex32 as C, Fft, FftPlanner};
use serde::Serialize;
use std::sync::Arc;

const N: usize = 256;
/// How far above the floor a bin must be, dB, when the same channel was over it in the frame
/// before too…
const THRESHOLD: f32 = 12.0;
/// …and on its own. (Real noise has a heavy tail: lone frames 12 dB up come several times a
/// second, but never twice running in the same place; nothing reaches 18 dB by chance.)
const LONE: f32 = 18.0;
/// Frames below the line before a burst counts as over (2 ms).
const HANG: usize = 16;
/// Longer than this is cut off and called continuous.
const MAX_SECONDS: f64 = 1.0;

#[derive(Clone, Debug, Serialize)]
pub struct Burst {
    pub n: u64,
    /// Milliseconds since 1970.
    pub t: u64,
    /// Centre frequency and occupied width, Hz.
    pub freq: f64,
    pub bw: f64,
    pub dur_us: u32,
    /// Peak power (dBFS) and how far above the noise.
    pub db: f32,
    pub snr: f32,
    /// "ook", "fsk", "lora", "carrier", "wide", "continuous" or "unknown".
    pub kind: &'static str,
    /// A plain description of the modulation.
    pub modulation: String,
    pub sf: Option<u32>,
    pub lora_bw: Option<f64>,
    pub baud: Option<f64>,
    pub deviation: Option<f64>,
    /// OOK: pulse and gap lengths (µs), alternating, starting with a pulse.
    pub pulses: Vec<u32>,
    /// OOK: the bits, when the pulses are width- or gap-coded.
    pub bits: Option<String>,
    /// What it probably is.
    pub guess: String,
    /// Same transmitter type (for spotting frequency hopping).
    pub sig: String,
}

pub struct Detector {
    rate: f64,
    pub center: f64,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    floor: Vec<f32>,
    warm: usize,
    pending: Vec<C>,
    buf: Vec<C>,
    // The burst going on.
    on: bool,
    quiet: usize,
    active_frames: usize,
    samples: Vec<C>,
    peak: Vec<f32>,
    /// Power summed over the burst's frames, per bin (for its width).
    sum: Vec<f64>,
    start_t: u64,
    prev: Vec<C>,
    /// Bins over the line in the frame before (widened by two bins each side).
    was_over: Vec<bool>,
    count: u64,
    /// Frames seen (for the time of a burst within a chunk).
    frames: u64,
}

impl Detector {
    pub fn new(rate: f64, center: f64) -> Self {
        let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
        Detector {
            rate,
            center,
            fft: FftPlanner::new().plan_fft_forward(N),
            window,
            floor: vec![-200.0; N],
            warm: 0,
            pending: Vec::new(),
            buf: vec![C::default(); N],
            on: false,
            quiet: 0,
            active_frames: 0,
            samples: Vec::new(),
            peak: vec![-200.0; N],
            sum: vec![0.0; N],
            start_t: 0,
            prev: Vec::new(),
            was_over: vec![false; N],
            count: 0,
            frames: 0,
        }
    }

    /// Starts over (after retuning).
    pub fn reset(&mut self, center: f64) {
        *self = Detector { count: self.count, ..Detector::new(self.rate, center) };
    }

    /// Takes the next samples; `now` is the time (ms) of the first. Returns the bursts that ended.
    pub fn feed(&mut self, x: &[C], now: u64) -> Vec<Burst> {
        let mut out = vec![];
        self.pending.extend_from_slice(x);
        let frame_ms = N as f64 / self.rate * 1e3;
        let mut at = 0;
        let mut k = 0u64;
        while at + N <= self.pending.len() {
            let frame: Vec<C> = self.pending[at..at + N].to_vec();
            at += N;
            let t = now + (k as f64 * frame_ms) as u64;
            k += 1;
            self.frames += 1;
            for (i, v) in self.buf.iter_mut().enumerate() {
                *v = frame[i] * self.window[i];
            }
            self.fft.process(&mut self.buf);
            let db: Vec<f32> = (0..N).map(|i| 10.0 * (self.buf[(i + N / 2) % N].norm_sqr() + 1e-20).log10()).collect();
            // The floor: learnt over the first frames (the mean in dB is close to the median).
            if self.warm < 64 {
                for i in 0..N {
                    self.floor[i] = if self.warm == 0 { db[i] } else { self.floor[i] + (db[i] - self.floor[i]) / (self.warm as f32 + 1.0) };
                }
                self.warm += 1;
                self.prev = frame;
                continue;
            }
            // Two neighbouring bins over the line (one alone is usually noise): far over, or over
            // where something was in the frame before.
            let over: Vec<bool> = (0..N).map(|i| db[i] > self.floor[i] + THRESHOLD).collect();
            let pair: Vec<bool> = (0..N).map(|i| i > 0 && over[i] && over[i - 1]).collect();
            let active = (1..N).any(|i| pair[i] && (self.was_over[i] || (db[i] > self.floor[i] + LONE && db[i - 1] > self.floor[i - 1] + LONE)));
            let mut was = vec![false; N];
            for i in 0..N {
                if pair[i] {
                    for j in i.saturating_sub(3)..(i + 3).min(N) {
                        was[j] = true;
                    }
                }
            }
            self.was_over = was;
            // The median of each bin, followed a small step at a time (16 dB a second): a burst
            // hardly moves it, a carrier that stays on becomes part of it in a few seconds.
            for i in 0..N {
                self.floor[i] += if db[i] > self.floor[i] { 0.002 } else { -0.002 };
            }
            if active {
                if !self.on {
                    self.on = true;
                    self.active_frames = 0;
                    self.samples.clear();
                    self.samples.extend_from_slice(&self.prev);
                    self.peak.iter_mut().for_each(|p| *p = -200.0);
                    self.sum.iter_mut().for_each(|p| *p = 0.0);
                    self.start_t = t;
                }
                self.quiet = 0;
                self.active_frames += 1;
            } else if self.on {
                self.quiet += 1;
            }
            if self.on {
                self.samples.extend_from_slice(&frame);
                for i in 0..N {
                    self.peak[i] = self.peak[i].max(db[i]);
                    if self.quiet == 0 {
                        self.sum[i] += 10f64.powf(db[i] as f64 / 10.0);
                    }
                }
                let long = self.samples.len() as f64 > self.rate * MAX_SECONDS;
                if self.quiet >= HANG || long {
                    self.on = false;
                    if self.active_frames >= 2 {
                        if let Some(b) = self.finish(long) {
                            out.push(b);
                        }
                    }
                }
            }
            self.prev = frame;
        }
        self.pending.drain(..at);
        out
    }

    fn finish(&mut self, long: bool) -> Option<Burst> {
        // The strongest bin, and the bins around it that stood out.
        let (pk, _) = self.peak.iter().enumerate().filter(|(i, _)| *i != N / 2).max_by(|a, b| a.1.total_cmp(b.1))?;
        // The width from the average over the burst (the strongest of many noisy frames would
        // stand out of the noise anywhere): bins 6 dB over the noise and within 25 dB of the
        // strongest (strong bursts splatter at their edges).
        let frames = self.active_frames.max(1) as f64;
        let avg: Vec<f32> = self.sum.iter().map(|p| (10.0 * (p / frames + 1e-20).log10()) as f32).collect();
        let top = avg.iter().enumerate().filter(|(i, _)| *i != N / 2).map(|(_, v)| *v).fold(f32::MIN, f32::max);
        let pk = avg.iter().enumerate().filter(|(i, _)| *i != N / 2).max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(pk);
        let line = |i: usize| avg[i] > (self.floor[i] + 6.0).max(top - 25.0);
        let (mut lo, mut hi) = (pk, pk);
        while lo > 0 && (line(lo - 1) || (lo > 1 && line(lo - 2))) {
            lo -= 1;
        }
        while hi + 1 < N && (line(hi + 1) || (hi + 2 < N && line(hi + 2))) {
            hi += 1;
        }
        let bin = self.rate / N as f64;
        let (mut w, mut wf) = (0f64, 0f64);
        for i in lo..=hi {
            let p = 10f64.powf(avg[i] as f64 / 10.0);
            w += p;
            wf += p * i as f64;
        }
        let ci = wf / w;
        let offset = (ci - N as f64 / 2.0) * bin;
        let bw = ((hi - lo + 1) as f64 * bin).max(bin);
        // Strength at the burst's own strongest bin (by average), against the noise there.
        let pdb = self.peak[pk];
        let snr = pdb - self.floor[pk];
        let dur_us = ((self.samples.len() - N * (HANG.min(self.quiet) + 1)) as f64 / self.rate * 1e6).max(1.0) as u32;
        self.count += 1;
        let freq = self.center + offset;
        let mut b = Burst {
            n: self.count,
            t: self.start_t,
            freq,
            bw,
            dur_us,
            // A full-scale tone through the Hann window comes out at (N/2)².
            db: pdb - 20.0 * (N as f32 * 0.5).log10(),
            snr,
            kind: "unknown",
            modulation: String::new(),
            sf: None,
            lora_bw: None,
            baud: None,
            deviation: None,
            pulses: vec![],
            bits: None,
            guess: String::new(),
            sig: String::new(),
        };
        if long {
            b.kind = "continuous";
            b.modulation = "continuous transmission (over a second)".into();
        } else if bw > 1.2e6 {
            b.kind = "wide";
            b.modulation = format!("wideband ({:.1} MHz)", bw / 1e6);
        } else {
            analyze(&self.samples, self.rate, offset, bw, &mut b);
        }
        b.guess = guess(&b);
        Some(b)
    }
}

/// Amplitude and instantaneous frequency of a burst, then what they show.
pub fn analyze(samples: &[C], rate: f64, offset: f64, bw: f64, b: &mut Burst) {
    let want = (2.5 * bw).max(40e3);
    let d = ((rate / want).floor() as usize).clamp(1, 64);
    let r = rate / d as f64;
    let mut x = samples.to_vec();
    Shift::new(offset, rate).apply(&mut x);
    let mut y = Vec::with_capacity(x.len() / d + 1);
    let cut = ((0.6 * bw).max(6e3) / rate).min(0.45 / d as f64) as f32;
    let mut dec = Decimator::new(lowpass(63, cut.max(0.002)), d);
    dec.process(&x, &mut y);
    if y.len() < 16 {
        b.kind = "unknown";
        b.modulation = "too short to tell".into();
        return;
    }
    // Amplitude, smoothed over ~8 µs (noise flickers; real pulses are tens of µs or more).
    let raw: Vec<f32> = y.iter().map(|v| v.norm()).collect();
    let aw = ((r * 8e-6).round() as usize).max(1);
    let amp: Vec<f32> = (0..raw.len()).map(|i| { let a = i.saturating_sub(aw / 2); let z = (a + aw).min(raw.len()); raw[a..z].iter().sum::<f32>() / (z - a) as f32 }).collect();
    let mut sorted = amp.clone();
    sorted.sort_by(f32::total_cmp);
    let hi = sorted[sorted.len() * 95 / 100];
    let mid = hi * 0.5;
    // On / off runs (with a little hysteresis).
    let mut runs: Vec<(bool, usize)> = vec![];
    let mut state = amp[0] > mid;
    let mut len = 0usize;
    for &a in &amp {
        let s = if state { a > mid * 0.8 } else { a > mid * 1.2 };
        if s == state {
            len += 1;
        } else {
            runs.push((state, len));
            state = s;
            len = 1;
        }
    }
    runs.push((state, len));
    // Runs shorter than 30 µs are glitches: they join their neighbours.
    let min_run = ((r * 30e-6).ceil() as usize).max(2);
    let mut merged: Vec<(bool, usize)> = vec![];
    for (st, n) in runs {
        match merged.last_mut() {
            Some(last) if n < min_run || last.0 == st => last.1 += n,
            _ => merged.push((st, n)),
        }
    }
    let mut runs = merged;
    // Leading and trailing quiet aren't part of it.
    while runs.first().map(|r| !r.0).unwrap_or(false) {
        runs.remove(0);
    }
    while runs.last().map(|r| !r.0).unwrap_or(false) {
        runs.pop();
    }
    let us = |n: usize| (n as f64 / r * 1e6).round() as u32;
    let on_runs: Vec<usize> = runs.iter().filter(|r| r.0).map(|r| r.1).collect();
    let real_pulses = on_runs.len();
    if real_pulses >= 4 {
        b.kind = "ook";
        b.pulses = runs.iter().take(400).map(|r| us(r.1)).collect();
        let (bits, coding) = ook_bits(&runs, r);
        b.bits = bits;
        let shortest = on_runs.iter().filter(|&&n| n >= 2).min().copied().unwrap_or(1);
        b.modulation = format!("OOK / ASK, {real_pulses} pulses{}, shortest {} µs", coding.map(|c| format!(", {c}")).unwrap_or_default(), us(shortest));
        b.sig = format!("ook {:.0}", b.freq / 1e5);
        return;
    }
    // Not on/off keyed, so the envelope is steady: the frequency is looked at from the first
    // "on" sample to the last (weak bursts dip below the line now and then).
    let s = amp.iter().position(|&a| a > mid).unwrap_or(0);
    let e = amp.iter().rposition(|&a| a > mid).unwrap_or(0);
    let n = e.saturating_sub(s);
    if n < 12 {
        b.kind = "unknown";
        b.modulation = "too short to tell".into();
        return;
    }
    let seg = &y[s..s + n];
    let k = r / (2.0 * std::f64::consts::PI);
    let f: Vec<f64> = seg.windows(2).map(|w| (w[1] * w[0].conj()).arg() as f64 * k).collect();
    // A little smoothing.
    let sm = (r / 50e3).ceil().max(2.0) as usize;
    let fs = smooth_by(&f, sm);
    if fs.len() < 8 {
        b.kind = "unknown";
        b.modulation = "too short to tell".into();
        return;
    }
    let mut fsorted = fs.clone();
    fsorted.sort_by(f64::total_cmp);
    let p = |q: f64| fsorted[((fsorted.len() - 1) as f64 * q) as usize];

    // LoRa: the frequency climbs steadily across the band and jumps back (up-chirps). Weak and
    // slow chirps need more smoothing to show, fast ones less.
    for m in [1, 4, 16] {
        if let Some((sf, lbw)) = chirp(&f, sm * m, r) {
            b.kind = "lora";
            b.sf = Some(sf);
            b.lora_bw = Some(lbw);
            b.modulation = format!("LoRa chirps (CSS), SF{sf}, {} kHz", lbw / 1e3);
            b.sig = format!("lora sf{sf} bw{}", lbw / 1e3);
            return;
        }
    }
    let span = p(0.97) - p(0.03);
    // FSK: two frequencies.
    let (mut c1, mut c2) = (p(0.2), p(0.8));
    for _ in 0..10 {
        let m = (c1 + c2) / 2.0;
        let (a, bb): (Vec<f64>, Vec<f64>) = fs.iter().partition(|&&v| v < m);
        if a.is_empty() || bb.is_empty() {
            break;
        }
        c1 = a.iter().sum::<f64>() / a.len() as f64;
        c2 = bb.iter().sum::<f64>() / bb.len() as f64;
    }
    let m = (c1 + c2) / 2.0;
    let sd = (fs.iter().map(|&v| { let c = if v < m { c1 } else { c2 }; (v - c).powi(2) }).sum::<f64>() / fs.len() as f64).sqrt();
    let low = fs.iter().filter(|&&v| v < m).count() as f64 / fs.len() as f64;
    if c2 - c1 > 3.0 * sd && c2 - c1 > 2e3 && (0.1..0.9).contains(&low) {
        // Symbol length: the short runs on one side or the other.
        let mut lens = vec![];
        let mut l = 0usize;
        let mut st = fs[0] < m;
        for &v in &fs {
            if (v < m) == st {
                l += 1;
            } else {
                lens.push(l);
                st = v < m;
                l = 1;
            }
        }
        lens.retain(|&n| n >= 2);
        lens.sort_unstable();
        let sym = lens.get(lens.len() / 5).copied().unwrap_or(1).max(1);
        let baud = r / sym as f64;
        let dev = (c2 - c1) / 2.0;
        b.kind = "fsk";
        b.baud = Some(round_baud(baud));
        b.deviation = Some((dev / 100.0).round() * 100.0);
        b.modulation = format!("FSK, ±{:.1} kHz, ~{} baud", dev / 1e3, fmt_baud(b.baud.unwrap()));
        b.sig = format!("fsk {:.0}k {:.0}", dev / 1e3, b.baud.unwrap() / 1e3);
        return;
    }
    if span < 3e3 + bw * 0.05 {
        b.kind = "carrier";
        b.modulation = "unmodulated carrier".into();
        b.sig = "carrier".into();
        return;
    }
    b.kind = "unknown";
    b.modulation = format!("constant envelope, {:.0} kHz of frequency movement (FM / PSK?)", span / 1e3);
    b.sig = format!("other {:.0}", span / 1e4);
}

/// Frequency averaged over `n` samples.
fn smooth_by(f: &[f64], n: usize) -> Vec<f64> {
    if f.len() <= n {
        return vec![];
    }
    let mut out = Vec::with_capacity(f.len() - n);
    let mut acc: f64 = f[..n].iter().sum();
    for i in 0..f.len() - n {
        out.push(acc / n as f64);
        acc += f[i + n] - f[i];
    }
    out
}

/// LoRa's chirps in an instantaneous-frequency trace (`r` samples a second), smoothed over `sm`:
/// the spreading factor and bandwidth.
fn chirp(f: &[f64], sm: usize, r: f64) -> Option<(u32, f64)> {
    let fs = smooth_by(f, sm);
    if fs.len() < 6 * sm {
        return None;
    }
    let mut fsorted = fs.clone();
    fsorted.sort_by(f64::total_cmp);
    let p = |q: f64| fsorted[((fsorted.len() - 1) as f64 * q) as usize];
    let span = p(0.97) - p(0.03);
    // The smoothing spreads each wrap over `sm` samples, so wraps are looked for over that lag.
    let jump = span * 0.5;
    let lag: Vec<f64> = (0..fs.len().saturating_sub(sm)).map(|i| fs[i + sm] - fs[i]).collect();
    let mut wrapping = vec![false; fs.len()];
    let mut wrap_signs: Vec<f64> = vec![];
    let mut inside = false;
    for (i, d) in lag.iter().enumerate() {
        let w = d.abs() >= jump;
        if w {
            for v in &mut wrapping[i..(i + sm + 1).min(fs.len())] {
                *v = true;
            }
            if !inside {
                wrap_signs.push(d.signum());
            }
        }
        inside = w;
    }
    // Between the wraps: straight lines, all with the same slope, for chirps.
    if wrap_signs.len() >= 2 && span > 20e3 {
        let down = wrap_signs.iter().filter(|&&x| x < 0.0).count() as f64 / wrap_signs.len() as f64;
        let one_way = down >= 0.8 || down <= 0.2;
        let mut fits: Vec<(f64, f64)> = vec![]; // (slope per sample, R²)
        let mut i = 0;
        while i < fs.len() {
            if wrapping[i] {
                i += 1;
                continue;
            }
            let st = i;
            while i < fs.len() && !wrapping[i] {
                i += 1;
            }
            let seg = &fs[st..i];
            if seg.len() >= (3 * sm).max(16) {
                let n = seg.len() as f64;
                let mx = (n - 1.0) / 2.0;
                let my = seg.iter().sum::<f64>() / n;
                let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
                for (k, &v) in seg.iter().enumerate() {
                    let dx = k as f64 - mx;
                    sxy += dx * (v - my);
                    sxx += dx * dx;
                    syy += (v - my) * (v - my);
                }
                let slope = sxy / sxx;
                let r2 = if syy > 0.0 { sxy * sxy / (sxx * syy) } else { 0.0 };
                fits.push((slope, r2));
            }
        }
        if one_way && fits.len() >= 2 {
            let mut sl: Vec<f64> = fits.iter().map(|f| f.0).collect();
            sl.sort_by(f64::total_cmp);
            let med = sl[sl.len() / 2];
            let mut r2: Vec<f64> = fits.iter().map(|f| f.1).collect();
            r2.sort_by(f64::total_cmp);
            let good = r2[r2.len() / 2] > 0.8;
            let opposite = med.signum() == -(if down >= 0.8 { -1.0 } else { 1.0 });
            let slope = med * r; // Hz per second
            if good && opposite && slope.abs() > 1e6 {
                let lbw = [62.5e3, 125e3, 250e3, 500e3].into_iter().min_by(|a, b| (a - span / 0.94).abs().total_cmp(&(b - span / 0.94).abs())).unwrap();
                let sf = ((lbw * lbw / slope.abs()).log2().round() as i32).clamp(5, 12) as u32;
                return Some((sf, lbw));
            }
        }
    }
    None
}

fn round_baud(b: f64) -> f64 {
    // Close to a usual rate? Use it.
    const USUAL: [f64; 18] = [1200.0, 2400.0, 4800.0, 9600.0, 10000.0, 17240.0, 19200.0, 20000.0, 32768.0, 38400.0, 50000.0, 57600.0, 64000.0, 76800.0, 100000.0, 115200.0, 150000.0, 250000.0];
    USUAL.iter().copied().find(|u| (u - b).abs() / u < 0.08).unwrap_or((b / 100.0).round() * 100.0)
}

fn fmt_baud(b: f64) -> String {
    if b >= 1000.0 {
        format!("{:.1}k", b / 1000.0)
    } else {
        format!("{b:.0}")
    }
}

/// Bits from OOK pulses: pulse-width coding (two pulse lengths) or pulse-position coding (two gap
/// lengths). Short = 0, long = 1.
fn ook_bits(runs: &[(bool, usize)], _r: f64) -> (Option<String>, Option<&'static str>) {
    let on: Vec<usize> = runs.iter().filter(|r| r.0).map(|r| r.1).collect();
    let off: Vec<usize> = runs.iter().filter(|r| !r.0).map(|r| r.1).collect();
    let two = |v: &[usize]| -> Option<f64> {
        if v.len() < 4 {
            return None;
        }
        let mut s = v.to_vec();
        s.sort_unstable();
        let (a, b) = (s[s.len() / 10] as f64, s[s.len() * 9 / 10] as f64);
        (b > a * 1.6).then_some((a + b) / 2.0)
    };
    let hex = |bits: &[bool]| -> String {
        let s: String = bits.iter().map(|&b| if b { '1' } else { '0' }).collect();
        let h: String = bits.chunks(4).map(|c| format!("{:X}", c.iter().fold(0u8, |a, &b| a << 1 | b as u8) << (4 - c.len()))).collect();
        format!("{h} ({} bits: {s})", bits.len())
    };
    if let Some(m) = two(&on) {
        // Gaps much longer than any pulse split packets: only the bits are kept.
        let bits: Vec<bool> = on.iter().map(|&n| n as f64 > m).collect();
        return (Some(hex(&bits)), Some("pulse-width coded"));
    }
    if let Some(m) = two(&off) {
        let big = off.iter().copied().max().unwrap_or(0) as f64;
        let bits: Vec<bool> = off.iter().filter(|&&n| (n as f64) < big * 0.8 || big < m * 3.0).map(|&n| n as f64 > m).collect();
        return (Some(hex(&bits)), Some("pulse-position (gap) coded"));
    }
    (None, None)
}

/// What a burst probably is, from where it is and how it's modulated.
pub fn guess(b: &Burst) -> String {
    let f = b.freq / 1e6;
    let in_ = |a: f64, z: f64| f >= a && f <= z;
    let region = if in_(863.0, 870.0) {
        "EU 868"
    } else if in_(902.0, 928.0) {
        if in_(915.0, 928.0) && in_(920.0, 925.0) {
            "AS923 / AU915 / US915"
        } else {
            "US915 / AU915"
        }
    } else if in_(433.05, 434.79) {
        "ISM 433"
    } else if in_(314.0, 316.0) {
        "315 MHz (US remotes)"
    } else {
        ""
    };
    match b.kind {
        "lora" => {
            let (sf, bw) = (b.sf.unwrap_or(0), b.lora_bw.unwrap_or(0.0) / 1e3);
            let mesh = match (sf, bw as u32) {
                (7, 500) => Some("Meshtastic ShortTurbo"),
                (7, 250) => Some("Meshtastic ShortFast"),
                (8, 250) => Some("Meshtastic ShortSlow"),
                (9, 250) => Some("Meshtastic MediumFast"),
                (10, 250) => Some("Meshtastic MediumSlow"),
                (11, 250) => Some("Meshtastic LongFast (the default)"),
                (11, 125) => Some("Meshtastic LongModerate"),
                (12, 125) => Some("Meshtastic LongSlow, or LoRaWAN SF12"),
                _ => None,
            };
            let lorawan = bw as u32 == 125 && (7..=12).contains(&sf);
            if bw as u32 == 500 && sf <= 9 && b.dur_us < 25_000 {
                format!("LoRa SF{sf}/500 kHz, short: an RC control link such as ExpressLRS (drones, RC models){}", if region.is_empty() { String::new() } else { format!(", {region}") })
            } else if let Some(m) = mesh {
                format!("LoRa SF{sf}/{bw} kHz: matches {m}{}", if lorawan && mesh.is_none() { "" } else { ". Could also be another LoRa network (LoRaWAN, a sensor)" })
            } else if lorawan {
                format!("LoRa SF{sf}/125 kHz: a LoRaWAN device (sensor, tracker, meter){}", if region.is_empty() { String::new() } else { format!(", {region}") })
            } else {
                format!("LoRa SF{sf}/{bw} kHz (mesh radio, sensor network or RC link)")
            }
        }
        "ook" => {
            let what = if in_(433.0, 435.0) || in_(314.0, 316.0) || in_(303.0, 304.0) || in_(868.0, 869.0) {
                "a remote control, doorbell, weather station, car key fob or wireless sensor"
            } else {
                "an on/off keyed device (remote, sensor, beacon)"
            };
            format!("OOK: {what}")
        }
        "fsk" => {
            let short = b.dur_us < 12_000;
            if region.starts_with("EU 868") || region.starts_with("US915") || region.starts_with("AS923") {
                if short {
                    "FSK bursts: a sensor, smart meter or alarm, or a frequency-hopping RC / telemetry link (ExpressLRS FSK, Crossfire, SiK telemetry: drones) if it repeats on many frequencies".into()
                } else {
                    "FSK: a sensor network, smart meter, pager or telemetry radio".into()
                }
            } else if region == "ISM 433" {
                "FSK: a wireless sensor, weather station, or 433 MHz telemetry radio (drones use these too)".into()
            } else {
                "FSK data (a two-way radio's data burst, pager, telemetry or sensor)".into()
            }
        }
        "carrier" => "An unmodulated carrier (a test tone, a key-down, or a transmitter idling)".into(),
        "wide" => {
            if in_(1080.0, 1360.0) {
                "Wideband in 1.08–1.36 GHz: possibly 1.2/1.3 GHz analog FPV video (drones) or a radar".into()
            } else {
                "A wideband signal (digital TV / DAB / data)".into()
            }
        }
        "continuous" => "A continuous transmission (broadcast, voice, video or a jammer)".into(),
        _ => "Not identified".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 2.048e6;

    fn noise(n: usize, seed: &mut u32, amp: f32) -> Vec<C> {
        (0..n)
            .map(|_| {
                let mut r = || {
                    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (*seed >> 8) as f32 / 16777216.0 - 0.5
                };
                C::new(r() * amp, r() * amp)
            })
            .collect()
    }

    fn run(signal: Vec<C>) -> Vec<Burst> {
        let mut seed = 7;
        let mut x = noise(200_000, &mut seed, 0.02);
        let start = x.len();
        x.extend(noise(signal.len(), &mut seed, 0.02));
        for (i, s) in signal.iter().enumerate() {
            x[start + i] += *s;
        }
        x.extend(noise(100_000, &mut seed, 0.02));
        let mut d = Detector::new(RATE, 433.92e6);
        let mut out = vec![];
        for c in x.chunks(16384) {
            out.extend(d.feed(c, 0));
        }
        out
    }

    #[test]
    fn ook_remote() {
        // 24 bits, pulse-width coded: 1 = 900 µs pulse, 0 = 300 µs, 1200 µs period, at +200 kHz.
        let bits = [1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 1];
        let mut sig = vec![];
        let mut ph = 0f64;
        for b in bits {
            let on = if b == 1 { 900 } else { 300 };
            for i in 0..(1200.0e-6 * RATE) as usize {
                ph += 2.0 * std::f64::consts::PI * 200e3 / RATE;
                let a = if (i as f64) < on as f64 * 1e-6 * RATE { 0.3 } else { 0.0 };
                sig.push(C::new((a * ph.cos()) as f32, (a * ph.sin()) as f32));
            }
        }
        let out = run(sig);
        assert_eq!(out.len(), 1, "{out:#?}");
        let b = &out[0];
        assert_eq!(b.kind, "ook", "{b:#?}");
        assert!((b.freq - 434.12e6).abs() < 10e3, "{}", b.freq);
        let bits_s: String = bits.iter().map(|b| if *b == 1 { '1' } else { '0' }).collect();
        assert!(b.bits.as_deref().unwrap_or("").contains(&bits_s), "{:?}", b.bits);
        assert!(b.guess.contains("remote"), "{}", b.guess);
    }

    #[test]
    fn fsk_burst() {
        // 9600 baud, ±20 kHz, 120 bits, at −300 kHz.
        let mut sig = vec![];
        let mut ph = 0f64;
        let spb = RATE / 9600.0;
        let mut seed = 3u32;
        for _ in 0..120 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let f = -300e3 + if seed >> 31 == 1 { 20e3 } else { -20e3 };
            for _ in 0..spb.round() as usize {
                ph += 2.0 * std::f64::consts::PI * f / RATE;
                sig.push(C::new(0.3 * ph.cos() as f32, 0.3 * ph.sin() as f32));
            }
        }
        let out = run(sig);
        assert_eq!(out.len(), 1, "{out:#?}");
        let b = &out[0];
        assert_eq!(b.kind, "fsk", "{b:#?}");
        assert!((b.deviation.unwrap() - 20e3).abs() < 3e3, "{:?}", b.deviation);
        assert!((b.baud.unwrap() - 9600.0).abs() < 1000.0, "{:?}", b.baud);
    }

    fn lora(sf: u32, bw: f64, symbols: usize, at: f64) -> Vec<C> {
        lora_amp(sf, bw, symbols, at, 0.3)
    }

    fn lora_amp(sf: u32, bw: f64, symbols: usize, at: f64, amp: f32) -> Vec<C> {
        let t_sym = (1u32 << sf) as f64 / bw;
        let n = (t_sym * RATE).round() as usize;
        let mut sig = vec![];
        let mut ph = 0f64;
        for s in 0..symbols {
            // Preamble-like up-chirps with a few data shifts.
            let shift = if s < 8 { 0.0 } else { ((s * 37) % (1 << sf)) as f64 / (1u32 << sf) as f64 };
            for i in 0..n {
                let frac = (i as f64 / n as f64 + shift) % 1.0;
                let f = at - bw / 2.0 + bw * frac;
                ph += 2.0 * std::f64::consts::PI * f / RATE;
                sig.push(C::new(amp * ph.cos() as f32, amp * ph.sin() as f32));
            }
        }
        sig
    }

    #[test]
    fn lora_chirps() {
        for (sf, bw) in [(7, 125e3), (9, 250e3), (11, 250e3), (7, 500e3)] {
            let out = run(lora(sf, bw, 14, 150e3));
            assert_eq!(out.len(), 1, "SF{sf} {bw}: {out:#?}");
            let b = &out[0];
            assert_eq!(b.kind, "lora", "SF{sf} {bw}: {b:#?}");
            assert_eq!(b.sf, Some(sf), "SF{sf} {bw}: {b:#?}");
            assert_eq!(b.lora_bw, Some(bw), "SF{sf} {bw}: {b:#?}");
        }
    }

    #[test]
    fn weak_lora() {
        // Noise is ±0.01 per sample over 2 MHz; within the LoRa channel that's ~8–14 dB SNR.
        for (sf, bw, amp) in [(11, 250e3, 0.012), (7, 125e3, 0.012), (12, 125e3, 0.008)] {
            let out = run(lora_amp(sf, bw, 12, -400e3, amp));
            let b = out.iter().find(|b| b.kind == "lora");
            assert!(b.is_some(), "SF{sf} {bw} amp {amp}: {:?}", out.iter().map(|b| (b.kind, b.modulation.clone(), b.snr)).collect::<Vec<_>>());
            assert_eq!(b.unwrap().sf, Some(sf));
        }
    }

    #[test]
    fn nothing_in_noise() {
        assert!(run(vec![]).is_empty());
    }
}
