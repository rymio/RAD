# RAD — Personal RAG Assistant

RAD is a desktop app written in Rust for macOS, Windows and Linux. You add documents, images and screenshots to **projects**. You then talk to all of it through one **global chat**, which can search every project or only the ones you pick.

- **Storage:** everything RAD knows lives in **one encrypted vault file** (`*.radvault`). You choose where that file is kept. The app asks for a **passphrase** to unlock it each time it starts.
- **Same data on every computer:** the vault can sit on a **RAD server** you own, on a network share, or on a local or USB disk. Whether you open RAD on a Mac, Windows or Linux machine, you get the same projects, files and chats.
- **Models:** RAD uses an **Ollama server**, or remote models (OpenAI-compatible, Anthropic, Gemini) if you choose them.
- **Always at hand:** RAD lives in the system **tray** and keeps working in the background.

---

## 1. Goals and non-goals

**Goals**

*Platforms and storage*
- One app that installs normally on macOS, Windows and Linux.
- **Single-file encrypted vault.** All metadata, the search index, chats, memories and *every stored document and image* sit inside one file, encrypted at rest.
- **The vault location can be chosen.** Create a vault, open one, switch between vaults, or keep a recent-vaults list (for example "Personal" and "Work").
- **A passphrase is required.** Nothing is readable until the vault is unlocked. RAD locks again automatically when idle, on sleep or when the screen locks.
- **Same data on every device.** The vault is reachable through a RAD server (recommended) or a network share, so it doesn't matter which OS you start RAD on.

*Content and chat*
- Projects can hold any file type: documents, spreadsheets, slides, code, email, ebooks, images and screenshots.
- One chat works across all projects. You can narrow its scope to all projects, a selection of projects, or one project named with `@project`.
- Answers are grounded in your files, with clickable citations that open the source at the right page.
- You can pick the model for each chat: Ollama models, or remote providers that use an API key.
- Capturing is fast: paste, drag and drop, or press a screenshot hotkey.

*Everyday use*
- Built to be used every day as a personal assistant, so a restart or crash must never corrupt the vault.
- **Always available:** RAD runs in the tray or menu bar, starts at login, and keeps working in the background.
- **A beautiful, polished interface** (see section 8).

**Non-goals (v1)**
- Several users or tenants. v1 has one owner who uses several devices.
- Opening the vault directly from cloud-sync folders such as Dropbox, iCloud or OneDrive. Section 4.6 explains why.
- Agents that take actions in the outside world (planned for later, see section 10).
- Training or fine-tuning models.

---

## 2. Tech stack

| Concern | Choice | Why |
|---|---|---|
| GUI shell | **Tauri 2** (Rust backend + webview) | Mature on all three OSes. Gives native dialogs, drag and drop, clipboard, tray, global shortcuts, a bundler and an auto-updater. Rich chat UIs are far easier to build in a webview. |
| Frontend | **SvelteKit (static) + TypeScript + Tailwind + shadcn-svelte (bits-ui)** | Fast and accessible. Uses `markdown-it` + `shiki` for rendering, KaTeX for math, `lucide` for icons and Svelte transitions for motion. |
| Tray / background | Tauri tray API, `tauri-plugin-autostart`, `-single-instance`, `-notification`, `-window-state`, `-global-shortcut` | Close-to-tray, launch at login, one running instance, notifications, hotkeys. |
| Async | `tokio` | |
| **Vault engine** | **SQLCipher** (`rusqlite` with feature `bundled-sqlcipher-vendored-openssl`) + **FTS5** + **`sqlite-vec`** (statically registered) | One file that is encrypted page by page (AES-256-CBC with an HMAC-SHA512 on every page). It holds keyword and vector search plus transactional writes. |
| Key derivation | `argon2` (Argon2id) → raw SQLCipher key | Resists brute force far better than the default PBKDF2. |
| Secrets in memory | `secrecy` + `zeroize` | The passphrase and key are wiped from RAM when the vault locks. |
| RAD server | `axum` + `tower-http` + WebSocket + `rustls` | Headless process that holds the vault open and serves clients over REST, SSE (chat) and WebSocket (live sync). Ships as a single binary and a Docker image. |
| API contract | Shared `rad-api` crate (serde) + `utoipa` OpenAPI | Client and server compile against the same types. |
| HTTP client / streaming | `reqwest` + `eventsource-stream` | Ollama streams NDJSON. OpenAI and Anthropic stream SSE. |
| Hashing | `blake3` | Identifies blobs by their content and removes duplicates. |
| Chunking | `text-splitter` (token-aware, Markdown- and code-aware) | |
| Embeddings | Ollama `/api/embed`, or in-process `fastembed` (ONNX) | `fastembed` keeps search working even when Ollama is down. |
| Reranking | `fastembed` cross-encoder (bge-reranker-base) | Clearly improves answer quality. |
| OCR | `ocrs` (pure Rust); Tesseract as an optional fallback | Reads text from screenshots and scanned PDFs. |
| Vision | A vision model from the provider (`qwen2.5vl`, `llama3.2-vision`, GPT or Claude vision) | Writes captions so images can be searched. |
| Screenshots | `xcap` | |
| Logging | `tracing` + `tracing-appender` | **Logs never contain document text or chat content.** |
| Errors | `thiserror` in the libraries, `anyhow` in the app | |

