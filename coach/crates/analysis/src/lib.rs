//! Speech analysis engine for Aussie Accent Coach.
//!
//! Everything in this crate is pure, synchronous DSP over 16 kHz mono `f32` audio.
//! It has no I/O and no platform dependencies, so it is tested headless against
//! synthetic signals and WAV fixtures.
//!
//! Frame rate is fixed at 100 frames per second (10 ms hop) across all modules,
//! so frame index `i` always means time `i * 0.01` seconds from the analysis start.

pub mod compare;
pub mod contour;
pub mod dtw;
pub mod energy;
pub mod formants;
pub mod live;
pub mod normalize;
pub mod pitch;
pub mod rate;
pub mod stats;
pub mod vad;

/// Engine sample rate. All audio entering the engine is resampled to this.
pub const SAMPLE_RATE: u32 = 16_000;
/// Analysis hop in samples (10 ms at 16 kHz).
pub const HOP: usize = 160;
/// Frames per second implied by [`HOP`].
pub const FRAME_RATE: f32 = SAMPLE_RATE as f32 / HOP as f32;

/// Convert a frame index to seconds.
#[inline]
pub fn frame_time(index: usize) -> f32 {
    index as f32 / FRAME_RATE
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AnalysisError {
    #[error("audio is too short to analyse ({0:.2} s)")]
    TooShort(f32),
    #[error("no speech detected")]
    NoSpeech,
    #[error("no voiced frames detected")]
    NoVoicing,
}

pub use compare::{analyse, analyse_with_segments, compare, Comparison, UtteranceAnalysis};
pub use live::{LiveAnalyzer, LiveFrame};
pub use pitch::{PitchConfig, PitchFrame, PitchTracker};

#[cfg(test)]
pub(crate) mod synth {
    //! Synthetic test signals.
    use std::f32::consts::PI;

    use crate::SAMPLE_RATE;

    /// A harmonic-rich "voiced" tone: sum of the first `n` harmonics with 1/k amplitude.
    pub fn harmonic(f0: f32, seconds: f32, amp: f32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        let sr = SAMPLE_RATE as f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                let mut s = 0.0;
                for k in 1..=8 {
                    let fk = f0 * k as f32;
                    if fk < sr / 2.0 {
                        s += (2.0 * PI * fk * t).sin() / k as f32;
                    }
                }
                amp * s / 2.0
            })
            .collect()
    }

    /// A harmonic tone whose pitch glides linearly from `f_start` to `f_end`.
    pub fn glide(f_start: f32, f_end: f32, seconds: f32, amp: f32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        let sr = SAMPLE_RATE as f32;
        let mut phase = 0.0f32;
        (0..n)
            .map(|i| {
                let f = f_start + (f_end - f_start) * i as f32 / n as f32;
                phase += 2.0 * PI * f / sr;
                let mut s = 0.0;
                for k in 1..=6 {
                    s += (phase * k as f32).sin() / k as f32;
                }
                amp * s / 2.0
            })
            .collect()
    }

    pub fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * SAMPLE_RATE as f32) as usize]
    }

    /// Deterministic low-level noise (LCG), so tests never depend on a RNG crate.
    pub fn noise(seconds: f32, amp: f32, seed: u32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let u = (state >> 8) as f32 / (1u32 << 24) as f32;
                amp * (2.0 * u - 1.0)
            })
            .collect()
    }

    /// `count` syllable-like bursts: Hann-enveloped voiced tone, separated by quiet gaps.
    pub fn syllables(count: usize, f0: f32, syl_s: f32, gap_s: f32) -> Vec<f32> {
        let mut out = silence(0.3);
        for _ in 0..count {
            let mut burst = harmonic(f0, syl_s, 0.5);
            let n = burst.len() as f32;
            for (i, s) in burst.iter_mut().enumerate() {
                *s *= 0.5 - 0.5 * (2.0 * PI * i as f32 / n).cos();
            }
            out.extend(burst);
            out.extend(noise(gap_s, 0.002, 7));
        }
        out.extend(silence(0.3));
        out
    }
}
