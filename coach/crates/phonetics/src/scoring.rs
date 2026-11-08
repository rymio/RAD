//! Goodness of pronunciation (GOP) against the reference speaker.
//!
//! No pronunciation dictionary is involved. The phone sequence a learner should
//! produce is read off the *reference recording* by the phoneme model itself, so
//! Australian vowels and the dropped R count as correct by construction. Each
//! learner phone is then scored relative to how confidently the model heard the
//! same phone from the reference speaker.

use serde::{Deserialize, Serialize};

use crate::ctc::{force_align, greedy, LabelSpan, Posteriors};
use crate::vocab::Vocab;

/// A word's time span in a clip, seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordSpan {
    pub word: String,
    pub start_s: f32,
    pub end_s: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreConfig {
    /// Score = exp(slope × min(0, relative GOP)). 0.5 → a 1.4-nat drop halves the score.
    pub slope: f32,
    /// Word score at or above this is green.
    pub good: f32,
    /// Word score at or above this (and below `good`) is amber; below is red.
    pub fair: f32,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        Self { slope: 0.5, good: 0.7, fair: 0.45 }
    }
}

/// Mean over the span's frames of `log P(label) − max log P(any non-blank)`. 0 = the model's top choice.
pub fn gop(p: &Posteriors, span: &LabelSpan, blank: usize) -> f32 {
    let mut total = 0.0f32;
    for t in span.start..span.end {
        let row = p.row(t);
        let best = row.iter().enumerate().filter(|(i, _)| *i != blank).map(|(_, v)| *v).fold(f32::NEG_INFINITY, f32::max);
        total += row[span.label] - best;
    }
    total / (span.end - span.start).max(1) as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    Good,
    Fair,
    Poor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneScore {
    pub phone: String,
    pub start_s: f32,
    pub end_s: f32,
    /// 0..1, 1 = as clear as the reference.
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordScore {
    /// Index into the sentence's words.
    pub index: usize,
    pub word: String,
    /// The word's span in the attempt, seconds.
    pub start_s: f32,
    pub end_s: f32,
    pub score: f32,
    pub grade: Grade,
    pub phones: Vec<PhoneScore>,
}

/// What the reference recording says, phone by phone, ready to score attempts against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferencePhones {
    pub labels: Vec<usize>,
    /// Text of each label.
    pub phones: Vec<String>,
    /// Each phone's span in the reference clip, seconds.
    pub spans_s: Vec<(f32, f32)>,
    /// Word index (into the sentence's words) for each phone; `None` between words.
    pub word_of: Vec<Option<usize>>,
    /// Sentence words, in order.
    pub words: Vec<String>,
    /// The reference speaker's own GOP for each phone.
    pub baseline: Vec<f32>,
}

impl ReferencePhones {
    /// Read the reference's phones from its posteriors and group them into `words`
    /// (spans in the reference clip, aligned to sentence word indices; `None` for a word
    /// the recogniser did not place).
    pub fn build(p: &Posteriors, vocab: &Vocab, words: &[String], word_spans: &[Option<(f32, f32)>]) -> Option<Self> {
        let decoded = greedy(p, vocab.blank, &vocab.ignore);
        if decoded.is_empty() {
            return None;
        }
        let labels: Vec<usize> = decoded.iter().map(|s| s.label).collect();
        // Self-alignment gives the reference's own GOP baseline over the same topology learners get.
        let aligned = force_align(p, &labels, vocab.blank).unwrap_or(decoded);
        let baseline = aligned.iter().map(|s| gop(p, s, vocab.blank)).collect();
        let spans_s: Vec<(f32, f32)> = aligned.iter().map(|s| (p.time(s.start), p.time(s.end))).collect();
        let word_of = assign_to_words(&spans_s, word_spans);
        Some(Self {
            phones: labels.iter().map(|&l| vocab.token(l).to_string()).collect(),
            labels,
            spans_s,
            word_of,
            words: words.to_vec(),
            baseline,
        })
    }

    /// Reference time span of word `index`, from its phones.
    pub fn word_span(&self, index: usize) -> Option<(f32, f32)> {
        let spans: Vec<(f32, f32)> =
            self.word_of.iter().zip(&self.spans_s).filter(|(w, _)| **w == Some(index)).map(|(_, s)| *s).collect();
        Some((spans.first()?.0, spans.last()?.1))
    }

    /// Score a learner attempt by forcing the reference's phones onto it.
    pub fn score(&self, attempt: &Posteriors, vocab: &Vocab, cfg: &ScoreConfig) -> Option<AttemptScore> {
        let spans = force_align(attempt, &self.labels, vocab.blank)?;
        let mut phones = Vec::with_capacity(spans.len());
        for (k, span) in spans.iter().enumerate() {
            let rel = gop(attempt, span, vocab.blank) - self.baseline[k];
            phones.push(PhoneScore {
                phone: self.phones[k].clone(),
                start_s: attempt.time(span.start),
                end_s: attempt.time(span.end),
                score: (cfg.slope * rel.min(0.0)).exp(),
            });
        }
        let overall = phones.iter().map(|p| p.score).sum::<f32>() / phones.len().max(1) as f32;

        let mut words = Vec::new();
        for (index, word) in self.words.iter().enumerate() {
            let members: Vec<&PhoneScore> =
                self.word_of.iter().zip(&phones).filter(|(w, _)| **w == Some(index)).map(|(_, p)| p).collect();
            if members.is_empty() {
                continue;
            }
            let score = members.iter().map(|p| p.score).sum::<f32>() / members.len() as f32;
            words.push(WordScore {
                index,
                word: word.clone(),
                start_s: members[0].start_s,
                end_s: members[members.len() - 1].end_s,
                score,
                grade: grade(score, cfg),
                phones: members.into_iter().cloned().collect(),
            });
        }
        Some(AttemptScore { overall, phones, words })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptScore {
    pub overall: f32,
    pub phones: Vec<PhoneScore>,
    pub words: Vec<WordScore>,
}

pub fn grade(score: f32, cfg: &ScoreConfig) -> Grade {
    if score >= cfg.good {
        Grade::Good
    } else if score >= cfg.fair {
        Grade::Fair
    } else {
        Grade::Poor
    }
}

/// Assign each phone to the word whose span contains its midpoint; a phone that falls in a
/// gap goes to the nearest word if within 80 ms, else to no word.
pub fn assign_to_words(phone_spans: &[(f32, f32)], word_spans: &[Option<(f32, f32)>]) -> Vec<Option<usize>> {
    phone_spans
        .iter()
        .map(|&(a, b)| {
            let mid = (a + b) / 2.0;
            let mut best: Option<(usize, f32)> = None;
            for (i, w) in word_spans.iter().enumerate() {
                let Some((ws, we)) = *w else { continue };
                let dist = if mid < ws { ws - mid } else if mid > we { mid - we } else { 0.0 };
                if best.is_none_or(|(_, d)| dist < d) {
                    best = Some((i, dist));
                }
            }
            best.filter(|(_, d)| *d <= 0.08).map(|(i, _)| i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctc::tests::peaked;

    fn vocab() -> Vocab {
        Vocab::from_json(r#"{"<pad>":0,"<s>":1,"m":2,"eɪ":3,"t":4,"d":5,"aɪ":6}"#).unwrap()
    }

    /// Reference says "m eɪ t" (mate); a learner says "m aɪ t" or the same.
    fn reference() -> (Posteriors, ReferencePhones) {
        let p = peaked(&[0, 2, 2, 0, 3, 3, 3, 0, 4, 0], 7, 6.0);
        let words = vec!["mate".to_string()];
        let r = ReferencePhones::build(&p, &vocab(), &words, &[Some((0.0, 0.2))]).unwrap();
        (p, r)
    }

    #[test]
    fn reference_phones_come_from_the_recording() {
        let (_, r) = reference();
        assert_eq!(r.phones, vec!["m", "eɪ", "t"]);
        assert_eq!(r.word_of, vec![Some(0), Some(0), Some(0)]);
        assert!(r.baseline.iter().all(|g| *g > -0.01), "{:?}", r.baseline);
        let (a, b) = r.word_span(0).unwrap();
        assert!(a < b);
    }

    #[test]
    fn identical_attempt_scores_full_marks() {
        let (p, r) = reference();
        let s = r.score(&p, &vocab(), &ScoreConfig::default()).unwrap();
        assert!(s.overall > 0.99, "{}", s.overall);
        assert_eq!(s.words[0].grade, Grade::Good);
    }

    #[test]
    fn wrong_vowel_is_marked_down_on_that_phone() {
        let (_, r) = reference();
        // Learner: m aɪ t ("mite") at a slower pace.
        let attempt = peaked(&[0, 0, 2, 2, 0, 6, 6, 6, 6, 0, 4, 4, 0], 7, 6.0);
        let s = r.score(&attempt, &vocab(), &ScoreConfig::default()).unwrap();
        assert!(s.phones[0].score > 0.95 && s.phones[2].score > 0.95, "{:?}", s.phones);
        assert!(s.phones[1].score < 0.2, "vowel score {}", s.phones[1].score);
        assert!(s.words[0].grade != Grade::Good);
    }

    #[test]
    fn phones_in_gaps_stay_unassigned() {
        let w = assign_to_words(&[(0.0, 0.1), (0.5, 0.6), (1.0, 1.1)], &[Some((0.0, 0.3)), None, Some((0.95, 1.2))]);
        assert_eq!(w, vec![Some(0), None, Some(2)]);
    }
}
