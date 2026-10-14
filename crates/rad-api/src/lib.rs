//! Wire types shared by `rad-server` and the RAD desktop client.

use serde::{Deserialize, Serialize};

pub type Id = i64;
/// Unix timestamp in milliseconds.
pub type Ts = i64;

pub const API_VERSION: u32 = 1;
pub const HEADER_SESSION: &str = "x-rad-session";
pub const HEADER_FILENAME: &str = "x-rad-filename";
pub const HEADER_SOURCE: &str = "x-rad-source";

// ---------------------------------------------------------------- status & auth

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub name: String,
    pub version: String,
    pub api_version: u32,
    /// A vault file exists at the configured path.
    pub initialized: bool,
    /// The vault is currently locked (key not in server memory).
    pub locked: bool,
    /// The bearer token sent with this request belongs to a paired, non-revoked device.
    pub device: Option<Device>,
    /// The session header sent with this request is valid.
    pub session_valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device_name: String,
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairResponse {
    pub device: Device,
    pub device_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub os: String,
    pub created_at: Ts,
    pub last_seen: Ts,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: Ts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitVaultRequest {
    pub passphrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockRequest {
    pub passphrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryUnlockRequest {
    pub recovery_key: String,
    pub new_passphrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePassphraseRequest {
    pub current: String,
    pub new: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub session: String,
    /// Present when a new recovery key was generated (vault creation, recovery, passphrase change).
    pub recovery_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryKit {
    pub recovery_key: String,
}

// ---------------------------------------------------------------- vault

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultInfo {
    pub path: String,
    pub file_bytes: u64,
    pub blob_bytes: u64,
    pub projects: i64,
    pub documents: i64,
    pub notes: i64,
    pub chunks: i64,
    pub embedded_chunks: i64,
    pub chats: i64,
    pub created_at: Ts,
    pub format_version: i64,
    pub embedding_model: Option<String>,
    pub backups: Vec<BackupInfo>,
    pub backup_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub file: String,
    pub bytes: u64,
    pub created_at: Ts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub messages: Vec<String>,
}

// ---------------------------------------------------------------- projects

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: String,
    pub color: String,
    pub icon: String,
    pub system_prompt: String,
    pub archived: bool,
    pub document_count: i64,
    pub note_count: i64,
    pub created_at: Ts,
    pub updated_at: Ts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub system_prompt: Option<String>,
    pub archived: Option<bool>,
}

// ---------------------------------------------------------------- documents & notes

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocStatus {
    Queued,
    Processing,
    Ready,
    Error,
}

impl DocStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocStatus::Queued => "queued",
            DocStatus::Processing => "processing",
            DocStatus::Ready => "ready",
            DocStatus::Error => "error",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "queued" => DocStatus::Queued,
            "processing" => DocStatus::Processing,
            "ready" => DocStatus::Ready,
            _ => DocStatus::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: Id,
    pub project_id: Id,
    pub title: String,
    pub original_name: String,
    pub mime: String,
    pub size: i64,
    pub hash: String,
    /// upload | paste | screenshot | note | chat
    pub source_kind: String,
    pub status: DocStatus,
    pub error: Option<String>,
    /// Non-fatal notice, e.g. "semantic index pending".
    pub notice: Option<String>,
    pub page_count: Option<i64>,
    pub chunk_count: i64,
    pub has_thumb: bool,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub created_at: Ts,
    pub updated_at: Ts,
}

impl Document {
    pub fn is_note(&self) -> bool {
        self.source_kind == "note"
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentPatch {
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub project_id: Option<Id>,
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteInput {
    pub title: String,
    /// Markdown body.
    pub content: String,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub document: Document,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FromBlobRequest {
    pub hash: String,
    pub filename: String,
    pub source_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResponse {
    pub document: Document,
    /// The exact same file already existed in this project.
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Anchor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Seconds into an audio/video file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<f64>,
}

impl Anchor {
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(f) = &self.file {
            parts.push(f.clone());
        }
        if let Some(p) = self.page {
            parts.push(format!("p. {p}"));
        }
        if let Some(s) = self.slide {
            parts.push(format!("slide {s}"));
        }
        if let Some(s) = &self.sheet {
            parts.push(format!("sheet {s}"));
        }
        if let Some(t) = self.time {
            let t = t.max(0.0) as u64;
            parts.push(if t >= 3600 {
                format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
            } else {
                format!("{}:{:02}", t / 60, t % 60)
            });
        }
        if let Some(h) = &self.heading {
            parts.push(h.clone());
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: Id,
    pub document_id: Id,
    pub ord: i64,
    pub text: String,
    pub anchor: Anchor,
    pub kind: String,
}

// ---------------------------------------------------------------- chats

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Scope {
    /// Empty = all projects.
    #[serde(default)]
    pub projects: Vec<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRef {
    pub provider_id: Id,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatMode {
    #[default]
    Rag,
    ChatOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: Id,
    pub title: String,
    pub scope: Scope,
    pub model: Option<ModelRef>,
    pub mode: ChatMode,
    pub pinned: bool,
    pub message_count: i64,
    pub last_message: Option<String>,
    pub created_at: Ts,
    pub updated_at: Ts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatInput {
    pub title: Option<String>,
    pub scope: Option<Scope>,
    /// `Some(None)` clears the per-chat model override.
    #[serde(default, with = "double_option", skip_serializing_if = "Option::is_none")]
    pub model: Option<Option<ModelRef>>,
    pub mode: Option<ChatMode>,
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatWithMessages {
    pub chat: Chat,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Id,
    pub chat_id: Id,
    pub role: String,
    pub content: String,
    pub attachments: Vec<Attachment>,
    pub citations: Vec<Citation>,
    pub model: Option<String>,
    pub error: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub created_at: Ts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub document_id: Id,
    pub title: String,
    pub mime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub n: u32,
    pub chunk_id: Id,
    pub document_id: Id,
    pub project_id: Id,
    pub project_name: String,
    pub title: String,
    pub mime: String,
    pub source_kind: String,
    pub anchor: Anchor,
    pub snippet: String,
    pub score: f32,
    /// Set when the source is an earlier conversation rather than a document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Id>,
    /// Overrides the chat's model for this message only.
    pub model: Option<ModelRef>,
    /// Delete this message and everything after it before sending (edit & resend).
    pub truncate_from: Option<Id>,
    /// Drop the trailing assistant message and answer the last user message again.
    #[serde(default)]
    pub regenerate: bool,
    /// Context for this answer only, not stored with the message — e.g. the text of the web
    /// page open in RAD's browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Images for this answer only, not stored — e.g. a screenshot of the browser. When present
    /// and no model is given, the vision model answers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<InlineImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineImage {
    pub mime: String,
    /// Base64.
    pub data: String,
}

/// Server-sent events on `POST /v1/chats/:id/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatStreamEvent {
    UserMessage { message: Message },
    Status { text: String },
    Sources { citations: Vec<Citation> },
    Delta { text: String },
    /// The message asked RAD to do something (open an SSH session, a web page…); the app carries it out.
    Action { action: AgentAction },
    Done { message: Message },
    Error { message: String },
}

/// Something the assistant asks the app to do for the user.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentAction {
    /// `ssh` (terminal session to `host`), `open_url` (RAD's browser) or `terminal` (local shell).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Private key file for SSH, e.g. `~/.ssh/id_ed25519`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ---------------------------------------------------------------- search

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub citation: Citation,
}

// ---------------------------------------------------------------- providers & models

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Ollama,
    /// Any OpenAI-compatible API (OpenAI, OpenRouter, Groq, Mistral, LM Studio, vLLM, Gemini's OpenAI endpoint…)
    Openai,
    Anthropic,
    /// RAD's own llama.cpp engine running GGUF models from the local `models` folder (embeddings).
    Builtin,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Ollama => "ollama",
            ProviderKind::Openai => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Builtin => "builtin",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ollama" => ProviderKind::Ollama,
            "openai" => ProviderKind::Openai,
            "anthropic" => ProviderKind::Anthropic,
            "builtin" => ProviderKind::Builtin,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: Id,
    pub kind: ProviderKind,
    pub name: String,
    pub base_url: String,
    pub has_api_key: bool,
    /// Header names only; values are never returned.
    pub header_names: Vec<String>,
    pub enabled: bool,
    pub created_at: Ts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderInput {
    pub kind: Option<ProviderKind>,
    pub name: Option<String>,
    pub base_url: Option<String>,
    /// `Some("")` clears the key.
    pub api_key: Option<String>,
    /// Replaces all extra headers when present.
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub size: Option<u64>,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    pub provider_id: Id,
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub checked_at: Ts,
}

/// A model of the built-in engine (`GET /v1/local-models`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModel {
    /// Use as `ModelRef.model` with the built-in provider.
    pub id: String,
    pub name: String,
    pub description: String,
    /// chat | embedding
    pub kind: String,
    pub file: String,
    pub bytes: u64,
    pub parameters: String,
    pub license: String,
    pub recommended: bool,
    pub installed: bool,
    /// Shipped with RAD; can't be deleted.
    pub bundled: bool,
    /// Bytes downloaded so far while a download is running.
    pub downloaded: Option<u64>,
    pub error: Option<String>,
    /// Memory needed when loaded (weights + context).
    pub memory_needed: u64,
    /// ok | tight | too_big for this machine.
    pub fit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModels {
    /// The built-in provider's id in this vault.
    pub provider_id: Option<Id>,
    /// Folder downloads go to (on the server in server mode).
    pub dir: String,
    pub total_memory: u64,
    pub models: Vec<LocalModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub model: String,
}

// ---------------------------------------------------------------- settings & memories

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub chat_model: Option<ModelRef>,
    /// Small fast model for rewriting follow-up questions into standalone queries.
    pub rewrite_model: Option<ModelRef>,
    pub vision_model: Option<ModelRef>,
    pub embedding_model: Option<ModelRef>,
    /// OpenAI-compatible `/audio/transcriptions` model (whisper-1, whisper-large-v3, …) for mp3/mp4/etc.
    pub transcription_model: Option<ModelRef>,
    pub persona: String,
    pub top_k: u32,
    pub context_chars: u32,
    pub chunk_chars: u32,
    pub chunk_overlap: u32,
    pub query_rewrite: bool,
    /// Search earlier conversations too, and cite them like documents.
    pub chat_recall: bool,
    pub max_output_tokens: u32,
    pub temperature: f32,
    /// Lock the server vault after this many idle minutes (0 = never).
    pub server_auto_lock_minutes: u32,
    pub backup_keep: u32,
    pub backup_daily: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            chat_model: None,
            rewrite_model: None,
            vision_model: None,
            embedding_model: None,
            transcription_model: None,
            persona: "You are RAD, a warm, precise personal assistant. You help your owner \
                      with their own documents, notes, images and projects."
                .into(),
            top_k: 8,
            context_chars: 24_000,
            chunk_chars: 1_800,
            chunk_overlap: 200,
            query_rewrite: true,
            chat_recall: true,
            max_output_tokens: 4096,
            temperature: 0.3,
            server_auto_lock_minutes: 0,
            backup_keep: 7,
            backup_daily: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: Id,
    pub text: String,
    pub enabled: bool,
    pub created_at: Ts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryInput {
    pub text: Option<String>,
    pub enabled: Option<bool>,
}

// ---------------------------------------------------------------- live sync

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Durable change recorded in the vault's change log.
    Change { seq: i64, entity: String, entity_id: Id, op: String },
    /// Ephemeral ingestion progress.
    DocumentStatus { document_id: Id, project_id: Id, status: DocStatus, error: Option<String> },
    ProviderHealth { health: Vec<ProviderHealth> },
    Locked,
    Unlocked,
    Hello { last_seq: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub seq: i64,
    pub entity: String,
    pub entity_id: Id,
    pub op: String,
    pub at: Ts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub queued: i64,
    pub running: i64,
    pub failed: i64,
}

mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(inner) => inner.serialize(s),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}