> Pure-Rust UI alternative: egui or Slint. A polished chat UI is much harder to build with them, so Tauri is recommended.

---

## 3. Architecture

```
  macOS laptop          Windows PC            Linux desktop
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ RAD app      │     │ RAD app      │     │ RAD app      │  Tauri + Svelte, tray,
│ (client)     │     │ (client)     │     │ (client)     │  hotkeys, screenshots
└──────┬───────┘     └──────┬───────┘     └──────┬───────┘
       │      HTTPS: REST + SSE + WebSocket     │
       └────────────────────┼───────────────────┘
                            ▼
        ┌───────────────────────────────────────────────┐
        │ rad-server (home server / NAS / VPS / Docker)  │
        │  starts LOCKED → unlocked by passphrase        │
        │  axum API · auth · job workers                 │
        │  rad-rag · rad-ingest · rad-embed · rad-llm    │
        │  rad-vault ─────────▶  personal.radvault       │
        │                        (one encrypted file)    │
        └───────────────┬───────────────────────────────┘
                        │ localhost / LAN
                        ▼
                 Ollama server  (+ optional remote APIs)
```

### Vault access modes (all use the same code and the same vault format)

| Mode | Where the `.radvault` file lives | Several devices at once? | Use it for |
|---|---|---|---|
| **Server (recommended)** | On the RAD server's disk. Chosen with `rad-server --vault /data/personal.radvault` or in the server's admin page | ✅ yes, with live sync | Using RAD from Mac, Windows and Linux at the same time. Ingestion runs next to Ollama. |
| **Network file** | An SMB or NFS share on a NAS, opened directly by the app | ❌ one device at a time (exclusive lease) | No server process, but the data still lives in one central place |
| **Local file** | Local disk or a USB/external drive | ❌ one device | Offline use, or carrying the vault on a USB stick between machines |

The UI always talks to a single `RadBackend` trait, which has two implementations:

- `InProcessBackend`: the app opens the vault itself. Used for the local and network-file modes.
- `HttpBackend`: used when connecting to a rad-server.

The UI code is the same for both.

### Cargo workspace layout

```
RAD/
├── Cargo.toml                # [workspace]
├── crates/
│   ├── rad-core/             # domain types, config, service facade
│   ├── rad-vault/            # vault create/open/unlock/lock, KDF, schema+migrations,
│   │                         # blob store inside the vault, FTS + vector search, backup, rekey
│   ├── rad-ingest/           # type detection, extractors (in-memory), chunker, OCR
│   ├── rad-embed/            # trait Embedder + Ollama/fastembed/OpenAI impls
│   ├── rad-llm/              # trait ChatModel + providers, streaming, model listing
│   ├── rad-rag/              # query rewrite, hybrid retrieval, rerank, prompt, citations
│   ├── rad-jobs/             # durable job queue (tables inside the vault)
│   ├── rad-api/              # shared request/response/event types, OpenAPI
│   ├── rad-server/           # axum binary: lock/unlock, auth, API, workers, Dockerfile
│   ├── rad-client/           # RadBackend trait: InProcessBackend + HttpBackend
│   └── rad-cli/              # dev/admin CLI: vault new/unlock/backup/rekey/verify, ingest, ask, eval
├── app/
│   ├── src-tauri/            # Tauri 2 app (uses rad-client): tray, hotkeys, screenshots, unlock screen
│   └── ui/                   # SvelteKit frontend
├── eval/                     # golden Q&A sets for retrieval regression
└── .github/workflows/        # CI + release matrix
```

### Core traits

```rust
#[async_trait]
pub trait ChatModel: Send + Sync {
    fn id(&self) -> &ModelRef;
    fn capabilities(&self) -> Capabilities;          // vision, tools, ctx_window
    async fn stream(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<ChatDelta>>>;
}

#[async_trait]
pub trait Embedder: Send + Sync {
    fn model_key(&self) -> &str;                     // "ollama:nomic-embed-text"
    fn dims(&self) -> usize;
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> Result<Vec<Vec<f32>>>;
}

pub trait Extractor: Send + Sync {
    fn supports(&self, mime: &Mime, ext: &str) -> bool;
    fn extract(&self, bytes: &[u8]) -> Result<ExtractedDoc>;   // in memory, never writes temp files
}

pub struct Vault { /* SQLCipher connection pool, SecretBox<Key>, lease */ }
impl Vault {
    pub fn create(path: &Path, pass: SecretString, opts: VaultOptions) -> Result<Self>;
    pub fn open(path: &Path, pass: SecretString) -> Result<Self>;  // fails on wrong passphrase
    pub fn lock(self);                                             // closes and zeroizes
    pub fn rekey(&self, new_pass: SecretString) -> Result<()>;
    pub fn backup_to(&self, dest: &Path, pass: Option<SecretString>) -> Result<()>;
    pub fn put_blob(&self, data: impl Read) -> Result<BlobId>;     // chunked, deduplicated
    pub fn read_blob(&self, id: BlobId) -> Result<impl Read + Seek>;
}
```

