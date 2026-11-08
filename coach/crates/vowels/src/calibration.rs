//! The calibration passage: short sentences of one-syllable words that between them cover
//! every vowel needed to normalise a speaker, plus the Australian target vowels.

use serde::Serialize;

use crate::sets::Set;
use crate::space::{VowelSpace, CORNERS};

pub const PASSAGE: &[&str] = &[
    "Pete hid ten bags.",
    "Dad got cups of tea.",
    "Mark parked the car at the farm.",
    "Paul saw four good books.",
    "The nurse heard a bird.",
    "Kids fed the dog a bun.",
    "Kate's nice boat is loud.",
    "Sue knew the blue moon.",
];

/// Normalising categories a calibration should contain before it is used.
pub const READY_SETS: usize = 7;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Coverage {
    /// `(category, keyword, tokens)` for every category measured.
    pub have: Vec<(Set, &'static str, usize)>,
    /// Normalising categories still missing.
    pub missing: Vec<(Set, &'static str)>,
    pub ready: bool,
}

pub fn coverage(space: &VowelSpace) -> Coverage {
    let have: Vec<(Set, &'static str, usize)> =
        space.sets().into_iter().map(|s| (s, s.keyword(), space.count(s))).collect();
    let missing: Vec<(Set, &'static str)> =
        Set::ALL.into_iter().filter(|s| s.normalises() && space.count(*s) == 0).map(|s| (s, s.keyword())).collect();
    let normalising = have.iter().filter(|(s, _, _)| s.normalises()).count();
    let corners = CORNERS.iter().all(|(_, sets)| sets.iter().any(|s| space.count(*s) > 0));
    Coverage { have, missing, ready: normalising >= READY_SETS && corners }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sets::{is_stopword, word_vowels};

    #[test]
    fn passage_covers_every_normalising_vowel_and_the_targets() {
        let mut counts = std::collections::BTreeMap::new();
        for sentence in PASSAGE {
            for w in sentence.split_whitespace().filter(|w| !is_stopword(w)) {
                let v = word_vowels(w).unwrap_or_else(|| panic!("{w} missing from lexicon"));
                *counts.entry(v.primary().unwrap().set).or_insert(0) += 1;
            }
        }
        for s in Set::ALL.into_iter().filter(|s| s.normalises() || s.is_target()) {
            assert!(counts.get(&s).copied().unwrap_or(0) >= 1, "{s:?} not covered: {counts:?}");
        }
        assert!(counts[&Set::Goose] >= 3 && counts[&Set::Nurse] >= 3 && counts[&Set::Start] >= 3);
    }
}
