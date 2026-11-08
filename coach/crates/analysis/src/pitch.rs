//! Fundamental frequency (F0) tracking with the YIN algorithm
//! (de Cheveigné & Kawahara, 2002), plus post-processing that removes
//! the usual tracker artefacts: isolated voiced blips and octave jumps.
//!
//! The tracker is streaming: push audio in any chunk size, get one
//! [`PitchFrame`] per 10 ms hop once enough audio has arrived.

use serde::{Deserialize, Serialize};

use crate::{stats, HOP, SAMPLE_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PitchConfig {
    pub sample_rate: u32,
    pub hop: usize,
    /// Lowest F0 searched, in Hz. 60 Hz covers low male voices.
    pub fmin: f32,
    /// Highest F0 searched, in Hz.
    pub fmax: f32,
    /// YIN absolute threshold on the cumulative-mean-normalised difference.
    pub threshold: f32,
    /// Frames quieter than this RMS (linear) are unvoiced without running YIN.
    pub silence_rms: f32,
    /// Integration window in samples (the `W` in the YIN paper).
    pub window: usize,
    /// Low-pass cutoff applied before YIN, Hz. Removes formant fine structure so the
    /// difference function has broad, reliable dips (0 disables).
    pub lowpass_hz: f32,
}

impl Default for PitchConfig {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            hop: HOP,
            fmin: 60.0,
            fmax: 500.0,
            threshold: 0.25,
            silence_rms: 0.003,
            window: 400, // 25 ms
            lowpass_hz: 1000.0,
        }
    }
}