---

## 4. The vault: storage & encryption

### 4.1 The file format
- A `.radvault` file is a **SQLCipher database**. Everything is inside it, including the original files.
- It is the same file on every OS. The byte order is fixed and there are no OS-specific paths, so a vault made on Windows opens on macOS or Linux.
- **While the vault is closed it is exactly one file.** While it is open, SQLite may create a `-journal` or `-wal` file next to it. These are encrypted too and are folded back into the main file when it closes cleanly.
  - Local mode uses WAL (`PRAGMA journal_mode=WAL`) for speed.
  - Network-file mode uses `journal_mode=DELETE`, because WAL needs shared memory and that doesn't work across machines.
- `PRAGMA auto_vacuum=INCREMENTAL` lets the file shrink after deletions without a full rewrite.

### 4.2 Documents stored as blobs inside the vault
```sql
blobs(id INTEGER PK, blake3 BLOB UNIQUE, size, mime, chunk_count, created_at, ref_count)
blob_chunks(blob_id, seq, data BLOB, PRIMARY KEY(blob_id, seq))  -- 1 MiB pieces
```
- Each uploaded file is split into 1 MiB chunks and streamed in, so a large PDF or video never has to fit in RAM.
- Files are deduplicated by their **blake3** hash. A screenshot pasted twice is stored once.
- Reading a blob works like reading a seekable file (`read_blob` → `Read + Seek`). The preview pane can jump to page 300 of a PDF without decrypting the whole file.
- Thumbnails and rendered previews are cached **inside the vault** too (`thumbs` table), so nothing in plain text is left on disk.
- Deleting a document lowers the blob's `ref_count`. Blobs nobody uses any more are removed by a background garbage collector, followed by `incremental_vacuum`.
- Practical limit: SQLite supports databases up to about 281 TB, far beyond personal needs. The plan targets ≥ 100 GB vaults with fast performance.

### 4.3 Encryption and unlock
- **Key derivation:**
  1. Read the 16-byte salt that SQLCipher stores at the start of the file.
  2. Run **Argon2id** on the passphrase and salt (default m = 256 MiB, t = 3, p = 4, tuned to about 1 s on the device) to get a 256-bit raw key.
  3. Open the database with `PRAGMA key = "x'<key><salt>'"`.
  - The Argon2 parameters are stored in a small plaintext header region (`cipher_plaintext_header_size`) together with a magic `RADVAULT` marker and the format version. This header contains no secrets.
- **Each page is encrypted** (AES-256) and authenticated (HMAC-SHA512). A wrong passphrase or a tampered file is detected straight away.
- **Unlock at every app start:**
  - Local and network-file mode: the app shows an unlock screen, derives the key in-process and opens the vault.
  - Server mode: the server **starts locked**. It knows the vault path but not the key.
    - The first client to connect sends the passphrase over TLS. The server derives the key, opens the vault, and keeps the key **only in RAM**.
    - Every later app start on any device still asks for the passphrase. The device token proves *which device* is connecting, and the passphrase proves *it's you*. The server checks it against an Argon2 verifier and never stores it.
    - The server locks again when it restarts, when you choose "Lock" from any client, or after an optional idle timeout.
- **Auto-lock on the client:** after N minutes idle (default 30), on system sleep or screen lock, or from tray → "Lock RAD". Locking clears the UI state and zeroizes the keys.
- **Changing the passphrase:** `PRAGMA rekey` rewrites the file with the new key. It shows progress and keeps a backup until it finishes.
- **Recovery:** when you create a vault, you can export a **recovery kit**: a printable raw key plus a QR code that opens the vault without the passphrase. **If you lose both the passphrase and the recovery kit, the data cannot be recovered.** The setup screen says this plainly.
- **Opt-in convenience (later):** "Unlock with Touch ID / Windows Hello" keeps a wrapped key in the OS keychain. It is off by default, because by default a passphrase is required.

### 4.4 Choosing and managing vaults
- **First run:**
  - **Create new vault…** Choose a location and name, set a passphrase with a strength meter (zxcvbn), and optionally save the recovery kit.
  - **Open existing vault…** Pick a file.
  - **Connect to a RAD server…** Enter the URL and pairing code.
- **Settings → Storage:**
  - Shows the current vault's path, size, how much space a compaction would free, and the last backup.
  - **Switch vault** (keeps a recent list), Change passphrase, Compact, Verify integrity (`PRAGMA cipher_integrity_check`), Back up now, **Move vault…** (copy to a new location and verify).
- The only things stored outside the vault are device-local settings (in the app config dir): the recent vault paths or server URLs, hotkeys, window state, and launch at login. **No content.**

