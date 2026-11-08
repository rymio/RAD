//! wav2vec2 phoneme CTC model through ONNX Runtime (feature `onnx`).
//!
//! ONNX Runtime is loaded at run time, so the app builds without downloading binaries.
//! `scripts/setup_phoneme_model.py` exports the model and copies a matching
//! `libonnxruntime.dylib` into the models folder.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::session::Session;
use ort::value::Tensor;

use crate::ctc::Posteriors;
use crate::vocab::Vocab;
use crate::PhoneticsError;

/// wav2vec2 emits one frame per 320 samples at 16 kHz.
pub const FRAME_S: f32 = 0.02;

static ENVIRONMENT: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

/// Places to look for the ONNX Runtime library, first match wins.
pub fn runtime_candidates(models_dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
        v.push(PathBuf::from(p));
    }
    for name in ["libonnxruntime.dylib", "libonnxruntime.so"] {
        v.push(models_dir.join(name));
    }
    v.push(PathBuf::from("/opt/homebrew/lib/libonnxruntime.dylib"));
    v.push(PathBuf::from("/usr/local/lib/libonnxruntime.dylib"));
    v
}

fn environment(models_dir: &Path) -> Result<&'static ort::environment::Environment, PhoneticsError> {
    let result = ENVIRONMENT.get_or_init(|| {
        let lib = runtime_candidates(models_dir)
            .into_iter()
            .find(|p| p.exists())
            .ok_or_else(|| "ONNX Runtime not found: run coach/scripts/setup_phoneme_model.py (in the RAD folder) or `brew install onnxruntime`".to_string())?;
        tracing::info!(lib = %lib.display(), "loading ONNX Runtime");
        let builder = ort::init_from(&lib).map_err(|e| format!("could not load {}: {e}", lib.display()))?;
        builder.with_name("aussie-accent-coach").build().map_err(|e| e.to_string())
    });
    result.as_ref().map_err(|e| PhoneticsError::Runtime(e.clone()))
}

pub struct PhonemeModel {
    session: Session,
    pub vocab: Vocab,
    input_name: String,
}

impl PhonemeModel {
    /// Load `model.onnx` and its `vocab.json`.
    pub fn load(model: &Path, vocab: &Path, models_dir: &Path) -> Result<Self, PhoneticsError> {
        let env = environment(models_dir)?;
        let vocab = Vocab::load(vocab)?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
        let session = Session::builder(env)
            .map_err(|e| PhoneticsError::Runtime(e.to_string()))?
            .with_intra_threads(threads)
            .map_err(|e| PhoneticsError::Runtime(e.to_string()))?
            .commit_from_file(model)
            .map_err(|e| PhoneticsError::Runtime(format!("could not open {}: {e}", model.display())))?;
        let input_name = session.inputs().first().map(|i| i.name().to_string()).unwrap_or_else(|| "input_values".into());
        Ok(Self { session, vocab, input_name })
    }

    /// Phone posteriors for 16 kHz mono audio.
    pub fn posteriors(&mut self, samples: &[f32]) -> Result<Posteriors, PhoneticsError> {
        if samples.len() < 1600 {
            return Err(PhoneticsError::TooShort);
        }
        // wav2vec2 feature extractor: zero mean, unit variance.
        let n = samples.len() as f32;
        let mean = samples.iter().sum::<f32>() / n;
        let var = samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n;
        let inv = 1.0 / (var + 1e-7).sqrt();
        let input: Vec<f32> = samples.iter().map(|s| (s - mean) * inv).collect();

        let tensor = Tensor::from_array((vec![1i64, samples.len() as i64], input))
            .map_err(|e| PhoneticsError::Runtime(e.to_string()))?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(|e| PhoneticsError::Runtime(e.to_string()))?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>().map_err(|e| PhoneticsError::Runtime(e.to_string()))?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        let [_, frames, vocab] = dims[..] else {
            return Err(PhoneticsError::Runtime(format!("unexpected logits shape {dims:?}")));
        };
        if vocab as usize != self.vocab.len() {
            return Err(PhoneticsError::Vocab(format!(
                "model outputs {vocab} classes but vocab.json has {}",
                self.vocab.len()
            )));
        }
        Ok(Posteriors::from_logits(logits, frames as usize, vocab as usize, FRAME_S))
    }
}
