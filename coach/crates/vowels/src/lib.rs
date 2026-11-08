//! Australian vowel features for Aussie Accent Coach (milestone M3).
//!
//! - [`lexicon`]: CMUdict plus Australian words, used only to know which vowel a word has.
//! - [`sets`]: Wells lexical sets (FACE, PRICE, GOAT…) and droppable R's.
//! - [`measure`]: find a vowel inside a word, measure F1–F3 at onset, middle and offset.
//! - [`extract`]: measured tokens for every content word of a recording.
//! - [`space`]: per-speaker vowel space and Lobanov normalisation over shared categories.
//! - [`score`]: learner-vs-reference comparison with plain-language hints.
//! - [`calibration`]: the passage a learner reads once to map their vowel space.

pub mod calibration;
pub mod extract;
pub mod lexicon;
pub mod measure;
pub mod score;
pub mod sets;
pub mod space;
#[cfg(test)]
mod synth;

pub use extract::{tokens, Token, WordInput};
pub use measure::{Intensity, PhoneSpan, Point, VowelMeasure};
pub use score::{compare, Grade, RhoticComparison, SentenceVowels, VowelComparison};
pub use sets::Set;
pub use space::{clouds, shared_norms, Cloud, Norm, SharedNorms, Shortfall, VowelSpace};

#[cfg(test)]
mod pipeline {
    //! Two synthetic speakers through the whole pipeline. Formant targets are approximate
    //! textbook values (General Australian vs a General American-like learner), used only
    //! to exercise the maths: real thresholds must be tuned on real recordings.
    use super::*;
    use crate::measure::measure;

    type Tri = (f32, f32, f32);

    fn aus(set: Set) -> [Tri; 2] {
        let m = |a: Tri| [a, a];
        match set {
            Set::Fleece => m((290.0, 2250.0, 2900.0)),
            Set::Kit => m((370.0, 2150.0, 2800.0)),
            Set::Dress => m((470.0, 2000.0, 2700.0)),
            Set::Trap => m((720.0, 1650.0, 2600.0)),
            Set::Start => m((800.0, 1350.0, 2500.0)),
            Set::Lot => m((620.0, 1000.0, 2500.0)),
            Set::Strut => m((760.0, 1400.0, 2550.0)),
            Set::Thought => m((440.0, 850.0, 2500.0)),
            Set::Foot => m((400.0, 1150.0, 2350.0)),
            Set::Nurse => m((480.0, 1600.0, 2450.0)),
            Set::Goose => m((330.0, 1700.0, 2300.0)),
            Set::Face => [(650.0, 1750.0, 2550.0), (420.0, 2150.0, 2750.0)],
            Set::Price => [(780.0, 1150.0, 2500.0), (470.0, 1900.0, 2600.0)],
            Set::Goat => [(600.0, 1300.0, 2500.0), (430.0, 1550.0, 2400.0)],
            Set::Mouth => [(720.0, 1700.0, 2550.0), (560.0, 1150.0, 2450.0)],
            _ => m((500.0, 1500.0, 2500.0)),
        }
    }

    fn american(set: Set) -> [Tri; 2] {
        let m = |a: Tri| [a, a];
        match set {
            Set::Fleece => m((280.0, 2300.0, 3000.0)),
            Set::Kit => m((400.0, 2000.0, 2650.0)),
            Set::Dress => m((580.0, 1800.0, 2600.0)),
            Set::Trap => m((700.0, 1750.0, 2550.0)),
            Set::Start => [(780.0, 1250.0, 2400.0), (560.0, 1300.0, 1650.0)], // r-coloured
            Set::Lot => m((760.0, 1150.0, 2550.0)),
            Set::Strut => m((640.0, 1250.0, 2550.0)),
            Set::Thought => m((600.0, 950.0, 2450.0)),
            Set::Foot => m((450.0, 1100.0, 2350.0)),
            Set::Nurse => m((480.0, 1350.0, 1700.0)),
            Set::Goose => m((310.0, 1050.0, 2250.0)),
            Set::Face => [(480.0, 1950.0, 2600.0), (360.0, 2250.0, 2800.0)],
            Set::Price => [(760.0, 1350.0, 2500.0), (450.0, 2000.0, 2600.0)],
            Set::Goat => [(480.0, 1000.0, 2450.0), (380.0, 850.0, 2350.0)],
            Set::Mouth => [(720.0, 1400.0, 2500.0), (460.0, 1050.0, 2400.0)],
            _ => m((500.0, 1500.0, 2500.0)),
        }
    }