### 4.5 Backup
- `Vault::backup_to` uses SQLite's online backup API, so the vault stays usable during a backup.
- It produces another single encrypted `.radvault` file, with the same passphrase or a different one.
- Backups can be scheduled (on the server or the client) to a folder you choose, keeping the last N copies. Backups are already encrypted, so they are **safe to put in cloud storage** (Dropbox, B2, an external disk).

### 4.6 Using the vault remotely, without corrupting it
- **Server mode (recommended):** only one process ever opens the file. Many devices use it at the same time through the API, and live sync goes over the WebSocket:
  - every write appends to `change_log(seq, entity, id, op)` and is pushed to all clients;
  - a client that reconnects catches up with `GET /v1/changes?since=<seq>`.
- **Network-file mode:** RAD takes an **exclusive lease** before opening the vault.
  - The lease is a sidecar file `.radvault.lock` holding the device name, PID and a heartbeat.
  - A second device sees "Vault is open on *MacBook* since 14:02". It can open the vault read-only after the lease expires, or after you choose "force unlock" because the other machine crashed.
  - Lease files are SMB-safe, because they don't rely only on POSIX locks.
- **Cloud-sync folders (Dropbox, iCloud, OneDrive, Google Drive) are blocked for the *live* vault.**
  - These tools copy files that are half-written and create "conflicted copies" when two machines touch the file, which corrupts SQLite.
  - RAD detects these folders and warns you, with an advanced override.
  - Backups *can* go there.
- **Uploads in server mode:**
  - The client checks the blake3 hash first (`HEAD /v1/blobs/<hash>`) and doesn't re-upload a file the server already has.
  - Otherwise it uploads in resumable 8 MB chunks over TLS, and the server writes the chunks straight into the vault.
  - **The client keeps nothing:** no cache of documents in plain text. Recent chats are held in memory only. Uploads made while offline are queued in an encrypted outbox.

### 4.7 Schema (inside the vault)
```sql
vault_meta(key, value)                       -- format version, created_at, embedding space, owner
projects(id, name, description, color, icon, system_prompt, created_at, archived)
documents(id, project_id, blob_id, title, original_name, mime, size,
          source_kind /*upload|paste|screenshot|watch*/, status /*queued|processing|ready|error*/,
          error, page_count, tags_json, created_at, updated_at)
blobs, blob_chunks, thumbs(blob_id, size_class, data)
chunks(id, document_id, project_id, ord, text, token_count,
       anchor_json /* {page, heading_path, sheet, slide, line_start, bbox} */,
       kind /*text|ocr|caption|table*/)
chunks_fts                                   -- FTS5 over chunks.text
chunk_vec_<space>                            -- sqlite-vec vec0, one table per embedding model
embedding_spaces(id, model_key, dims, created_at, active)
chats(id, title, scope_json, model_ref, created_at, updated_at, pinned)
messages(id, chat_id, role, content, attachments_json, model_ref, tokens_in, tokens_out, created_at)
message_citations(message_id, n, chunk_id, score)
memories(id, text, source_message_id, created_at, enabled)
providers(id, kind, name, base_url, secret_enc, enabled)   -- API keys live in the encrypted vault
settings(key, value_json)                    -- synced settings (models, retrieval, theme, accent)
devices(id, name, os, token_hash, created_at, last_seen, revoked)   -- server mode
jobs(id, kind, payload_json, state, attempts, progress, error, created_at, updated_at)
change_log(seq, entity, entity_id, op, at)
```
- **API keys and Ollama auth headers live inside the encrypted vault.** Every device that unlocks the vault gets your providers automatically, and no OS keychain entries are needed.

---

## 5. Ingestion pipeline

```
upload / paste / screenshot / drag-drop
   → stream into vault as blob (blake3, chunked, dedup) → documents(status=queued) → enqueue job
worker (server in server mode, the app otherwise):
   detect type → extract IN MEMORY (no plaintext temp files)
   → images: OCR + vision caption
   → normalize → chunk (~500–800 tokens, 10–15% overlap, keep tables/code intact)
   → contextualize ("Doc: <title> › <heading path>")
   → embed in batches → chunks + FTS + vectors in one transaction
   → status=ready, emit progress event
```

| Format | Approach |
|---|---|
| txt, md, code, json, yaml, toml, log | native + `encoding_rs` |
| PDF | `pdfium-render` (loads from memory; bundled pdfium); OCR pages that have no text layer; `pdf-extract` as fallback |
| DOCX / ODT | `zip` + `quick-xml` (keeps headings and tables) |
| XLSX / XLS / ODS / CSV | `calamine` / `csv`; each sheet becomes Markdown tables, chunked by rows with the header repeated |
| PPTX / ODP | `zip` + `quick-xml`; one section per slide, plus speaker notes |
| HTML | `scraper` + `html2md`, readability-style cleanup |
| EPUB | `epub` |
| Email (.eml/.mbox) | `mail-parser`; attachments are processed recursively |
| Images (png/jpg/webp/heic/gif) | `image` → OCR + vision caption + thumbnail |
| ZIP | unpacked recursively, with depth and size limits |
| Audio/video (later) | `whisper-rs` |
| Anything else | stored in the vault and found by name or metadata, marked "not indexed" |

