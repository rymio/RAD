RAD by Robert Rymarczyk — your private, encrypted second brain

Licence MIT


RAD is a personal RAG assistant. You put documents, notes, screenshots, images and recordings into **projects**. Then you chat with all of it from one place, and every answer comes with clickable citations.

- **One encrypted vault file.** Everything is stored in a single SQLCipher file (`*.radvault`): metadata, every uploaded file, notes, the search index, chats, memories and API keys. Pages are encrypted with AES-256 and authenticated with HMAC-SHA512, and the key comes from your passphrase through Argon2id. Nothing is readable until you unlock it.
- **Two ways to run.**
  - **Standalone:** no server at all. The app runs the RAD engine itself, on a vault file you choose (local disk, external or USB drive).
  - **Server mode:** the vault lives on a RAD server you own, and the apps on macOS, Windows and Linux are thin clients that all see the same data, live.
  - Both modes run the same code, so every feature works in each. The vault file format is identical, so you can start standalone and move the file to a server later.
- **Any model.** Ollama, LM Studio, OpenAI, Anthropic, OpenRouter, Groq, Gemini, Mistral, or any OpenAI-compatible endpoint. The server is the only thing that talks to them. You choose the model for each role (chat, embeddings, vision, transcription, query rewriting).
- **Built-in models (llama.cpp).** RAD compiles in llama.cpp and ships `nomic-embed-text v1.5` (GGUF, 8-bit, ~146 MB), so semantic search works offline with nothing else installed. Under **Settings → Models → Built-in models** you can also download a light chat model (Qwen3 4B — recommended, Gemma 3 4B, Llama 3.2 3B; 2–2.5 GB each) and answer fully offline. RAD shows whether each one fits your memory before you download. Models are plain files in a local `models` folder (`~/Library/Application Support/com.rymtech.rad/models` on macOS), never inside the vault.
- **Always at hand.** RAD lives in the tray / menu bar.
  - **Quick Ask:** <kbd>⌥ Space</kbd>
  - **Screenshot into the vault:** <kbd>⌥ ⇧ S</kbd>
  - **Lock:** <kbd>⌥ ⇧ L</kbd>
  - It locks itself when idle and when the computer sleeps.

```
 macOS / Windows / Linux app ──HTTPS──▶ rad-server ──▶ personal.radvault (one encrypted file)
   (Tauri + Svelte, tray)              (Rust, axum)  └─▶ Ollama / LM Studio / cloud APIs
```

## What it understands

| Kind | Formats | How |
|---|---|---|
| Documents | PDF, DOCX/ODT, PPTX/ODP, XLSX/XLS/ODS/CSV, EPUB, HTML, RTF, EML (with attachments), ZIP (recursively) | Text extraction with page, slide, sheet and heading anchors |
| Text | Markdown, TXT, code, JSON, YAML, logs… | Heading-aware chunking |
| Notes | Written in RAD or pasted text | Markdown editor with autosave; re-indexed whenever you edit |
| Images & screenshots | PNG, JPG, WebP, GIF, BMP, TIFF | A vision model transcribes the text and describes the image |
| Audio & video | MP3, M4A, WAV, FLAC, OGG, MP4, MOV, MKV, WebM | ffmpeg on the server + Whisper-style transcription; citations jump to the timestamp |
| Anything else | — | Stored encrypted and searchable by name |

**How search works:**
1. Keyword (BM25) and semantic (vector) search run together, and their results are merged with reciprocal-rank fusion.
2. MMR picks a diverse set of passages, and neighbouring chunks are merged.
3. Follow-up questions are rewritten into standalone searches first.

**Ways to steer it:**
- `@Project` in a message limits the search to that project.
- "Remember that…" saves a long-term memory.

## Quick start

### Standalone (no server)

1. Build and open the app (see step 2 below).
2. Choose **Standalone → Create vault…** and pick where the `.radvault` file goes.
3. Set a passphrase and save the recovery kit.
4. Pick a chat model under **Settings → Models**: download a built-in one (Qwen3 4B is a good default), or add Ollama, LM Studio or a cloud provider. Semantic search already works through the built-in embedding model.

Notes:
- **Ports:** in standalone mode RAD serves itself on a random `127.0.0.1` port that only the app can use. Nothing is opened to the network.
- **Audio/video:** transcription needs `ffmpeg` installed (`brew install ffmpeg`, `winget install ffmpeg`, `apt install ffmpeg`).

### Server mode

### 1. Run the server

On a home server, a NAS, or the machine that runs Ollama:

```bash
# Docker (includes ffmpeg)
docker compose -f deploy/docker-compose.yml up -d
docker compose -f deploy/docker-compose.yml logs rad      # shows the pairing code

# …or natively (needs cmake and clang/libclang: llama.cpp is compiled in)
./scripts/fetch-models.sh                                   # optional: built-in model into ./models
cargo build --release -p rad-server
RAD_VAULT=~/rad/personal.radvault ./target/release/rad-server
```

