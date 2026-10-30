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

## Web browser

Click **Web Browser** in the sidebar (or pick it in <kbd>⌘K</kbd>) for a full browser inside RAD, built on the [Servo](https://servo.org) engine. It has tabs, an address bar that also searches, back/forward/reload, zoom, page dialogs and `<select>` menus. The chat sits at the bottom:

- **Ask about the page.** Each question carries the page's text, links and your selection (**Page text**).
- **Vision.** With **Screenshot: auto**, the vision model also gets a screenshot whenever the question is about how the page looks, or the page has little text (charts, image galleries, canvas apps). Use **always** or **off** to override.
- **Vault.** Switch **Page only** to **Vault** to also search your own documents.
- **Keep it.** The camera button saves a screenshot to your vault, where the vision model describes it for search. The download button saves the page's text as a Markdown document.

Notes:
- Links in answers open in the RAD browser.
- The page context goes to the model with the question but isn't stored in the chat.
- Browsing data (cookies, storage, cache) is kept in memory only and is gone when RAD quits.
- Servo is still pre-1.0, so some sites don't render perfectly yet.
- The engine runs as a separate process (`rad-browser`) because Servo pins its own SQLite and ICU, which clash with the vault's. A page that crashes it can't take the vault down with it.

The ☆ in the address bar adds a page to **Favourites**, which are kept encrypted in your vault and shown under the toolbar. Right-click a favourite to remove it. The chat collapses to a single line (⌄), and − / + change its height.

Build the engine once with `./scripts/build-browser.sh`. The first build compiles Servo and takes a few minutes. `cargo tauri dev` then finds it automatically; `./scripts/build-app.sh` puts it inside the installable app. Note: Servo pins `webrender 0.70`, which no longer compiles on rustc 1.86+; RAD builds a one-line-patched copy from `browser/vendor/webrender` via a `[patch.crates-io]` override.

## Tor browser

**TOR** in the sidebar is the same Servo browser, but every page is routed through the Tor network by an embedded [Arti](https://blog.torproject.org/arti_2_0_0_released/) client (Tor in Rust). The engine runs its own HTTP CONNECT proxy on `127.0.0.1` (Servo's proxy connector only speaks HTTP CONNECT), points Servo at it, and bootstraps Tor in the background — a note shows the connection progress. If Tor can't start or connect, pages simply don't load; the browser never falls back to the open network.

- No browsing data is kept: Tor's state and caches live in temporary folders that are deleted when RAD quits.
- The Tor engine is a separate process (`rad-tor-browser`), so it can't take the vault down with it.
- Like Servo, it's still pre-1.0 — some sites don't render perfectly yet, and it can't use RAD's VPN (it has its own tunnel).

Build it with `./scripts/build-tor-browser.sh` (this also compiles Arti, so the first build is longer than the plain browser). `./scripts/build-app.sh` bundles it inside the app; set `RAD_NO_TOR=1` to skip it. Linux builds need OpenSSL development headers for Arti's TLS.

## Terminal, Files and actions from chat

- **Terminal.** Local shells and SSH sessions in tabs, built on xterm.js (the terminal component VS Code uses) and a real pseudo-terminal. Sessions keep running, with their scrollback, while you look at something else. **SSH…** connects with a host, user, port and key file. Recent servers are kept in your vault; passwords are typed into the session and never stored.
- **Files.** Browse this computer, preview images and text, then **Add to vault** (pick a project) or **Ask AI about these**, which attaches them to a new chat.
- **Actions from any chat.** Ask “open ssh to coursemat.au”, “open servo.org” or “open a terminal” in any chat, including Quick Ask and the browser's chat, and RAD does it.
  - For SSH, RAD searches your vault for the host's details (user, port, key file), opens the session in **Terminal**, cites the note it used, and remembers the details as a long-term memory, so next time no search is needed.
  - Host and user names are validated, and SSH is started directly, never through a shell.
- **Hide the sidebar** with ⌘\ for a full-width view.

## RYMNET (Rymnet and RYMWIN)

[Rymnet](rymnet/README.md), the encrypted, decentralised peer-to-peer network, is part of RAD. Its source is in `rymnet/` (its own Cargo workspace), and **RYMNET** in the sidebar drives it:

- **RYMWIN** (sidebar, or **Start RYMWIN**) runs the Rymnet Desktop inside RAD, with its own login. `rymwin --embed` renders off-screen with wgpu and streams frames to RAD over the same pipe protocol as the browser engines; RAD sends back mouse, keyboard, clipboard and file drops.
- **Files into RYMWIN.** Drag files or folders from Finder onto it, use **Send to RYMWIN** in Files, or **Add files / From vault** in its toolbar. They are copied into the account's `/Downloads`.
- **Initiate RYMWIN** joins this computer to the network. It creates the node identity if needed, starts the peer daemon (`rymnetd`, stopped when RAD quits), and connects to the node address you type in, along with the peers that node shares. It then opens **rymsh**, the Rymnet shell, as a Terminal tab. The page also lists your peers, the node ID and the daemon's log.
- **Assistant.** A chat that knows the RYMWIN User Guide and runs `rym` commands for you: peers, domains, publishing sites, search. It never runs a shell. Commands that remove or undo something wait for your OK.
- **User guide in the vault.** The first time you open RYMNET, [the RYMWIN User Guide](rymnet/docs/RYMWIN_USER_GUIDE.md) is filed as a note in a **Rymnet** project, so every chat can answer from it. A newer guide in a new RAD replaces it.

Build the Rymnet programs with `./scripts/build-rymnet.sh`. `cargo tauri dev` finds them in `rymnet/target/release`, and `./scripts/build-app.sh` puts them inside the app.

## VPN (WireGuard inside RAD)

The **VPN** switch in the top bar sends RAD's own traffic through a WireGuard tunnel to your server. It needs no administrator rights and installs nothing: `crates/rad-tunnel` runs WireGuard ([boringtun](https://github.com/cloudflare/boringtun)) and its own TCP/IP stack ([smoltcp](https://github.com/smoltcp-rs/smoltcp)) inside RAD, behind a SOCKS5 proxy on 127.0.0.1. The same code runs on macOS, Windows and Linux.

- **Set up** in **Settings → VPN**. Either create a key there (RAD shows the `[Peer]` lines for your server) or import the `wg-quick` `.conf` file from your server. Keys are kept in the OS keychain.
- **Choose what goes through it:**
  - **RAD & AI:** your RAD server, AI providers, model and voice downloads
  - **Chromium:** pages and name lookups (it restarts when the VPN changes)
  - **Email**
  - **SSH terminals** (on Linux this needs `netcat-openbsd`)
  - **Rymnet** peer connections, via `RYMNET_SOCKS`

  This computer and your local network always stay direct. Servo can't use a proxy, so it shows a note when it isn't covered.
- **No silent fallback:** while the VPN is on, covered connections go through the tunnel or fail; they never fall back to the open network. Names are looked up through the tunnel's DNS too.
- **Server:** a standard WireGuard server, for example Ubuntu with `wg-quick` on UDP 51820 and `unbound` answering DNS on the tunnel address.

## Voice (text to speech)

**Settings → Voice** downloads **MioTTS-0.1B** (about 510 MB, checksum-verified), which then reads answers aloud on this computer, in English and Japanese.

- **Voices:** four preset voices, or your own voice made from a recording of up to 30 s, recorded right there under **Your own voice** or chosen from a file (this needs an extra 94 MB WavLM model).
- **Parameters:** speed, volume, expressiveness (temperature), top-p, top-k and repetition penalty.
- **Reading:** a speaker button appears on every answer, or turn on **Read answers aloud automatically**. Long answers are read sentence by sentence while the next one is prepared.
- **Privacy:** audio is made in memory and never written to disk.

The engine is [mio-tts-cpp](https://github.com/mmnga/mio-tts-cpp) (MIT; llama.cpp + ggml). Build it once with `./scripts/build-tts.sh`.

Licences: MioTTS-0.1B is under the Falcon-LLM licence, MioCodec is MIT, and WavLM is CC BY-SA 3.0.

## Gilgamesh (talk face to face)

**Gilgamesh** in the sidebar is a spoken conversation with RAD through a talking head. Choose one or more photos of faces, press the microphone, and talk.

- **The face.** The photo becomes a talking head: lips follow the speech, the eyes blink and glance, the head moves slightly and breathes, and the face shows a mood (happy, amused, concerned, surprised, thinking). The answer chooses the mood with a tag such as `[happy]`, which is never shown or read aloud. A clear, front-facing portrait with closed lips works best.
- **Listening.** Speech is recognised on this computer by **NVIDIA Parakeet TDT 0.6B v3** (25 European languages, with punctuation; about 670 MB, downloaded once from the Gilgamesh view and checksum-verified). **Silero VAD** finds speech, and live captions show what has been heard. The turn ends after a pause, which is shorter when your words end a sentence.
- **Talking.** The answer comes from the normal RAD chat: Gilgamesh searches your vault and shows the sources he used under his answer (click one to open it), and knows your long-term memories. Each sentence is spoken with MioTTS as soon as it's written; the voice is chosen in Gilgamesh's settings.
- **Memory.** Every conversation is kept (**Earlier** in the side panel), and you can continue any of them or delete it. When a conversation ends, Gilgamesh writes himself a short recap; his recent recaps, the date and your next week's calendar go into every answer. When you tell him something worth knowing, he saves it to RAD's memories (**Settings → Memories**), which every chat uses.
- **Doing things.** He can save a fact or a note (in the Inbox project), add an event to the calendar, open part of RAD or a web page, search the web, play, pause or pick music, and give your **Bots** tasks (and pass on your OK or answers to them). Each action shows under his answer. The SSH, browser and terminal actions from chat work too. Anything that leaves the view waits until he has finished speaking, and is cancelled if you interrupt him.
- **Expressions.** He has 29 moods: neutral, happy, amused, excited, concerned, sad, surprised, curious, skeptical, thinking, playful, sympathetic, serious, proud, angry, annoyed, disgusted, worried, scared, confused, shy, smug, bored, sleepy, relieved, determined, tender, mischievous and disappointed. Each one shows on the face (brows, lids, lips, a blush on the cheeks) and in how the head is held, and some start with a gesture of their own. His 15 gestures are nod, shake, tilt, wink, laugh, eye-roll, sigh, gasp, smirk, eyebrow flash, yawn, blush, shrug, look away and bow. He gives small nods while you talk, lifts his brows or moves his head on stressed words, shifts his posture now and then, makes small expressions when idle, and his eyes follow the mouse pointer. Try them under **Settings → Moods / Gestures** in the view.
- **Faces.** Give him several faces: choose more than one photo, or drop photos onto the view (up to 24). Switch between them from the strip at the bottom of the picture, or rename and remove them in settings. With **A new face for each conversation** turned on, each new conversation brings the next face. Each conversation remembers the face it was held with.
- **Everywhere.** Gilgamesh's face sits in the sidebar, under the search box, with his microphone, a stop button and a box to type to him, so you can talk with him from any view. In push-to-talk mode, hold **Talk** there. The eye button hides his face and keeps the controls. The microphone stays on as you move between views; it goes off when you turn it off or lock the vault. While the sidebar is hidden, a red microphone button in the corner shows that he's listening.
- **Transcripts.** Press **Transcripts** and **Start transcribing**: Gilgamesh listens without answering and writes down what's said, line by line with times (a meeting, a call, a lecture). Each transcript is saved as it goes, as a note in the **Transcripts** project, so the vault indexes it and every chat can find it. Open one to read it, **Summarise** it (overview, key points, decisions and actions; added to the note), or **Discuss** it: a conversation with Gilgamesh with the transcript in his context. He also knows your recent transcripts, so “what did we decide yesterday?” works.
- **Websites.** Gilgamesh checks the sites in **Web Monitoring** when RAD starts and every three hours, and says out loud what's offline, back online, or whose certificate has a problem or expires within 14 days (each problem once, not at every check; at start-up, a reminder of everything still wrong). Ask him “how are my websites?” and he answers from the latest check, or “check my websites” for a fresh one.
- **Watching.** Gilgamesh knows which part of RAD you have open and where you were before. The browser tells him the page, Files the folder and selection, and Email the folder and the email you're reading (its subject and sender, not its text). That goes into every answer, so "what's this page about?" or "help me with this" works.
- **Sight.** Turn on his sight (settings → **Sight**: off, while he's on, or **Watch for me**) and he sees you through the camera: his eyes and head follow your face, he knows when you're there, when you've gone and when you come back (and says hello after ten minutes or more away, with what your bots did and what's next in your calendar), and smiles back. **Teach him your face** (a few seconds in front of the camera) and he recognises you: with **Only answer when I look at him**, talking to someone else in the room doesn't set him off, and someone he doesn't know alone at the computer can make him go quiet or lock RAD. Ask him to look at something ("what am I holding?") and he takes one picture and asks the chat model. A camera button in the sidebar shows he can see; click it to turn his sight off. Everything runs on this computer: MediaPipe (the same model that animates his photo) tracks the face, and **SFace** (OpenCV Zoo, Apache-2.0, 39 MB, downloaded once and checksum-verified) recognises it, run in Rust with tract (`app/src-tauri/src/faceid.rs`). Only 128 numbers per look at your face are kept, in the vault; never a picture. Recognising a face is for being friendly, not security: a photo can fool it.
- **Turn-taking.** In **Hands-free** mode, talking over Gilgamesh stops him when **I'm using headphones** is on. Through speakers the microphone is ignored while he speaks, so he doesn't hear himself; <kbd>Esc</kbd> stops him. In **Push to talk** mode, hold <kbd>Space</kbd> while you talk.
- **Privacy.** The photo is processed on this computer and kept only in the vault. Audio stays in memory, and the microphone is released when you leave the view.

How it works:
- **Face:** MediaPipe Face Landmarker, bundled and run locally, finds 478 face landmarks. The face is then animated with WebGL as a triangle mesh over the photo (`app/ui/src/lib/gilgamesh/`). The inside of the mouth is drawn where the lips part.
- **Tags:** answers carry tags in square brackets, which are acted on and never shown or read: a mood (`[happy]`), a gesture (`[nod]`) or an action (`[remember: …]`, `[note: title | text]`, `[event: YYYY-MM-DD HH:MM | title]`, `[open: …]`, `[search: …]`, `[music: …]`, `[bot: Name | task]`). See `text.ts` and `tools.ts`.
- **Lips:** they are driven by analysing each sentence's audio before it plays: loudness and the balance of formant bands give jaw, “ee” and “oo” shapes, and the mouth moves slightly ahead of the sound.
- **Speech to text:** this runs in the app (`app/src-tauri/src/stt.rs`) on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx), which is linked statically. The microphone is read with cpal.

Licences: Parakeet TDT 0.6B v3 is CC BY 4.0, Silero VAD is MIT, and sherpa-onnx and MediaPipe are Apache-2.0.

## Web Monitoring

**Web Monitoring** in the sidebar watches your websites. Add an address (e.g. `https://booksia.com.au`) and each site gets a card:

- **Status:** online or offline, the HTTP status and response time, where it redirects to, and a strip of the last 48 checks with an uptime percentage.
- **IP addresses:** from DNS, IPv4 first; click one to copy it.
- **Certificate:** expiry date and days left (amber within 14 days, red when expired), the issuer, the TLS version, and any problem: expired, wrong name, untrusted issuer, revoked. An invalid certificate is still read, so its dates are shown.
- **Thumbnail:** a picture of the first screen, taken by a Chromium engine of its own with a separate, throwaway profile (Servo if Chromium isn't built), so your tabs and sign-ins aren't touched. It's taken again after a day. Click it to open the site in RAD's Chromium.
- **SSH:** add the server's SSH details and a button on the card opens the connection in the Terminal.
- **Notes:** logins and anything else, blurred on the card until you show them, with a copy button.

Checks run when RAD starts, every three hours, and from **Check now**. Sites, SSH details and notes are kept in the encrypted vault as app data, not as notes, so credentials never appear in search or chat answers. The speaker button in the header turns Gilgamesh's spoken alerts on or off. The checks and thumbnails are in `app/src-tauri/src/monitor.rs`; the scheduling and alerts are in `app/ui/src/lib/monitor.svelte.ts`.

## Bots

**Bots** in the sidebar are assistants that do things for you on a computer of their own. Make a bot, give it a name and a role ("shopping and bookings", "keeps an eye on my supplier portals"), and give it tasks: it browses, searches, fills in forms, logs in, downloads and reports back, while you do something else.

- **Its computer.** Each bot works in its own Linux VM (a **Desktop** container: Debian with Chromium on a virtual 1280×800 screen). Its drive keeps the browser's logins and the bot's files between tasks; files it makes for you go in `/root/shared`, which is on your Mac. The first bot's computer takes a few minutes to set up (Debian and Chromium are downloaded once); the next ones are instant clones.
- **How it works.** The bot reads the page as text with numbered buttons, links and fields, and answers with one action at a time (open, click, type, choose, scroll, a shell command…). That works with any chat model, local ones included. With **Sees the screen** on, it also gets a screenshot of every page (needs a model that reads images); it can also ask for one, or use the whole screen for windows outside the page.
- **Its screen.** You watch its screen as it works. **Take over** lets you click and type on it yourself: for a CAPTCHA, a code you were sent, or to show it something.
- **Asking first.** Clicks that buy, pay, send, post, delete or sign up, and logging in, wait for your OK (**Allow** / **Don't**), unless you turn that off for the bot. The bot can also ask you a question and wait for the answer.
- **Logins.** **Logins** are kept in the encrypted vault, each for one site and the bots you give it to. The model never sees them: it names a login, RAD writes it into the VM's memory for that one step, and the browser helper types it only on that site (and deletes it at once). A login for `github.com` is never typed anywhere else, whatever a page says.
- **Pages aren't instructions.** Bots are told that pages, emails and files are data, never orders, and to mention any page that tries to give them some. The OK before risky clicks and the site-bound logins hold even if a page fools the model.
- **Memory.** A bot keeps what it learns for later tasks (where things are, your preferences); see and edit it in the bot's settings.
- **Skills.** A skill is a set of instructions a bot can follow. **Save as a skill** after a task turns what worked into one; **Record a skill** turns what you do on a bot's screen into one (passwords aren't recorded); or write one. Run a skill on any bot.
- **Routines.** Run a skill on a bot every hour, every day, on weekdays or once a week, while RAD is open. Gilgamesh can tell you the result.
- **Bots together.** A bot can hand part of a task to another bot and use its answer.
- **Gilgamesh.** Ask him: "get Ada to find the cheapest flight to Perth on Friday". He passes it on, tells you when it's done or when the bot needs your OK, and can pass on your yes, no or answer. He knows what each bot is doing.

The bots and their loop are in `app/ui/src/lib/bots.svelte.ts`; the browser helper that runs in each bot's VM is `sandbox/desktop/rad-browse.py` (the DevTools protocol, and xdotool for the whole screen); the Desktop container is in `app/src-tauri/src/containers.rs`.

## AI API server (share your models)

**Settings → AI API server** shares the models RAD runs with other programs and computers. They connect as if RAD were **OpenAI**, **Anthropic** or **Ollama**, so most AI tools and SDKs work with it unchanged.

- **What's shared:** the downloaded built-in (llama.cpp) chat and embedding models. Optionally, the models of your other providers too, named like `ollama/llama3.2`; their use counts against those accounts. Each model can be shared or hidden.
- **Endpoints**, on their own port (11435 by default):
  - **OpenAI:** `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/models`
  - **Anthropic:** `/v1/messages`, `/v1/messages/count_tokens`
  - **Ollama:** `/api/chat`, `/api/generate`, `/api/embed`, `/api/embeddings`, `/api/tags`, `/api/ps`, `/api/show`, `/api/version`
  - Answers stream as each API does it.
- **Statistics:** `/rad/stats` lists the shared models, which ones are loaded, and their requests, tokens and tokens per second. The same numbers, plus use per key and per API, show in the settings.
- **Access:** only this computer by default, or your local network. Programs send an API key made in the settings (`Authorization: Bearer radai_…` or `x-api-key`). Keys can be made optional for tools that can't send one; web pages on other sites are then turned away.
- **When:** it runs while the vault is unlocked, since its settings and keys, and your providers' credentials, are kept there. It stops when RAD locks, and starts again when RAD unlocks.

Built-in models answer one request at a time. Images work with providers' vision models only. The server is `crates/rad-server/src/aiserver/`.

## Linux sandboxes

**Terminal → Sandboxes…** creates small Linux machines for playing with Python and scripts, isolated from your Mac. They need macOS on Apple Silicon.

- **Distros:**

  | Distro | Size | Package manager |
  |---|---|---|
  | Alpine 3.24 | about 10 MB | `apk add python3` |
  | Debian 13 | about 450 MB | `apt install python3` |
  | Arch Linux ARM | about 900 MB | `pacman -S python` |

  You choose CPUs, memory, drive size and whether the sandbox has internet.
- **Opening one:** a sandbox opens as a terminal tab and boots in about a second.
- **Run:** code blocks in chat answers (Python, shell, JavaScript, Ruby, Perl) get a **▶ Run** button that runs them in your sandbox.
- **Files:** **Files → Send to sandbox** copies files in.
- **Reset and delete:** **Reset** returns a sandbox to a fresh system; **Delete** removes it.

**How it's isolated:**
- **The VM:** each sandbox is a [libkrun](https://github.com/containers/libkrun) microVM on Apple's Hypervisor.framework.
- **Its own drive:** it boots from its own ext4 drive, a sparse disk image that only uses the space it fills. That gives real Linux ownership, permissions and case-sensitive names.
- **Files:** it sees nothing of your Mac except its exchange folder (`~/shared` inside, with a Finder shortcut in Sandboxes).
- **Network:** traffic goes through [gvproxy](https://github.com/containers/gvisor-tap-vsock)'s private NAT. The internet is reachable; your Mac's `localhost` (LM Studio, Ollama, dev servers) is not. Other devices on your LAN are reachable, as from any computer.
- **No network:** with internet off, there's no network device at all.
- **Downloads:** distro images are checked against their published SHA-256 checksums. Debian and Arch come from [Linux Containers](https://images.linuxcontainers.org); Alpine is a pinned official release.

Build the engine once with `./scripts/build-sandbox.sh`. It needs `brew install lld llvm` to compile libkrun's guest init.

## Building the app with its engines

`./scripts/build-app.sh` builds the browser, voice and sandbox engines and bundles RAD with all of them inside, signed so the sandbox keeps permission to use the hypervisor. An installed RAD only finds engines bundled inside it, so a plain `cargo tauri build` produces an app without them.

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
cargo tauri build          # .dmg/.app, .msi/.exe, .AppImage/.deb/.rpm — or ./scripts/build-app.sh to include the browser, voice and sandbox engines
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
crates/rad-tunnel   RAD's VPN: WireGuard in user space (boringtun + smoltcp) behind a local SOCKS5 proxy
crates/rad-server   axum server: auth, unlock, uploads, jobs, RAG, live sync (SSE), backups
app/src-tauri       desktop shell: tray, hotkeys, screenshots, rad:// file protocol, keychain
app/ui              Svelte 5 interface
browser/rad-browser web browser engine: Servo rendering offscreen, driven by the app (its own Cargo workspace)
browser/rad-tor-browser the Tor browser engine: the same Servo engine with an embedded Arti (Tor) HTTP CONNECT proxy (its own Cargo workspace)
rymnet/             Rymnet: peer network, rymwin desktop (embedded via --embed), rymsh, rym CLI, rymnetd daemon (its own Cargo workspace); docs/ has the guides
tts/                text-to-speech engine build (mio-tts-cpp, fetched and built by scripts/build-tts.sh)
sandbox/launcher    Linux sandbox launcher (libkrun); scripts/build-sandbox.sh builds it with libkrun and gvproxy
sandbox/desktop     rad-browse.py: a bot's hands in Chromium, inside its desktop VM
deploy/             Dockerfile, docker-compose
config/             Ubuntu deployment: nginx (HTTPS) site, systemd unit + env, certbot hook — see config/README.md
```

Tests: `cargo test -p rad-vault -p rad-ingest -p rad-server` (no model server needed).
