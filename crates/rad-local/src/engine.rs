//! llama.cpp worker threads, shared by the whole process. llama.cpp's backend can only be
//! initialised once per process, and RAD may run several server instances in one process
//! (standalone mode switching vaults), so every instance uses these threads.
//!
//! Embeddings and chat have a thread each, so indexing never stalls an answer. Embeddings run
//! on the CPU (small model, same everywhere); chat uses the GPU where llama.cpp has one (Metal
//! on macOS). Idle models are unloaded after a few minutes to give the memory back.

use std::num::NonZeroU32;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use tokio::sync::{mpsc as tmpsc, oneshot};

use crate::{fit, LocalError, ModelSpec, Result};

const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// Embedding sequences decoded together in one batch.
const MAX_SEQS: usize = 32;
/// Upper bound for an embedding context, whatever the model was trained with.
const MAX_EMBED_CTX: u32 = 8192;
/// Prompt tokens decoded per step.
const PREFILL_BATCH: usize = 512;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// system | user | assistant
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ChatOptions {
    pub max_tokens: u32,
    /// `None` or ≤ 0 = deterministic (greedy).
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone)]
pub enum ChatEvent {
    Text(String),
    Done { prompt_tokens: usize, completion_tokens: usize },
}

enum Job {
    Embed { path: PathBuf, spec: &'static ModelSpec, texts: Vec<String>, reply: oneshot::Sender<Result<Vec<Vec<f32>>>> },
    Chat { path: PathBuf, spec: &'static ModelSpec, messages: Vec<ChatMessage>, opts: ChatOptions, tx: tmpsc::Sender<Result<ChatEvent>> },
    Unload(mpsc::Sender<()>),
}

type Slot = Option<(PathBuf, LlamaModel)>;

fn backend() -> std::result::Result<&'static LlamaBackend, String> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            // Metal "residency sets" make llama.cpp abort at process exit if a model is still
            // loaded (quitting the app mid-answer). Without them it only loses a little speed.
            #[cfg(target_os = "macos")]
            if std::env::var_os("GGML_METAL_NO_RESIDENCY").is_none() {
                std::env::set_var("GGML_METAL_NO_RESIDENCY", "1");
            }
            llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
            LlamaBackend::init().map_err(|e| format!("cannot start llama.cpp: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

static EMBED_TX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
static CHAT_TX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

fn embed_worker() -> &'static mpsc::Sender<Job> {
    EMBED_TX.get_or_init(|| spawn("rad-llama-embed"))
}

fn chat_worker() -> &'static mpsc::Sender<Job> {
    CHAT_TX.get_or_init(|| spawn("rad-llama-chat"))
}

fn spawn(name: &str) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name(name.into()).spawn(move || run(rx)).expect("spawn llama.cpp worker");
    tx
}

fn stopped() -> LocalError {
    LocalError::Engine("the built-in engine has stopped".into())
}

pub async fn embed(path: PathBuf, spec: &'static ModelSpec, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let (reply, rx) = oneshot::channel();
    embed_worker().send(Job::Embed { path, spec, texts, reply }).map_err(|_| stopped())?;
    rx.await.map_err(|_| stopped())?
}

pub fn chat(path: PathBuf, spec: &'static ModelSpec, messages: Vec<ChatMessage>, opts: ChatOptions) -> Result<tmpsc::Receiver<Result<ChatEvent>>> {
    let (tx, rx) = tmpsc::channel(256);
    chat_worker().send(Job::Chat { path, spec, messages, opts, tx }).map_err(|_| stopped())?;
    Ok(rx)
}

/// Unloads every model (waiting up to a few seconds for running work). Call before exiting.
pub fn shutdown() {
    let mut acks = Vec::new();
    for w in [EMBED_TX.get(), CHAT_TX.get()].into_iter().flatten() {
        let (tx, rx) = mpsc::channel();
        if w.send(Job::Unload(tx)).is_ok() {
            acks.push(rx);
        }
    }
    for rx in acks {
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
}

fn run(rx: mpsc::Receiver<Job>) {
    let mut slot: Slot = None;
    loop {
        let job = match rx.recv_timeout(IDLE_UNLOAD) {
            Ok(j) => j,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some((p, _)) = slot.take() {
                    tracing::info!("unloaded idle built-in model {}", p.display());
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let backend = match backend() {
            Ok(b) => b,
            Err(msg) => {
                match job {
                    Job::Embed { reply, .. } => drop(reply.send(Err(LocalError::Engine(msg)))),
                    Job::Chat { tx, .. } => drop(tx.blocking_send(Err(LocalError::Engine(msg)))),
                    Job::Unload(ack) => drop(ack.send(())),
                }
                continue;
            }
        };
        match job {
            Job::Embed { path, spec, texts, reply } => {
                let res = guarded(&mut slot, |slot| embed_now(backend, slot, &path, spec, &texts));
                let _ = reply.send(res);
            }
            Job::Chat { path, spec, messages, opts, tx } => {
                let res = guarded(&mut slot, |slot| chat_now(backend, slot, &path, spec, messages, opts, &tx));
                if let Err(e) = res {
                    let _ = tx.blocking_send(Err(e));
                }
            }
            Job::Unload(ack) => {
                slot = None;
                let _ = ack.send(());
            }
        }
    }
}

/// Turns a panic inside llama.cpp work into an error (and drops the possibly broken model).
fn guarded<T>(slot: &mut Slot, f: impl FnOnce(&mut Slot) -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(AssertUnwindSafe(|| f(&mut *slot))).unwrap_or_else(|p| {
        *slot = None;
        let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
        Err(LocalError::Engine(msg.unwrap_or_else(|| "panic".into())))
    })
}

fn engine_err(e: impl std::fmt::Display) -> LocalError {
    LocalError::Engine(e.to_string())
}

fn load<'s>(backend: &LlamaBackend, slot: &'s mut Slot, path: &Path, spec: &ModelSpec, gpu: bool) -> Result<&'s LlamaModel> {
    if slot.as_ref().is_none_or(|(p, _)| p != path) {
        *slot = None;
        fit::check_memory(spec)?;
        // GPU layers only matter where llama.cpp was built with a GPU backend (Metal on macOS).
        let params = LlamaModelParams::default().with_n_gpu_layers(if gpu { 999 } else { 0 });
        let model = LlamaModel::load_from_file(backend, path, &params).map_err(engine_err)?;
        tracing::info!("loaded built-in model {} ({} params)", spec.name, model.n_params());
        *slot = Some((path.to_path_buf(), model));
    }
    Ok(&slot.as_ref().expect("model loaded above").1)
}

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| n.get()).min(8) as i32
}

