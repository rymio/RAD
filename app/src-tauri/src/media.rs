//! The Music and Video players. Playback runs in the media engine (`rad-media`, libVLC, see
//! `media/rad-media`), one process per player, so RAD itself never links libVLC. Pictures come
//! back as frames, like the browsers', and are drawn by the view.
//!
//! Vault files are streamed to the engine through a loopback HTTP proxy: each file gets an
//! unguessable address that forwards (with Range, for seeking) to the server with this device's
//! credentials, so nothing decrypted is written to disk.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::{json, Value};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::client::{CmdError, CmdResult};

const BIN: &str = if cfg!(windows) { "rad-media.exe" } else { "rad-media" };

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Music,
    Video,
}

impl Kind {
    fn parse(s: &str) -> Kind {
        if s == "video" { Kind::Video } else { Kind::Music }
    }
    fn name(self) -> &'static str {
        match self {
            Kind::Music => "music",
            Kind::Video => "video",
        }
    }
}

#[derive(Default)]
struct Frame {
    w: u32,
    h: u32,
    px: Vec<u8>,
    fresh: bool,
}

struct Engine {
    child: Child,
    stdin: ChildStdin,
}

pub struct Player {
    kind: Kind,
    engine: Mutex<Option<Engine>>,
    frame: Arc<Mutex<Frame>>,
    frame_signalled: Arc<AtomicBool>,
    /// The latest of each event type (`ready`, `status`, `meta`, `tracks`), replayed when the
    /// view is opened again.
    last: Arc<Mutex<HashMap<String, Value>>>,
}

pub struct Media {
    music: Player,
    video: Player,
    proxy: tokio::sync::Mutex<Option<Proxy>>,
}

impl Default for Media {
    fn default() -> Self {
        Media { music: Player::new(Kind::Music), video: Player::new(Kind::Video), proxy: tokio::sync::Mutex::new(None) }
    }
}

impl Media {
    fn get(&self, player: &str) -> &Player {
        match Kind::parse(player) {
            Kind::Music => &self.music,
            Kind::Video => &self.video,
        }
    }

    pub fn stop(&self) {
        self.music.stop();
        self.video.stop();
    }
}

impl Player {
    fn new(kind: Kind) -> Player {
        Player { kind, engine: Mutex::new(None), frame: Arc::default(), frame_signalled: Arc::default(), last: Arc::default() }
    }

    fn stop(&self) {
        if let Some(mut e) = self.engine.lock().unwrap().take() {
            let _ = e.child.kill();
            let _ = e.child.wait();
        }
    }

    fn running(&self) -> bool {
        let mut g = self.engine.lock().unwrap();
        let alive = g.as_mut().is_some_and(|e| matches!(e.child.try_wait(), Ok(None)));
        if !alive {
            *g = None;
        }
        alive
    }

    fn send(&self, cmd: &Value) -> CmdResult<()> {
        let mut g = self.engine.lock().unwrap();
        let e = g.as_mut().ok_or_else(|| CmdError::new("media_stopped", "The media engine isn't running"))?;
        let mut line = serde_json::to_vec(cmd)?;
        line.push(b'\n');
        e.stdin.write_all(&line).and_then(|_| e.stdin.flush()).map_err(|e| CmdError::new("media_stopped", format!("The media engine stopped: {e}")))
    }
}

