use std::collections::BTreeMap;

use analysis::rate::RateReport;
use analysis::Comparison;

use crate::schema::{feature, CoachOutput, Focus, Next};
use crate::validate::{quoted_spans, validate, Rejection};
use crate::{grounded, issues::build_input, rules, CoachSource};

fn rate(sps: f32, pauses: usize) -> RateReport {
    RateReport {
        syllables: 10,
        phonation_s: 10.0 / sps,
        span_s: 10.0 / sps,
        articulation_sps: sps,
        speaking_sps: sps,
        pause_count: pauses,
        mean_pause_s: 0.3,
    }
}

fn comparison(you_sps: f32, corr: f32, you_rise: bool, ref_rise: bool, range_ratio: f32, you_pauses: usize) -> Comparison {
    Comparison {
        contour_corr: Some(corr),
        range_ratio,
        articulation_ratio: you_sps / 5.0,
        speaking_ratio: you_sps / 5.0,
        attempt_rate: rate(you_sps, you_pauses),
        reference_rate: rate(5.0, 0),
        final_movement_st: Some(if you_rise { 2.5 } else { -1.0 }),
        reference_final_movement_st: Some(if ref_rise { 2.5 } else { -1.0 }),
        final_rise: you_rise,
        reference_final_rise: ref_rise,
        overlay: Vec::new(),
        tempo: Vec::new(),
        alignment_cost: 0.5,
    }
}

const SENTENCE: &str = "G'day mate, how's it going?";

#[test]
fn a_good_attempt_has_no_issues_and_moves_on() {
    let input = build_input(SENTENCE, &comparison(5.1, 0.85, true, true, 0.95, 0), BTreeMap::new());
    assert!(input.issues.is_empty(), "{:?}", input.issues);
    let out = rules::coach(&input);
    assert_eq!(out.next, Next::NextSentence);
    assert!(out.focus.is_empty());
    validate(&input, &out).unwrap();
}

#[test]
fn slow_flat_attempt_flags_pace_and_rise_most_severe_first() {
    let input = build_input(SENTENCE, &comparison(3.6, 0.7, false, true, 0.9, 0), BTreeMap::new());
    let feats: Vec<&str> = input.issues.iter().map(|i| i.feature.as_str()).collect();
    assert_eq!(feats, vec![feature::PACE, feature::FINAL_RISE]);
    assert!(input.issues[0].detail.contains("28% slower"), "{}", input.issues[0].detail);
    let out = rules::coach(&input);
    assert_eq!(out.next, Next::RepeatSentence);
    assert_eq!(out.focus.len(), 2);
    assert!(out.spoken.split_whitespace().count() <= 40, "{}", out.spoken);
    validate(&input, &out).unwrap();
}

#[test]
fn rules_output_always_validates() {
    for &(sps, corr, you, re, rr, p) in &[
        (2.5, 0.1, false, true, 0.4, 3),
        (7.0, 0.3, true, false, 0.5, 0),
        (4.0, 0.45, false, false, 1.0, 1),
        (5.0, -0.2, true, true, 0.65, 2),
    ] {
        let input = build_input(SENTENCE, &comparison(sps, corr, you, re, rr, p), BTreeMap::new());
        let out = rules::coach(&input);
        validate(&input, &out).unwrap_or_else(|e| panic!("{e}: {}", out.spoken));
    }
}

#[test]
fn validator_rejects_invented_issues() {
    let input = build_input(SENTENCE, &comparison(5.0, 0.9, true, true, 1.0, 0), BTreeMap::new());
    let invented = CoachOutput {
        spoken: "You rushed that, slow down a bit.".into(),
        focus: vec![],
        next: Next::RepeatSentence,
    };
    assert_eq!(validate(&input, &invented), Err(Rejection::UnflaggedMention(feature::PACE)));

    let bad_focus = CoachOutput {
        spoken: "Work on your 'day'.".into(),
        focus: vec![Focus { word: Some("day".into()), feature: "FACE".into() }],
        next: Next::DrillWord,
    };
    assert!(matches!(validate(&input, &bad_focus), Err(Rejection::UnknownFocus(_))));

    let long = CoachOutput { spoken: "word ".repeat(45), focus: vec![], next: Next::NextSentence };
    assert_eq!(validate(&input, &long), Err(Rejection::TooLong(45)));
}

#[test]
fn validator_checks_quoted_words_but_allows_sound_hints() {
    let input = build_input(SENTENCE, &comparison(3.5, 0.9, true, true, 1.0, 0), BTreeMap::new());
    let ok = CoachOutput {
        spoken: "You were a bit slower. Say 'mate' crisply, a bit like 'mite'.".into(),
        focus: vec![Focus { word: None, feature: feature::PACE.into() }],
        next: Next::RepeatSentence,
    };
    validate(&input, &ok).unwrap();
    let wrong = CoachOutput {
        spoken: "You were slower, especially on 'wonderful weather'.".into(),
        focus: vec![],
        next: Next::RepeatSentence,
    };
    assert!(matches!(validate(&input, &wrong), Err(Rejection::UnknownWord(_))));
}

