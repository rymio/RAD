//! RAD's built-in model engine: llama.cpp compiled into the process, plus a small library of
//! GGUF models kept as plain files in a local `models` folder (never inside the vault —
//! llama.cpp memory-maps them, and they are public weights).
//!
//! The embedding model ships with RAD so semantic search always works. Light chat models from
//! a curated list can be downloaded into the same folder to answer offline as well.

mod engine;
mod fit;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub use engine::{shutdown, ChatEvent, ChatMessage, ChatOptions};
pub use fit::{memory_needed, total_memory, FitLevel};

#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    #[error("unknown built-in model “{0}”")]
    UnknownModel(String),
    #[error("{0} isn't downloaded yet — get it under Settings → Models → Built-in models")]
    NotInstalled(String),
    #[error("downloading {model}: {message}")]
    Download { model: String, message: String },
    #[error("{0}")]
    DoesNotFit(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("llama.cpp: {0}")]
    Engine(String),
}

impl LocalError {
    /// Worth retrying later (network trouble, memory temporarily short).
    pub fn is_transient(&self) -> bool {
        matches!(self, LocalError::Download { .. } | LocalError::DoesNotFit(_))
    }
}

pub type Result<T> = std::result::Result<T, LocalError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Embedding,
    Chat,
}

impl ModelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelKind::Embedding => "embedding",
            ModelKind::Chat => "chat",
        }
    }
}

/// A model RAD knows how to fetch, verify and run.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    /// Stable id stored in settings (`ModelRef.model`).
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    pub kind: ModelKind,
    pub family: &'static str,
    pub parameters: &'static str,
    pub license: &'static str,
    /// Context window RAD allocates when running the model (tokens).
    pub ctx: u32,
    /// KV-cache bytes per token of context (layers × kv-heads × head-dim × K+V × f16).
    pub kv_bytes_per_token: u64,
    pub recommended: bool,
}

/// nomic-embed-text v1.5, 8-bit: 768-dim vectors, 2048-token context, ~146 MB. Ships with RAD.
pub const NOMIC_EMBED: ModelSpec = ModelSpec {
    id: "nomic-embed-text-v1.5-q8",
    name: "nomic-embed-text v1.5",
    description: "Embeddings for semantic search. Ships with RAD.",
    file: "nomic-embed-text-v1.5.Q8_0.gguf",
    url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q8_0.gguf",
    sha256: "3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7",
    bytes: 146_146_432,
    kind: ModelKind::Embedding,
    family: "nomic-bert",
    parameters: "137M",
    license: "Apache-2.0",
    ctx: 2048,
    kv_bytes_per_token: 0,
    recommended: true,
};

/// Light chat models that answer well from retrieved passages (RAG) on an ordinary laptop.
pub const CHAT_MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "qwen3-4b-instruct-2507-q4",
        name: "Qwen3 4B Instruct 2507",
        description: "Best all-rounder for RAG: follows instructions and cites sources reliably, strong in Polish and other languages, no hidden “thinking”.",
        file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
        bytes: 2_497_281_120,
        kind: ModelKind::Chat,
        family: "qwen3",
        parameters: "4B",
        license: "Apache-2.0",
        ctx: 12_288,
        kv_bytes_per_token: 36 * 8 * 128 * 2 * 2,
        recommended: true,
    },
    ModelSpec {
        id: "gemma-3-4b-it-q4",
        name: "Gemma 3 4B",
        description: "Google's small model: natural, well-written answers in many languages.",
        file: "gemma-3-4b-it-Q4_K_M.gguf",
        url: "https://huggingface.co/ggml-org/gemma-3-4b-it-GGUF/resolve/main/gemma-3-4b-it-Q4_K_M.gguf",
        sha256: "882e8d2db44dc554fb0ea5077cb7e4bc49e7342a1f0da57901c0802ea21a0863",
        bytes: 2_489_757_856,
        kind: ModelKind::Chat,
        family: "gemma3",
        parameters: "4B",
        license: "Gemma Terms of Use",
        ctx: 12_288,
        kv_bytes_per_token: 34 * 4 * 256 * 2 * 2,
        recommended: false,
    },
    ModelSpec {
        id: "llama-3.2-3b-instruct-q4",
        name: "Llama 3.2 3B Instruct",
        description: "The lightest option — fastest, fine for 8 GB machines; English is its strongest language.",
        file: "Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf",
        sha256: "6c1a2b41161032677be168d354123594c0e6e67d2b9227c84f296ad037c728ff",
        bytes: 2_019_377_696,
        kind: ModelKind::Chat,
        family: "llama",
        parameters: "3B",
        license: "Llama 3.2 Community License",
        ctx: 12_288,
        kv_bytes_per_token: 28 * 8 * 128 * 2 * 2,
        recommended: false,
    },
];

