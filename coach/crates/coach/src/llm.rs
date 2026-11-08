//! Local LLM coach on llama.cpp (feature `llama`; add `metal` on Apple Silicon).
//!
//! Output is constrained by [`OUTPUT_GRAMMAR`] so the model can only emit valid
//! [`CoachOutput`] JSON, then checked by the validator through [`crate::grounded`].

use std::num::NonZeroU32;
use std::path::Path;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use crate::schema::{CoachInput, CoachOutput, CoachReply, OUTPUT_GRAMMAR};
use crate::{grounded, prompt, Coach};

/// Generation budget. The JSON answer is about 60–90 tokens.
const MAX_NEW_TOKENS: usize = 200;
const CONTEXT: u32 = 4096;

pub struct LlamaCoach {
    /// llama.cpp's backend is one per process; inside RAD it is rad-local's.
    backend: &'static LlamaBackend,
    model: LlamaModel,
    name: String,
    seed: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("could not load the coach model: {0}")]
pub struct LoadError(String);

impl LlamaCoach {
    /// Load a GGUF instruct model. On Metal builds every layer goes to the GPU.
    ///
    /// `backend` is the process's llama.cpp backend: it can be started only once per process,
    /// and freeing it would pull it from under anyone else using llama.cpp (RAD's own models).
    pub fn load(backend: &'static LlamaBackend, path: &Path) -> Result<Self, LoadError> {
        let params = LlamaModelParams::default().with_n_gpu_layers(1000);
        let model = LlamaModel::load_from_file(backend, path, &params).map_err(|e| LoadError(e.to_string()))?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("llm").to_string();
        Ok(Self { backend, model, name, seed: 1234 })
    }

    fn generate(&mut self, input: &CoachInput) -> Result<CoachOutput, String> {
        let template = self.model.chat_template(None).map_err(|e| format!("no chat template: {e}"))?;
        let messages = vec![
            LlamaChatMessage::new("system".into(), prompt::SYSTEM_PROMPT.into()).map_err(|e| e.to_string())?,
            LlamaChatMessage::new("user".into(), prompt::user_message(input)).map_err(|e| e.to_string())?,
        ];
        let text = self.model.apply_chat_template(&template, &messages, true).map_err(|e| e.to_string())?;

        let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4);
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(CONTEXT))
            .with_n_batch(CONTEXT)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = self.model.new_context(self.backend, ctx_params).map_err(|e| e.to_string())?;

        let vocab = self.model.vocab();
        let tokens = vocab.tokenize(text.as_bytes(), false, true);
        if tokens.len() + MAX_NEW_TOKENS > CONTEXT as usize {
            return Err(format!("prompt too long ({} tokens)", tokens.len()));
        }
        let mut batch = LlamaBatch::new(CONTEXT as usize, 1);
        let last = tokens.len() as i32 - 1;
        for (i, t) in tokens.iter().enumerate() {
            batch.add(*t, i as i32, &[0], i as i32 == last).map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch).map_err(|e| e.to_string())?;

        self.seed = self.seed.wrapping_add(1);
        let grammar = LlamaSampler::grammar(&self.model, OUTPUT_GRAMMAR, "root").map_err(|e| e.to_string())?;
        let mut sampler = LlamaSampler::chain_simple([
            grammar,
            LlamaSampler::top_k(40),
            LlamaSampler::temp(0.4),
            LlamaSampler::dist(self.seed),
        ]);

        let mut out_bytes: Vec<u8> = Vec::new();
        let mut pos = tokens.len() as i32;
        for _ in 0..MAX_NEW_TOKENS {
            // `sample` already accepts the token into every sampler (llama_sampler_sample does);
            // accepting it again feeds the grammar each token twice and llama.cpp aborts.
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if vocab.is_eog(token) {
                break;
            }
            out_bytes.extend(vocab.token_to_piece(token, false, None));
            // The grammar closes the object with '}'; stop as soon as the JSON parses.
            if out_bytes.ends_with(b"}") {
                if let Ok(parsed) = serde_json::from_slice::<CoachOutput>(&out_bytes) {
                    return Ok(parsed);
                }
            }
            batch.clear();
            batch.add(token, pos, &[0], true).map_err(|e| e.to_string())?;
            pos += 1;
            ctx.decode(&mut batch).map_err(|e| e.to_string())?;
        }
        serde_json::from_slice(&out_bytes)
            .map_err(|e| format!("model output was not valid JSON ({e}): {}", String::from_utf8_lossy(&out_bytes)))
    }
}

impl Coach for LlamaCoach {
    fn name(&self) -> &str {
        &self.name
    }

    fn reply(&mut self, input: &CoachInput) -> CoachReply {
        let started = std::time::Instant::now();
        let candidate = self.generate(input);
        tracing::info!(elapsed_ms = started.elapsed().as_millis() as u64, ok = candidate.is_ok(), "LLM coach");
        grounded(input, candidate)
    }
}