- **Robustness:** every job has a timeout and runs under `catch_unwind`. A file that fails is marked `error` with a retry option. Jobs live in the vault, so they survive restarts.
- **Locked vault:** ingestion pauses while the vault is locked and resumes when it is unlocked.

---

## 6. Retrieval and chat

1. **Scope:** the chat's project scope, plus any `@project` or `#document` mentions.
2. **Query rewrite:** a small model turns the history and the new message into one standalone query, plus optional extra phrasings.
3. **Hybrid search:** take the top 40 from vector search (`sqlite-vec`) and the top 40 from BM25 (FTS5), both filtered by project, and merge them with **Reciprocal Rank Fusion**.
4. **Rerank** down to the top 8–12 with the cross-encoder.
5. **MMR + neighbour merging:** keep the results varied and join adjacent chunks into larger passages.
6. **Token budget:** sized to the model's context window.
7. **Prompt:** the RAD persona, the project's system prompt (when one project is in scope), enabled memories, and numbered sources `[1] <title, p.4>`. The model is told to cite `[n]` and to say when the answer isn't in your documents.
8. **Stream** the answer, then store the message and its citations.
9. **Citations open a preview:** the PDF at that page, the image with its OCR text, or the document scrolled to the chunk. Previews are streamed out of the vault.

- **Modes:** RAG (default), Chat only, Single document.
- **Images in chat:** an image is sent to a vision-capable model and also saved into the active project.

---

## 7. Model providers

| Provider | Chat | Embeddings | Vision | Notes |
|---|---|---|---|---|
| **Ollama server** (primary) | `/api/chat` | `/api/embed` | yes | Configured base URL, e.g. `http://192.168.1.50:11434`; list, pull and delete models from the UI; several server profiles |
| OpenAI-compatible | `/v1/chat/completions` | `/v1/embeddings` | yes | Covers OpenAI, LM Studio, llama.cpp, vLLM, OpenRouter, Groq, Mistral |
| Anthropic | Messages API | — | yes | Native adapter |
| Google Gemini | streamGenerateContent | yes | yes | Phase 4 |
| Built-in fastembed | — | yes | — | Offline embeddings and reranking |

### Ollama server mode
- **Profiles:** each has a name, URL, timeout and optional auth (Bearer token, custom headers, Basic auth, or a pinned self-signed certificate). Profiles are stored in the vault.
- **Who connects:**
  - In server mode, `rad-server` talks to Ollama, usually over `localhost`, so Ollama never needs to be exposed.
  - In local and network-file mode, the app talks to the Ollama server directly.
- **Health:** `/api/version` and `/api/tags` are polled every 30 s, and `/api/ps` shows which models are loaded. A status dot in the tray and title bar shows green (online), amber (slow) or red (offline).
- **Offline:** chats show a friendly banner and ingestion retries with backoff. Keyword search still works, and vector search does too when using fastembed.
- **Throughput:** embedding requests run with limited concurrency (2–4). `keep_alive` is configurable.
- **Setup note:** on the server, run `OLLAMA_HOST=0.0.0.0:11434 ollama serve`, or keep Ollama on localhost when rad-server runs on the same machine.

### Other model settings
- Default models are set separately for chat, query rewriting, vision and embeddings. Each chat can override them.
- **Switching the embedding model** starts a background re-index into a new `embedding_space`. The old index keeps answering until the new one is ready.
- **Privacy badge:** each chat shows "local" or "remote". The first time document context is about to go to a remote model, RAD warns you.

---

## 8. UI / UX

```
┌──────────────┬──────────────────────────────────────────┬────────────────┐
│ 🔒 Personal ▾│  Chat: "Tax 2025 questions"   [Llama3.1▾]│ SOURCES        │
│ PROJECTS     │  Scope: [All projects ▾]  ● local        │ [1] W2.pdf p1  │
│ ● All        │ ─────────────────────────────────────────│ [2] receipt.png│
│ ▸ Taxes      │  you: what did I pay for the roof?       │ [3] notes.md   │
│ ▸ House      │  RAD: You paid $12,400 [2], invoice      │                │
│ ▸ Work       │       dated 2025-05-03 [1]…              │ preview pane   │
│ + New        │                                          │ (pdf/image/md) │
│ CHATS        │ ─────────────────────────────────────────│                │
│ ▸ Tax 2025…  │ [📎][🖼 paste][📷]  Ask anything…  [Send] │                │
└──────────────┴──────────────────────────────────────────┴────────────────┘
```

- **Unlock screen:**
  - A centred card with the vault name and location ("Personal · on rad.home.lan" or "Personal · /Volumes/USB/personal.radvault").
  - A passphrase field and an Unlock button.
  - A "Switch vault" link.
  - Small animated progress while the key is derived (about 1 s).
  - It shakes gently on a wrong passphrase, with rate-limiting after repeated failures.
