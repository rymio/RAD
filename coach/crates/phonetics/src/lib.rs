//! Pronunciation scoring for Aussie Accent Coach (milestone M2).
//!
//! - [`ctc`]: CTC posteriors, greedy decoding, Viterbi forced alignment.
//! - [`scoring`]: reference-derived phone sequences, goodness of pronunciation, word grades.
//! - [`vocab`]: the phoneme model's token table.
//! - [`model`] (feature `onnx`): wav2vec2 inference through ONNX Runtime.
//!
//! Everything except `model` is pure Rust and tested against synthetic posteriors.

pub mod ctc;
#[cfg(feature = "onnx")]
pub mod model;
pub mod scoring;
pub mod vocab;

pub use ctc::{LabelSpan, Posteriors};
pub use scoring::{AttemptScore, Grade, PhoneScore, ReferencePhones, ScoreConfig, WordScore, WordSpan};
pub use vocab::Vocab;

/// File names the setup script writes into the models folder.
pub const MODEL_FILE: &str = "wav2vec2-phonemes.onnx";
pub const VOCAB_FILE: &str = "wav2vec2-phonemes.vocab.json";

#[derive(Debug, thiserror::Error)]
pub enum PhoneticsError {
    #[error("phoneme model vocabulary: {0}")]
    Vocab(String),
    #[error("phoneme model: {0}")]
    Runtime(String),
    #[error("audio is too short for phoneme analysis")]
    TooShort,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
