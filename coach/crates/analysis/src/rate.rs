//! Speaking-rate measurement by syllable-nucleus detection, after
//! de Jong & Wempe (2009): a syllable nucleus is a voiced intensity peak
//! that stands out from its neighbours by a minimum dip.

use serde::{Deserialize, Serialize};

use crate::{energy, stats, vad::Segment, FRAME_RATE};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateConfig {
    /// Peaks must be at least this loud relative to the 99th-percentile intensity.
    pub below_top_db: f32,
    /// Required intensity dip between two neighbouring nuclei.
    pub min_dip_db: f32,
    /// Minimum spacing between nuclei.
    pub min_gap_s: f32,
}

impl Default for RateConfig {
    fn default() -> Self {
        Self { below_top_db: 25.0, min_dip_db: 2.0, min_gap_s: 0.07 }
    }
}

/// Frame indices of syllable nuclei.
///
/// `db` is intensity per frame; `voiced[i]` says whether frame `i` had pitch.
/// Pass `None` for `voiced` to skip the voicing check.
pub fn nuclei(db: &[f32], voiced: Option<&[bool]>, cfg: &RateConfig) -> Vec<usize> {
    if db.len() < 3 {
        return Vec::new();
    }
    let smoothed = energy::smooth(db, 5);
    let top = stats::percentile(&smoothed, 99.0).unwrap_or(0.0);
    let threshold = top - cfg.below_top_db;

    // Candidate local maxima above threshold.
    let mut peaks: Vec<usize> = (1..smoothed.len() - 1)
        .filter(|&i| {
            smoothed[i] > threshold && smoothed[i] >= smoothed[i - 1] && smoothed[i] > smoothed[i + 1]
        })
        .collect();

    // Merge peaks that are not separated by a real dip, keeping the louder one.
    let min_gap = (cfg.min_gap_s * FRAME_RATE).round() as usize;
    let mut changed = true;
    while changed && peaks.len() > 1 {
        changed = false;
        let mut kept: Vec<usize> = Vec::with_capacity(peaks.len());
        for &p in &peaks {
            if let Some(&prev) = kept.last() {
                let dip = smoothed[prev..=p].iter().copied().fold(f32::INFINITY, f32::min);
                let separated = smoothed[prev] - dip >= cfg.min_dip_db
                    && smoothed[p] - dip >= cfg.min_dip_db
                    && p - prev >= min_gap;
                if !separated {
                    if smoothed[p] > smoothed[prev] {
                        *kept.last_mut().unwrap() = p;
                    }
                    changed = true;
                    continue;
                }
            }
            kept.push(p);
        }
        peaks = kept;
    }

    if let Some(v) = voiced {
        // A nucleus needs voicing at or right next to the peak.
        peaks.retain(|&p| {
            let lo = p.saturating_sub(2);
            let hi = (p + 3).min(v.len());
            lo < hi && v[lo..hi].iter().any(|&x| x)
        });
    }
    peaks
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateReport {
    pub syllables: usize,
    /// Time spent speaking (sum of VAD segments), seconds.
    pub phonation_s: f32,
    /// First speech onset to last speech offset, seconds.
    pub span_s: f32,
    /// Syllables per second of phonation (pauses excluded).
    pub articulation_sps: f32,
    /// Syllables per second over the whole span (pauses included).
    pub speaking_sps: f32,
    pub pause_count: usize,
    pub mean_pause_s: f32,
}

pub fn report(nuclei_count: usize, segments: &[Segment]) -> RateReport {
    let phonation: f32 = segments.iter().map(Segment::duration).sum();
    let span = match (segments.first(), segments.last()) {
        (Some(a), Some(b)) => b.end - a.start,
        _ => 0.0,
    };
    let pauses = crate::vad::pauses(segments);
    RateReport {
        syllables: nuclei_count,
        phonation_s: phonation,
        span_s: span,
        articulation_sps: if phonation > 0.0 { nuclei_count as f32 / phonation } else { 0.0 },
        speaking_sps: if span > 0.0 { nuclei_count as f32 / span } else { 0.0 },
        pause_count: pauses.len(),
        mean_pause_s: stats::mean(&pauses).unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pitch, synth};

    fn count(audio: &[f32]) -> usize {
        let frames = pitch::track(audio, &pitch::PitchConfig::default());
        let db: Vec<f32> = frames.iter().map(|f| energy::rms_to_db(f.rms)).collect();
        let voiced: Vec<bool> = frames.iter().map(|f| f.f0.is_some()).collect();
        nuclei(&db, Some(&voiced), &RateConfig::default()).len()
    }

    #[test]
    fn counts_separated_syllables() {
        for n in [3, 5, 8] {
            assert_eq!(count(&synth::syllables(n, 150.0, 0.16, 0.08)), n);
        }
    }

    #[test]
    fn counts_syllables_without_silent_gaps() {
        // Hann-enveloped bursts back to back: dips but no silence, like connected speech.
        assert_eq!(count(&synth::syllables(6, 130.0, 0.18, 0.0)), 6);
    }

    #[test]
    fn unvoiced_noise_has_no_nuclei() {
        let mut audio = synth::noise(1.0, 0.3, 11);
        audio.extend(synth::silence(0.2));
        assert_eq!(count(&audio), 0);
    }

    #[test]
    fn report_rates() {
        let segs = [Segment { start: 0.0, end: 1.0 }, Segment { start: 1.5, end: 2.5 }];
        let r = report(10, &segs);
        assert!((r.articulation_sps - 5.0).abs() < 1e-5);
        assert!((r.speaking_sps - 4.0).abs() < 1e-5);
        assert_eq!(r.pause_count, 1);
        assert!((r.mean_pause_s - 0.5).abs() < 1e-5);
    }
}