- **Vault switcher:** the top-left of the sidebar shows the vault name and lock state, and lists recent vaults and servers.
- **Project view:**
  - File grid or list with status badges (queued, processing, ready, error), and a drag-and-drop area.
  - An image gallery and a search box.
  - Tag and type filters.
- **Global chat:**
  - A scope picker that allows several selections, and `@` autocomplete.
  - A model picker.
  - Stop, regenerate and edit-and-resend.
  - Export to Markdown.
- **Settings:** Storage/Vault, Devices, Providers, Models, Retrieval, Appearance, Hotkeys, Backup.
- **Command palette:** `⌘K`, keyboard-first.

### 8a. Visual design ("make it beautiful")
- **Design language:** calm, spacious and content-first, in the spirit of Linear, Raycast and Arc.
  - Neutral surfaces, with one accent colour you can change (default indigo/violet).
  - Each project has its own colour dot.
- **Design tokens:** CSS variables for light, dark and OLED-black themes. Uses an 8-pt grid, 12 px radii and soft layered shadows.
- **Typography:** Inter for the UI and JetBrains Mono for code, both bundled. Chat text is 15–16 px with 1.6 line height and lines of about 72 characters at most.
- **Native feel:**
  - macOS: vibrancy (`window-vibrancy`) with the window buttons inset into the content.
  - Windows 11: Mica or Acrylic.
  - Linux: solid surfaces.
  - Native context menus.
- **Motion:** 150–200 ms ease-out transitions, a smooth fade as tokens stream in, skeleton loaders, and a shimmer while files process. Respects `prefers-reduced-motion`.
- **Chat details:**
  - A soft bubble for RAD's replies and an accent-tinted bubble for yours.
  - Citation pills with a hover preview card.
  - Code blocks with a copy button.
  - Proper tables.
  - An image lightbox.
- **Onboarding:** illustrated first-run steps:
  1. Create, open or connect a vault.
  2. Set a passphrase and save the recovery kit.
  3. Connect Ollama.
  4. Create the first project.
  5. Drop in files.
- **Drag-and-drop overlay:** a full-window frosted panel that says "Drop to add to *Project*".
- **Accessibility:** AA contrast in both themes, visible focus rings, full keyboard navigation and screen-reader labels.
- **Process:** a component gallery (`/dev/gallery`) is built in M0, and every component is reviewed in both themes before screens are assembled.

### 8b. Tray & background mode
- **Closing the window hides it to the tray** instead of quitting. To quit, use the tray menu or `⌘Q`/`Ctrl+Q`. A one-time hint explains this.
- **Tray icon** (a monochrome template icon on macOS) shows badges for: locked or unlocked, server and Ollama status, ingestion in progress, and outbox items waiting.
- **Tray menu:** Quick Ask… · Capture screenshot → (projects) · Recent chats ▸ · Open RAD · **Lock RAD** · Pause ingestion · Server/Ollama status · Settings · Quit.
- **Clicking the tray icon** opens the **Quick Ask popover**: frameless and anchored to the tray icon. It streams an answer with sources and has "Open in full chat". If the vault is locked, it shows the compact unlock field first.
- **Global hotkeys** (configurable): Quick Ask `⌥Space`, Screenshot to RAD `⌥⇧S`, Paste to RAD `⌥⇧V`, Lock `⌥⇧L`.
- **Launch at login** starts RAD hidden in the tray and *locked*. It unlocks the first time you use it.
- **Single instance:** files dropped on the dock or taskbar icon, or opened with "Open with RAD", go to the running instance.
- **Background work:**
  - The WebSocket and the outbox keep working while the window is hidden, and ingestion keeps running.
  - Notifications arrive when a batch finishes.
  - On macOS, there is an option to hide the dock icon.
- **Idle footprint:** the webview is suspended. The target is under 150 MB RAM.
- **Linux:** the tray uses AppIndicator. GNOME needs the extension; without a tray, RAD falls back to a normal window.

---

## 9. Security and privacy

- **At rest:** everything, including original files, thumbnails, embeddings, chats and API keys, is inside the SQLCipher vault, encrypted with AES-256 and authenticated with HMAC on every page.
- **No plain-text leaks on disk:**
  - Extraction happens in memory.
  - There is no plain-text cache on the client.
  - Logs are scrubbed of content.
  - The outbox is encrypted.
  - Crash reports are off.
- **In memory:** keys are held in `secrecy`/`zeroize` containers and wiped on lock. The client UI state is cleared on lock.
- **Passphrase:**
  - Argon2id key derivation, tuned to the device.
  - A strength meter and a 12-character minimum.
  - Unlock attempts are rate-limited, with exponential backoff on the server.
