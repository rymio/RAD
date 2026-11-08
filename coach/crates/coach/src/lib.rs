//! The coach: measurements in, short human feedback out.
//!
//! The LLM never hears audio. It reads a [`CoachInput`] built from measured
//! [`analysis::Comparison`] data and must answer with a [`CoachOutput`] that
//! passes [`validate::validate`]; otherwise the rule-based coach answers.

pub mod issues;
#[cfg(feature = "llama")]
pub mod llm;
pub mod prompt;
pub mod rules;
pub mod schema;
pub mod validate;

pub use schema::{CoachInput, CoachOutput, CoachReply, CoachSource, Focus, Issue, Next};

/// Anything that can produce coaching for an attempt.
pub trait Coach: Send {
    fn name(&self) -> &str;
    fn reply(&mut self, input: &CoachInput) -> CoachReply;
}

/// The always-available rule-based coach.
#[derive(Debug, Default)]
pub struct RuleCoach;

impl Coach for RuleCoach {
    fn name(&self) -> &str {
        "rules"
    }
    fn reply(&mut self, input: &CoachInput) -> CoachReply {
        CoachReply { output: rules::coach(input), source: CoachSource::Rules, rejected_reason: None }
    }
}

/// Validate an LLM answer, falling back to rules if it fails.
pub fn grounded(input: &CoachInput, candidate: Result<CoachOutput, String>) -> CoachReply {
    match candidate {
        Ok(out) => match validate::validate(input, &out) {
            Ok(()) => CoachReply { output: out, source: CoachSource::Llm, rejected_reason: None },
            Err(why) => {
                tracing::info!(%why, "LLM feedback rejected; using rules");
                CoachReply { output: rules::coach(input), source: CoachSource::Rules, rejected_reason: Some(why.to_string()) }
            }
        },
        Err(why) => CoachReply { output: rules::coach(input), source: CoachSource::Rules, rejected_reason: Some(why) },
    }
}

#[cfg(test)]
mod tests;
