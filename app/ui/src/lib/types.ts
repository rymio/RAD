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
  source_kind: 'upload' | 'paste' | 'screenshot' | 'note' | 'chat' | 'web' | string;
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
  /** Set when the source is an earlier conversation. */
  chat_id?: Id;
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
  | { type: 'action'; action: AgentAction }
  | { type: 'done'; message: Message }
  | { type: 'error'; message: string }
  | { type: 'stopped' };

export type ProviderKind = 'ollama' | 'openai' | 'anthropic' | 'builtin';
export interface LocalModel {
  id: string; name: string; description: string; kind: 'chat' | 'embedding'; file: string; bytes: number;
  parameters: string; license: string; recommended: boolean; installed: boolean; bundled: boolean;
  downloaded: number | null; error: string | null; memory_needed: number; fit: 'ok' | 'tight' | 'too_big';
}
export interface LocalModels { provider_id: Id | null; dir: string; total_memory: number; models: LocalModel[] }
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
  query_rewrite: boolean; chat_recall: boolean; max_output_tokens: number; temperature: number;
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
  sidebar_collapsed: boolean;
  tts_enabled: boolean; tts_auto_read: boolean; tts_voice: string; tts_speed: number; tts_volume: number;
  tts_temperature: number; tts_top_p: number; tts_top_k: number; tts_repetition_penalty: number;
}

/** A calendar entry. `start`/`end` are "HH:MM"; no `start` means all day. */
export interface CalendarEvent { id: string; title: string; date: string; start: string | null; end: string | null; notes: string; color: string }

/** The email account, saved in the vault. The password stays in this device's keychain. */
export interface EmailAccount { email: string; host: string; port: number; security: 'ssl' | 'starttls'; username: string }
export interface MailFolder { name: string; label: string; special: 'inbox' | 'sent' | 'drafts' | 'archive' | 'junk' | 'trash' | 'all' | 'flagged' | null }
export interface MailSummary { uid: number; from_name: string; from_addr: string; to: string; subject: string; date: number; seen: boolean; flagged: boolean; attachments: boolean; size: number }
export interface MailPage { total: number; messages: MailSummary[]; next: number | null }
export interface MailMessage {
  uid: number; subject: string; from: string; to: string; cc: string; reply_to: string; date: number;
  text: string; html: string | null; attachments: { name: string; mime: string; size: number }[]; flagged: boolean;
}

/** One vault file used by several computers (on a network share). */
export interface VaultSharing {
  shared: boolean; location: 'local' | 'network' | 'synced'; location_label: string;
  sessions: { machine: string; this: boolean; since: number; last_seen: number }[];
}

/** The media players (libVLC engine, one process each): Music plays sound only. */
export type MediaPlayerKind = 'music' | 'video';
export type MediaState = 'idle' | 'opening' | 'buffering' | 'playing' | 'paused' | 'stopped' | 'ended' | 'error';
export interface MediaStatus {
  state: MediaState; has_media: boolean; time: number; length: number; seekable: boolean; rate: number;
  volume: number; muted: boolean; video: [number, number] | null; aspect: number | null;
}
export interface MediaMeta { title: string | null; artist: string | null; album: string | null; genre: string | null; date: string | null; track: string | null; now_playing: string | null }
export interface MediaTrackInfo { id: number; name: string }
export interface MediaTracks { audio: MediaTrackInfo[]; audio_current: number; spu: MediaTrackInfo[]; spu_current: number }
export interface EqPreset { name: string; preamp: number; bands: number[] }
export interface EqInfo { freqs: number[]; presets: EqPreset[] }
/** Where a playlist entry plays from: a file on this computer or a file in the vault. */
export type MediaSource = { kind: 'file'; path: string } | { kind: 'vault'; id: Id };
export interface MediaItem { key: string; src: MediaSource; title: string; artist?: string | null; album?: string | null; duration?: number }

/** The built-in browsers: Servo ("Web Browser"), Chromium (CEF) and Tor (Servo over Arti). */
export type BrowserEngine = 'servo' | 'chromium' | 'tor' | 'rymwin';

