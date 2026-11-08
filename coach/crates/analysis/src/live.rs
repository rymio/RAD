//! Real-time analysis for the Practice screen: pitch, intensity, speech/non-speech,
//! and a rolling articulation rate, emitted per 10 ms frame.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::{
    contour, energy,
    pitch::{PitchConfig, PitchTracker},
    rate::{self, RateConfig},
    stats, FRAME_RATE,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LiveFrame {
    /// Seconds since the analyser started.
    pub t: f32,
    pub f0_hz: Option<f32>,
    /// Semitones relative to the speaker baseline.
    pub st: Option<f32>,
    pub db: f32,
    pub speaking: bool,
}

#[derive(Debug, Clone)]
pub struct LiveAnalyzer {
    tracker: PitchTracker,
    /// Calibrated speaker median F0; if `None`, adapted from voiced frames seen so far.
    calibrated_hz: Option<f32>,
    seen_f0: VecDeque<f32>,
    db_hist: VecDeque<f32>,
    voiced_hist: VecDeque<bool>,
    speaking_hist: VecDeque<bool>,
    onset: Option<f32>,
    last_speech: Option<f32>,
    /// Noise floor estimate in dBFS: drops instantly, rises slowly (≈ 5 s time constant),
    /// so sustained speech never becomes "the floor".
    floor_db: f32,
}

/// Rolling window used for the noise floor and the live rate (3 s).
const HISTORY: usize = 300;

impl LiveAnalyzer {
    pub fn new(calibrated_hz: Option<f32>) -> Self {
        Self {
            tracker: PitchTracker::new(PitchConfig::default()),
            calibrated_hz,
            seen_f0: VecDeque::new(),
            db_hist: VecDeque::with_capacity(HISTORY),
            voiced_hist: VecDeque::with_capacity(HISTORY),
            speaking_hist: VecDeque::with_capacity(HISTORY),
            onset: None,
            last_speech: None,
            floor_db: -70.0,
        }
    }

    /// Current pitch baseline in Hz.
    pub fn baseline_hz(&self) -> Option<f32> {
        self.calibrated_hz.or_else(|| {
            let v: Vec<f32> = self.seen_f0.iter().copied().collect();
            if v.len() >= 20 { stats::median(&v) } else { None }
        })
    }

    /// Time of the first detected speech, seconds since start.
    pub fn onset(&self) -> Option<f32> {
        self.onset
    }

    /// Time of the most recent speech frame.
    pub fn last_speech(&self) -> Option<f32> {
        self.last_speech
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<LiveFrame> {
        let frames = self.tracker.push(samples);
        let mut out = Vec::with_capacity(frames.len());
        for f in frames {
            let db = energy::rms_to_db(f.rms);
            push_capped(&mut self.db_hist, db);
            if db < self.floor_db {
                self.floor_db = db.max(-90.0);
            } else {
                self.floor_db = (self.floor_db + (db - self.floor_db) * 0.002).min(-35.0);
            }
            let speaking = db > -60.0 && db > self.floor_db + 10.0;
            let f0 = if speaking { f.f0 } else { None };
            push_capped(&mut self.voiced_hist, f0.is_some());
            push_capped(&mut self.speaking_hist, speaking);
            if speaking {
                self.onset.get_or_insert(f.time);
                self.last_speech = Some(f.time);
            }
            if let Some(hz) = f0 {
                self.seen_f0.push_back(hz);
                if self.seen_f0.len() > 1000 {
                    self.seen_f0.pop_front();
                }
            }
            let st = match (f0, self.baseline_hz()) {
                (Some(hz), Some(base)) => Some(contour::semitones(hz, base)),
                _ => None,
            };
            out.push(LiveFrame { t: f.time, f0_hz: f0, st, db, speaking });
        }
        out
    }

    /// Articulation rate (syllables per second of speech) over the last 3 s.
    /// `None` until there is at least half a second of speech in the window.
    pub fn rate_sps(&mut self) -> Option<f32> {
        let speech_frames = self.speaking_hist.iter().filter(|&&s| s).count();
        let speech_s = speech_frames as f32 / FRAME_RATE;
        if speech_s < 0.5 {
            return None;
        }
        let db = self.db_hist.make_contiguous().to_vec();
        let voiced = self.voiced_hist.make_contiguous().to_vec();
        let n = rate::nuclei(&db, Some(&voiced), &RateConfig::default()).len();
        Some(n as f32 / speech_s)
    }
}

fn push_capped<T>(q: &mut VecDeque<T>, v: T) {
    if q.len() == HISTORY {
        q.pop_front();
    }
    q.push_back(v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    #[test]
    fn live_tracks_pitch_onset_and_rate() {
        let mut audio = synth::noise(0.5, 0.0005, 9);
        audio.extend(synth::syllables(10, 160.0, 0.15, 0.05));
        let mut live = LiveAnalyzer::new(Some(160.0));
        let mut frames = Vec::new();
        for chunk in audio.chunks(512) {
            frames.extend(live.push(chunk));
        }
        let onset = live.onset().expect("onset");
        assert!(onset > 0.75 && onset < 0.9, "onset {onset}"); // 0.5 s noise + 0.3 s lead silence
        let voiced: Vec<f32> = frames.iter().filter_map(|f| f.st).collect();
        let med = stats::median(&voiced).unwrap();
        assert!(med.abs() < 0.3, "median {med} st from baseline");
        // 10 syllables of 0.15 s ≈ 6.7 syllables per second of phonation; window holds most of them.
        let r = live.rate_sps().expect("rate");
        assert!((4.0..9.0).contains(&r), "rate {r}");
    }

    #[test]
    fn baseline_adapts_without_calibration() {
        let audio = synth::harmonic(200.0, 1.0, 0.4);
        let mut live = LiveAnalyzer::new(None);
        live.push(&audio);
        let b = live.baseline_hz().unwrap();
        assert!((b - 200.0).abs() < 3.0, "{b}");
    }
}
