//! Wells lexical sets: which vowel a word's stressed syllable belongs to, accent-neutrally,
//! and where a rhotic speaker would pronounce an R that Australians drop.

use serde::{Deserialize, Serialize};

use crate::lexicon::{normalise, Lexicon};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Set {
    Fleece,
    Kit,
    Dress,
    Trap,
    Start,
    Lot,
    Strut,
    /// THOUGHT and NORTH, one vowel in Australian English.
    Thought,
    Foot,
    Goose,
    Nurse,
    Face,
    Price,
    Goat,
    Mouth,
    Choice,
    Near,
    Square,
    Cure,
    /// Unstressed schwa (lettER, commA).
    Schwa,
}

impl Set {
    pub const ALL: [Set; 20] = [
        Set::Fleece, Set::Kit, Set::Dress, Set::Trap, Set::Start, Set::Lot, Set::Strut, Set::Thought, Set::Foot,
        Set::Goose, Set::Nurse, Set::Face, Set::Price, Set::Goat, Set::Mouth, Set::Choice, Set::Near, Set::Square,
        Set::Cure, Set::Schwa,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Set::Fleece => "FLEECE",
            Set::Kit => "KIT",
            Set::Dress => "DRESS",
            Set::Trap => "TRAP",
            Set::Start => "START",
            Set::Lot => "LOT",
            Set::Strut => "STRUT",
            Set::Thought => "THOUGHT",
            Set::Foot => "FOOT",
            Set::Goose => "GOOSE",
            Set::Nurse => "NURSE",
            Set::Face => "FACE",
            Set::Price => "PRICE",
            Set::Goat => "GOAT",
            Set::Mouth => "MOUTH",
            Set::Choice => "CHOICE",
            Set::Near => "NEAR",
            Set::Square => "SQUARE",
            Set::Cure => "CURE",
            Set::Schwa => "SCHWA",
        }
    }

    /// A short everyday word with this vowel, for learners who don't know Wells' names.
    pub fn keyword(self) -> &'static str {
        match self {
            Set::Fleece => "see",
            Set::Kit => "sit",
            Set::Dress => "bed",
            Set::Trap => "cat",
            Set::Start => "car",
            Set::Lot => "hot",
            Set::Strut => "cup",
            Set::Thought => "saw",
            Set::Foot => "put",
            Set::Goose => "who",
            Set::Nurse => "bird",
            Set::Face => "day",
            Set::Price => "my",
            Set::Goat => "no",
            Set::Mouth => "now",
            Set::Choice => "boy",
            Set::Near => "here",
            Set::Square => "hair",
            Set::Cure => "tour",
            Set::Schwa => "a",
        }
    }

    /// Measured as a movement (onset → offset) rather than a single point.
    pub fn is_diphthong(self) -> bool {
        matches!(self, Set::Face | Set::Price | Set::Goat | Set::Mouth | Set::Choice | Set::Near | Set::Cure)
    }

    /// Stable monophthongs used to normalise a speaker's vowel space.
    pub fn normalises(self) -> bool {
        matches!(
            self,
            Set::Fleece | Set::Kit | Set::Dress | Set::Trap | Set::Start | Set::Lot | Set::Strut | Set::Thought | Set::Foot | Set::Nurse
        )
    }

    /// The vowels the coach gives named feedback on.
    pub fn is_target(self) -> bool {
        matches!(self, Set::Face | Set::Price | Set::Goat | Set::Mouth | Set::Goose)
    }

    pub fn from_code(code: &str) -> Option<Set> {
        Set::ALL.into_iter().find(|s| s.code() == code)
    }
}

/// A vowel slot in a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub set: Set,
    /// CMUdict stress: 1 primary, 2 secondary, 0 unstressed.
    pub stress: u8,
    /// 0-based syllable index in the word.
    pub syllable: usize,
    /// An R follows that is not followed by a vowel (or the vowel is ER): a rhotic speaker
    /// says it, an Australian does not.
    pub r_after: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordVowels {
    pub syllables: usize,
    pub slots: Vec<Slot>,
    pub starts_with_vowel: bool,
    /// The word ends in R or an r-coloured vowel, so a following vowel may link it.
    pub ends_in_r: bool,
}

