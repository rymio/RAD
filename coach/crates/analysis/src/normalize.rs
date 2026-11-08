//! Lobanov vowel normalisation: z-score each formant against the speaker's own
//! distribution, measured once during calibration. After this, a learner and the
//! reference speaker can be plotted on one vowel chart and compared fairly.

use serde::{Deserialize, Serialize};

use crate::{formants::Formants, stats};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeakerVowelSpace {
    pub f1_mean: f32,
    pub f1_sd: f32,
    pub f2_mean: f32,
    pub f2_sd: f32,
    pub samples: usize,
}

impl SpeakerVowelSpace {
    /// Build from formant measurements spread across the speaker's vowel space.
    /// Needs at least a handful of different vowels to be meaningful.
    pub fn from_measurements(measurements: &[Formants]) -> Option<Self> {
        if measurements.len() < 5 {
            return None;
        }
        let f1: Vec<f32> = measurements.iter().map(|m| m.f1).collect();
        let f2: Vec<f32> = measurements.iter().map(|m| m.f2).collect();
        Some(Self {
            f1_mean: stats::mean(&f1)?,
            f1_sd: stats::std_dev(&f1)?.max(1.0),
            f2_mean: stats::mean(&f2)?,
            f2_sd: stats::std_dev(&f2)?.max(1.0),
            samples: measurements.len(),
        })
    }

    /// Lobanov z-scores `(z1, z2)`.
    pub fn normalize(&self, f: &Formants) -> (f32, f32) {
        ((f.f1 - self.f1_mean) / self.f1_sd, (f.f2 - self.f2_mean) / self.f2_sd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(f1: f32, f2: f32) -> Formants {
        Formants { f1, f2, f3: 2600.0, bandwidths: [80.0, 90.0, 120.0] }
    }

    #[test]
    fn scaled_speakers_normalise_identically() {
        // A speaker with a shorter vocal tract has every formant ~18% higher.
        let base = [f(300.0, 2300.0), f(450.0, 1900.0), f(700.0, 1300.0), f(600.0, 900.0), f(350.0, 800.0)];
        let scaled: Vec<Formants> = base.iter().map(|m| f(m.f1 * 1.18, m.f2 * 1.18)).collect();
        let a = SpeakerVowelSpace::from_measurements(&base).unwrap();
        let b = SpeakerVowelSpace::from_measurements(&scaled).unwrap();
        for (x, y) in base.iter().zip(&scaled) {
            let (ax, ay) = a.normalize(x);
            let (bx, by) = b.normalize(y);
            assert!((ax - bx).abs() < 1e-3 && (ay - by).abs() < 1e-3);
        }
    }

    #[test]
    fn needs_enough_samples() {
        assert!(SpeakerVowelSpace::from_measurements(&[f(300.0, 2000.0)]).is_none());
    }
}