/// The engine is a folder: the executable with libVLC (`lib/`) and its plugins (`plugins/`)
/// next to it — in the app bundle (macOS `Contents/Helpers`, elsewhere the resources) or, in
/// development, where `scripts/build-media.sh` lays it out. `RAD_MEDIA_BIN` overrides these.
fn engine_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_MEDIA_BIN") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join("../Helpers/rad-media").join(BIN));
        candidates.push(dir.join("rad-media").join(BIN));
    }
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("rad-media").join(BIN));
    }
    if cfg!(debug_assertions) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../media/rad-media/target");
        candidates.push(root.join("bundle/rad-media").join(BIN));
        // A plain `cargo build` of the engine: libVLC from an installed VLC.
        candidates.push(root.join("release").join(BIN));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn spawn(app: &AppHandle, p: &Player) -> CmdResult<()> {
    let path = engine_path(app).ok_or_else(|| {
        CmdError::new(
            "media_missing",
            if cfg!(debug_assertions) {
                "The media engine isn't built yet. Install VLC, run ./scripts/build-media.sh, then reopen this view."
            } else {
                "This copy of RAD was built without the media engine. Rebuild it with ./scripts/build-app.sh and reinstall."
            },
        )
    })?;
    let mode = if p.kind == Kind::Video { "video" } else { "audio" };
    let mut child = Command::new(&path)
        .args(["--mode", mode])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("media_missing", format!("Couldn't start the media engine ({}): {e}", path.display())))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            tracing::debug!(target: "rad_media", "{line}");
        }
    });
    *p.frame.lock().unwrap() = Frame::default();
    p.last.lock().unwrap().clear();
    let (kind, frame, signalled, last) = (p.kind, p.frame.clone(), p.frame_signalled.clone(), p.last.clone());
    let app2 = app.clone();
    std::thread::spawn(move || read_engine(app2, kind, stdout, frame, signalled, last));
    *p.engine.lock().unwrap() = Some(Engine { child, stdin });
    Ok(())
}

fn read_engine(app: AppHandle, kind: Kind, stdout: std::process::ChildStdout, frame: Arc<Mutex<Frame>>, signalled: Arc<AtomicBool>, last: Arc<Mutex<HashMap<String, Value>>>) {
    let mut r = std::io::BufReader::with_capacity(1 << 20, stdout);
    let mut head = [0u8; 5];
    let mut buf = Vec::new();
    while r.read_exact(&mut head).is_ok() {
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        buf.resize(len, 0);
        if r.read_exact(&mut buf).is_err() {
            break;
        }
        match head[0] {
            1 => {
                let Ok(mut v) = serde_json::from_slice::<Value>(&buf) else { continue };
                v["player"] = json!(kind.name());
                if let Some(t @ ("ready" | "status" | "meta" | "tracks")) = v["type"].as_str() {
                    last.lock().unwrap().insert(t.to_string(), v.clone());
                }
                let _ = app.emit_to("main", "media-event", v);
            }
            // Whole frames only: [w, h, 0, h] then RGBA.
            2 if buf.len() >= 16 => {
                let n = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
                let (w, h) = (n(0), n(4));
                if buf.len() < 16 + w as usize * h as usize * 4 {
                    continue;
                }
                {
                    let mut f = frame.lock().unwrap();
                    f.w = w;
                    f.h = h;
                    f.px.clear();
                    f.px.extend_from_slice(&buf[16..16 + w as usize * h as usize * 4]);
                    f.fresh = true;
                }
                // One notification until the view pulls; frames in between are dropped.
                if !signalled.swap(true, Ordering::AcqRel) {
                    let _ = app.emit_to("main", "media-frame", kind.name());
                }
            }
            _ => {}
        }
    }
    last.lock().unwrap().clear();
    let _ = app.emit_to("main", "media-event", json!({"type": "exited", "player": kind.name()}));
}

// ------------------------------------------------------------------ vault proxy

struct Proxy {
    port: u16,
    files: Arc<Mutex<HashMap<String, i64>>>,
}

fn token() -> CmdResult<String> {
    let mut b = [0u8; 24];
    getrandom::getrandom(&mut b).map_err(|e| CmdError::new("media", e.to_string()))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

async fn start_proxy(app: AppHandle) -> CmdResult<Proxy> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| CmdError::new("media", e.to_string()))?;
    let port = listener.local_addr().map_err(|e| CmdError::new("media", e.to_string()))?.port();
    let files: Arc<Mutex<HashMap<String, i64>>> = Arc::default();
    let files2 = files.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let (app, files) = (app.clone(), files2.clone());
            tauri::async_runtime::spawn(async move {
                let _ = serve(app, files, sock).await;
            });
        }
    });
    Ok(Proxy { port, files })
}

