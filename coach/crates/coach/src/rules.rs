//! Rule-based coach. Used when no LLM is loaded, and whenever the LLM's answer fails validation.

use crate::schema::{feature, CoachInput, CoachOutput, Focus, Issue, Next};

fn advice(issue: &Issue) -> String {
    if let Some(hint) = &issue.hint {
        return hint.clone();
    }
    match issue.feature.as_str() {
        feature::PACE if issue.detail.contains("slower") => format!(
            "You were {}. Keep unstressed syllables short and run the words together.",
            issue.detail
        ),
        feature::PACE => format!("You were {}. Let the stressed words take their time.", issue.detail),
        feature::PAUSES => format!("You had {}. Try the whole sentence on one breath.", issue.detail),
        feature::INTONATION => "Your tune wandered from the reference. Hum the melody first, then say it.".into(),
        feature::FINAL_RISE if issue.detail.starts_with("reference rises") => {
            "The reference lifts at the end; yours drops. Let your voice rise on the last word.".into()
        }
        feature::FINAL_RISE => "Yours rises at the end but the reference settles down. Let the last word fall.".into(),
        feature::PITCH_RANGE => "Your pitch was flatter than the reference. Exaggerate the ups and downs.".into(),
        feature::WORD_SOUND => match &issue.word {
            Some(w) => format!("Your '{w}' sounded least like the reference. Click it to hear both, then copy the vowel."),
            None => "One word sounded off. Click the red word to compare.".into(),
        },
        feature::MISSED_WORDS => format!("You {}. Read every word, even the small ones.", issue.detail),
        _ => format!("Watch the {}: {}.", issue.feature.to_lowercase(), issue.detail),
    }
}

fn praise(input: &CoachInput) -> Option<&'static str> {
    if input.issues.iter().any(|i| i.feature == feature::MISSED_WORDS) {
        return None;
    }
    let pace_ok = !input.issues.iter().any(|i| i.feature == feature::PACE);
    let tune_ok = !input.issues.iter().any(|i| i.feature == feature::INTONATION);
    match (pace_ok, tune_ok) {
        (true, true) => Some("Pace and tune are close to the reference."),
        (true, false) => Some("Good pace."),
        (false, true) => Some("Nice tune."),
        (false, false) => None,
    }
}

pub fn coach(input: &CoachInput) -> CoachOutput {
    let top: Vec<&Issue> = input.issues.iter().filter(|i| i.severity >= 0.25).take(2).collect();
    if top.is_empty() {
        return CoachOutput {
            spoken: format!("{} Well done — go again to lock it in, or move on.", praise(input).unwrap_or("That's close.")),
            focus: Vec::new(),
            next: Next::NextSentence,
        };
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = praise(input) {
        parts.push(p.to_string());
    }
    parts.push(advice(top[0]));
    let mut mentioned = vec![top[0]];
    // A second point only if it is serious and the message stays short.
    if let Some(second) = top.get(1) {
        let candidate = advice(second);
        let words = parts.iter().chain(std::iter::once(&candidate)).map(|s| s.split_whitespace().count()).sum::<usize>();
        if second.severity >= 0.4 && words <= 40 {
            parts.push(candidate);
            mentioned.push(second);
        }
    }
    let spoken = parts.join(" ");
    CoachOutput {
        spoken,
        focus: mentioned.iter().map(|i| Focus { word: i.word.clone(), feature: i.feature.clone() }).collect(),
        next: Next::RepeatSentence,
    }
}