// ---------------------------------------------------------------- embeddings

fn embed_now(backend: &LlamaBackend, slot: &mut Slot, path: &Path, spec: &ModelSpec, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let model = load(backend, slot, path, spec, false)?;

    // Embedding models attend over the whole input at once, so every sequence in a batch must
    // fit in a single micro-batch: context = batch = ubatch, shared by all sequences.
    let n_ctx = model.n_ctx_train().clamp(512, MAX_EMBED_CTX);
    let params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(n_ctx))
        .with_n_batch(n_ctx)
        .with_n_ubatch(n_ctx)
        .with_n_seq_max(MAX_SEQS as u32)
        .with_kv_unified(true)
        .with_embeddings(true)
        .with_offload_kqv(false)
        .with_op_offload(false)
        .with_n_threads(threads())
        .with_n_threads_batch(threads());
    let mut ctx = model.new_context(backend, params).map_err(engine_err)?;

    let vocab = model.vocab();
    let max = n_ctx as usize;
    let tokens: Vec<Vec<LlamaToken>> = texts
        .iter()
        .map(|t| {
            let mut v = vocab.tokenize(t.as_bytes(), true, false);
            // Too long for the model: keep the start and the closing special token.
            if v.len() > max {
                let last = v[v.len() - 1];
                v.truncate(max - 1);
                v.push(last);
            }
            v
        })
        .collect();

    let mut out = Vec::with_capacity(texts.len());
    let mut batch = LlamaBatch::new(max, MAX_SEQS as i32);
    let mut i = 0;
    while i < tokens.len() {
        batch.clear();
        let (mut seqs, mut used) = (0usize, 0usize);
        while i + seqs < tokens.len() && seqs < MAX_SEQS && used + tokens[i + seqs].len() <= max {
            batch.add_sequence(&tokens[i + seqs], seqs as i32, true).map_err(engine_err)?;
            used += tokens[i + seqs].len();
            seqs += 1;
        }
        ctx.clear_kv_cache();
        ctx.decode(&mut batch).map_err(engine_err)?;
        for s in 0..seqs {
            let e = ctx.embeddings_seq_ith(s as i32).map_err(engine_err)?;
            out.push(normalize(e));
        }
        i += seqs;
    }
    Ok(out)
}

fn normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

// ---------------------------------------------------------------- chat