impl WordVowels {
    /// The primary-stressed vowel (or the only one).
    pub fn primary(&self) -> Option<&Slot> {
        self.slots
            .iter()
            .find(|s| s.stress == 1)
            .or_else(|| self.slots.iter().find(|s| s.stress == 2))
            .or_else(|| (self.slots.len() == 1).then(|| &self.slots[0]))
    }

    /// The last vowel followed by a non-prevocalic R.
    pub fn rhotic_slot(&self) -> Option<&Slot> {
        self.slots.iter().rev().find(|s| s.r_after)
    }
}

const ARPA_VOWELS: &[&str] = &["AA", "AE", "AH", "AO", "AW", "AY", "EH", "ER", "EY", "IH", "IY", "OW", "OY", "UH", "UW"];

fn split_vowel(phone: &str) -> Option<(&str, u8)> {
    let (base, digit) = phone.split_at(phone.len().saturating_sub(1));
    let stress = digit.parse::<u8>().ok()?;
    ARPA_VOWELS.contains(&base).then_some((base, stress))
}

fn is_vowel(phone: &str) -> bool {
    split_vowel(phone).is_some()
}

/// Lexical sets from ARPAbet phones, with the word's spelling to split CMUdict's merged
/// American categories (AA covers LOT and PALM, AO covers THOUGHT and much of LOT).
pub fn analyse_word(word: &str, phones: &[String]) -> WordVowels {
    let spelling = normalise(word);
    let mut slots = Vec::new();
    let mut syllable = 0;
    for (i, phone) in phones.iter().enumerate() {
        let Some((base, stress)) = split_vowel(phone) else { continue };
        let next = phones.get(i + 1).map(String::as_str);
        let after_next = phones.get(i + 2).map(String::as_str);
        let r_follows = next == Some("R") && !after_next.is_some_and(is_vowel);
        let er_final = base == "ER" && !next.is_some_and(is_vowel);
        let set = if r_follows {
            match base {
                "IY" | "IH" => Set::Near,
                "EH" | "AE" | "EY" => Set::Square,
                "AA" => Set::Start,
                "AO" | "OW" => Set::Thought,
                "UH" | "UW" => Set::Cure,
                "AY" => Set::Price,
                "AW" => Set::Mouth,
                "OY" => Set::Choice,
                "AH" | "ER" => Set::Nurse,
                _ => Set::Schwa,
            }
        } else {
            match base {
                "IY" => Set::Fleece,
                "IH" => Set::Kit,
                "EH" => Set::Dress,
                "AE" => Set::Trap,
                "AA" => aa_set(&spelling),
                "AO" => ao_set(&spelling),
                "AH" if stress > 0 => Set::Strut,
                "AH" => Set::Schwa,
                "UH" => Set::Foot,
                "UW" => Set::Goose,
                "ER" if stress > 0 => Set::Nurse,
                "ER" => Set::Schwa,
                "EY" => Set::Face,
                "AY" => Set::Price,
                "OW" => Set::Goat,
                "AW" => Set::Mouth,
                "OY" => Set::Choice,
                _ => Set::Schwa,
            }
        };
        slots.push(Slot { set, stress, syllable, r_after: r_follows || er_final });
        syllable += 1;
    }
    let last = phones.last().map(String::as_str).unwrap_or("");
    WordVowels {
        syllables: syllable,
        slots,
        starts_with_vowel: phones.first().is_some_and(|p| is_vowel(p)),
        ends_in_r: last == "R" || last.starts_with("ER"),
    }
}

fn aa_set(spelling: &str) -> Set {
    if ["au", "aw", "augh", "ough"].iter().any(|s| spelling.contains(s)) {
        Set::Thought // caught, saw
    } else if spelling.contains("wa") || spelling.contains("qua") {
        Set::Lot // watch, want
    } else if spelling.contains('a') && !spelling.contains('o') {
        Set::Start // father, calm, spa
    } else {
        Set::Lot // hot, got
    }
}