For a permanent install on Ubuntu behind nginx with HTTPS and systemd, follow **[config/README.md](config/README.md)**.

The server prints its address and a **one-time pairing code**, valid for 15 minutes. More codes are available in the app under **Settings → Devices**.

Useful options (flags or environment variables):

| Variable | Default | |
|---|---|---|
| `RAD_VAULT` | `./data/personal.radvault` | The vault file; created from the app on first use |
| `RAD_LISTEN` | `0.0.0.0:8765` | |
| `RAD_BACKUP_DIR` | `<vault dir>/backups` | Daily encrypted backups, safe to sync to the cloud |
| `RAD_TLS_CERT` / `RAD_TLS_KEY` | — | Serve HTTPS directly |
| `RAD_FFMPEG` | `ffmpeg` | Needed for audio/video |
| `RAD_MAX_UPLOAD_MB` | `8192` | |
| `RAD_MODELS_DIR` | `<vault dir>/models` | Built-in GGUF models; the embedding model is downloaded (and checksum-verified) here on first start if missing |
| `RAD_BUNDLED_MODELS_DIR` | — | Read-only folder checked first (the Docker image ships the model in `/opt/rad/models`) |

CLI helpers:

- `rad-server init` creates a vault from the terminal and prints its recovery kit.
- `rad-server verify` checks the vault's encryption and integrity.
- `rad-server backup --to file.radvault` writes an encrypted backup.

### 2. Install and connect the app

```bash
./scripts/fetch-models.sh  # built-in embedding model → models/ (bundled into the app; git-ignored)
cd app/ui && npm ci && cd ..
cargo tauri build          # .dmg/.app, .msi/.exe, .AppImage/.deb/.rpm
# development: cargo tauri dev
```

Open RAD, then:
1. Enter the server address.
2. Enter the pairing code.
3. Create your vault passphrase.
4. **Save the recovery kit.**

On your other computers, pair them and unlock with the same passphrase.

### 3. Add models

**Settings → Models → Add provider.** Then pick a model for each role:
- **Chat**
- **Embeddings**, e.g. `nomic-embed-text`, `bge-m3`, `text-embedding-3-small`
- **Vision**, e.g. `qwen2.5vl`, `gemma3/4`, `gpt-4o`, Claude
- **Transcription**, e.g. `whisper-1` or Groq's `whisper-large-v3`

`localhost` in a provider URL means **the server machine**. From Docker, use `http://host.docker.internal:11434` (Ollama) or `:1234/v1` (LM Studio).

## Security model

- **Unlocking.** The server starts **locked** and keeps the vault key only in RAM after someone unlocks it. Every app start asks for the passphrase: a paired device token proves *which device*, and the passphrase proves *it's you*.
- **Recovery kit.** This is a printable raw key, offered when you create the vault. Changing the passphrase re-encrypts the vault and issues a new kit; the old kit then stops working. **If you lose both the passphrase and the kit, your data cannot be recovered.**
- **Nothing readable on disk.**
  - Server side: files are streamed into the vault in 1 MiB pieces, and extraction happens in memory. ffmpeg reads media over a loopback-only endpoint, so decrypted media never lands on disk.
  - Client side: no plaintext cache, and the device token sits in the OS keychain.
  - The test suite checks the vault file for leaked plaintext.
- **Rate limits.** Unlock attempts are rate-limited, and devices can be revoked under **Settings → Devices**.
- **Don't put the live vault in Dropbox, iCloud or OneDrive.** Sync tools corrupt live databases. Backups are fine there.
- **Remote access.** Install **Tailscale** on the server and your computers, then connect to `http://<server>.<tailnet>.ts.net:8765`, or run `tailscale serve` for HTTPS. No port forwarding needed.

## Repository layout

```
crates/rad-api      wire types shared by server and app
crates/rad-vault    encrypted single-file vault: KDF, recovery kit, blobs, schema, FTS5 + sqlite-vec search
crates/rad-ingest   extractors (PDF, Office, email, zip…) and the chunker — all in memory
crates/rad-llm      Ollama / OpenAI-compatible / Anthropic adapters: chat, embeddings, vision, transcription
crates/rad-server   axum server: auth, unlock, uploads, jobs, RAG, live sync (SSE), backups
app/src-tauri       desktop shell: tray, hotkeys, screenshots, rad:// file protocol, keychain
app/ui              Svelte 5 interface
deploy/             Dockerfile, docker-compose
config/             Ubuntu deployment: nginx (HTTPS) site, systemd unit + env, certbot hook — see config/README.md
```

Tests: `cargo test -p rad-vault -p rad-ingest -p rad-server` (no model server needed).
