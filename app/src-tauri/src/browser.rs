//! The built-in web browser. Pages are rendered by Servo in a separate engine process
//! (`rad-browser`, see `browser/rad-browser`) and shown in the main window as frames, so the
//! app always has the exact pixels on screen: for the vision model, screenshots and scraping.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use crate::client::{CmdError, CmdResult};

const BIN: &str = if cfg!(windows) { "rad-browser.exe" } else { "rad-browser" };
/// Longest side of a screenshot sent to a vision model (the server re-checks this).
const VISION_MAX_SIDE: u32 = 1568;
/// Page text sent to the model with a question.
const CONTEXT_CHARS: usize = 24_000;

#[derive(Default)]
struct Frame {
    w: u32,
    h: u32,
    px: Vec<u8>,
    /// Rows changed since the UI last pulled (start, end).
    dirty: Option<(u32, u32)>,
}

struct Engine {
    child: Child,
    stdin: ChildStdin,
}

#[derive(Default)]
pub struct Browser {
    engine: Mutex<Option<Engine>>,
    frame: Arc<Mutex<Frame>>,
    frame_signalled: Arc<AtomicBool>,
    evals: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    next_eval: AtomicU64,
    /// Last tab state, replayed when the browser view is opened again.
    state: Arc<Mutex<Value>>,
}

impl Browser {
    pub fn stop(&self) {
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
        let e = g.as_mut().ok_or_else(|| CmdError::new("browser_stopped", "The browser engine isn't running"))?;
        let mut line = serde_json::to_vec(cmd)?;
        line.push(b'\n');
        e.stdin.write_all(&line).and_then(|_| e.stdin.flush()).map_err(|e| CmdError::new("browser_stopped", format!("The browser engine stopped: {e}")))
    }

    async fn eval(&self, script: &str) -> CmdResult<Value> {
        let id = self.next_eval.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.evals.lock().unwrap().insert(id, tx);
        if let Err(e) = self.send(&json!({"cmd": "eval", "id": id, "script": script})) {
            self.evals.lock().unwrap().remove(&id);
            return Err(e);
        }
        let r = tokio::time::timeout(Duration::from_secs(15), rx).await;
        self.evals.lock().unwrap().remove(&id);
        let v = r.map_err(|_| CmdError::new("browser", "The page didn't answer in time"))?.map_err(|_| CmdError::new("browser", "The browser engine stopped"))?;
        if v["ok"] == true {
            Ok(v["value"].clone())
        } else {
            Err(CmdError::new("browser", format!("The page script failed: {}", v["error"].as_str().unwrap_or("unknown error"))))
        }
    }

    /// The whole current frame as an image.
    fn image(&self) -> CmdResult<image::RgbaImage> {
        let f = self.frame.lock().unwrap();
        if f.px.is_empty() {
            return Err(CmdError::new("browser", "Nothing is on screen yet"));
        }
        image::RgbaImage::from_raw(f.w, f.h, f.px.clone()).ok_or_else(|| CmdError::new("browser", "bad frame"))
    }

    /// Text, links and metadata of the page in the active tab.
    async fn scrape(&self, max_chars: usize) -> CmdResult<Value> {
        let script = SCRAPE_JS.replace("__MAX__", &max_chars.to_string());
        let v = self.eval(&script).await?;
        match v.as_str() {
            Some(s) => serde_json::from_str(s).map_err(|e| CmdError::new("browser", e.to_string())),
            None => Err(CmdError::new("browser", "The page returned nothing")),
        }
    }
}

const SCRAPE_JS: &str = r#"(() => {
  const max = __MAX__;
  const clean = (s) => (s || '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
  const body = document.body;
  let text = body ? clean(body.innerText || body.textContent) : '';
  if (text.length > max) text = text.slice(0, max) + '\n…(truncated)';
  const seen = new Set();
  const links = [];
  for (const a of document.querySelectorAll('a[href]')) {
    const t = clean(a.innerText || a.textContent).replace(/\s+/g, ' ').slice(0, 120);
    if (!t || seen.has(a.href) || !/^https?:/.test(a.href)) continue;
    seen.add(a.href);
    links.push({ text: t, href: a.href });
    if (links.length >= 120) break;
  }
  const meta = document.querySelector('meta[name="description"], meta[property="og:description"]');
  const sel = window.getSelection ? String(window.getSelection()) : '';
  return JSON.stringify({
    url: location.href, title: document.title, description: meta ? meta.getAttribute('content') || '' : '',
    selection: sel.slice(0, 4000), text, links, scroll_y: Math.round(window.scrollY),
    page_height: document.documentElement ? document.documentElement.scrollHeight : 0,
  });
})()"#;

