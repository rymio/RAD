// Mirrors crates/rad-api.

export type Id = number;

export interface Device { id: string; name: string; os: string; created_at: number; last_seen: number; revoked: boolean }
export interface ServerStatus {
  name: string; version: string; api_version: number;
  initialized: boolean; locked: boolean; device: Device | null; session_valid: boolean;
}

export interface Project {
  id: Id; name: string; description: string; color: string; icon: string; system_prompt: string;
  archived: boolean; document_count: number; note_count: number; created_at: number; updated_at: number;
}

export type DocStatus = 'queued' | 'processing' | 'ready' | 'error';
export interface Document {
  id: Id; project_id: Id; title: string; original_name: string; mime: string; size: number; hash: string;
  source_kind: 'upload' | 'paste' | 'screenshot' | 'note' | 'chat' | string;
  status: DocStatus; error: string | null; notice: string | null; page_count: number | null; chunk_count: number;
  has_thumb: boolean; tags: string[]; pinned: boolean; created_at: number; updated_at: number;
}
export interface Note { document: Document; content: string }
export interface UploadResponse { document: Document; duplicate: boolean }

export interface Anchor { page?: number; heading?: string; sheet?: string; slide?: number; file?: string; time?: number }
export interface Chunk { id: Id; document_id: Id; ord: number; text: string; anchor: Anchor; kind: string }

export interface ModelRef { provider_id: Id; model: string }
export interface Scope { projects: Id[] }
export type ChatMode = 'rag' | 'chat_only';
export interface Chat {
  id: Id; title: string; scope: Scope; model: ModelRef | null; mode: ChatMode; pinned: boolean;
  message_count: number; last_message: string | null; created_at: number; updated_at: number;
}
export interface ChatWithMessages { chat: Chat; messages: Message[] }
export interface Attachment { document_id: Id; title: string; mime: string }
export interface Citation {
  n: number; chunk_id: Id; document_id: Id; project_id: Id; project_name: string; title: string; mime: string;
  source_kind: string; anchor: Anchor; snippet: string; score: number;
}
export interface Message {
  id: Id; chat_id: Id; role: 'user' | 'assistant' | string; content: string; attachments: Attachment[];
  citations: Citation[]; model: string | null; error: string | null; tokens_in: number | null; tokens_out: number | null;
  created_at: number;
}
export type ChatStreamEvent =
  | { type: 'user_message'; message: Message }
  | { type: 'status'; text: string }
  | { type: 'sources'; citations: Citation[] }
  | { type: 'delta'; text: string }
  | { type: 'done'; message: Message }
  | { type: 'error'; message: string }
  | { type: 'stopped' };

export type ProviderKind = 'ollama' | 'openai' | 'anthropic';
export interface Provider {
  id: Id; kind: ProviderKind; name: string; base_url: string; has_api_key: boolean; header_names: string[];
  enabled: boolean; created_at: number;
}
export interface ModelInfo { id: string; size: number | null; family: string | null; parameter_size: string | null }
export interface ProviderHealth { provider_id: Id; ok: boolean; latency_ms: number | null; error: string | null; checked_at: number }

export interface Settings {
  chat_model: ModelRef | null; rewrite_model: ModelRef | null; vision_model: ModelRef | null;
  embedding_model: ModelRef | null; transcription_model: ModelRef | null;
  persona: string; top_k: number; context_chars: number; chunk_chars: number; chunk_overlap: number;
  query_rewrite: boolean; max_output_tokens: number; temperature: number;
  server_auto_lock_minutes: number; backup_keep: number; backup_daily: boolean;
}
export interface Memory { id: Id; text: string; enabled: boolean; created_at: number }

export interface BackupInfo { file: string; bytes: number; created_at: number }
export interface VaultInfo {
  path: string; file_bytes: number; blob_bytes: number; projects: number; documents: number; notes: number;
  chunks: number; embedded_chunks: number; chats: number; created_at: number; format_version: number;
  embedding_model: string | null; backups: BackupInfo[]; backup_dir: string;
}
export interface JobSummary { queued: number; running: number; failed: number }

export type RadEvent =
  | { type: 'change'; seq: number; entity: string; entity_id: Id; op: string }
  | { type: 'document_status'; document_id: Id; project_id: Id; status: DocStatus; error: string | null }
  | { type: 'provider_health'; health: ProviderHealth[] }
  | { type: 'unlocked' }
  | { type: 'locked' }
  | { type: 'hello'; last_seq: number };

export interface ClientConfig {
  mode: 'server' | 'local' | string; local_vault: string | null; recent_vaults: string[];
  server_url: string | null; server_name: string | null; device_name: string;
  auto_lock_minutes: number; lock_on_sleep: boolean; close_to_tray: boolean; hide_dock_icon: boolean;
  hotkey_quick_ask: string; hotkey_screenshot: string; hotkey_lock: string; screenshot_project_id: Id | null;
  theme: 'system' | 'light' | 'dark' | 'oled' | string; accent: string; tray_hint_shown: boolean; launched_before: boolean;
}

export type Route =
  | { kind: 'chat'; id: Id | null }
  | { kind: 'project'; id: Id; doc?: Id }
  | { kind: 'settings'; tab?: string };
