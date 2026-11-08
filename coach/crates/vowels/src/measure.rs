//! Find a vowel inside a word and measure its formants.

use analysis::formants::{self, FormantConfig};
use analysis::rate::{self, RateConfig};
use analysis::stats;
use serde::{Deserialize, Serialize};

/// F1–F3 at one point, Hz.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub f1: f32,
    pub f2: f32,
    pub f3: f32,
}

/// A vowel's formants at 20%, 50% and 80% of its duration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VowelMeasure {
    pub start_s: f32,
    pub end_s: f32,
    pub onset: Point,
    pub mid: Point,
    pub offset: Point,
}

/// A phone and its time span, from the phoneme model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneSpan {
    pub phone: String,
    pub start_s: f32,
    pub end_s: f32,
}

/// Per-frame intensity at 100 frames/s; frame 0 is centred at `offset_s`.
#[derive(Debug, Clone, Copy)]
pub struct Intensity<'a> {
    pub db: &'a [f32],
    pub offset_s: f32,
}

impl Intensity<'_> {
    fn frame(&self, t: f32) -> usize {
        (((t - self.offset_s) * analysis::FRAME_RATE).round().max(0.0) as usize).min(self.db.len())
    }
}

const IPA_VOWELS: &str = "aeiouyæɐɑɒɔəɘɛɜɞɤɨɪʉʊʌʏøœɵɯɚɝ";

fn is_vowel_phone(phone: &str) -> bool {
    phone.chars().any(|c| IPA_VOWELS.contains(c))
}

/// Vowel nuclei in a word's phones: consecutive vowel phones merge (a diphthong may be
/// transcribed as two symbols).
pub fn vowel_nuclei(phones: &[PhoneSpan]) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    let mut prev_vowel = false;
    for p in phones {
        let v = is_vowel_phone(&p.phone);
        if v {
            match out.last_mut() {
                Some(last) if prev_vowel => last.1 = p.end_s,
                _ => out.push((p.start_s, p.end_s)),
            }
        }
        prev_vowel = v;
    }
    out
}

fn pick(candidates: &[(f32, f32)], syllables: usize, target: usize) -> Option<(f32, f32)> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == syllables {
        return candidates.get(target).copied();
    }
    if syllables <= 1 {
        // Monosyllable with extra nuclei found: the longest is the vowel.
        return candidates.iter().copied().max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)));
    }
    let k = (target as f32 * (candidates.len() - 1) as f32 / (syllables - 1) as f32).round() as usize;
    candidates.get(k.min(candidates.len() - 1)).copied()
}

/// Locate syllable `target` (of `syllables`) inside a word span. Uses the phoneme model's
/// vowel phones when available, otherwise intensity peaks.
pub fn locate(
    word: (f32, f32),
    phones: &[PhoneSpan],
    syllables: usize,
    target: usize,
    intensity: Intensity<'_>,
) -> Option<(f32, f32)> {
    let from_phones = vowel_nuclei(phones);
    if let Some(span) = pick(&from_phones, syllables, target) {
        return Some(span);
    }
    let (lo, hi) = (intensity.frame(word.0), intensity.frame(word.1));
    if hi <= lo + 3 {
        return None;
    }
    let db = &intensity.db[lo..hi];
    let cfg = RateConfig { below_top_db: 15.0, min_dip_db: 2.0, min_gap_s: 0.06 };
    let mut peaks = rate::nuclei(db, None, &cfg);
    if peaks.is_empty() {
        let max = db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?.0;
        peaks.push(max);
    }
    let peak = if peaks.len() == syllables {
        peaks[target.min(peaks.len() - 1)]
    } else if syllables <= 1 {
        // Extra peaks in a one-syllable word (a loud nasal, an R tail): the vowel is the loudest.
        *peaks.iter().max_by(|a, b| db[**a].total_cmp(&db[**b]))?
    } else {
        let k = (target as f32 * (peaks.len() - 1) as f32 / (syllables - 1) as f32).round() as usize;
        peaks[k.min(peaks.len() - 1)]
    };
    // Grow around the peak while within 7 dB of it.
    let floor = db[peak] - 7.0;
    let mut a = peak;
    while a > 0 && db[a - 1] > floor {
        a -= 1;
    }
    let mut b = peak;
    while b + 1 < db.len() && db[b + 1] > floor {
        b += 1;
    }
    let t = |i: usize| intensity.offset_s + (lo + i) as f32 / analysis::FRAME_RATE;
    Some((t(a), t(b + 1)))
}

fn plausible(f: &formants::Formants) -> bool {
    (150.0..1300.0).contains(&f.f1) && f.f2 > f.f1 + 120.0 && f.f2 < 3400.0 && f.f3 > f.f2 + 80.0
}

fn median_point(frames: &[(f32, formants::Formants)]) -> Option<Point> {
    if frames.len() < 2 {
        return None;
    }
    let col = |g: fn(&formants::Formants) -> f32| stats::median(&frames.iter().map(|(_, f)| g(f)).collect::<Vec<_>>());
    Some(Point { f1: col(|f| f.f1)?, f2: col(|f| f.f2)?, f3: col(|f| f.f3)? })
}