/// Finds the engine: next to the app (bundled sidecar), in the app's resources, or — in
/// development — in the engine's own build folder. `RAD_BROWSER_BIN` overrides all of these.
fn engine_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_BROWSER_BIN") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join(BIN));
    }
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join(BIN));
    }
    if cfg!(debug_assertions) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../browser/rad-browser/target");
        candidates.push(root.join("release").join(BIN));
        candidates.push(root.join("debug").join(BIN));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn spawn(app: &AppHandle, b: &Browser, w: u32, h: u32, scale: f64, url: &str) -> CmdResult<()> {
    let path = engine_path(app).ok_or_else(|| {
        CmdError::new(
            "browser_missing",
            if cfg!(debug_assertions) {
                "The web browser engine isn't built yet. Run ./scripts/build-browser.sh, then reopen the browser."
            } else {
                "This copy of RAD was built without the web browser engine. Rebuild it with ./scripts/build-app.sh and reinstall."
            },
        )
    })?;
    let mut child = Command::new(&path)
        .args(["--width", &w.to_string(), "--height", &h.to_string(), "--scale", &scale.to_string(), "--url", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("browser_missing", format!("Couldn't start the browser engine ({}): {e}", path.display())))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            tracing::debug!(target: "rad_browser", "{line}");
        }
    });
    *b.frame.lock().unwrap() = Frame::default();
    let (frame, signalled, evals, state) = (b.frame.clone(), b.frame_signalled.clone(), b.evals.clone(), b.state.clone());
    let app2 = app.clone();
    std::thread::spawn(move || read_engine(app2, stdout, frame, signalled, evals, state));
    *b.engine.lock().unwrap() = Some(Engine { child, stdin });
    Ok(())
}

fn read_engine(
    app: AppHandle,
    stdout: std::process::ChildStdout,
    frame: Arc<Mutex<Frame>>,
    signalled: Arc<AtomicBool>,
    evals: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    state: Arc<Mutex<Value>>,
) {
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
                let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
                if v["type"] == "eval" {
                    if let Some(tx) = v["id"].as_u64().and_then(|id| evals.lock().unwrap().remove(&id)) {
                        let _ = tx.send(v);
                    }
                    continue;
                }
                if v["type"] == "state" {
                    *state.lock().unwrap() = v.clone();
                }
                let _ = app.emit_to("main", "browser-event", v);
            }
            2 if buf.len() >= 16 => {
                let n = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
                let (w, h, y0, rows) = (n(0), n(4), n(8), n(12));
                let stride = w as usize * 4;
                if y0 + rows > h || buf.len() < 16 + rows as usize * stride {
                    continue;
                }
                {
                    let mut f = frame.lock().unwrap();
                    if f.w != w || f.h != h {
                        *f = Frame { w, h, px: vec![0; stride * h as usize], dirty: None };
                    }
                    let start = y0 as usize * stride;
                    f.px[start..start + rows as usize * stride].copy_from_slice(&buf[16..16 + rows as usize * stride]);
                    let (a, b) = f.dirty.map_or((y0, y0 + rows), |(a, b)| (a.min(y0), b.max(y0 + rows)));
                    f.dirty = Some((a, b));
                }
                // One notification until the UI pulls; frames in between are merged.
                if !signalled.swap(true, Ordering::AcqRel) {
                    let _ = app.emit_to("main", "browser-frame", ());
                }
            }
            _ => {}
        }
    }
    evals.lock().unwrap().clear();
    *state.lock().unwrap() = Value::Null;
    let _ = app.emit_to("main", "browser-event", json!({"type": "exited"}));
}

// ------------------------------------------------------------------ commands

/// Starts the engine (or re-attaches to it) at the viewport's size in device pixels.
#[tauri::command]
pub fn browser_start(app: AppHandle, b: State<'_, Browser>, width: u32, height: u32, scale: f64, url: Option<String>) -> CmdResult<Value> {
    let (w, h) = (width.max(1), height.max(1));
    if b.running() {
        // A fresh view: hand it the whole current frame, then follow the engine as usual.
        {
            let mut f = b.frame.lock().unwrap();
            f.dirty = (f.h > 0).then_some((0, f.h));
        }
        b.frame_signalled.store(false, Ordering::Release);
        b.send(&json!({"cmd": "resize", "w": w, "h": h, "scale": scale}))?;
        if let Some(u) = url {
            b.send(&json!({"cmd": "load", "url": u}))?;
        }
        let _ = app.emit_to("main", "browser-frame", ());
        return Ok(b.state.lock().unwrap().clone());
    }
    spawn(&app, &b, w, h, scale, url.as_deref().unwrap_or("https://servo.org"))?;
    Ok(Value::Null)
}

#[tauri::command]
pub fn browser_stop(b: State<'_, Browser>) {
    b.stop();
}

