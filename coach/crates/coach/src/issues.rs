//! Turn an [`analysis::Comparison`] into a ranked list of measured issues.
//! Thresholds sit outside the variation a speaker shows when repeating their own sentence.

use std::collections::BTreeMap;

use analysis::Comparison;

use crate::schema::{feature, CoachInput, IntonationSummary, Issue, RateSummary, SCHEMA_VERSION};

/// Build the coach input for one attempt.
pub fn build_input(sentence: &str, cmp: &Comparison, history: BTreeMap<String, String>) -> CoachInput {
    let mut issues = Vec::new();

    // Pace (articulation rate, pauses excluded).
    let r = cmp.articulation_ratio;
    if r > 0.0 && r < 0.85 {
        issues.push(Issue {
            hint: None,
            word: None,
            feature: feature::PACE.into(),
            severity: ((0.95 - r) / 0.35).clamp(0.2, 1.0),
            detail: format!("{:.0}% slower than the reference", (1.0 - r) * 100.0),
        });
    } else if r > 1.18 {
        issues.push(Issue {
            hint: None,
            word: None,
            feature: feature::PACE.into(),
            severity: ((r - 1.08) / 0.4).clamp(0.2, 1.0),
            detail: format!("{:.0}% faster than the reference", (r - 1.0) * 100.0),
        });
    }

    // Extra pauses.
    let extra = cmp.attempt_rate.pause_count as i64 - cmp.reference_rate.pause_count as i64;
    if extra >= 1 {
        issues.push(Issue {
            hint: None,
            word: None,
            feature: feature::PAUSES.into(),
            severity: (0.3 + 0.2 * extra as f32).min(1.0),
            detail: format!(
                "{} pause{} where the reference has {}",
                cmp.attempt_rate.pause_count,
                if cmp.attempt_rate.pause_count == 1 { "" } else { "s" },
                cmp.reference_rate.pause_count
            ),
        });
    }

    // Overall tune.
    if let Some(corr) = cmp.contour_corr {
        if corr < 0.5 {
            issues.push(Issue {
            hint: None,
                word: None,
                feature: feature::INTONATION.into(),
                severity: ((0.7 - corr) / 0.9).clamp(0.25, 1.0),
                detail: format!("pitch pattern only loosely follows the reference (match {:.0}%)", corr.max(0.0) * 100.0),
            });
        }
    }

    // Final rise or fall.
    if cmp.reference_final_rise && !cmp.final_rise {
        issues.push(Issue {
            hint: None,
            word: None,
            feature: feature::FINAL_RISE.into(),
            severity: 0.6,
            detail: "reference rises at the end; yours stays level or falls".into(),
        });
    } else if cmp.final_rise && !cmp.reference_final_rise {
        issues.push(Issue {
            hint: None,
            word: None,
            feature: feature::FINAL_RISE.into(),
            severity: 0.4,
            detail: "yours rises at the end; the reference falls".into(),
        });
    }

    // Pitch range.
    let rr = cmp.range_ratio;
    if rr > 0.0 && rr < 0.7 {
        issues.push(Issue {
            hint: None,
            word: None,
            feature: feature::PITCH_RANGE.into(),
            severity: ((0.9 - rr) / 0.6).clamp(0.2, 1.0),
            detail: format!("pitch range {:.0}% of the reference's, flatter", rr * 100.0),
        });
    }

    issues.sort_by(|a, b| b.severity.total_cmp(&a.severity));

    CoachInput {
        schema_version: SCHEMA_VERSION,
        sentence: sentence.to_string(),
        words_missed: Vec::new(),
        rate: RateSummary {
            articulation_sps: cmp.attempt_rate.articulation_sps,
            reference_sps: cmp.reference_rate.articulation_sps,
            pause_count: cmp.attempt_rate.pause_count,
            reference_pause_count: cmp.reference_rate.pause_count,
        },
        intonation: IntonationSummary {
            contour_corr: cmp.contour_corr,
            final_rise: cmp.final_rise,
            reference_final_rise: cmp.reference_final_rise,
            range_ratio: cmp.range_ratio,
        },
        issues,
        history,
    }
}

/// A word's pronunciation score from phoneme scoring (0..1, 1 = like the reference).
#[derive(Debug, Clone, PartialEq)]
pub struct WordFinding {
    pub word: String,
    pub score: f32,
}

/// Word scores below this are clearly off; between this and `GOOD_WORD` they are close.
pub const POOR_WORD: f32 = 0.45;
pub const GOOD_WORD: f32 = 0.7;

/// Add word-level findings (M2) to the coach input: skipped words and the words that
/// sounded furthest from the reference.
pub fn add_word_issues(input: &mut CoachInput, words: &[WordFinding], missed: &[String]) {
    input.words_missed = missed.to_vec();
    if !missed.is_empty() {
        let list = missed.iter().map(|w| format!("'{w}'")).collect::<Vec<_>>().join(", ");
        input.issues.push(Issue {
            hint: None,
            word: (missed.len() == 1).then(|| missed[0].clone()),
            feature: feature::MISSED_WORDS.into(),
            severity: (0.55 + 0.15 * missed.len() as f32).min(1.0),
            detail: format!("skipped {list}"),
        });
    }
    let mut ranked: Vec<&WordFinding> = words.iter().filter(|w| !missed.contains(&w.word)).collect();
    ranked.sort_by(|a, b| a.score.total_cmp(&b.score));
    let poor: Vec<&&WordFinding> = ranked.iter().filter(|w| w.score < POOR_WORD).take(2).collect();
    let chosen: Vec<&&WordFinding> = if poor.is_empty() {
        ranked.iter().filter(|w| w.score < GOOD_WORD).take(1).collect()
    } else {
        poor
    };
    for w in chosen {
        let severity = if w.score < POOR_WORD { (1.0 - w.score).clamp(0.4, 1.0) } else { 0.3 };
        input.issues.push(Issue {
            hint: None,
            word: Some(w.word.clone()),
            feature: feature::WORD_SOUND.into(),
            severity,
            detail: format!("'{}' was the least like the reference ({:.0}% match)", w.word, w.score * 100.0),
        });
    }
    input.issues.sort_by(|a, b| b.severity.total_cmp(&a.severity));
}

/// A measured vowel or R difference for one word (M3).
#[derive(Debug, Clone, PartialEq)]
pub struct VowelFinding {
    /// One of `feature::VOWELS` or `feature::NON_RHOTIC`.
    pub feature: String,
    pub word: String,
    pub severity: f32,
    pub detail: String,
    pub hint: String,
}

/// Add vowel findings, keeping only the most severe word per feature.
pub fn add_vowel_issues(input: &mut CoachInput, findings: &[VowelFinding]) {
    let mut worst: Vec<&VowelFinding> = Vec::new();
    for f in findings {
        match worst.iter_mut().find(|w| w.feature == f.feature) {
            Some(w) if f.severity > w.severity => *w = f,
            Some(_) => {}
            None => worst.push(f),
        }
    }
    for f in worst {
        input.issues.push(Issue {
            word: Some(f.word.clone()),
            feature: f.feature.clone(),
            severity: f.severity.clamp(0.0, 1.0),
            detail: f.detail.clone(),
            hint: Some(f.hint.clone()),
        });
    }
    input.issues.sort_by(|a, b| b.severity.total_cmp(&a.severity));
}