impl PitchConfig {
    fn tau_min(&self) -> usize {
        (self.sample_rate as f32 / self.fmax).floor().max(2.0) as usize
    }
    fn tau_max(&self) -> usize {
        (self.sample_rate as f32 / self.fmin).ceil() as usize
    }
    /// Samples needed for one analysis frame.
    pub fn frame_len(&self) -> usize {
        self.window + self.tau_max() + 2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PitchFrame {
    /// Frame centre, seconds from the start of the stream.
    pub time: f32,
    /// F0 in Hz, `None` when unvoiced.
    pub f0: Option<f32>,
    /// 1 − aperiodicity at the chosen lag (0..1). High means clearly periodic.
    pub confidence: f32,
    /// Frame RMS (linear).
    pub rms: f32,
}

/// Estimate F0 for one frame. `frame.len()` must be at least `cfg.frame_len()`.
/// Returns `(f0, confidence)`.
pub fn yin_frame(frame: &[f32], cfg: &PitchConfig) -> (Option<f32>, f32) {
    let w = cfg.window;
    let tau_min = cfg.tau_min();
    let tau_max = cfg.tau_max().min(frame.len().saturating_sub(w + 2));
    if tau_max <= tau_min + 2 {
        return (None, 0.0);
    }

    // Difference function d(tau).
    let mut d = vec![0.0f32; tau_max + 2];
    for (tau, slot) in d.iter_mut().enumerate().skip(1) {
        let mut sum = 0.0f32;
        for j in 0..w {
            let diff = frame[j] - frame[j + tau];
            sum += diff * diff;
        }
        *slot = sum;
    }

    // Cumulative mean normalised difference d'(tau).
    let mut cmnd = vec![1.0f32; d.len()];
    let mut running = 0.0f32;
    for tau in 1..d.len() {
        running += d[tau];
        cmnd[tau] = if running > 0.0 { d[tau] * tau as f32 / running } else { 1.0 };
    }

    // Absolute threshold: first dip below threshold, then descend to its local minimum.
    let mut chosen = None;
    let mut tau = tau_min;
    while tau <= tau_max {
        if cmnd[tau] < cfg.threshold {
            while tau < tau_max && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            chosen = Some(tau);
            break;
        }
        tau += 1;
    }
    let Some(tau) = chosen else {
        return (None, 0.0);
    };

    // Parabolic interpolation on d'(tau) for sub-sample precision.
    let refined = if tau > 1 && tau + 1 < cmnd.len() {
        let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
        let denom = a - 2.0 * b + c;
        if denom.abs() > 1e-9 {
            tau as f32 + 0.5 * (a - c) / denom
        } else {
            tau as f32
        }
    } else {
        tau as f32
    };

    let f0 = cfg.sample_rate as f32 / refined;
    if !(cfg.fmin * 0.95..=cfg.fmax * 1.05).contains(&f0) {
        return (None, 0.0);
    }
    (Some(f0), (1.0 - cmnd[tau]).clamp(0.0, 1.0))
}

/// RBJ biquad low-pass section.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn lowpass(cutoff: f32, sample_rate: f32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * cutoff / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Streaming YIN tracker producing one frame per hop.
#[derive(Debug, Clone)]
pub struct PitchTracker {
    cfg: PitchConfig,
    /// Raw audio (for intensity).
    buf: Vec<f32>,
    /// Low-passed audio (for YIN), aligned with `buf`.
    filtered: Vec<f32>,
    filters: Option<[Biquad; 2]>,
    /// Absolute sample index of `buf[0]`.
    buf_start: usize,
    /// Absolute sample index where the next frame starts.
    next_frame: usize,
}

impl PitchTracker {
    pub fn new(cfg: PitchConfig) -> Self {
        let filters = (cfg.lowpass_hz > 0.0).then(|| {
            let sr = cfg.sample_rate as f32;
            // Two Butterworth sections ≈ 4th-order low-pass.
            [Biquad::lowpass(cfg.lowpass_hz, sr, 0.541), Biquad::lowpass(cfg.lowpass_hz, sr, 1.307)]
        });
        Self {
            cfg,
            buf: Vec::with_capacity(cfg.frame_len() * 4),
            filtered: Vec::with_capacity(cfg.frame_len() * 4),
            filters,
            buf_start: 0,
            next_frame: 0,
        }
    }

    pub fn config(&self) -> &PitchConfig {
        &self.cfg
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.cfg);
    }

    /// Push audio; returns every frame that became complete.
    pub fn push(&mut self, samples: &[f32]) -> Vec<PitchFrame> {
        self.buf.extend_from_slice(samples);
        match self.filters.as_mut() {
            Some([a, b]) => self.filtered.extend(samples.iter().map(|&x| b.process(a.process(x)))),
            None => self.filtered.extend_from_slice(samples),
        }
        let frame_len = self.cfg.frame_len();
        let mut out = Vec::new();
        loop {
            let rel = self.next_frame - self.buf_start;
            if rel + frame_len > self.buf.len() {
                break;
            }
            let rms = rms(&self.buf[rel..rel + self.cfg.window]);
            let (f0, confidence) = if rms < self.cfg.silence_rms {
                (None, 0.0)
            } else {
                yin_frame(&self.filtered[rel..rel + frame_len], &self.cfg)
            };
            let centre = self.next_frame + self.cfg.window / 2;
            out.push(PitchFrame {
                time: centre as f32 / self.cfg.sample_rate as f32,
                f0,
                confidence,
                rms,
            });
            self.next_frame += self.cfg.hop;
        }
        // Drop consumed audio, keeping what the next frame needs.
        let rel = self.next_frame - self.buf_start;
        if rel > 0 {
            self.buf.drain(..rel);
            self.filtered.drain(..rel);
            self.buf_start = self.next_frame;
        }
        out
    }
}

/// Track a complete clip. Frame `i` is centred at `i * hop + window / 2` samples.
pub fn track(samples: &[f32], cfg: &PitchConfig) -> Vec<PitchFrame> {
    let mut tracker = PitchTracker::new(*cfg);
    let mut frames = tracker.push(samples);
    // Pad the tail so the final hops are analysed too.
    frames.extend(tracker.push(&vec![0.0; cfg.frame_len()]));
    let max_time = samples.len() as f32 / cfg.sample_rate as f32;
    frames.retain(|f| f.time <= max_time);
    frames
}

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

/// Clean a raw F0 track:
/// 1. drop voiced runs shorter than `min_run` frames (blips),
/// 2. fold octave jumps back toward the local median,
/// 3. median-smooth each voiced run.
pub fn clean(frames: &[PitchFrame], min_run: usize) -> Vec<Option<f32>> {
    let mut f0: Vec<Option<f32>> = frames.iter().map(|f| f.f0).collect();

    // 1. Remove short voiced runs.
    let mut i = 0;
    while i < f0.len() {
        if f0[i].is_some() {
            let start = i;
            while i < f0.len() && f0[i].is_some() {
                i += 1;
            }
            if i - start < min_run {
                f0[start..i].iter_mut().for_each(|v| *v = None);
            }
        } else {
            i += 1;
        }
    }

    // 2. Octave correction against the utterance median.
    let voiced: Vec<f32> = f0.iter().flatten().copied().collect();
    if let Some(med) = stats::median(&voiced) {
        for v in f0.iter_mut().flatten() {
            let ratio = *v / med;
            if ratio > 1.8 {
                *v /= 2.0;
            } else if ratio < 0.55 {
                *v *= 2.0;
            }
        }
    }

    // 3. Median smoothing within voiced runs.
    let mut i = 0;
    while i < f0.len() {
        if f0[i].is_some() {
            let start = i;
            while i < f0.len() && f0[i].is_some() {
                i += 1;
            }
            let run: Vec<f32> = f0[start..i].iter().map(|v| v.unwrap()).collect();
            for (k, v) in stats::median_filter(&run, 5).into_iter().enumerate() {
                f0[start + k] = Some(v);
            }
        } else {
            i += 1;
        }
    }
    f0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    fn voiced_median(frames: &[PitchFrame]) -> f32 {
        let v: Vec<f32> = frames.iter().filter_map(|f| f.f0).collect();
        stats::median(&v).expect("some voiced frames")
    }

    #[test]
    fn detects_steady_pitch() {
        for f0 in [85.0, 120.0, 210.0, 330.0] {
            let audio = synth::harmonic(f0, 0.5, 0.4);
            let frames = track(&audio, &PitchConfig::default());
            let est = voiced_median(&frames);
            assert!((est - f0).abs() / f0 < 0.01, "f0 {f0}: estimated {est}");
            let voiced = frames.iter().filter(|f| f.f0.is_some()).count();
            assert!(voiced as f32 > frames.len() as f32 * 0.9, "f0 {f0}: only {voiced} voiced");
        }
    }

    #[test]
    fn pure_sine_is_tracked() {
        let n = 8000;
        let audio: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 150.0 * i as f32 / 16000.0).sin())
            .collect();
        let est = voiced_median(&track(&audio, &PitchConfig::default()));
        assert!((est - 150.0).abs() < 1.5, "estimated {est}");
    }

    #[test]
    fn silence_and_noise_are_unvoiced() {
        let cfg = PitchConfig::default();
        assert!(track(&synth::silence(0.5), &cfg).iter().all(|f| f.f0.is_none()));
        let noisy = track(&synth::noise(0.5, 0.2, 3), &cfg);
        let voiced = noisy.iter().filter(|f| f.f0.is_some()).count();
        assert!(voiced < noisy.len() / 10, "{voiced} of {} noise frames voiced", noisy.len());
    }

    #[test]
    fn follows_a_glide() {
        let audio = synth::glide(120.0, 240.0, 1.0, 0.4);
        let frames = track(&audio, &PitchConfig::default());
        let voiced: Vec<&PitchFrame> = frames.iter().filter(|f| f.f0.is_some()).collect();
        let first = voiced[5].f0.unwrap();
        let last = voiced[voiced.len() - 5].f0.unwrap();
        assert!(first < 135.0, "start {first}");
        assert!(last > 220.0, "end {last}");
    }

    #[test]
    fn streaming_matches_offline() {
        let audio = synth::harmonic(180.0, 0.4, 0.4);
        let cfg = PitchConfig::default();
        let offline = track(&audio, &cfg);
        let mut tracker = PitchTracker::new(cfg);
        let mut streamed = Vec::new();
        for chunk in audio.chunks(237) {
            streamed.extend(tracker.push(chunk));
        }
        assert!(!streamed.is_empty());
        for (a, b) in streamed.iter().zip(&offline) {
            assert_eq!(a.time, b.time);
            assert_eq!(a.f0, b.f0);
        }
    }

    #[test]
    fn frames_are_ten_ms_apart() {
        let frames = track(&synth::harmonic(150.0, 0.3, 0.4), &PitchConfig::default());
        for pair in frames.windows(2) {
            assert!((pair[1].time - pair[0].time - 0.01).abs() < 1e-5);
        }
    }

    #[test]
    fn clean_removes_blips_and_octave_jumps() {
        let mk = |f0: Option<f32>| PitchFrame { time: 0.0, f0, confidence: 1.0, rms: 0.1 };
        let mut frames: Vec<PitchFrame> = (0..30).map(|_| mk(Some(150.0))).collect();
        frames[10] = mk(Some(300.0)); // octave error
        frames.push(mk(None));
        frames.push(mk(Some(200.0))); // blip
        frames.push(mk(None));
        let cleaned = clean(&frames, 3);
        assert!((cleaned[10].unwrap() - 150.0).abs() < 1.0);
        assert_eq!(cleaned[31], None);
    }
}