/// The embedding model every vault gets unless the user picks another one.
pub const DEFAULT_EMBEDDING: ModelSpec = NOMIC_EMBED;

pub fn catalog() -> impl Iterator<Item = &'static ModelSpec> {
    std::iter::once(&NOMIC_EMBED).chain(CHAT_MODELS.iter())
}

pub fn spec(id: &str) -> Option<&'static ModelSpec> {
    catalog().find(|s| s.id == id)
}

/// Where a model stands on this machine.
#[derive(Debug, Clone)]
pub struct ModelStatus {
    pub spec: &'static ModelSpec,
    pub path: Option<PathBuf>,
    /// Shipped with RAD (read-only; can't be deleted).
    pub bundled: bool,
    /// (bytes done, total) while downloading or queued.
    pub downloading: Option<(u64, u64)>,
    /// Last download failure.
    pub error: Option<String>,
    pub memory_needed: u64,
    pub fit: FitLevel,
}

#[derive(Debug, Clone, Default)]
struct Progress {
    done: u64,
    active: bool,
    error: Option<String>,
}

/// The models folder: one writable directory (downloads land here) plus read-only fallbacks
/// such as the copy bundled with the desktop app or the Docker image.
#[derive(Debug)]
pub struct Library {
    dir: PathBuf,
    bundled: Vec<PathBuf>,
    http: reqwest::Client,
    downloading: tokio::sync::Mutex<()>,
    progress: Mutex<HashMap<&'static str, Progress>>,
}

