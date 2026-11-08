//! Compare a learner's vowels with the reference speaker's, word by word, in normalised space.

use serde::{Deserialize, Serialize};

use crate::extract::Token;
use crate::sets::Set;
use crate::space::Norm;

/// Normalised distances: below `GOOD` is close; from `FAIR` up is clearly different.
pub const GOOD: f32 = 0.6;
pub const FAIR: f32 = 1.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    Good,
    Fair,
    Poor,
}

pub fn grade(distance: f32) -> Grade {
    if distance < GOOD {
        Grade::Good
    } else if distance < FAIR {
        Grade::Fair
    } else {
        Grade::Poor
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VowelComparison {
    pub word_index: usize,
    pub word: String,
    pub set: Set,
    pub keyword: &'static str,
    pub target: bool,
    /// Normalised `[z1, z2]` points: onset and offset for diphthongs, the middle otherwise.
    pub reference: Vec<[f32; 2]>,
    pub you: Vec<[f32; 2]>,
    /// Largest distance between corresponding points.
    pub distance: f32,
    pub grade: Grade,
    /// Measured difference in plain words.
    pub detail: String,
    /// What to do about it; `None` when the vowel is close.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RhoticComparison {
    pub word_index: usize,
    pub word: String,
    /// Lowest F3 near the R as a share of the speaker's usual F3 (1 = no R colour).
    pub reference_ratio: f32,
    pub you_ratio: f32,
    pub flagged: bool,
    pub detail: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SentenceVowels {
    pub vowels: Vec<VowelComparison>,
    pub rhotic: Vec<RhoticComparison>,
}

fn points(t: &Token, norm: &Norm) -> Vec<[f32; 2]> {
    if t.set.is_diphthong() {
        vec![norm.z(t.measure.onset), norm.z(t.measure.offset)]
    } else {
        vec![norm.z(t.measure.mid)]
    }
}

const STEP: f32 = 0.35;

/// "closer", "further back": how the learner's vowel differs.
fn difference(dz: [f32; 2]) -> String {
    let mut parts = Vec::new();
    if dz[0] <= -STEP {
        parts.push("closer (tongue higher)");
    } else if dz[0] >= STEP {
        parts.push("more open");
    }
    if dz[1] >= STEP {
        parts.push("further forward");
    } else if dz[1] <= -STEP {
        parts.push("further back");
    }
    if parts.is_empty() {
        "a little different".into()
    } else {
        parts.join(" and ")
    }
}

fn hint(set: Set, word: &str, phase: usize, diphthong: bool, dz: [f32; 2]) -> String {
    let onset = diphthong && phase == 0;
    let offset = diphthong && phase == 1;
    match set {
        Set::Face if onset && dz[0] <= -STEP => format!("Start '{word}' with your mouth more open, almost like 'eye'."),
        Set::Price if onset && dz[1] >= STEP => format!("Start '{word}' further back, from 'ah'."),
        Set::Goat if onset && (dz[1] <= -STEP || dz[0] <= -STEP) => format!("Start '{word}' from a relaxed 'uh', not 'oh'."),
        Set::Goat if offset && dz[1] <= -STEP => {
            format!("Finish '{word}' with your tongue further forward and your lips less rounded.")
        }
        Set::Mouth if onset && dz[1] <= -STEP => format!("Start '{word}' from the vowel in 'cat'."),
        Set::Goose if dz[1] <= -STEP => {
            format!("Say '{word}' with your tongue further forward and your lips only lightly rounded.")
        }
        _ => {
            let action = if dz[0].abs() >= dz[1].abs() {
                if dz[0] < 0.0 { "open your mouth a little more" } else { "close your mouth a little" }
            } else if dz[1] > 0.0 {
                "pull your tongue back a little"
            } else {
                "push your tongue forward a little"
            };
            let where_ = if onset {
                format!("At the start of '{word}'")
            } else if offset {
                format!("At the end of '{word}'")
            } else {
                format!("In '{word}'")
            };
            format!("{where_}, {action}.")
        }
    }
}

/// The learner is flagged when their F3 near the R drops this much further than the reference's.
pub const RHOTIC_MARGIN: f32 = 0.10;

/// Compare one sentence's tokens. Tokens pair up by word index and vowel category.
pub fn compare(reference: &[Token], attempt: &[Token], ref_norm: &Norm, you_norm: &Norm) -> SentenceVowels {
    let mut out = SentenceVowels::default();
    for r in reference {
        let Some(y) = attempt.iter().find(|y| y.word_index == r.word_index && y.set == r.set) else { continue };
        let rp = points(r, ref_norm);
        let yp = points(y, you_norm);
        let dists: Vec<f32> = rp.iter().zip(&yp).map(|(a, b)| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()).collect();
        let (worst, distance) = dists.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((0, 0.0));
        let dz = [yp[worst][0] - rp[worst][0], yp[worst][1] - rp[worst][1]];
        let g = grade(distance);
        let diph = r.set.is_diphthong();
        let phase = if !diph {
            "it is"
        } else if worst == 0 {
            "the start is"
        } else {
            "the end is"
        };
        out.vowels.push(VowelComparison {
            word_index: r.word_index,
            word: r.word.clone(),
            set: r.set,
            keyword: r.set.keyword(),
            target: r.set.is_target(),
            reference: rp,
            you: yp,
            distance,
            grade: g,
            detail: format!(
                "'{}' ({} vowel): {phase} {} than the reference",
                r.word,
                r.set.keyword(),
                difference(dz)
            ),
            hint: (g != Grade::Good).then(|| hint(r.set, &r.word, worst, diph, dz)),
        });

        if let (Some(rf), Some(yf), Some(rb), Some(yb)) = (r.f3_r, y.f3_r, ref_norm.f3_baseline, you_norm.f3_baseline) {
            let reference_ratio = rf / rb;
            let you_ratio = yf / yb;
            let flagged = you_ratio < reference_ratio - RHOTIC_MARGIN && you_ratio < 0.9;
            out.rhotic.push(RhoticComparison {
                word_index: r.word_index,
                word: r.word.clone(),
                reference_ratio,
                you_ratio,
                flagged,
                detail: format!(
                    "'{}': your F3 dips to {:.0}% of its usual level (reference {:.0}%), the sound of a pronounced R",
                    r.word,
                    you_ratio * 100.0,
                    reference_ratio * 100.0
                ),
                hint: flagged.then(|| format!("Drop the R in '{}': finish the vowel without curling your tongue back.", r.word)),
            });
        }
    }
    out
}