export type Route =
  | { kind: 'chat'; id: Id | null }
  | { kind: 'project'; id: Id; doc?: Id }
  | { kind: 'settings'; tab?: string }
  | { kind: 'browser' }
  | { kind: 'chromium' }
  | { kind: 'tor' }
  | { kind: 'terminal' }
  | { kind: 'files' }
  | { kind: 'calendar' }
  | { kind: 'email' }
  | { kind: 'music' }
  | { kind: 'video' }
  | { kind: 'rymnet' }
  | { kind: 'rymwin' }
  | { kind: 'container'; type: 'rust' | 'python' }
  | { kind: 'sidex' }
  | { kind: 'database' }
  | { kind: 'games' }
  | { kind: 'zx' }
  | { kind: 'zxlab' }
  | { kind: 'c64' }
  | { kind: 'c64lab' }
  | { kind: 'dos' }
  | { kind: 'gilgamesh' }
  | { kind: 'monitor' };

/** Something the assistant asks the app to do (see crates/rad-server/src/agent.rs). */
export interface AgentAction { kind: 'ssh' | 'open_url' | 'terminal' | string; host?: string; user?: string; port?: number; identity_file?: string; url?: string }
export interface Bookmark { title: string; url: string }
export interface SshHost { host: string; user?: string; port?: number; identity_file?: string; last_used?: number }
export interface TermInfo { id: number; title: string; kind: 'local' | 'ssh' | 'sandbox' | 'container' | string; exited: string | null; sandbox: string | null; container?: string | null }
export interface SandboxInfo {
  id: string; name: string; distro: string; distro_name: string; cpus: number; mem_mb: number; disk_gb: number;
  network: boolean; created_at: number; disk_used: number; running: boolean;
}
export interface SandboxStatus {
  supported: boolean; engine: boolean; dev_build: boolean;
  distros: { id: string; name: string; description: string }[];
  sandboxes: SandboxInfo[];
  progress: SandboxProgress | null;
}
export interface SandboxProgress { phase: string; done?: number; total?: number }
/** Databases (Turso) — see crates/rad-server/src/routes/db.rs. */
export interface DbInfo { id: string; name: string; created_at: number; size: number }
export interface DbColumn { name: string; type: string; not_null: boolean; default: string | null; pk: number; unique: boolean }
export interface DbTable { name: string; kind: 'table' | 'view'; sql: string; columns: DbColumn[]; indexes: { name: string; unique: boolean; columns: string[] }[]; rows: number | null }
/** A cell: null, number, string, or a blob as {$blob: base64}. */
export type DbValue = null | number | string | { $blob: string };
export interface DbRowsPage { columns: string[]; rows: { rowid: number | null; values: DbValue[] }[]; total: number; offset: number; limit: number }
export interface DbResult { sql: string; columns: string[]; rows: DbValue[][]; changes: number; last_insert_rowid: number; truncated: boolean }
export interface DbKey { id: string; name: string; prefix: string; write: boolean; dbs: string[]; created_at: number; last_used: number | null }
/** The Spectrum lab (crates/rad-zx/src/lab.rs). */
export interface LabNotes { labels: Record<number, string>; comments: Record<number, string> }
export interface LabLine { addr: number; bytes: number[]; kind: 'code' | 'reachable' | 'data' | 'text'; text: string; label: string | null; comment: string | null; target: number | null; variable: boolean }
export interface LabTapeBlock { index: number; speed: string; flag: number; length: number; screen: boolean; header: { kind: number; name: string; length: number; param1: number; param2: number } | null }
export interface LabSummary {
  model: '48' | '128' | 'C64'; entry: number | null; range: [number, number]; functions: number; function_addrs: number[]; code_bytes: number; reachable_bytes: number; variables: number;
  ports: Record<string, number>; rom_calls: { addr: number; count: number; name: string | null; what: string | null }[]; basic: string | null;
  tape: LabTapeBlock[]; map: string[]; strings: { addr: number; text: string }[]; steps: number; seconds: number; udg: number;
  /** C64 only (crates/rad-c64/src/lab.rs): chip registers its code used, the disk's files, what the VIC-II showed. */
  hardware?: { addr: number; chip: string | null; name: string | null; what: string | null; reads: number; writes: number }[];
  files?: string[]; screen_addr?: number; charset?: { addr: number; rom: boolean }; bitmap?: number | null;
  sprites?: { n: number; addr: number; multicolor: boolean }[]; pictures?: { title: string; data: string }[];
  cartridge?: boolean; roms?: 'open' | 'commodore';
}
export interface FrameStats {
  fetch_rom: number; fetch_ram_low: number; fetch_ram_high: number; write_screen: number; write_ram_low: number; write_ram_high: number;
  in_fe: number; out_fe: number; in_joy: number; other_io: number; halts: number; instructions: number; pc: number; border: number;
}
export interface SidexStatus {
  supported: boolean; available: boolean; dev_build: boolean; running: boolean;
  recent: { path: string; name: string; exists: boolean }[];
}
export type ContainerKindId = 'rust' | 'python';
export interface ContainerKind { id: ContainerKindId; name: string; description: string; files: string[]; run: string; ready: boolean }
export interface ContainerStats { disk_total: number; disk_used: number; mem_total: number; mem_used: number; uptime: number }
export interface ContainerInfo {
  id: string; name: string; kind: ContainerKindId; kind_name: string; cpus: number; mem_mb: number; disk_gb: number;
  network: boolean; ports: number[]; created_at: number; disk_used: number; disk_total: number;
  running: boolean; starting: boolean; started_at: number | null; disk_on_mac: number; stats: ContainerStats | null; grow_pending: boolean;
}
export interface ContainerStatus {
  supported: boolean; engine: boolean; dev_build: boolean;
  kinds: ContainerKind[];
  containers: ContainerInfo[];
  progress: SandboxProgress | null;
  host: { mem_mb: number; cpus: number };
  project: string;
}
export interface ContainerEntry { name: string; dir: boolean; size: number }
export interface ExecResult { exit_code: number | null; output: string; truncated: boolean; timed_out: boolean; cwd: string | null }
/** What the Terminal should open next (from chat actions, code blocks, Files…). */
export type TerminalRequest = AgentAction | { kind: 'sandbox_run'; code: string; lang: string } | { kind: 'sandbox'; id: string } | { kind: 'container'; id: string } | { kind: 'rymsh' };
export interface FsEntry { name: string; path: string; is_dir: boolean; size: number; modified: number; hidden: boolean }
export interface FsListing { path: string; parent: string | null; entries: FsEntry[] }
export interface FsPlace { name: string; path: string; icon: string }
export type FsPreview = { kind: 'image'; mime: string; data: string } | { kind: 'text'; text: string; truncated: boolean } | { kind: 'other'; size: number };