#[test]
fn contractions_are_not_quotes() {
    assert!(quoted_spans("Don't rush, you're fine and it's close").is_empty());
    assert_eq!(quoted_spans("Say 'day' like \u{201C}dye\u{201D}."), vec!["day".to_string(), "dye".to_string()]);
}

#[test]
fn grounded_falls_back_to_rules() {
    let input = build_input(SENTENCE, &comparison(5.0, 0.9, true, true, 1.0, 0), BTreeMap::new());
    let reply = grounded(&input, Ok(CoachOutput { spoken: "Too fast!".into(), focus: vec![], next: Next::RepeatSentence }));
    assert_eq!(reply.source, CoachSource::Rules);
    assert!(reply.rejected_reason.is_some());
    let reply = grounded(&input, Ok(CoachOutput { spoken: "Lovely, spot on.".into(), focus: vec![], next: Next::NextSentence }));
    assert_eq!(reply.source, CoachSource::Llm);
}

#[test]
fn word_issues_pick_the_worst_words_and_missed_ones() {
    use crate::issues::{add_word_issues, WordFinding};
    let mut input = build_input(SENTENCE, &comparison(5.0, 0.9, true, true, 1.0, 0), BTreeMap::new());
    let words = [
        WordFinding { word: "G'day".into(), score: 0.9 },
        WordFinding { word: "mate".into(), score: 0.3 },
        WordFinding { word: "how's".into(), score: 0.6 },
        WordFinding { word: "going".into(), score: 0.4 },
    ];
    add_word_issues(&mut input, &words, &["it".to_string()]);
    let feats: Vec<(&str, Option<&str>)> = input.issues.iter().map(|i| (i.feature.as_str(), i.word.as_deref())).collect();
    assert_eq!(feats, vec![(feature::MISSED_WORDS, Some("it")), (feature::WORD_SOUND, Some("mate")), (feature::WORD_SOUND, Some("going"))]);
    assert_eq!(input.words_missed, vec!["it".to_string()]);
    let out = rules::coach(&input);
    validate(&input, &out).unwrap_or_else(|e| panic!("{e}: {}", out.spoken));
    assert!(out.spoken.contains("skipped 'it'"), "{}", out.spoken);
    assert!(out.spoken.contains("'mate'"), "{}", out.spoken);
}

#[test]
fn close_words_only_get_a_gentle_note() {
    use crate::issues::{add_word_issues, WordFinding};
    let mut input = build_input(SENTENCE, &comparison(5.0, 0.9, true, true, 1.0, 0), BTreeMap::new());
    add_word_issues(&mut input, &[WordFinding { word: "mate".into(), score: 0.6 }, WordFinding { word: "it".into(), score: 0.9 }], &[]);
    assert_eq!(input.issues.len(), 1);
    assert!((input.issues[0].severity - 0.3).abs() < 1e-6);
    validate(&input, &rules::coach(&input)).unwrap();
}

#[test]
fn vowel_findings_keep_the_worst_word_per_feature_and_use_the_hint() {
    use crate::issues::{add_vowel_issues, VowelFinding};
    let mut input = build_input(SENTENCE, &comparison(5.0, 0.9, true, true, 1.0, 0), BTreeMap::new());
    let f = |feature: &str, word: &str, severity: f32, hint: &str| VowelFinding {
        feature: feature.into(),
        word: word.into(),
        severity,
        detail: format!("'{word}' differs"),
        hint: hint.into(),
    };
    add_vowel_issues(
        &mut input,
        &[
            f(feature::FACE, "G'day", 0.5, "Start 'G'day' more open."),
            f(feature::FACE, "mate", 0.8, "Start 'mate' with your mouth more open, almost like 'eye'."),
            f(feature::GOAT, "going", 0.6, "Start 'going' from a relaxed 'uh', not 'oh'."),
        ],
    );
    let feats: Vec<(&str, Option<&str>)> = input.issues.iter().map(|i| (i.feature.as_str(), i.word.as_deref())).collect();
    assert_eq!(feats, vec![(feature::FACE, Some("mate")), (feature::GOAT, Some("going"))]);
    let out = rules::coach(&input);
    assert!(out.spoken.contains("almost like 'eye'"), "{}", out.spoken);
    validate(&input, &out).unwrap_or_else(|e| panic!("{e}: {}", out.spoken));
    let json = serde_json::to_string(&input).unwrap();
    assert!(json.contains("\"hint\""));
}

#[test]
fn unflagged_r_advice_is_rejected() {
    let input = build_input(SENTENCE, &comparison(5.0, 0.9, true, true, 1.0, 0), BTreeMap::new());
    let out = CoachOutput { spoken: "Drop the R in 'mate'.".into(), focus: vec![], next: Next::RepeatSentence };
    assert_eq!(validate(&input, &out), Err(Rejection::UnflaggedMention(feature::NON_RHOTIC)));
}