- **Server mode:**
  - **Device pairing:** a one-time pairing code or QR code gives the device a token. Tokens are stored hashed on the server, and you can list and revoke devices.
  - **Passphrase on every app start:** a device token alone is not enough.
  - **TLS always on:** built-in `rustls` + ACME, or Caddy, or **Tailscale** (recommended for reaching the server from outside home).
  - **API limits:** auth endpoints are rate-limited, CORS is locked to the app, and uploads have size limits.
  - **Vault key lives in RAM only:** the server never writes the vault key to disk, and it starts locked after every reboot.
- **Data sent to models:** Ollama receives plain text, because that is how it works. That's another reason to run Ollama on the same machine as rad-server or on the LAN. Remote models are flagged in the UI.
- **Tauri hardening:** a capability allow-list, no shell access, and a strict CSP.
- **Parser limits:** size and depth limits defend against zip bombs and oversized files.

---

## 10. Roadmap / milestones

**M0 — Skeleton + vault (weeks 1–2)**
- Cargo workspace, Tauri 2 + Svelte shell, `tracing`, device-local config.
- **`rad-vault` spike first:**
  - Confirm that SQLCipher + FTS5 + `sqlite-vec` run together in one encrypted file on mac, Windows and Linux.
  - Confirm that the Argon2id raw key + salt combination works.
  - Benchmark blob chunk read and write speed.
- Vault create/open/lock/rekey, unlock screen, auto-lock, recovery kit.
- Design tokens, themes, component gallery, app shell.
- Tray, close-to-tray, single-instance, autostart (starts locked).
- Ollama profile + health check. Streaming chat stored in the vault (no RAG yet).
- CI: fmt, clippy and tests on all three OSes.

**M1 — Projects + basic RAG (weeks 3–4)**
- Projects CRUD. Drag-and-drop and file picker → blobs streamed into the vault with dedup. Job queue.
- Extractors: txt/md/code, PDF, DOCX.
- Chunking, Ollama embeddings, `sqlite-vec`.
- Global chat with scope, citations and history.
- `rad-cli vault|ingest|search|ask`.

**M2 — Server mode & remote access (weeks 5–6)**
- `rad-server`: starts locked; unlock and lock API; device pairing; REST + SSE + WebSocket; `change_log` sync.
- `HttpBackend` in the app. Resumable deduplicated uploads. Encrypted outbox.
- Network-file mode with an exclusive lease. Detection of cloud-sync folders.
- Docker image + `docker-compose.yml` (rad-server + Ollama). Tailscale and Caddy guide.
- Test: chat on the Mac, watch it appear live on Windows and Linux.

**M3 — Retrieval quality (week 7)**
- FTS5 + RRF, fastembed + reranker, query rewrite, MMR, neighbour merging.
- Source preview pane (PDF pages streamed from the vault).
- `eval/` golden set + `rad-cli eval`.

**M4 — Images & screenshots (week 8)**
- Paste, drag and drop, screenshot hotkey. OCR, vision captions, thumbnails stored in the vault, gallery.
- Images in chat go to vision models.

**M5 — Formats & providers (week 9)**
- XLSX/CSV, PPTX, HTML, EPUB, EML, ZIP; scanned-PDF OCR.
- OpenAI-compatible, Anthropic and Gemini adapters. Ollama model pull/list UI.
- Embedding-model switching with background re-index.

**M6 — Assistant features & vault tools (weeks 10–11)**
- Memories, Quick Ask polish, notifications, paste-to-RAD.
- Scheduled encrypted backups. Compact, verify and move vault. Devices screen.
- Watch folders (stream into the vault), tags, document versions.

**M7 — Ship it (week 12)**
- Signed and notarized `.dmg`, signed `.msi`/NSIS `.exe`, and `.AppImage`/`.deb`/`.rpm`.
- `rad-server`: multi-arch Docker image (amd64/arm64), static binaries, systemd unit.
- Auto-updater from GitHub Releases. Release pipeline as a GitHub Actions matrix.

**Later**
- Touch ID / Windows Hello unlock (opt-in).
- Agent mode (web search, calendar, email).
- `whisper-rs` audio transcription.
- CLIP image search.
- Mobile web UI served by rad-server.
- Knowledge-graph view.

---

## 11. Testing strategy

- **Unit tests:**
  - Chunker, RRF and MMR, prompt budget.
  - Extractors against fixture files.
  - KDF parameters.
  - Blob chunking and dedup, including round-trips of files ≥ 1 GB.
- **Vault tests:**
  - A wrong passphrase is rejected.
  - Tampering is detected: flip a byte, and the HMAC check fails.
  - Rekey round-trip.
  - Backup + restore.
  - Crash safety: kill -9 during a write, then reopen and run the integrity check.
  - Opening on another OS: CI builds a vault on Linux and opens it on mac and Windows.
- **Lease tests:** two processes race to open the same network-file vault.
- **Leak tests:** after a full ingest, scan the app data dir and the temp dirs for any plain text from the fixtures. The test fails if any is found.
- **API tests:** run rad-server in-process. Pair, unlock, upload, ingest and ask. Check that a second client receives sync events and catches up with `since`.
- **Integration tests:** mock `ChatModel` and `Embedder`, so CI never needs Ollama.
- **Retrieval eval:** `rad-cli eval` tracks recall@10 and MRR.
- **E2E:** `tauri-driver` smoke test, plus a manual check on mac (Apple Silicon), Windows 11 and Ubuntu before each release.

