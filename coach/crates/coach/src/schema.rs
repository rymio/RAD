//! The coach's input and output contracts. Versioned and shared by the analysis
//! side (which fills [`CoachInput`]) and the LLM side (which must answer with
//! [`CoachOutput`]).

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Feature codes the coach may talk about. M1 is prosody, M2 adds whole-word sound and
/// skipped words; vowel and
/// consonant features (FACE, PRICE, GOAT, MOUTH, GOOSE, NON_RHOTIC, T_FLAP…)
/// arrive with phoneme alignment in M2–M3.
pub mod feature {
    pub const PACE: &str = "PACE";
    pub const PAUSES: &str = "PAUSES";
    pub const INTONATION: &str = "INTONATION";
    pub const FINAL_RISE: &str = "FINAL_RISE";
    pub const PITCH_RANGE: &str = "PITCH_RANGE";
    /// A word whose phones scored furthest from the reference (M2).
    pub const WORD_SOUND: &str = "WORD_SOUND";
    /// Words the recogniser did not hear (M2).
    pub const MISSED_WORDS: &str = "MISSED_WORDS";
    /// Australian target vowels, by Wells lexical set (M3).
    pub const FACE: &str = "FACE";
    pub const PRICE: &str = "PRICE";
    pub const GOAT: &str = "GOAT";
    pub const MOUTH: &str = "MOUTH";
    pub const GOOSE: &str = "GOOSE";
    /// An R pronounced where Australians drop it ("car", "better") (M3).
    pub const NON_RHOTIC: &str = "NON_RHOTIC";

    pub const VOWELS: [&str; 5] = [FACE, PRICE, GOAT, MOUTH, GOOSE];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachInput {
    pub schema_version: u32,
    pub sentence: String,
    pub words_missed: Vec<String>,
    pub rate: RateSummary,
    pub intonation: IntonationSummary,
    /// Problems found, most severe first. The coach may only mention these.
    pub issues: Vec<Issue>,
    /// One line per feature from session memory, e.g. `"PACE": "improving, 5 sessions"`.
    pub history: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateSummary {
    pub articulation_sps: f32,
    pub reference_sps: f32,
    pub pause_count: usize,
    pub reference_pause_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IntonationSummary {
    pub contour_corr: Option<f32>,
    pub final_rise: bool,
    pub reference_final_rise: bool,
    pub range_ratio: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    /// The word concerned, once word alignment exists (M2). `None` = whole sentence.
    pub word: Option<String>,
    pub feature: String,
    /// 0 (fine) to 1 (far off).
    pub severity: f32,
    /// Measured fact in plain words, e.g. "22% slower than the reference".
    pub detail: String,
    /// A ready-made instruction derived from the measurement (vowel features), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Next {
    RepeatSentence,
    NextSentence,
    DrillWord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Focus {
    #[serde(default)]
    pub word: Option<String>,
    pub feature: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachOutput {
    /// Feedback to show and read aloud; under 40 words.
    pub spoken: String,
    /// At most two issues being worked on.
    pub focus: Vec<Focus>,
    pub next: Next,
}

/// Where the feedback came from, for the UI and the logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoachSource {
    Llm,
    Rules,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachReply {
    pub output: CoachOutput,
    pub source: CoachSource,
    /// Set when the LLM answered but was rejected and rules were used instead.
    pub rejected_reason: Option<String>,
}

/// GBNF grammar constraining llama.cpp to valid [`CoachOutput`] JSON with at most two focus items.
pub const OUTPUT_GRAMMAR: &str = r#"root   ::= "{" ws "\"spoken\":" ws string "," ws "\"focus\":" ws focus "," ws "\"next\":" ws next ws "}"
focus  ::= "[" ws ( item ( "," ws item )? )? ws "]"
item   ::= "{" ws "\"word\":" ws ( string | "null" ) "," ws "\"feature\":" ws string ws "}"
next   ::= "\"repeat_sentence\"" | "\"next_sentence\"" | "\"drill_word\""
string ::= "\"" ( [^"\\\x7F\x00-\x1F] | "\\" ["\\/bfnrt] )* "\""
ws     ::= [ \t\n]?
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_round_trips_and_matches_the_scope_example() {
        let json = r#"{"spoken":"Nice pace on that one.","focus":[{"word":"day","feature":"FACE"}],"next":"repeat_sentence"}"#;
        let out: CoachOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.next, Next::RepeatSentence);
        assert_eq!(out.focus[0].word.as_deref(), Some("day"));
        let back = serde_json::to_string(&out).unwrap();
        assert_eq!(serde_json::from_str::<CoachOutput>(&back).unwrap(), out);
    }

    #[test]
    fn focus_word_may_be_null() {
        let json = r#"{"spoken":"x","focus":[{"word":null,"feature":"PACE"}],"next":"next_sentence"}"#;
        let out: CoachOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.focus[0].word, None);
    }
}