fn chat_now(
    backend: &LlamaBackend,
    slot: &mut Slot,
    path: &Path,
    spec: &ModelSpec,
    messages: Vec<ChatMessage>,
    opts: ChatOptions,
    tx: &tmpsc::Sender<Result<ChatEvent>>,
) -> Result<()> {
    let model = load(backend, slot, path, spec, true)?;
    let n_ctx = spec.ctx.min(model.n_ctx_train().max(2048));
    // Keep room for the answer; the prompt gets the rest.
    let reserve = opts.max_tokens.clamp(256, n_ctx / 3) as usize;
    let prompt = fit_prompt(model, messages, n_ctx as usize - reserve)?;

    let params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(n_ctx))
        .with_n_batch(PREFILL_BATCH as u32)
        .with_n_ubatch(PREFILL_BATCH as u32)
        .with_n_threads(threads())
        .with_n_threads_batch(threads());
    let mut ctx = model.new_context(backend, params).map_err(engine_err)?;

    // Read the prompt.
    let mut batch = LlamaBatch::new(PREFILL_BATCH, 1);
    let last = prompt.len() - 1;
    for (c, chunk) in prompt.chunks(PREFILL_BATCH).enumerate() {
        batch.clear();
        for (j, t) in chunk.iter().enumerate() {
            let pos = c * PREFILL_BATCH + j;
            batch.add(*t, pos as i32, &[0], pos == last).map_err(engine_err)?;
        }
        ctx.decode(&mut batch).map_err(engine_err)?;
        if tx.is_closed() {
            return Ok(());
        }
    }

    // Write the answer.
    let temperature = opts.temperature.unwrap_or(0.0);
    let mut sampler = if temperature <= 0.0 {
        LlamaSampler::greedy()
    } else {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        LlamaSampler::chain_simple([
            LlamaSampler::top_k(40),
            LlamaSampler::top_p(0.95, 1),
            LlamaSampler::min_p(0.05, 1),
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(seed),
        ])
    };
    let vocab = model.vocab();
    let max_out = (opts.max_tokens as usize).min(n_ctx as usize - prompt.len());
    let mut n_past = prompt.len();
    let mut idx = batch.n_tokens() - 1;
    let mut pending: Vec<u8> = Vec::new();
    let mut produced = 0;
    while produced < max_out {
        let token = sampler.sample(&ctx, idx);
        if vocab.is_eog(token) {
            break;
        }
        produced += 1;
        pending.extend(vocab.token_to_piece(token, false, None));
        // Tokens can end mid-character; send only complete UTF-8.
        let valid = match std::str::from_utf8(&pending) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid > 0 {
            let text = String::from_utf8(pending.drain(..valid).collect()).expect("validated above");
            if tx.blocking_send(Ok(ChatEvent::Text(text))).is_err() {
                return Ok(()); // the listener went away (chat cancelled)
            }
        }
        batch.clear();
        batch.add(token, n_past as i32, &[0], true).map_err(engine_err)?;
        n_past += 1;
        ctx.decode(&mut batch).map_err(engine_err)?;
        idx = 0;
    }
    if !pending.is_empty() {
        let _ = tx.blocking_send(Ok(ChatEvent::Text(String::from_utf8_lossy(&pending).into_owned())));
    }
    let _ = tx.blocking_send(Ok(ChatEvent::Done { prompt_tokens: prompt.len(), completion_tokens: produced }));
    Ok(())
}

/// Renders the conversation with the model's chat template and makes it fit `budget` tokens:
/// older turns are dropped first, then the longest message (usually the retrieved passages)
/// is shortened.
fn fit_prompt(model: &LlamaModel, mut messages: Vec<ChatMessage>, budget: usize) -> Result<Vec<LlamaToken>> {
    let template = model.chat_template(None).ok();
    // Each round drops one old turn or shortens one message.
    for _ in 0..messages.len() + 10 {
        let text = render(model, template.as_ref(), &messages)?;
        let tokens = model.vocab().tokenize(text.as_bytes(), true, true);
        if tokens.len() <= budget {
            return Ok(tokens);
        }
        let turns: Vec<usize> = (0..messages.len()).filter(|&i| messages[i].role != "system").collect();
        if turns.len() > 1 {
            messages.remove(turns[0]);
            continue;
        }
        let (longest, _) = messages.iter().enumerate().max_by_key(|(_, m)| m.content.len()).expect("at least one message");
        let keep = (messages[longest].content.len() as f64 * budget as f64 / tokens.len() as f64 * 0.9) as usize;
        let mut cut = keep.min(messages[longest].content.len());
        while !messages[longest].content.is_char_boundary(cut) {
            cut -= 1;
        }
        messages[longest].content.truncate(cut);
        messages[longest].content.push_str("\n…[shortened to fit the built-in model]");
    }
    Err(LocalError::Engine("the conversation doesn't fit the built-in model's context".into()))
}

fn render(model: &LlamaModel, template: Option<&LlamaChatTemplate>, messages: &[ChatMessage]) -> Result<String> {
    if let Some(t) = template {
        let msgs = messages
            .iter()
            .map(|m| LlamaChatMessage::new(m.role.clone(), m.content.replace('\0', "")))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(engine_err)?;
        if let Ok(s) = model.apply_chat_template(t, &msgs, true) {
            return Ok(s);
        }
    }
    // No (supported) template in the file: ChatML is the most widely understood format.
    let mut s = String::new();
    for m in messages {
        s.push_str(&format!("<|im_start|>{}\n{}<|im_end|>\n", m.role, m.content));
    }
    s.push_str("<|im_start|>assistant\n");
    Ok(s)
}