/// Shortest span measured; shorter vowels are widened symmetrically.
const MIN_VOWEL_S: f32 = 0.06;

/// Measure a vowel's formants at onset (10–30%), middle (40–60%) and offset (70–90%).
pub fn measure(samples: &[f32], span: (f32, f32)) -> Option<VowelMeasure> {
    let (mut a, mut b) = span;
    if b - a < MIN_VOWEL_S {
        let c = (a + b) / 2.0;
        a = c - MIN_VOWEL_S / 2.0;
        b = c + MIN_VOWEL_S / 2.0;
    }
    let cfg = FormantConfig::default();
    let frames: Vec<(f32, formants::Formants)> =
        formants::track(samples, a, b, 0.005, &cfg).into_iter().filter(|(_, f)| plausible(f)).collect();
    let dur = b - a;
    let window = |lo: f32, hi: f32| -> Vec<(f32, formants::Formants)> {
        frames.iter().filter(|(t, _)| *t >= a + lo * dur && *t <= a + hi * dur).copied().collect()
    };
    let mid = median_point(&window(0.35, 0.65))?;
    let onset = median_point(&window(0.1, 0.3)).unwrap_or(mid);
    let offset = median_point(&window(0.7, 0.9)).unwrap_or(mid);
    Some(VowelMeasure { start_s: span.0, end_s: span.1, onset, mid, offset })
}

/// Lowest F3 (15th percentile) over the last `fraction` of a span: an R-coloured ending pulls
/// F3 down toward F2.
pub fn f3_low(samples: &[f32], span: (f32, f32), fraction: f32) -> Option<f32> {
    let (a, b) = span;
    let start = b - (b - a) * fraction.clamp(0.1, 1.0);
    let cfg = FormantConfig::default();
    let f3: Vec<f32> =
        formants::track(samples, start, b, 0.005, &cfg).into_iter().filter(|(_, f)| plausible(f)).map(|(_, f)| f.f3).collect();
    if f3.len() < 3 {
        return None;
    }
    stats::percentile(&f3, 15.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    #[test]
    fn nuclei_merge_split_diphthongs() {
        let p = |phone: &str, a: f32, b: f32| PhoneSpan { phone: phone.into(), start_s: a, end_s: b };
        let phones = [p("ɡ", 0.0, 0.05), p("ə", 0.05, 0.1), p("d", 0.1, 0.15), p("e", 0.15, 0.25), p("ɪ", 0.25, 0.35)];
        assert_eq!(vowel_nuclei(&phones), vec![(0.05, 0.1), (0.15, 0.35)]);
        let int = Intensity { db: &[], offset_s: 0.0 };
        assert_eq!(locate((0.0, 0.35), &phones, 2, 1, int), Some((0.15, 0.35)));
    }

    #[test]
    fn locates_by_intensity_without_phones() {
        // Two syllables: quiet then loud; the stressed (second) is found.
        let mut audio = synth::silence(0.1);
        audio.extend(synth::vowel(120.0, [(500.0, 1500.0, 2500.0); 2], 0.15, 0.15));
        audio.extend(synth::silence(0.05));
        audio.extend(synth::vowel(120.0, [(700.0, 1200.0, 2500.0); 2], 0.2, 0.5));
        audio.extend(synth::silence(0.1));
        let (db, offset) = synth::intensity(&audio);
        let span = locate((0.0, 0.6), &[], 2, 1, Intensity { db: &db, offset_s: offset }).unwrap();
        assert!(span.0 > 0.28 && span.0 < 0.36 && span.1 > 0.45, "{span:?}");
    }

    #[test]
    fn measures_a_diphthong_movement() {
        // FACE-like: onset (650, 1750) → offset (420, 2150).
        let audio = synth::vowel(115.0, [(650.0, 1750.0, 2600.0), (420.0, 2150.0, 2750.0)], 0.3, 0.5);
        let m = measure(&audio, (0.0, 0.3)).unwrap();
        assert!((m.onset.f1 - 640.0).abs() < 70.0, "{m:?}");
        assert!((m.offset.f2 - 2120.0).abs() < 120.0, "{m:?}");
        assert!(m.onset.f1 > m.offset.f1 + 120.0 && m.offset.f2 > m.onset.f2 + 200.0, "{m:?}");
    }

    #[test]
    fn f3_dip_shows_an_r() {
        let flat = synth::vowel(115.0, [(750.0, 1250.0, 2500.0); 2], 0.3, 0.5);
        let rhotic = synth::vowel(115.0, [(750.0, 1250.0, 2500.0), (500.0, 1300.0, 1650.0)], 0.3, 0.5);
        let a = f3_low(&flat, (0.0, 0.3), 0.6).unwrap();
        let b = f3_low(&rhotic, (0.0, 0.3), 0.6).unwrap();
        assert!(a > 2350.0 && b < 2100.0, "flat {a}, rhotic {b}");
    }
}
