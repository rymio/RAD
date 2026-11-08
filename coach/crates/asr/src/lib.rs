//! Speech recognition for Aussie Accent Coach (milestone M2).
//!
//! - [`words`]: sentence words, transcript alignment, missed-word detection (pure Rust).
//! - [`whisper`] (feature `whisper`): Whisper word timings and Silero VAD via whisper.cpp.
//! - [`remote`]: the same, run by the `rad-coach-asr` helper process (how RAD uses it).

#[cfg(feature = "whisper")]
pub mod whisper;
pub mod remote;
pub mod words;

pub use words::{sentence_words, SentenceWord, WordTiming};

#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("speech recognition model: {0}")]
    Model(String),
    #[error("speech recognition failed: {0}")]
    Run(String),
}
