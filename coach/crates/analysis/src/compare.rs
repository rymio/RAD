//! Whole-utterance analysis and learner-vs-reference comparison.
//!
//! [`analyse`] turns a clip into contours, segments and rate. [`compare`] aligns a
//! learner attempt to a reference with DTW over pitch and intensity, then reports
//! intonation similarity, pace, pauses, the final rise, a pitch overlay on the
//! reference's timeline and a local tempo curve (where you rushed or dragged).

use serde::{Deserialize, Serialize};

use crate::{
    contour, dtw, energy, frame_time, pitch,
    rate::{self, RateConfig, RateReport},
    stats,
    vad::{self, Segment, VadConfig},
    AnalysisError, FRAME_RATE, SAMPLE_RATE,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UtteranceAnalysis {
    pub duration_s: f32,
    /// Time of frame 0's centre, seconds.
    pub frame_offset_s: f32,
    /// Cleaned F0 per frame (Hz).
    pub f0: Vec<Option<f32>>,
    /// F0 in semitones relative to `median_f0_hz`.
    pub semitones: Vec<Option<f32>>,
    pub intensity_db: Vec<f32>,
    pub median_f0_hz: f32,
    /// 10th–90th percentile pitch spread over speech, semitones.
    pub range_st: f32,
    /// Pitch movement over the last 300 ms of voicing, semitones.
    pub final_movement_st: Option<f32>,
    pub segments: Vec<Segment>,
    /// Frame indices of syllable nuclei.
    pub nuclei: Vec<usize>,
    pub rate: RateReport,
}

impl UtteranceAnalysis {
    /// Frame range covering first speech onset to last speech offset.
    pub fn speech_frames(&self) -> std::ops::Range<usize> {
        let to_frame = |t: f32| (((t - self.frame_offset_s) * FRAME_RATE).round().max(0.0) as usize).min(self.f0.len());
        match (self.segments.first(), self.segments.last()) {
            (Some(a), Some(b)) => to_frame(a.start)..to_frame(b.end),
            _ => 0..0,
        }
    }

    pub fn time_of(&self, frame: usize) -> f32 {
        self.frame_offset_s + frame_time(frame)
    }
}

/// Analyse a complete 16 kHz mono clip.
pub fn analyse(samples: &[f32]) -> Result<UtteranceAnalysis, AnalysisError> {
    analyse_with_segments(samples, None)
}

/// As [`analyse`], with speech segments from an external detector (Silero VAD) instead of
/// the built-in energy detector. Segments shorter than 50 ms are ignored.
pub fn analyse_with_segments(samples: &[f32], external: Option<Vec<Segment>>) -> Result<UtteranceAnalysis, AnalysisError> {
    let duration_s = samples.len() as f32 / SAMPLE_RATE as f32;
    if duration_s < 0.25 {
        return Err(AnalysisError::TooShort(duration_s));
    }
    let frames = pitch::track(samples, &pitch::PitchConfig::default());
    let frame_offset_s = frames.first().map(|f| f.time).unwrap_or(0.0);
    let intensity_db: Vec<f32> = frames.iter().map(|f| energy::rms_to_db(f.rms)).collect();
    let segments = match external {
        Some(ext) if ext.iter().any(|s| s.duration() >= 0.05) => {
            ext.into_iter().filter(|s| s.duration() >= 0.05).collect()
        }
        _ => vad::segments(&intensity_db, frame_offset_s, &VadConfig::default()),
    };
    if segments.is_empty() {
        return Err(AnalysisError::NoSpeech);
    }

    // Only trust voicing inside speech; outside it, YIN finds hum and breath.
    let mut cleaned = pitch::clean(&frames, 4);
    let in_speech = |t: f32| segments.iter().any(|s| t >= s.start - 0.02 && t <= s.end + 0.02);
    for (v, f) in cleaned.iter_mut().zip(&frames) {
        if !in_speech(f.time) {
            *v = None;
        }
    }
    let (semitones, median_f0_hz) = contour::to_semitones(&cleaned).ok_or(AnalysisError::NoVoicing)?;

    let voiced: Vec<bool> = cleaned.iter().map(Option::is_some).collect();
    let nuclei = rate::nuclei(&intensity_db, Some(&voiced), &RateConfig::default());
    let rate = rate::report(nuclei.len(), &segments);

    Ok(UtteranceAnalysis {
        duration_s,
        frame_offset_s,
        range_st: contour::range(&semitones),
        final_movement_st: contour::final_movement(&semitones, 0.3),
        f0: cleaned,
        semitones,
        intensity_db,
        median_f0_hz,
        segments,
        nuclei,
        rate,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OverlayPoint {
    /// Seconds from the reference's speech onset.
    pub t: f32,
    pub reference_st: Option<f32>,
    pub attempt_st: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoPoint {
    /// Seconds from the reference's speech onset (window centre).
    pub t: f32,
    /// Your duration / reference duration for this stretch. Above 1 = slower.
    pub ratio: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// Pearson correlation of the aligned pitch contours (−1..1).
    pub contour_corr: Option<f32>,
    /// Your pitch range / reference pitch range.
    pub range_ratio: f32,
    /// Your articulation rate / reference articulation rate (pauses excluded).
    pub articulation_ratio: f32,
    /// Your speaking rate / reference speaking rate (pauses included).
    pub speaking_ratio: f32,
    pub attempt_rate: RateReport,
    pub reference_rate: RateReport,
    pub final_movement_st: Option<f32>,
    pub reference_final_movement_st: Option<f32>,
    pub final_rise: bool,
    pub reference_final_rise: bool,
    /// Your pitch mapped onto the reference timeline.
    pub overlay: Vec<OverlayPoint>,
    pub tempo: Vec<TempoPoint>,
    /// Mean per-step DTW cost; lower = closer in prosody.
    pub alignment_cost: f32,
}

/// Rate report with a known syllable count (from the sentence text) instead of detected nuclei.
pub fn with_syllables(r: &RateReport, syllables: usize) -> RateReport {
    let n = syllables as f32;
    RateReport {
        syllables,
        articulation_sps: if r.phonation_s > 0.0 { n / r.phonation_s } else { 0.0 },
        speaking_sps: if r.span_s > 0.0 { n / r.span_s } else { 0.0 },
        ..*r
    }
}

/// Compare a learner attempt against a reference reading of the same sentence.
///
/// Both readings say the same words, so pace is the ratio of speaking time
/// (pauses excluded) — far steadier than counting syllables in each. Pass the
/// sentence's syllable count to report syllables per second from it.
pub fn compare(
    attempt: &UtteranceAnalysis,
    reference: &UtteranceAnalysis,
    syllables: Option<usize>,
) -> Result<Comparison, AnalysisError> {
    let a_range = attempt.speech_frames();
    let r_range = reference.speech_frames();
    if a_range.len() < 5 || r_range.len() < 5 {
        return Err(AnalysisError::NoSpeech);
    }
    let a_st = contour::interpolate(&attempt.semitones[a_range.clone()]).ok_or(AnalysisError::NoVoicing)?;
    let r_st = contour::interpolate(&reference.semitones[r_range.clone()]).ok_or(AnalysisError::NoVoicing)?;
    let norm_db = |db: &[f32]| -> Vec<f32> {
        let med = stats::median(db).unwrap_or(0.0);
        db.iter().map(|d| (d - med).clamp(-30.0, 15.0)).collect()
    };
    let a_db = norm_db(&attempt.intensity_db[a_range.clone()]);
    let r_db = norm_db(&reference.intensity_db[r_range.clone()]);

    // Intensity carries timing (syllables, pauses); pitch carries the tune.
    let alignment = dtw::align(a_st.len(), r_st.len(), Some(0.3), |i, j| {
        (a_db[i] - r_db[j]).abs() / 6.0 + (a_st[i] - r_st[j]).abs() / 3.0
    })
    .ok_or(AnalysisError::NoSpeech)?;
    let mapped = dtw::map_to_second(&alignment, r_st.len());

    let a_raw = &attempt.semitones[a_range.clone()];
    let r_raw = &reference.semitones[r_range.clone()];
    let mut overlay = Vec::with_capacity(r_raw.len());
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for (j, &ref_st) in r_raw.iter().enumerate() {
        let i = mapped[j];
        let att_st = if i.is_finite() { a_raw.get(i.round() as usize).copied().flatten() } else { None };
        if let (Some(r), Some(a)) = (ref_st, att_st) {
            xs.push(r);
            ys.push(a);
        }
        overlay.push(OverlayPoint { t: frame_time(j), reference_st: ref_st, attempt_st: att_st });
    }

    let window = 30usize;
    let tempo = (0..r_st.len().saturating_sub(window))
        .step_by(10)
        .filter_map(|j| {
            let (i0, i1) = (mapped[j], mapped[j + window]);
            (i0.is_finite() && i1.is_finite())
                .then(|| TempoPoint { t: frame_time(j + window / 2), ratio: ((i1 - i0) / window as f32).max(0.0) })
        })
        .collect();

    let ratio = |a: f32, b: f32| if b > 0.0 { a / b } else { 0.0 };
    let (attempt_rate, reference_rate) = match syllables {
        Some(n) if n > 0 => (with_syllables(&attempt.rate, n), with_syllables(&reference.rate, n)),
        _ => (attempt.rate, reference.rate),
    };
    let rises = |m: Option<f32>| m.is_some_and(|v| v > contour::RISE_THRESHOLD_ST);
    Ok(Comparison {
        contour_corr: stats::pearson(&xs, &ys),
        range_ratio: ratio(attempt.range_st, reference.range_st),
        // Same words in both, so rate ratio = inverse time ratio.
        articulation_ratio: ratio(reference.rate.phonation_s, attempt.rate.phonation_s),
        speaking_ratio: ratio(reference.rate.span_s, attempt.rate.span_s),
        attempt_rate,
        reference_rate,
        final_movement_st: attempt.final_movement_st,
        reference_final_movement_st: reference.final_movement_st,
        final_rise: rises(attempt.final_movement_st),
        reference_final_rise: rises(reference.final_movement_st),
        overlay,
        tempo,
        alignment_cost: alignment.normalized_cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    /// A synthetic "sentence": syllables whose pitch follows `tune` (one Hz value per syllable).
    fn sentence(tune: &[f32], syl_s: f32) -> Vec<f32> {
        let mut out = synth::silence(0.3);
        for &f in tune {
            let mut burst = synth::glide(f, f * 1.03, syl_s, 0.5);
            let n = burst.len() as f32;
            for (i, s) in burst.iter_mut().enumerate() {
                *s *= 0.55 - 0.45 * (2.0 * std::f32::consts::PI * i as f32 / n).cos();
            }
            out.extend(burst);
        }
        out.extend(synth::silence(0.3));
        out
    }

    #[test]
    fn analyse_reports_rate_and_rise() {
        let rising = sentence(&[150.0, 150.0, 140.0, 150.0, 170.0, 195.0], 0.18);
        let a = analyse(&rising).unwrap();
        assert_eq!(a.rate.syllables, 6, "{:?}", a.rate);
        assert!((a.median_f0_hz - 150.0).abs() < 12.0, "{}", a.median_f0_hz);
        assert!(a.final_movement_st.unwrap() > contour::RISE_THRESHOLD_ST, "{:?}", a.final_movement_st);
    }

    #[test]
    fn same_tune_different_voice_and_pace_correlates() {
        let tune = [150.0, 180.0, 140.0, 160.0, 120.0, 135.0];
        let reference = analyse(&sentence(&tune, 0.16)).unwrap();
        let low: Vec<f32> = tune.iter().map(|f| f * 0.7).collect();
        let attempt = analyse(&sentence(&low, 0.22)).unwrap(); // deeper voice, slower
        let c = compare(&attempt, &reference, None).unwrap();
        assert!(c.contour_corr.unwrap() > 0.8, "corr {:?}", c.contour_corr);
        assert!(c.articulation_ratio < 0.85, "articulation ratio {}", c.articulation_ratio);
        let mean_tempo = stats::mean(&c.tempo.iter().map(|p| p.ratio).collect::<Vec<_>>()).unwrap();
        assert!(mean_tempo > 1.15, "tempo {mean_tempo}");
        assert_eq!(c.overlay.len(), reference.speech_frames().len());
    }

    #[test]
    fn different_tune_scores_lower() {
        let tune = [150.0, 190.0, 130.0, 170.0, 120.0, 140.0];
        let flipped = [150.0, 120.0, 175.0, 125.0, 185.0, 150.0];
        let reference = analyse(&sentence(&tune, 0.16)).unwrap();
        let good = compare(&analyse(&sentence(&tune, 0.17)).unwrap(), &reference, None).unwrap();
        let bad = compare(&analyse(&sentence(&flipped, 0.17)).unwrap(), &reference, None).unwrap();
        assert!(good.contour_corr.unwrap() > bad.contour_corr.unwrap_or(-1.0) + 0.3, "{:?} vs {:?}", good.contour_corr, bad.contour_corr);
    }

    #[test]
    fn external_segments_override_energy_vad() {
        let audio = sentence(&[150.0, 160.0, 140.0, 150.0], 0.18);
        let own = analyse(&audio).unwrap();
        let ext = vec![Segment { start: 0.3, end: 0.66 }, Segment { start: 0.86, end: 1.02 }, Segment { start: 2.0, end: 2.01 }];
        let a = analyse_with_segments(&audio, Some(ext)).unwrap();
        assert_eq!(a.segments.len(), 2);
        assert_eq!(a.rate.pause_count, 1);
        assert_eq!(own.rate.pause_count, 0);
    }

    #[test]
    fn rejects_silence() {
        assert_eq!(analyse(&synth::silence(1.0)), Err(AnalysisError::NoSpeech));
        assert!(matches!(analyse(&synth::silence(0.1)), Err(AnalysisError::TooShort(_))));
    }

    #[test]
    fn comparison_serialises() {
        let tune = [150.0, 170.0, 140.0];
        let r = analyse(&sentence(&tune, 0.18)).unwrap();
        let c = compare(&r, &r, Some(3)).unwrap();
        assert_eq!(c.attempt_rate.syllables, 3);
        assert!((c.articulation_ratio - 1.0).abs() < 1e-6);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("contour_corr"));
    }
}