    const SETS: [Set; 15] = [
        Set::Fleece, Set::Kit, Set::Dress, Set::Trap, Set::Start, Set::Lot, Set::Strut, Set::Thought, Set::Foot,
        Set::Nurse, Set::Goose, Set::Face, Set::Price, Set::Goat, Set::Mouth,
    ];

    /// One token per set; `scale` mimics a different vocal tract length.
    fn speaker(table: fn(Set) -> [Tri; 2], scale: f32, f0: f32) -> Vec<Token> {
        SETS.iter()
            .enumerate()
            .map(|(i, &set)| {
                let [a, b] = table(set);
                let s = |t: Tri| (t.0 * scale, t.1 * scale, t.2 * scale);
                let audio = synth::vowel(f0, [s(a), s(b)], 0.28, 0.5);
                let measure = measure(&audio, (0.0, 0.28)).unwrap_or_else(|| panic!("{set:?} unmeasurable"));
                let r = set == Set::Start || set == Set::Nurse;
                let f3_r = r.then(|| crate::measure::f3_low(&audio, (0.0, 0.28), 0.6).unwrap());
                Token { word_index: i, word: set.keyword().to_string(), set, measure, r_context: r, f3_r }
            })
            .collect()
    }

    fn verdicts(learner: &[Token], reference: &[Token]) -> SentenceVowels {
        let rs = VowelSpace::new(reference.to_vec());
        let ls = VowelSpace::new(learner.to_vec());
        let norms = shared_norms(&rs, &ls).expect("enough shared vowels");
        compare(reference, learner, &norms.reference, &norms.learner)
    }

    #[test]
    fn an_australian_with_a_different_voice_matches_the_reference() {
        let reference = speaker(aus, 1.0, 115.0);
        let other_aussie = speaker(aus, 1.16, 195.0);
        let v = verdicts(&other_aussie, &reference);
        for c in v.vowels.iter().filter(|c| c.target || c.set.normalises()) {
            assert_eq!(c.grade, Grade::Good, "{:?} distance {:.2}: {}", c.set, c.distance, c.detail);
        }
        assert!(v.rhotic.iter().all(|r| !r.flagged), "{:?}", v.rhotic);
    }

    #[test]
    fn an_american_like_learner_gets_the_right_corrections() {
        let reference = speaker(aus, 1.0, 115.0);
        let learner = speaker(american, 1.12, 180.0);
        let v = verdicts(&learner, &reference);
        let get = |s: Set| v.vowels.iter().find(|c| c.set == s).unwrap();

        let face = get(Set::Face);
        assert_eq!(face.grade, Grade::Poor, "{}", face.detail);
        assert!(face.detail.contains("start") && face.detail.contains("closer"), "{}", face.detail);
        assert!(face.hint.as_deref().unwrap().contains("'eye'"), "{:?}", face.hint);

        let goose = get(Set::Goose);
        assert_ne!(goose.grade, Grade::Good);
        assert!(goose.hint.as_deref().unwrap().contains("further forward"), "{:?}", goose.hint);

        assert_ne!(get(Set::Goat).grade, Grade::Good, "{}", get(Set::Goat).detail);
        assert_ne!(get(Set::Mouth).grade, Grade::Good, "{}", get(Set::Mouth).detail);

        let car = v.rhotic.iter().find(|r| r.word == "car").expect("rhotic check on START");
        assert!(car.flagged, "{car:?}");
        assert!(car.hint.as_deref().unwrap().starts_with("Drop the R in 'car'"));
    }

    #[test]
    fn too_few_shared_vowels_is_reported() {
        let reference = speaker(aus, 1.0, 115.0);
        let few: Vec<Token> = reference.iter().filter(|t| matches!(t.set, Set::Fleece | Set::Trap | Set::Face)).cloned().collect();
        let err = shared_norms(&VowelSpace::new(reference), &VowelSpace::new(few)).unwrap_err();
        assert_eq!(err.shared, vec![Set::Fleece, Set::Trap]);
        assert_eq!(err.missing_corners, vec![space::CORNERS[2].0]);
    }

    #[test]
    fn chart_clouds_have_diphthong_arrows() {
        let reference = speaker(aus, 1.0, 115.0);
        let space = VowelSpace::new(reference.clone());
        let norms = shared_norms(&space, &space).unwrap();
        let c = clouds(&space, &norms.reference);
        let face = c.iter().find(|c| c.set == Set::Face).unwrap();
        let (on, off) = (face.onset.unwrap(), face.offset.unwrap());
        assert!(on[0] > off[0] && off[1] > on[1], "FACE moves closer and forward: {on:?} → {off:?}");
        assert!(c.iter().find(|c| c.set == Set::Fleece).unwrap().onset.is_none());
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"FACE\""));
    }
}