/// One request from the engine: `GET|HEAD /m/{token}`, optionally with `Range`.
async fn serve(app: AppHandle, files: Arc<Mutex<HashMap<String, i64>>>, mut sock: tokio::net::TcpStream) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = sock.read(&mut chunk).await?;
        if n == 0 || head.len() > 16 * 1024 {
            return Ok(());
        }
        head.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let range = lines.filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case("range")).map(|(_, v)| v.trim().to_string());
    let doc = target.strip_prefix("/m/").and_then(|t| files.lock().unwrap().get(t).copied());
    let reply = |status: &str| format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let (Some(doc), "GET" | "HEAD") = (doc, method) else {
        return sock.write_all(reply("404 Not Found").as_bytes()).await;
    };

    let st = app.state::<crate::App>();
    let m = if method == "HEAD" { reqwest::Method::HEAD } else { reqwest::Method::GET };
    let Ok(mut rb) = st.client.request(&st.client.stream_http, m, &format!("/v1/documents/{doc}/content")) else {
        return sock.write_all(reply("503 Service Unavailable").as_bytes()).await;
    };
    if let Some(r) = &range {
        rb = rb.header("range", r.as_str());
    }
    let Ok(resp) = rb.send().await else {
        return sock.write_all(reply("502 Bad Gateway").as_bytes()).await;
    };
    let status = resp.status();
    let mut out = format!("HTTP/1.1 {} {}\r\nConnection: close\r\n", status.as_u16(), status.canonical_reason().unwrap_or(""));
    for h in ["content-type", "content-length", "content-range", "accept-ranges"] {
        if let Some(v) = resp.headers().get(h).and_then(|v| v.to_str().ok()) {
            out.push_str(&format!("{h}: {v}\r\n"));
        }
    }
    out.push_str("\r\n");
    sock.write_all(out.as_bytes()).await?;
    if method == "GET" {
        let mut body = resp.bytes_stream();
        while let Some(Ok(b)) = body.next().await {
            sock.write_all(&b).await?;
        }
    }
    sock.flush().await
}

// ------------------------------------------------------------------ commands

/// Starts the player's engine, or re-attaches to it: returns the latest `ready`, `status`,
/// `meta` and `tracks` events (null when it was just started — they follow as events).
#[tauri::command]
pub fn media_start(app: AppHandle, media: State<'_, Media>, player: String) -> CmdResult<Value> {
    let p = media.get(&player);
    if p.running() {
        p.frame.lock().unwrap().fresh = true;
        p.frame_signalled.store(false, Ordering::Release);
        let _ = app.emit_to("main", "media-frame", p.kind.name());
        let last = p.last.lock().unwrap();
        return Ok(json!(last.clone()));
    }
    spawn(&app, p)?;
    Ok(Value::Null)
}

#[tauri::command]
pub fn media_stop(media: State<'_, Media>, player: String) {
    media.get(&player).stop();
}

/// Forwards a command to the engine (open, play, pause, seek, volume, eq…).
#[tauri::command]
pub fn media_send(media: State<'_, Media>, player: String, cmd: Value) -> CmdResult<()> {
    media.get(&player).send(&cmd)
}

/// The newest picture: `[w, h, 0, h]` (u32 LE) then RGBA; just the 16 zero bytes when there's
/// nothing new.
#[tauri::command]
pub fn media_frame(media: State<'_, Media>, player: String) -> Response {
    let p = media.get(&player);
    let mut f = p.frame.lock().unwrap();
    p.frame_signalled.store(false, Ordering::Release);
    if !f.fresh || f.px.is_empty() {
        return Response::new(vec![0u8; 16]);
    }
    f.fresh = false;
    let mut out = Vec::with_capacity(16 + f.px.len());
    for v in [f.w, f.h, 0, f.h] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&f.px);
    Response::new(out)
}

/// An address the engine can stream a vault file from (see the module docs).
#[tauri::command]
pub async fn media_vault_url(app: AppHandle, media: State<'_, Media>, document_id: i64) -> CmdResult<String> {
    let mut g = media.proxy.lock().await;
    if g.is_none() {
        *g = Some(start_proxy(app).await?);
    }
    let p = g.as_ref().unwrap();
    let mut files = p.files.lock().unwrap();
    let t = match files.iter().find(|(_, d)| **d == document_id) {
        Some((t, _)) => t.clone(),
        None => {
            let t = token()?;
            files.insert(t.clone(), document_id);
            t
        }
    };
    Ok(format!("http://127.0.0.1:{}/m/{t}", p.port))
}
