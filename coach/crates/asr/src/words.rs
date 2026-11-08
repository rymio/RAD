//! Sentence words and alignment of a recogniser transcript to them.

use serde::{Deserialize, Serialize};

/// A word of the practice sentence as shown on screen, and its matching key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentenceWord {
    /// As written, with punctuation ("mate,").
    pub display: String,
    /// Lower-case letters and digits only ("mate"), numbers spelled out.
    pub key: String,
}

/// A recognised word with its time span, seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordTiming {
    pub text: String,
    pub start_s: f32,
    pub end_s: f32,
    /// Recogniser confidence 0..1.
    pub probability: f32,
}

const NUMBERS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
    "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty",
];

/// Matching key for a word: lower case, letters and digits only, small numbers as words.
pub fn key(word: &str) -> String {
    let k: String = word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    match k.parse::<usize>() {
        Ok(n) if n < NUMBERS.len() => NUMBERS[n].to_string(),
        _ => k,
    }
}

/// Split a sentence into words for display and matching.
pub fn sentence_words(sentence: &str) -> Vec<SentenceWord> {
    sentence
        .split_whitespace()
        .filter_map(|w| {
            let k = key(w);
            (!k.is_empty()).then(|| SentenceWord { display: w.to_string(), key: k })
        })
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 0 for a match, 0.5 for a near match (a recogniser spelling or a small slip), 1 otherwise.
fn word_cost(expected: &str, heard: &str) -> f32 {
    if expected == heard {
        return 0.0;
    }
    let d = edit_distance(expected, heard) as f32;
    let len = expected.chars().count().max(heard.chars().count()) as f32;
    if d / len <= 0.34 {
        0.5
    } else {
        1.0
    }
}

/// For each expected word, the index of the heard word it matched (exactly or nearly), if any.
pub fn align(expected: &[String], heard: &[String]) -> Vec<Option<usize>> {
    let (n, m) = (expected.len(), heard.len());
    let mut d = vec![vec![0.0f32; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i as f32;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j as f32;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = d[i - 1][j - 1] + word_cost(&expected[i - 1], &heard[j - 1]);
            d[i][j] = sub.min(d[i - 1][j] + 1.0).min(d[i][j - 1] + 1.0);
        }
    }
    let mut out = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let c = word_cost(&expected[i - 1], &heard[j - 1]);
        if (d[i][j] - (d[i - 1][j - 1] + c)).abs() < 1e-6 {
            if c < 1.0 {
                out[i - 1] = Some(j - 1);
            }
            i -= 1;
            j -= 1;
        } else if (d[i][j] - (d[i - 1][j] + 1.0)).abs() < 1e-6 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    out
}

/// Time span of each sentence word, from recognised word timings.
pub fn word_spans(sentence: &[SentenceWord], heard: &[WordTiming]) -> Vec<Option<(f32, f32)>> {
    let expected: Vec<String> = sentence.iter().map(|w| w.key.clone()).collect();
    let heard_keys: Vec<String> = heard.iter().map(|w| key(&w.text)).collect();
    align(&expected, &heard_keys).into_iter().map(|m| m.map(|j| (heard[j].start_s, heard[j].end_s))).collect()
}

/// Sentence words the recogniser did not hear.
pub fn missed(sentence: &[SentenceWord], heard: &[WordTiming]) -> Vec<usize> {
    let expected: Vec<String> = sentence.iter().map(|w| w.key.clone()).collect();
    let heard_keys: Vec<String> = heard.iter().map(|w| key(&w.text)).collect();
    align(&expected, &heard_keys).iter().enumerate().filter(|(_, m)| m.is_none()).map(|(i, _)| i).collect()
}

/// Split a transcript of a whole page, read in one go, into its sentences.
///
/// Aligns every page word against the transcript at once, then returns for each sentence the
/// range of heard words (first..=last matched) that belong to it, or `None` when none of its
/// words were heard (a skipped sentence).
pub fn split_page(sentences: &[Vec<SentenceWord>], heard: &[WordTiming]) -> Vec<Option<(usize, usize)>> {
    let expected: Vec<String> = sentences.iter().flatten().map(|w| w.key.clone()).collect();
    let heard_keys: Vec<String> = heard.iter().map(|w| key(&w.text)).collect();
    let matches = align(&expected, &heard_keys);
    let mut out = Vec::with_capacity(sentences.len());
    let mut at = 0;
    for sentence in sentences {
        let mine = &matches[at..at + sentence.len()];
        at += sentence.len();
        let first = mine.iter().flatten().min().copied();
        let last = mine.iter().flatten().max().copied();
        out.push(first.zip(last));
    }
    out
}

/// Recognisers sometimes split a word into pieces ("G", "'day"); join a piece that starts
/// without a space onto the previous word.
pub fn merge_pieces(pieces: Vec<(String, f32, f32, f32)>) -> Vec<WordTiming> {
    let mut out: Vec<WordTiming> = Vec::new();
    for (text, start, end, p) in pieces {
        let starts_word = text.starts_with(' ') || out.is_empty();
        let clean = text.trim().to_string();
        if clean.is_empty() {
            continue;
        }
        if clean.chars().all(|c| !c.is_alphanumeric()) {
            continue; // punctuation-only token
        }
        match out.last_mut() {
            Some(last) if !starts_word => {
                last.text.push_str(&clean);
                last.end_s = end;
                last.probability = last.probability.min(p);
            }
            _ => out.push(WordTiming { text: clean, start_s: start, end_s: end, probability: p }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| key(w)).collect()
    }

    #[test]
    fn keys_strip_punctuation_and_spell_numbers() {
        assert_eq!(key("G'day"), "gday");
        assert_eq!(key("mate,"), "mate");
        assert_eq!(key("4"), "four");
        assert_eq!(sentence_words("No worries, I'll be there by 4.").len(), 7);
    }

    fn timed(words: &[&str]) -> Vec<WordTiming> {
        words
            .iter()
            .enumerate()
            .map(|(i, w)| WordTiming { text: w.to_string(), start_s: i as f32 * 0.4, end_s: i as f32 * 0.4 + 0.3, probability: 0.9 })
            .collect()
    }

    #[test]
    fn splits_a_page_into_sentences() {
        let page = [sentence_words("G'day mate."), sentence_words("How's it going?"), sentence_words("See you later.")];
        // The second sentence is skipped and an "um" is inserted.
        let heard = timed(&["G'day", "mate", "um", "see", "you", "later"]);
        assert_eq!(split_page(&page, &heard), vec![Some((0, 1)), None, Some((3, 5))]);
    }

    #[test]
    fn aligns_with_a_skipped_word() {
        let expected = keys(&["G'day", "mate,", "how's", "it", "going?"]);
        let heard = keys(&["G'day", "how's", "it", "going"]);
        assert_eq!(align(&expected, &heard), vec![Some(0), None, Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn near_matches_count_as_heard() {
        let expected = keys(&["We're", "heading", "to", "the", "beach"]);
        let heard = keys(&["Were", "headin", "to", "a", "beach"]);
        let a = align(&expected, &heard);
        assert_eq!(a[0], Some(0));
        assert_eq!(a[1], Some(1));
        assert_eq!(a[3], None); // "the" vs "a" is a substitution, not a match
        assert_eq!(a[4], Some(4));
    }

    #[test]
    fn spans_and_misses() {
        let sentence = sentence_words("Do you want to swim?");
        let heard = vec![
            WordTiming { text: "Do".into(), start_s: 0.1, end_s: 0.2, probability: 0.9 },
            WordTiming { text: "want".into(), start_s: 0.3, end_s: 0.5, probability: 0.9 },
            WordTiming { text: "to".into(), start_s: 0.5, end_s: 0.6, probability: 0.9 },
            WordTiming { text: "swim".into(), start_s: 0.6, end_s: 1.0, probability: 0.9 },
        ];
        assert_eq!(missed(&sentence, &heard), vec![1]);
        let spans = word_spans(&sentence, &heard);
        assert_eq!(spans[1], None);
        assert_eq!(spans[4], Some((0.6, 1.0)));
    }

    #[test]
    fn merges_word_pieces_and_drops_punctuation() {
        let pieces = vec![
            (" G".to_string(), 0.0, 0.1, 0.8),
            ("'day".to_string(), 0.1, 0.3, 0.9),
            (",".to_string(), 0.3, 0.3, 0.9),
            (" mate".to_string(), 0.3, 0.6, 0.95),
        ];
        let w = merge_pieces(pieces);
        assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), vec!["G'day", "mate"]);
        assert_eq!((w[0].start_s, w[0].end_s), (0.0, 0.3));
        assert!((w[0].probability - 0.8).abs() < 1e-6);
    }
}
