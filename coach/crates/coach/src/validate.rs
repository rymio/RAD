//! The anti-hallucination check: the coach may only talk about what was measured.

use crate::schema::{feature, CoachInput, CoachOutput};

pub const MAX_SPOKEN_WORDS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("spoken text is empty")]
    Empty,
    #[error("spoken text is {0} words (max {MAX_SPOKEN_WORDS})")]
    TooLong(usize),
    #[error("more than two focus items")]
    TooManyFocus,
    #[error("focus on {0} which is not among the measured issues")]
    UnknownFocus(String),
    #[error("mentions {0} although it was not flagged")]
    UnflaggedMention(&'static str),
    #[error("mentions a word not in the sentence: {0}")]
    UnknownWord(String),
}

/// Words that betray the model talking about a feature it was not given.
const FEATURE_CUES: &[(&str, &[&str])] = &[
    (feature::PACE, &["slower", "faster", "too slow", "too fast", "rushed", "speed up", "slow down"]),
    (feature::PAUSES, &["pause", "breath"]),
    (feature::FINAL_RISE, &["rise at the end", "rising at the end", "lift at the end", "end of the sentence"]),
    (feature::PITCH_RANGE, &["flatter", "monotone", "more range"]),
    (feature::MISSED_WORDS, &["skipped", "missed out", "left out", "you missed"]),
    (feature::NON_RHOTIC, &["drop the r", "the r in", "curling your tongue", "curl your tongue"]),
];

pub fn validate(input: &CoachInput, output: &CoachOutput) -> Result<(), Rejection> {
    let words = output.spoken.split_whitespace().count();
    if words == 0 {
        return Err(Rejection::Empty);
    }
    if words > MAX_SPOKEN_WORDS {
        return Err(Rejection::TooLong(words));
    }
    if output.focus.len() > 2 {
        return Err(Rejection::TooManyFocus);
    }
    for f in &output.focus {
        let known = input.issues.iter().any(|i| i.feature == f.feature && (f.word.is_none() || f.word == i.word));
        if !known {
            return Err(Rejection::UnknownFocus(match &f.word {
                Some(w) => format!("{} in '{w}'", f.feature),
                None => f.feature.clone(),
            }));
        }
    }
    let lower = output.spoken.to_lowercase();
    for (feat, cues) in FEATURE_CUES {
        let flagged = input.issues.iter().any(|i| i.feature == *feat);
        // Praise for pace is allowed ("good pace") — only corrective cues count.
        if !flagged && cues.iter().any(|c| lower.contains(c)) {
            return Err(Rejection::UnflaggedMention(feat));
        }
    }
    // Quoted words must come from the sentence.
    let sentence = input.sentence.to_lowercase();
    for quoted in quoted_spans(&output.spoken) {
        let q = quoted.trim().to_lowercase();
        let looks_like_word = !q.is_empty() && q.split_whitespace().count() <= 3;
        if looks_like_word && !sentence.contains(&q) && !is_sound_hint(&q) {
            return Err(Rejection::UnknownWord(quoted));
        }
    }
    Ok(())
}

/// Text inside quotes. An opening quote must follow a space or the start, and a closing
/// quote must precede a space, punctuation or the end, so contractions ("don't") are ignored.
pub fn quoted_spans(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let open = ['\'', '"', '‘', '“'];
    let close = ['\'', '"', '’', '”'];
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts = open.contains(&chars[i]) && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(');
        if starts {
            let mut j = i + 1;
            while j < chars.len() {
                let ends = close.contains(&chars[j])
                    && chars.get(j + 1).is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation());
                if ends {
                    out.push(chars[i + 1..j].iter().collect());
                    break;
                }
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    out
}

/// Sound comparisons like 'dye' or 'deh' are allowed even though they are not in the sentence.
fn is_sound_hint(q: &str) -> bool {
    q.len() <= 5 && !q.contains(' ')
}
