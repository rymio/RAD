//! whisper.cpp through whisper-rs: word timings and Silero voice activity detection.

use std::path::Path;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext, WhisperVadContextParams,
    WhisperVadParams,
};

use crate::words::{merge_pieces, WordTiming};
use crate::AsrError;

fn threads() -> i32 {
    std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).min(8)
}

pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    pub fn load(model: &Path) -> Result<Self, AsrError> {
        let path = model.to_str().ok_or_else(|| AsrError::Model("model path is not UTF-8".into()))?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| AsrError::Model(format!("could not load {}: {e}", model.display())))?;
        Ok(Self { ctx })
    }

    /// Words with timings for 16 kHz mono audio.
    ///
    /// `prompt` biases recognition toward known text. Use it for the reference (we know what
    /// it says), never for a learner attempt, where it would hide skipped words.
    pub fn words(&self, samples: &[f32], prompt: Option<&str>) -> Result<Vec<WordTiming>, AsrError> {
        let mut state = self.ctx.create_state().map_err(|e| AsrError::Run(e.to_string()))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads());
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_temperature(0.0);
        // One segment per word, with token-level timestamps.
        params.set_token_timestamps(true);
        params.set_split_on_word(true);
        params.set_max_len(1);
        if let Some(p) = prompt {
            params.set_initial_prompt(p);
        }

        // whisper.cpp is unreliable on clips under a second; pad the end with silence.
        let mut audio = samples.to_vec();
        if audio.len() < 24_000 {
            audio.resize(24_000, 0.0);
        }
        state.full(params, &audio).map_err(|e| AsrError::Run(e.to_string()))?;

        let duration = samples.len() as f32 / 16_000.0;
        let mut pieces = Vec::new();
        for segment in state.as_iter() {
            let text = segment.to_str_lossy().map_err(|e| AsrError::Run(e.to_string()))?.to_string();
            let start = segment.start_timestamp() as f32 / 100.0;
            let end = segment.end_timestamp() as f32 / 100.0;
            let n = segment.n_tokens();
            let probability = if n > 0 {
                (0..n).filter_map(|i| segment.get_token(i)).map(|t| t.token_probability()).sum::<f32>() / n as f32
            } else {
                0.0
            };
            if start < duration {
                pieces.push((text, start, end.min(duration), probability));
            }
        }
        Ok(merge_pieces(pieces))
    }
}

/// Silero VAD (ggml) as shipped with whisper.cpp.
pub struct SileroVad {
    ctx: WhisperVadContext,
}

impl SileroVad {
    pub fn load(model: &Path) -> Result<Self, AsrError> {
        let path = model.to_str().ok_or_else(|| AsrError::Model("model path is not UTF-8".into()))?;
        let mut params = WhisperVadContextParams::default();
        params.set_n_threads(1);
        params.set_use_gpu(false);
        let ctx = WhisperVadContext::new(path, params).map_err(|e| AsrError::Model(format!("VAD model: {e}")))?;
        Ok(Self { ctx })
    }

    /// Speech segments `(start_s, end_s)` in 16 kHz mono audio.
    pub fn segments(&mut self, samples: &[f32]) -> Result<Vec<(f32, f32)>, AsrError> {
        let segs =
            self.ctx.segments_from_samples(WhisperVadParams::new(), samples).map_err(|e| AsrError::Run(e.to_string()))?;
        Ok(segs.into_iter().map(|s| (s.start / 100.0, s.end / 100.0)).collect())
    }
}
