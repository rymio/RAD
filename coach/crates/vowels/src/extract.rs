//! Turn a recording's words into measured vowel tokens.

use serde::{Deserialize, Serialize};

use crate::measure::{self, Intensity, PhoneSpan, VowelMeasure};
use crate::sets::{is_stopword, word_vowels, Set, WordVowels};

/// A word of the sentence as located in a recording.
#[derive(Debug, Clone, PartialEq)]
pub struct WordInput {
    /// Index into the sentence's words.
    pub index: usize,
    pub text: String,
    /// Where the word is in this recording; `None` if it was not found.
    pub span: Option<(f32, f32)>,
    /// The word's phones from the phoneme model (may be empty).
    pub phones: Vec<PhoneSpan>,
}

/// One measured stressed vowel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub word_index: usize,
    pub word: String,
    pub set: Set,
    pub measure: VowelMeasure,
    /// The word contains an R that Australians drop; such vowels are left out of the F3 baseline.
    pub r_context: bool,
    /// Lowest F3 over the part where a rhotic speaker would say the R (absent when the R
    /// links into a following vowel, which Australians pronounce too).
    pub f3_r: Option<f32>,
}

/// How far past the located vowel an R-coloured ending is searched.
const R_TAIL_S: f32 = 0.10;

/// Measure the primary-stressed vowel of every content word with a known span.
pub fn tokens(samples: &[f32], intensity: Intensity<'_>, words: &[WordInput]) -> Vec<Token> {
    let analysed: Vec<Option<WordVowels>> =
        words.iter().map(|w| if is_stopword(&w.text) { None } else { word_vowels(&w.text) }).collect();
    let mut out = Vec::new();
    for (k, w) in words.iter().enumerate() {
        let (Some(info), Some(span)) = (&analysed[k], w.span) else { continue };
        let Some(primary) = info.primary() else { continue };
        if primary.set == Set::Schwa {
            continue;
        }
        let Some(vowel_span) = measure::locate(span, &w.phones, info.syllables, primary.syllable, intensity) else {
            continue;
        };
        let Some(m) = measure::measure(samples, vowel_span) else { continue };

        let rhotic = info.rhotic_slot();
        let next_starts_with_vowel = words
            .get(k + 1)
            .and_then(|n| word_vowels(&n.text))
            .is_some_and(|n| n.starts_with_vowel);
        let f3_r = rhotic.and_then(|r| {
            let final_r = r.syllable + 1 == info.syllables && info.ends_in_r;
            if final_r && next_starts_with_vowel {
                return None; // linking R: "far away"
            }
            if r.syllable == primary.syllable {
                // The R follows the loudest part of the vowel: look from mid-vowel to a little after it.
                let mid = (vowel_span.0 + vowel_span.1) / 2.0;
                let end = (vowel_span.1 + R_TAIL_S).min(span.1).max(vowel_span.1);
                measure::f3_low(samples, (mid, end), 1.0)
            } else if final_r {
                measure::f3_low(samples, span, 0.45)
            } else {
                let s = measure::locate(span, &w.phones, info.syllables, r.syllable, intensity)?;
                measure::f3_low(samples, s, 0.6)
            }
        });
        out.push(Token {
            word_index: w.index,
            word: w.text.trim_matches(|c: char| !c.is_alphanumeric()).to_string(),
            set: primary.set,
            measure: m,
            r_context: rhotic.is_some(),
            f3_r,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    #[test]
    fn measures_content_words_and_skips_function_words() {
        // "Kate's nice car" as three vowels; "the" is skipped even though it has a span.
        let mut audio = synth::silence(0.1);
        let mut spans = Vec::new();
        for (targets, dur) in [
            ([(650.0, 1750.0, 2550.0), (420.0, 2150.0, 2750.0)], 0.25),
            ([(780.0, 1150.0, 2500.0), (470.0, 1900.0, 2600.0)], 0.25),
            ([(800.0, 1350.0, 2500.0), (800.0, 1350.0, 2500.0)], 0.3),
        ] {
            let start = audio.len() as f32 / 16_000.0;
            audio.extend(synth::vowel(120.0, targets, dur, 0.5));
            spans.push((start - 0.03, start + dur + 0.03));
            audio.extend(synth::silence(0.12));
        }
        let (db, offset) = synth::intensity(&audio);
        let words: Vec<WordInput> = ["Kate's", "nice", "car.", "the"]
            .iter()
            .enumerate()
            .map(|(i, t)| WordInput { index: i, text: t.to_string(), span: spans.get(i).copied().or(Some((0.0, 0.1))), phones: vec![] })
            .collect();
        let toks = tokens(&audio, Intensity { db: &db, offset_s: offset }, &words);
        let sets: Vec<Set> = toks.iter().map(|t| t.set).collect();
        assert_eq!(sets, vec![Set::Face, Set::Price, Set::Start]);
        assert_eq!(toks[2].word, "car");
        assert!(toks[2].r_context && toks[2].f3_r.is_some());
        assert!(toks[0].f3_r.is_none());
        assert!(toks[0].measure.onset.f1 > toks[0].measure.offset.f1 + 100.0, "{:?}", toks[0].measure);
    }

    #[test]
    fn a_quiet_r_after_the_vowel_is_still_heard() {
        // "hard": loud vowel, then a quieter r-coloured tail before the stop.
        let make = |tail_f3: f32| {
            let mut audio = synth::silence(0.1);
            audio.extend(synth::vowel(120.0, [(800.0, 1300.0, 2500.0); 2], 0.3, 0.5));
            audio.extend(synth::vowel(120.0, [(560.0, 1300.0, tail_f3); 2], 0.1, 0.12));
            audio.extend(synth::silence(0.1));
            let (db, offset) = synth::intensity(&audio);
            let words = vec![WordInput { index: 0, text: "hard".into(), span: Some((0.08, 0.52)), phones: vec![] }];
            tokens(&audio, Intensity { db: &db, offset_s: offset }, &words)[0].f3_r.unwrap()
        };
        let rhotic = make(1650.0);
        let plain = make(2500.0);
        assert!(rhotic < 2100.0 && plain > 2300.0, "rhotic {rhotic}, plain {plain}");
    }

    #[test]
    fn linking_r_is_not_scored() {
        let audio = synth::vowel(120.0, [(800.0, 1350.0, 2500.0); 2], 0.3, 0.5);
        let (db, offset) = synth::intensity(&audio);
        let words = vec![
            WordInput { index: 0, text: "far".into(), span: Some((0.0, 0.3)), phones: vec![] },
            WordInput { index: 1, text: "away".into(), span: None, phones: vec![] },
        ];
        let toks = tokens(&audio, Intensity { db: &db, offset_s: offset }, &words);
        assert_eq!(toks.len(), 1);
        assert!(toks[0].r_context && toks[0].f3_r.is_none());
    }
}