fn ao_set(spelling: &str) -> Set {
    if ["au", "aw", "ough", "augh", "al", "wa"].iter().any(|s| spelling.contains(s)) {
        Set::Thought // thought, saw, talk, water
    } else {
        Set::Lot // dog, long, off
    }
}

/// Look up a word and analyse its vowels. `None` for words the lexicon does not know.
pub fn word_vowels(word: &str) -> Option<WordVowels> {
    Lexicon::global().lookup(word).map(|phones| analyse_word(word, &phones))
}

/// Function words that are usually reduced in connected speech; never scored.
pub const STOPWORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "and", "or", "for", "at", "in", "on", "is", "are", "was", "were", "be", "been", "by",
    "you", "your", "my", "me", "i", "he", "him", "his", "she", "her", "we", "us", "our", "they", "them", "their", "it",
    "its", "that", "this", "as", "from", "with", "but", "so", "do", "does", "did", "can", "could", "would", "should",
    "will", "have", "has", "had", "than", "then", "if", "into", "just", "am", "i'm", "it's", "i'll", "you're",
    "we're", "they're", "it'll", "he's", "she's", "what's", "that's", "there's", "some", "any", "not",
];

pub fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&normalise(word).as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primary(word: &str) -> Set {
        word_vowels(word).unwrap_or_else(|| panic!("{word} not in lexicon")).primary().unwrap().set
    }

    #[test]
    fn target_vowels_from_everyday_words() {
        assert_eq!(primary("mate,"), Set::Face);
        assert_eq!(primary("G'day"), Set::Face);
        assert_eq!(primary("today"), Set::Face);
        assert_eq!(primary("nice"), Set::Price);
        assert_eq!(primary("going?"), Set::Goat);
        assert_eq!(primary("how's"), Set::Mouth);
        assert_eq!(primary("down"), Set::Mouth);
        assert_eq!(primary("knew"), Set::Goose);
        assert_eq!(primary("food"), Set::Goose);
    }

    #[test]
    fn american_merged_vowels_are_split_by_spelling() {
        assert_eq!(primary("dog"), Set::Lot);
        assert_eq!(primary("got"), Set::Lot);
        assert_eq!(primary("watch"), Set::Lot);
        assert_eq!(primary("saw"), Set::Thought);
        assert_eq!(primary("caught"), Set::Thought);
        assert_eq!(primary("water"), Set::Thought);
        assert_eq!(primary("talk"), Set::Thought);
        assert_eq!(primary("father"), Set::Start);
    }

    #[test]
    fn r_coloured_sets_and_rhotic_slots() {
        assert_eq!(primary("car"), Set::Start);
        assert_eq!(primary("four"), Set::Thought);
        assert_eq!(primary("here"), Set::Near);
        assert_eq!(primary("hair"), Set::Square);
        assert_eq!(primary("heard"), Set::Nurse);
        let better = word_vowels("better").unwrap();
        assert_eq!(better.primary().unwrap().set, Set::Dress);
        let r = better.rhotic_slot().unwrap();
        assert_eq!((r.set, r.syllable), (Set::Schwa, 1));
        assert!(better.ends_in_r);
        // R before a vowel is pronounced in Australian English too.
        let carry = word_vowels("carry").unwrap();
        assert_eq!(carry.primary().unwrap().set, Set::Trap);
        assert!(carry.rhotic_slot().is_none());
        assert!(word_vowels("parked").unwrap().rhotic_slot().is_some());
    }

    #[test]
    fn word_shape() {
        let w = word_vowels("away").unwrap();
        assert!(w.starts_with_vowel);
        assert_eq!(w.syllables, 2);
        assert_eq!(w.primary().unwrap().syllable, 1);
        assert!(is_stopword("The"));
        assert!(!is_stopword("mate"));
        assert_eq!(Set::from_code("FACE"), Some(Set::Face));
    }
}