---

## 12. Decisions to confirm

1. **Main access mode:** server mode with the vault on the server (recommended for using RAD from several OSes, even at the same time), or network-file mode (no server, one device at a time).
2. **Server host:** the same machine as Ollama (recommended), a NAS with Docker, or a VPS. **Remote access** through Tailscale (recommended) or a public domain with Caddy.
3. **Auto-lock timeout** (default 30 min). Should RAD also lock on sleep or screen lock? (Default yes.)
4. **Recovery kit** offered at vault creation (recommended), or passphrase only with no recovery possible at all.
5. **UI stack:** Tauri 2 + Svelte (recommended).
6. **Default models:** chat `llama3.1:8b` or `qwen2.5:7b`; embeddings `nomic-embed-text` or `bge-m3` (multilingual); vision `qwen2.5vl:7b`.
7. **Document languages:** if they aren't English-only, use `bge-m3` and OCR language packs.

## 13. Prerequisites

- **This Mac:**
  - Rust 1.97 (installed).
  - Node 20+ and pnpm.
  - Tauri CLI: `cargo install tauri-cli --version "^2"`.
  - Ollama is not needed here.
- **Server box** (ideally the Ollama machine):
  - Docker, or the `rad-server` binary.
  - `OLLAMA_HOST=0.0.0.0:11434 ollama serve`, or keep it on localhost if rad-server runs on the same box.
  - Pull the models: `ollama pull llama3.1:8b && ollama pull nomic-embed-text && ollama pull qwen2.5vl:7b`.
- **Tell me** which machine will host the vault server and Ollama (OS, GPU, whether Docker is available), and M0 will be set up for it.

---

## 14. Implementation status (v0.1, 2026-10-11)

**Built and tested**
- **Server mode:**
  - `rad-server` starts locked.
  - Device pairing; passphrase unlock on every app start.
  - Recovery kit, passphrase change, recovery re-key.
  - Unlock rate limiting, device revocation.
  - Server auto-lock, daily encrypted backups.
  - Integrity check, compaction.
- **Vault:** SQLCipher + FTS5 + `sqlite-vec` in one file; Argon2id raw key reusing the SQLCipher salt; blobs stored as 1 MiB rows with BLAKE3 dedup; tests confirm no plaintext appears in the vault file.
- **Ingestion:**
  - Formats: PDF, DOCX/ODT, PPTX/ODP, XLSX/ODS/CSV, EPUB, HTML, EML, ZIP, Markdown/text/code.
  - Images: vision captioning + transcription of visible text.
  - Audio/video: ffmpeg over a loopback endpoint, then Whisper-style transcription with timestamps.
  - Notes: create, paste, edit; re-indexed on save.
- **RAG:** hybrid BM25 + vectors, RRF, MMR, neighbour merging, query rewriting, `@project` scoping, memories ("remember that…"), citations kept only when the answer uses them.
- **Providers:** Ollama (including pulling models), any OpenAI-compatible API, Anthropic.
- **Desktop app (Tauri 2 + Svelte 5):**
  - Onboarding, unlock and recovery screens.
  - Chat with streaming, citations, sources panel, PDF/image/audio/video previews.
  - Projects with files, notes, images and media; drag-drop, paste, screenshots.
  - ⌘K palette, Settings.
  - Tray with Quick Ask, global hotkeys, close-to-tray, launch at login, single instance.
  - Idle and sleep auto-lock, live sync across devices (SSE).
- **Verified end to end:**
  - Against LM Studio (gemma-4-e4b + nomic-embed): curl smoke tests and a headless walkthrough of the real UI.
  - The macOS release `.app` builds.

**Added after v0.1: standalone mode**
- The desktop app embeds `rad-server` as a library (`rad_server::embedded::Embedded`). It serves on a random loopback port with an in-memory device token, so all features use the same code path as server mode.
- You choose the vault file (create or open, with a recent-vaults list). Locking or quitting drops the key and checkpoints the vault back into a single file.
- Covered by `crates/rad-server/tests/standalone.rs`: create, index, search, a real RAG answer when LM Studio is running, stop, reopen locked, unlock, lock.

**Not yet / deferred**
- Docker image written but not built here: the Docker daemon wasn't running.
- Windows/Linux builds are wired in CI but haven't been run yet.
- Code signing and notarization need your Apple and Windows certificates.
- OCR for scanned PDFs that have no text layer.
- Cross-encoder reranking (fastembed).
- Resumable chunked uploads.
- Persistent offline outbox: uploads currently fail if the server is unreachable.
- Touch ID / Windows Hello unlock.
- Watch folders.
- Region selection for screenshots on Windows/Linux: full screen only for now; macOS uses the native region tool.
- Video frame captioning; video is indexed by its transcript only.