impl Library {
    pub fn new(dir: PathBuf, bundled: Vec<PathBuf>) -> Arc<Library> {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("can't create models folder {}: {e}", dir.display());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .user_agent(concat!("RAD/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Arc::new(Library { dir, bundled, http, downloading: tokio::sync::Mutex::new(()), progress: Mutex::new(HashMap::new()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where a model's file is, if it is present and complete.
    pub fn locate(&self, spec: &ModelSpec) -> Option<PathBuf> {
        std::iter::once(&self.dir)
            .chain(self.bundled.iter())
            .map(|d| d.join(spec.file))
            .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == spec.bytes))
    }

    pub fn status(&self) -> Vec<ModelStatus> {
        let progress = self.progress.lock().unwrap().clone();
        let total = total_memory();
        catalog()
            .map(|spec| {
                let path = self.locate(spec);
                let bundled = path.as_ref().is_some_and(|p| !p.starts_with(&self.dir));
                let p = progress.get(spec.id).cloned().unwrap_or_default();
                let needed = memory_needed(spec);
                ModelStatus {
                    spec,
                    bundled,
                    downloading: p.active.then_some((p.done, spec.bytes)),
                    error: p.error,
                    memory_needed: needed,
                    fit: FitLevel::of(needed, total),
                    path,
                }
            })
            .collect()
    }

    /// Returns the model's path, downloading and verifying it first if needed.
    pub async fn ensure(&self, spec: &'static ModelSpec) -> Result<PathBuf> {
        if let Some(p) = self.locate(spec) {
            return Ok(p);
        }
        self.set_progress(spec, |p| *p = Progress { active: true, ..Default::default() });
        let res = async {
            let _one_at_a_time = self.downloading.lock().await;
            match self.locate(spec) {
                Some(p) => Ok(p),
                None => self.download(spec).await,
            }
        }
        .await;
        self.set_progress(spec, |p| {
            p.active = false;
            p.error = res.as_ref().err().map(|e| e.to_string());
        });
        res
    }

    /// Starts a download in the background (no-op when present or already downloading).
    pub fn start_download(self: &Arc<Self>, spec: &'static ModelSpec) -> Result<()> {
        if self.locate(spec).is_some() || self.progress.lock().unwrap().get(spec.id).is_some_and(|p| p.active) {
            return Ok(());
        }
        let needed = memory_needed(spec);
        if FitLevel::of(needed, total_memory()) == FitLevel::TooBig {
            return Err(LocalError::DoesNotFit(format!(
                "{} needs about {} of memory — more than this machine can spare ({} total)",
                spec.name,
                fit::human(needed),
                fit::human(total_memory())
            )));
        }
        let lib = self.clone();
        tokio::spawn(async move {
            if let Err(e) = lib.ensure(spec).await {
                tracing::warn!("{e}");
            }
        });
        Ok(())
    }

    /// Deletes a downloaded model. Models shipped with RAD stay.
    pub fn delete(&self, spec: &ModelSpec) -> Result<()> {
        let path = self.dir.join(spec.file);
        if path.exists() {
            std::fs::remove_file(&path)?;
        } else if self.locate(spec).is_some() {
            return Err(LocalError::Engine(format!("{} ships with RAD and can't be removed", spec.name)));
        }
        self.progress.lock().unwrap().remove(spec.id);
        Ok(())
    }

    fn set_progress(&self, spec: &'static ModelSpec, f: impl FnOnce(&mut Progress)) {
        f(self.progress.lock().unwrap().entry(spec.id).or_default());
    }

    async fn download(&self, spec: &'static ModelSpec) -> Result<PathBuf> {
        let err = |message: String| LocalError::Download { model: spec.name.to_string(), message };
        tokio::fs::create_dir_all(&self.dir).await?;
        if let Some(free) = fit::disk_free(&self.dir) {
            let need = spec.bytes + 256 * 1024 * 1024;
            if free < need {
                return Err(LocalError::DoesNotFit(format!(
                    "not enough disk space in {} for {} ({} needed, {} free)",
                    self.dir.display(),
                    spec.name,
                    fit::human(need),
                    fit::human(free)
                )));
            }
        }
        tracing::info!("downloading {} ({}) into {}", spec.name, fit::human(spec.bytes), self.dir.display());
        let resp = self
            .http
            .get(spec.url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| err(e.to_string()))?;
        let part = self.dir.join(format!("{}.part", spec.file));
        let res = async {
            let mut out = tokio::fs::File::create(&part).await?;
            let mut hasher = Sha256::new();
            let mut written = 0u64;
            let mut body = resp.bytes_stream();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| err(e.to_string()))?;
                hasher.update(&chunk);
                out.write_all(&chunk).await?;
                written += chunk.len() as u64;
                self.set_progress(spec, |p| p.done = written);
            }
            out.flush().await?;
            let digest = hex::encode(hasher.finalize());
            if written != spec.bytes || digest != spec.sha256 {
                return Err(err(format!("checksum mismatch (got {written} bytes, sha256 {digest})")));
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        let dest = self.dir.join(spec.file);
        tokio::fs::rename(&part, &dest).await?;
        tracing::info!("{} ready at {}", spec.name, dest.display());
        Ok(dest)
    }

    /// Embeds `texts` with a built-in model. Inputs longer than the model's context are truncated.
    /// The default embedding model is fetched on demand if it's missing.
    pub async fn embed(&self, model: &str, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let spec = spec(model).filter(|s| s.kind == ModelKind::Embedding).ok_or_else(|| LocalError::UnknownModel(model.to_string()))?;
        let path = self.ensure(spec).await?;
        engine::embed(path, spec, texts).await
    }

    /// Streams a chat answer from a downloaded built-in chat model.
    pub fn chat(&self, model: &str, messages: Vec<ChatMessage>, opts: ChatOptions) -> Result<tokio::sync::mpsc::Receiver<Result<ChatEvent>>> {
        let spec = spec(model).filter(|s| s.kind == ModelKind::Chat).ok_or_else(|| LocalError::UnknownModel(model.to_string()))?;
        let path = self.locate(spec).ok_or_else(|| LocalError::NotInstalled(spec.name.to_string()))?;
        engine::chat(path, spec, messages, opts)
    }
}