/// Forwards a command to the engine (navigation, input, tabs, dialogs…).
#[tauri::command]
pub fn browser_send(b: State<'_, Browser>, cmd: Value) -> CmdResult<()> {
    if cmd["cmd"] == "eval" {
        return Err(CmdError::new("bad_request", "use browser_eval"));
    }
    b.send(&cmd)
}

/// The rows that changed since the last call: `[w, h, y0, rows]` (u32 LE) followed by RGBA.
#[tauri::command]
pub fn browser_frame(b: State<'_, Browser>) -> Response {
    let mut f = b.frame.lock().unwrap();
    b.frame_signalled.store(false, Ordering::Release);
    let Some((y0, y1)) = f.dirty.take() else {
        return Response::new(vec![0u8; 16]);
    };
    let stride = f.w as usize * 4;
    let body = &f.px[y0 as usize * stride..y1 as usize * stride];
    let mut out = Vec::with_capacity(16 + body.len());
    for v in [f.w, f.h, y0, y1 - y0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(body);
    Response::new(out)
}

#[tauri::command]
pub async fn browser_eval(b: State<'_, Browser>, script: String) -> CmdResult<Value> {
    b.eval(&script).await
}

#[tauri::command]
pub async fn browser_scrape(b: State<'_, Browser>, max_chars: Option<usize>) -> CmdResult<Value> {
    b.scrape(max_chars.unwrap_or(CONTEXT_CHARS)).await
}

/// What the AI gets to see with a question: the page's text and/or a screenshot
/// (JPEG, base64), as requested.
#[tauri::command]
pub async fn browser_snapshot(b: State<'_, Browser>, text: bool, image: bool) -> CmdResult<Value> {
    let mut out = json!({});
    if text {
        match b.scrape(CONTEXT_CHARS).await {
            Ok(page) => out["page"] = page,
            Err(e) if !image => return Err(e),
            Err(e) => tracing::warn!("page text unavailable: {}", e.message),
        }
    }
    if image {
        let img = b.image()?;
        let jpeg = tokio::task::spawn_blocking(move || -> CmdResult<Vec<u8>> {
            let img = image::DynamicImage::ImageRgba8(img);
            let img = if img.width().max(img.height()) > VISION_MAX_SIDE {
                img.resize(VISION_MAX_SIDE, VISION_MAX_SIDE, image::imageops::FilterType::Triangle)
            } else {
                img
            };
            let mut buf = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut buf, image::ImageFormat::Jpeg).map_err(|e| CmdError::new("browser", e.to_string()))?;
            Ok(buf.into_inner())
        })
        .await
        .map_err(|e| CmdError::new("browser", e.to_string()))??;
        out["image"] = json!({"mime": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(jpeg)});
    }
    Ok(out)
}

fn file_title(page: &Value) -> String {
    let t = page["title"].as_str().filter(|t| !t.trim().is_empty()).or(page["url"].as_str()).unwrap_or("Web page");
    let t: String = t.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '-' } else { c }).take(80).collect();
    t.trim().to_string()
}

/// Saves what's on screen into the vault as a screenshot (captioned by the vision model).
#[tauri::command]
pub async fn browser_save_screenshot(app: AppHandle, b: State<'_, Browser>, project_id: Option<i64>) -> CmdResult<Value> {
    let project = crate::capture::target_project(&app, project_id).await?;
    let title = b.scrape(200).await.map(|p| file_title(&p)).unwrap_or_else(|_| "Web page".into());
    let img = b.image()?;
    let png = tokio::task::spawn_blocking(move || {
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).map(|_| buf.into_inner())
    })
    .await
    .map_err(|e| CmdError::new("browser", e.to_string()))?
    .map_err(|e| CmdError::new("browser", e.to_string()))?;
    let name = format!("{title} — {}.png", crate::capture::chrono_like_now());
    crate::commands::upload_bytes(&app, project, &name, "screenshot", png).await
}

/// Saves the page's text into the vault as a Markdown document, with its address.
#[tauri::command]
pub async fn browser_save_page(app: AppHandle, b: State<'_, Browser>, project_id: Option<i64>) -> CmdResult<Value> {
    let project = crate::capture::target_project(&app, project_id).await?;
    let page = b.scrape(2_000_000).await?;
    let title = file_title(&page);
    let mut md = format!("# {}\n\nSource: <{}>  \nSaved: {}\n\n", page["title"].as_str().unwrap_or(&title), page["url"].as_str().unwrap_or(""), crate::capture::chrono_like_now());
    if let Some(d) = page["description"].as_str().filter(|d| !d.is_empty()) {
        md.push_str(&format!("> {d}\n\n"));
    }
    md.push_str(page["text"].as_str().unwrap_or(""));
    md.push('\n');
    crate::commands::upload_bytes(&app, project, &format!("{title}.md"), "web", md.into_bytes()).await
}