/** An image sent with one message only (e.g. a browser screenshot); base64 data. */
export interface InlineImage { mime: string; data: string }

/** Built-in browser (Servo engine). */
export interface BrowserTab { id: number; url: string; title: string; loading: boolean; can_back: boolean; can_forward: boolean; zoom: number }
export interface BrowserState { type: 'state'; tabs: BrowserTab[]; active: number }
export interface PageScrape {
  url: string; title: string; description: string; selection: string; text: string;
  links: { text: string; href: string }[]; scroll_y: number; page_height: number;
}
export interface BrowserSnapshot { page?: PageScrape; image?: InlineImage }

export interface RymnetPeer { name: string; public_key: string; endpoint: string }
export interface RymnetStatus {
  installed: boolean; bin_dir: string | null; initialized: boolean; node_id: string | null; listen_addr: string | null;
  daemon: 'running' | 'external' | 'stopped'; rymwin: boolean; peers: RymnetPeer[]; daemon_log: string[];
}

export interface VpnScopes { services: boolean; browsers: boolean; email: boolean; ssh: boolean; rymnet: boolean }
export interface VpnSettings {
  endpoint: string; peer_public_key: string; address: string[]; dns: string[]; keepalive: number | null; mtu: number | null;
  scopes: VpnScopes; auto_connect: boolean; was_on: boolean;
}
export interface VpnStatus { up: boolean; handshake_ago_secs: number | null; tx_bytes: number; rx_bytes: number; connections: number; last_error: string | null }
export interface VpnInfo { settings: VpnSettings; public_key: string | null; has_psk: boolean; configured: boolean; on: boolean; socks: string | null; status: VpnStatus | null }
