//! Energy-based voice activity detection with an adaptive noise floor.
//!
//! This is the M1 detector. The scope's Silero VAD (ONNX) slots in behind the same
//! [`Segment`] output in M2 for noisy rooms; the energy detector stays as the
//! zero-dependency fallback.

use serde::{Deserialize, Serialize};

use crate::{frame_time, stats, FRAME_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VadConfig {
    /// Speech must be this many dB above the estimated noise floor.
    pub above_floor_db: f32,
    /// …and no more than this many dB below the loudest frame.
    pub below_peak_db: f32,
    /// Shorter speech runs are discarded (clicks, breaths).
    pub min_speech_s: f32,
    /// Gaps shorter than this are bridged (stop closures, short dips).
    pub min_pause_s: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self { above_floor_db: 12.0, below_peak_db: 40.0, min_speech_s: 0.08, min_pause_s: 0.18 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f32,
    pub end: f32,
}

impl Segment {
    pub fn duration(&self) -> f32 {
        self.end - self.start
    }
}

/// Per-frame speech decision before smoothing.
pub fn speech_mask(db: &[f32], cfg: &VadConfig) -> Vec<bool> {
    let floor = stats::percentile(db, 10.0).unwrap_or(-100.0);
    let peak = stats::percentile(db, 99.0).unwrap_or(0.0);
    let threshold = (floor + cfg.above_floor_db).max(peak - cfg.below_peak_db);
    db.iter().map(|&d| d > threshold).collect()
}

/// Speech segments from a per-frame intensity track (dBFS, 100 frames/s).
/// `frame_offset_s` is the time of frame 0's centre.
pub fn segments(db: &[f32], frame_offset_s: f32, cfg: &VadConfig) -> Vec<Segment> {
    let mask = speech_mask(db, cfg);
    let min_pause = (cfg.min_pause_s * FRAME_RATE).round() as usize;
    let min_speech = (cfg.min_speech_s * FRAME_RATE).round() as usize;

    // Collect raw runs.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        if mask[i] {
            let start = i;
            while i < mask.len() && mask[i] {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }

    // Bridge short gaps.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for run in runs {
        match merged.last_mut() {
            Some(last) if run.0 - last.1 < min_pause => last.1 = run.1,
            _ => merged.push(run),
        }
    }

    merged
        .into_iter()
        .filter(|(s, e)| e - s >= min_speech)
        .map(|(s, e)| Segment {
            start: frame_offset_s + frame_time(s),
            end: frame_offset_s + frame_time(e),
        })
        .collect()
}

/// Pauses between segments, in seconds.
pub fn pauses(segments: &[Segment]) -> Vec<f32> {
    segments.windows(2).map(|w| w[1].start - w[0].end).collect()
}

/// Split speech into phrases separated by pauses of at least `min_pause_s`.
/// Used to cut a long reference narration into practice-sized pieces.
pub fn phrases(segments: &[Segment], min_pause_s: f32) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for seg in segments {
        match out.last_mut() {
            Some(last) if seg.start - last.end < min_pause_s => last.end = seg.end,
            _ => out.push(*seg),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{energy, pitch, synth};

    fn db_track(audio: &[f32]) -> (Vec<f32>, f32) {
        let frames = pitch::track(audio, &pitch::PitchConfig::default());
        let offset = frames.first().map(|f| f.time).unwrap_or(0.0);
        (frames.iter().map(|f| energy::rms_to_db(f.rms)).collect(), offset)
    }

    #[test]
    fn finds_two_words_with_a_pause() {
        let mut audio = synth::noise(0.4, 0.001, 1);
        audio.extend(synth::harmonic(140.0, 0.5, 0.4));
        audio.extend(synth::noise(0.6, 0.001, 2));
        audio.extend(synth::harmonic(160.0, 0.4, 0.4));
        audio.extend(synth::noise(0.4, 0.001, 3));
        let (db, offset) = db_track(&audio);
        let segs = segments(&db, offset, &VadConfig::default());
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert!((segs[0].start - 0.4).abs() < 0.05, "{segs:?}");
        assert!((segs[0].duration() - 0.5).abs() < 0.06, "{segs:?}");
        let p = pauses(&segs);
        assert!((p[0] - 0.6).abs() < 0.06, "{p:?}");
    }

    #[test]
    fn short_gaps_are_bridged_and_phrases_join() {
        let mut audio = synth::silence(0.3);
        audio.extend(synth::harmonic(140.0, 0.3, 0.4));
        audio.extend(synth::noise(0.1, 0.001, 4)); // stop closure, bridged
        audio.extend(synth::harmonic(140.0, 0.3, 0.4));
        audio.extend(synth::noise(0.25, 0.001, 5)); // real pause
        audio.extend(synth::harmonic(140.0, 0.3, 0.4));
        audio.extend(synth::silence(0.3));
        let (db, offset) = db_track(&audio);
        let segs = segments(&db, offset, &VadConfig::default());
        assert_eq!(segs.len(), 2, "{segs:?}");
        assert_eq!(phrases(&segs, 0.35).len(), 1);
        assert_eq!(phrases(&segs, 0.2).len(), 2);
    }
}
