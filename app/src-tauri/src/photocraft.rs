//! PhotoCraft, the image editor (https://github.com/storytold/photocraft, MIT or Apache-2.0): a
//! Photoshop-style editor in Rust. It is its own app, so RAD launches it rather than embedding it,
//! and drives it through its own automation interfaces (docs/control-protocol.md there):
//!
//! - **The app**, for drawing and editing by hand: `PhotoCraft --control PORT --control-token-file
//!   F`, a loopback-only TCP channel. The first line authenticates, then each request is a JSON
//!   line `{id, method, params}` answered by `{id, ok, result | error}`. Any of its 500+ commands
//!   (`engine.execute`), its UI (tools, pointer strokes, keys) and screenshots.
//! - **Headless**, for edits in the background (Gilgamesh, bots): `photocraft-cli serve`, the same
//!   envelope over stdin/stdout, with no window.
//!
//! Both may only read and write files in one folder, ~/Pictures/RAD PhotoCraft (their automation
//! roots); RAD copies files in and out of it. The app is downloaded once on request: the pinned
//! release's disk image, checked against its SHA-256, copied into RAD's app data. It's signed and
//! notarized by its makers. lib/photocraft.svelte.ts has the agent that uses all this.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::{CmdError, CmdResult};

const VERSION: &str = "0.18.0";
const DMG_URL: &str = "https://github.com/storytold/photocraft/releases/download/v0.18.0/photocraft-0.18.0-macos-universal.dmg";
const DMG_SHA256: &str = "20335daabedacfed91fa579e77a2ac18d261a735057782b87492827876cb0f91";
const DMG_BYTES: u64 = 244_539_770;
/// The app answers within 60 s or says it timed out; a little longer here.
const CALL_TIMEOUT: Duration = Duration::from_secs(70);

struct Headless {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

#[derive(Default)]
pub struct Photocraft {
    downloading: Mutex<Option<Value>>,
    app: Mutex<Option<(std::process::Child, u16)>>,
    headless: tokio::sync::Mutex<Option<Headless>>,
    ids: AtomicU64,
}

impl Photocraft {
    /// RAD is quitting: the headless editor goes (the app stays, with the owner's work).
    pub fn stop(&self) {
        if let Ok(mut h) = self.headless.try_lock() {
            if let Some(mut x) = h.take() {
                let _ = x.child.start_kill();
            }
        }
    }
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn data_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().map(|d| d.join("photocraft")).unwrap_or_else(|_| "photocraft".into())
}

/// The one folder PhotoCraft may read and write.
pub fn work_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| ".".into());
    home.join("Pictures").join("RAD PhotoCraft")
}

/// PhotoCraft.app: the one RAD installed, else one in Applications.
fn app_bundle(app: &AppHandle) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    [data_dir(app).join("PhotoCraft.app"), home.join("Applications/PhotoCraft.app"), PathBuf::from("/Applications/PhotoCraft.app")]
        .into_iter()
        .find(|p| p.join("Contents/MacOS/PhotoCraft").is_file())
}

fn need_bundle(app: &AppHandle) -> CmdResult<PathBuf> {
    app_bundle(app).ok_or_else(|| CmdError::new("photocraft_missing", "PhotoCraft isn't installed yet"))
}

/// A file name inside the work folder: plain names and subfolders only.
fn in_work(name: &str) -> CmdResult<PathBuf> {
    let bad = name.is_empty() || name.starts_with('/') || name.contains('\\') || name.split('/').any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        return Err(CmdError::new("bad_request", "invalid file name"));
    }
    Ok(work_dir().join(name))
}

// ------------------------------------------------------------------ the app

fn app_running(p: &Photocraft) -> Option<u16> {
    let mut a = p.app.lock().unwrap();
    if let Some((child, port)) = a.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Some(*port);
        }
    }
    *a = None;
    None
}

fn free_port() -> CmdResult<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(io)?;
    Ok(l.local_addr().map_err(io)?.port())
}

fn token(app: &AppHandle) -> CmdResult<String> {
    let t = std::fs::read_to_string(data_dir(app).join("control.token")).map_err(|_| CmdError::new("photocraft", "PhotoCraft hasn't written its control token"))?;
    Ok(t.trim().to_string())
}

/// One request to the running app: connect, authenticate, ask, read the answer.
async fn app_call(app: &AppHandle, port: u16, method: &str, params: Value) -> CmdResult<Value> {
    let tok = token(app)?;
    let fut = async {
        let s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.map_err(|_| CmdError::new("not_running", "PhotoCraft isn't answering"))?;
        let (r, mut w) = s.into_split();
        let mut lines = BufReader::new(r).lines();
        w.write_all(format!("{}\n", json!({"id": "auth", "method": "auth", "params": {"token": tok}})).as_bytes()).await.map_err(io)?;
        let auth: Value = serde_json::from_str(&lines.next_line().await.map_err(io)?.unwrap_or_default()).unwrap_or(Value::Null);
        if auth["ok"] != true {
            return Err(CmdError::new("photocraft", format!("PhotoCraft refused RAD: {}", auth["error"].as_str().unwrap_or("no answer"))));
        }
        w.write_all(format!("{}\n", json!({"id": 1, "method": method, "params": params})).as_bytes()).await.map_err(io)?;
        loop {
            let Some(line) = lines.next_line().await.map_err(io)? else {
                return Err(CmdError::new("photocraft", "PhotoCraft closed the connection"));
            };
            let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if v["id"] == 1 {
                return answer(v);
            }
        }
    };
    tokio::time::timeout(CALL_TIMEOUT, fut).await.map_err(|_| CmdError::new("timeout", "PhotoCraft didn't answer in time"))?
}

fn answer(v: Value) -> CmdResult<Value> {
    if v["ok"] == true {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    } else {
        Err(CmdError::new("photocraft", v["error"].as_str().map(String::from).unwrap_or_else(|| v["error"].to_string())))
    }
}

/// Starts the app (with its control channel) unless it's running, and waits until it answers.
async fn ensure_app(app: &AppHandle, p: &Photocraft, open: Option<&Path>) -> CmdResult<u16> {
    if let Some(port) = app_running(p) {
        return Ok(port);
    }
    let bundle = need_bundle(app)?;
    let work = work_dir();
    std::fs::create_dir_all(&work).map_err(io)?;
    std::fs::create_dir_all(data_dir(app)).map_err(io)?;
    let port = free_port()?;
    let mut cmd = std::process::Command::new(bundle.join("Contents/MacOS/PhotoCraft"));
    cmd.args(["--control", &port.to_string()])
        .arg("--control-token-file")
        .arg(data_dir(app).join("control.token"))
        .arg("--automation-read-root")
        .arg(&work)
        .arg("--automation-write-root")
        .arg(&work)
        .current_dir(&work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(f) = open {
        cmd.arg(f);
    }
    let child = cmd.spawn().map_err(|e| CmdError::new("photocraft", format!("Couldn't start PhotoCraft: {e}")))?;
    *p.app.lock().unwrap() = Some((child, port));
    // It answers once its window is up.
    let until = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        if app_call(app, port, "ui.inspect", json!({})).await.is_ok() {
            return Ok(port);
        }
        if app_running(p).is_none() {
            return Err(CmdError::new("photocraft", "PhotoCraft quit while starting"));
        }
        if std::time::Instant::now() > until {
            return Err(CmdError::new("timeout", "PhotoCraft didn't start in time"));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

// ------------------------------------------------------------------ headless

async fn headless_call(app: &AppHandle, p: &Photocraft, method: &str, params: Value) -> CmdResult<Value> {
    let bundle = need_bundle(app)?;
    headless_call_with(&bundle, &work_dir(), p, method, params).await
}

async fn headless_call_with(bundle: &Path, work: &Path, p: &Photocraft, method: &str, params: Value) -> CmdResult<Value> {
    let mut h = p.headless.lock().await;
    if h.as_mut().is_some_and(|x| !matches!(x.child.try_wait(), Ok(None))) {
        *h = None;
    }
    if h.is_none() {
        std::fs::create_dir_all(work).map_err(io)?;
        let mut child = tokio::process::Command::new(bundle.join("Contents/MacOS/photocraft-cli"))
            .arg("serve")
            .arg("--automation-read-root")
            .arg(work)
            .arg("--automation-write-root")
            .arg(work)
            .current_dir(work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| CmdError::new("photocraft", format!("Couldn't start PhotoCraft's engine: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        *h = Some(Headless { child, stdin, stdout });
    }
    let x = h.as_mut().unwrap();
    let id = p.ids.fetch_add(1, Ordering::Relaxed) + 1;
    let fut = async {
        x.stdin.write_all(format!("{}\n", json!({"id": id, "method": method, "params": params})).as_bytes()).await.map_err(io)?;
        loop {
            let Some(line) = x.stdout.next_line().await.map_err(io)? else {
                return Err(CmdError::new("photocraft", "PhotoCraft's engine stopped"));
            };
            let v: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if v["id"] == id {
                return answer(v);
            }
        }
    };
    match tokio::time::timeout(CALL_TIMEOUT, fut).await {
        Ok(r) => r,
        Err(_) => {
            // A reply may still come later and be mistaken for the next one's: start afresh.
            *h = None;
            Err(CmdError::new("timeout", "PhotoCraft's engine didn't answer in time"))
        }
    }
}

// ------------------------------------------------------------------ installing

async fn download(app: &AppHandle, p: &Photocraft) -> CmdResult<()> {
    let d = data_dir(app);
    tokio::fs::create_dir_all(&d).await.map_err(io)?;
    let dmg = d.join("photocraft.dmg.part");
    let http = reqwest::Client::builder().proxy(rad_tunnel::route::reqwest_proxy()).connect_timeout(Duration::from_secs(15)).build().map_err(io)?;
    let resp = http.get(DMG_URL).send().await.map_err(|e| CmdError::new("offline", format!("Download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(CmdError::new("photocraft", format!("Download failed: HTTP {}", resp.status())));
    }
    let mut file = tokio::fs::File::create(&dmg).await.map_err(io)?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let (mut done, mut last) = (0u64, 0u64);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CmdError::new("offline", format!("Download interrupted: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(io)?;
        done += chunk.len() as u64;
        if done - last > 2_000_000 {
            last = done;
            let v = json!({"phase": "Downloading", "done": done, "total": DMG_BYTES});
            *p.downloading.lock().unwrap() = Some(v.clone());
            let _ = app.emit("photocraft-progress", v);
        }
    }
    file.flush().await.map_err(io)?;
    drop(file);
    let hash: String = hasher.finalize().iter().map(|x| format!("{x:02x}")).collect();
    if hash != DMG_SHA256 {
        let _ = tokio::fs::remove_file(&dmg).await;
        return Err(CmdError::new("photocraft", "The download didn't match its checksum — try again"));
    }
    let v = json!({"phase": "Installing", "done": DMG_BYTES, "total": DMG_BYTES});
    *p.downloading.lock().unwrap() = Some(v.clone());
    let _ = app.emit("photocraft-progress", v);
    let d2 = d.clone();
    tokio::task::spawn_blocking(move || install_from_dmg(&d2.join("photocraft.dmg.part"), &d2)).await.map_err(io)??;
    let _ = tokio::fs::remove_file(&dmg).await;
    Ok(())
}

/// Copies PhotoCraft.app out of the disk image (ditto keeps its signature intact).
fn install_from_dmg(dmg: &Path, dest: &Path) -> CmdResult<()> {
    let mnt = dest.join("mnt");
    let _ = std::fs::create_dir_all(&mnt);
    let ok = std::process::Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"])
        .arg(&mnt)
        .arg(dmg)
        .stdout(Stdio::null())
        .status()
        .map_err(io)?
        .success();
    if !ok {
        return Err(CmdError::new("photocraft", "Couldn't open PhotoCraft's disk image"));
    }
    let tmp = dest.join("PhotoCraft.app.tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    let copied = std::process::Command::new("/usr/bin/ditto").arg(mnt.join("PhotoCraft.app")).arg(&tmp).status().map_err(io).map(|s| s.success());
    let _ = std::process::Command::new("/usr/bin/hdiutil").arg("detach").arg(&mnt).stdout(Stdio::null()).status();
    let _ = std::fs::remove_dir(&mnt);
    if !copied? || !tmp.join("Contents/MacOS/PhotoCraft").is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(CmdError::new("photocraft", "Couldn't copy PhotoCraft out of its disk image"));
    }
    let app = dest.join("PhotoCraft.app");
    let _ = std::fs::remove_dir_all(&app);
    std::fs::rename(&tmp, &app).map_err(io)
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn photocraft_status(app: AppHandle, p: State<'_, Photocraft>) -> Value {
    let bundle = app_bundle(&app);
    let version = bundle.as_ref().and_then(|b| {
        let out = std::process::Command::new("/usr/bin/defaults").arg("read").arg(b.join("Contents/Info")).arg("CFBundleShortVersionString").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    json!({
        "supported": cfg!(target_os = "macos"),
        "installed": bundle.is_some(),
        "path": bundle,
        "version": version,
        "pinned": VERSION,
        "bytes": DMG_BYTES,
        "running": app_running(&p).is_some(),
        "work_dir": work_dir(),
        "downloading": p.downloading.lock().unwrap().clone(),
    })
}

#[tauri::command]
pub async fn photocraft_install(app: AppHandle, p: State<'_, Photocraft>) -> CmdResult<()> {
    if !cfg!(target_os = "macos") {
        return Err(CmdError::new("unsupported", "RAD installs PhotoCraft on a Mac only for now"));
    }
    {
        let mut d = p.downloading.lock().unwrap();
        if d.is_some() {
            return Err(CmdError::new("busy", "PhotoCraft is already being installed"));
        }
        *d = Some(json!({"phase": "Downloading", "done": 0, "total": DMG_BYTES}));
    }
    let r = download(&app, &p).await;
    *p.downloading.lock().unwrap() = None;
    let _ = app.emit("photocraft-progress", json!({"done": true, "error": r.as_ref().err().map(|e| e.message.clone())}));
    r
}

/// Opens the app (started if needed), with a file of the work folder if given.
#[tauri::command]
pub async fn photocraft_open(app: AppHandle, p: State<'_, Photocraft>, name: Option<String>) -> CmdResult<()> {
    let file = name.as_deref().map(in_work).transpose()?;
    let was = app_running(&p).is_some();
    let port = ensure_app(&app, &p, if was { None } else { file.as_deref() }).await?;
    if let (true, Some(f)) = (was, &file) {
        app_call(&app, port, "app.open", json!({"path": f})).await?;
    }
    let _ = app_call(&app, port, "ui.focus", json!({})).await;
    Ok(())
}

/// One request: to the app (`target` "app", started if needed) or the headless engine.
#[tauri::command]
pub async fn photocraft_call(app: AppHandle, p: State<'_, Photocraft>, target: String, method: String, params: Option<Value>) -> CmdResult<Value> {
    let params = params.unwrap_or_else(|| json!({}));
    if target == "app" {
        let port = ensure_app(&app, &p, None).await?;
        app_call(&app, port, &method, params).await
    } else {
        headless_call(&app, &p, &method, params).await
    }
}

/// A picture of the work: the app's window, or the headless document rendered (PNG bytes).
#[tauri::command]
pub async fn photocraft_picture(app: AppHandle, p: State<'_, Photocraft>, target: String) -> CmdResult<tauri::ipc::Response> {
    let name = format!(".rad/shot-{}.png", p.ids.fetch_add(1, Ordering::Relaxed));
    let path = in_work(&name)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
    if target == "app" {
        let port = ensure_app(&app, &p, None).await?;
        app_call(&app, port, "ui.screenshot", json!({"path": name, "focus": false})).await?;
    } else {
        headless_call(&app, &p, "doc.render", json!({"path": name, "maxSide": 1280})).await?;
    }
    let b = std::fs::read(&path).map_err(|_| CmdError::new("photocraft", "PhotoCraft didn't save the picture"))?;
    let _ = std::fs::remove_file(&path);
    Ok(tauri::ipc::Response::new(b))
}

/// The pictures in the work folder, newest first.
#[tauri::command]
pub fn photocraft_files() -> Vec<Value> {
    let mut v: Vec<(u64, Value)> = std::fs::read_dir(work_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let at = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
            Some((at, json!({"name": e.file_name().to_string_lossy(), "size": m.len(), "modified": at})))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|x| x.1).collect()
}

/// Copies a file into the work folder (from Files, or a bot's exchange folder); returns its
/// name there (a number is added rather than replacing a file).
#[tauri::command]
pub fn photocraft_import(path: String) -> CmdResult<String> {
    let src = PathBuf::from(&path);
    if !src.is_file() {
        return Err(CmdError::new("not_found", "That file isn't there"));
    }
    let work = work_dir();
    std::fs::create_dir_all(&work).map_err(io)?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
    let ext = src.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let mut name = format!("{stem}{ext}");
    let mut n = 2;
    while work.join(&name).exists() {
        name = format!("{stem} {n}{ext}");
        n += 1;
    }
    std::fs::copy(&src, work.join(&name)).map_err(io)?;
    Ok(name)
}

/// Copies a file of the work folder to a container's exchange folder (a bot's result).
#[tauri::command]
pub fn photocraft_export_to_container(app: AppHandle, name: String, container: String) -> CmdResult<String> {
    let src = in_work(&name)?;
    if container.is_empty() || !container.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(CmdError::new("bad_request", "invalid container"));
    }
    let shared = app.path().app_data_dir().map_err(io)?.join("containers").join(&container).join("shared");
    if !shared.is_dir() {
        return Err(CmdError::new("not_found", "That container doesn't exist"));
    }
    let base = Path::new(&name).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or(name.clone());
    std::fs::copy(&src, shared.join(&base)).map_err(io)?;
    Ok(format!("/root/shared/{base}"))
}

#[tauri::command]
pub fn photocraft_reveal() -> CmdResult<()> {
    let w = work_dir();
    std::fs::create_dir_all(&w).map_err(io)?;
    std::process::Command::new("/usr/bin/open").arg(&w).spawn().map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_in_the_work_folder() {
        assert!(in_work("a.png").is_ok());
        assert!(in_work("sub/a.png").is_ok());
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "a//b", "./a", "a\\b"] {
            assert!(in_work(bad).is_err(), "{bad}");
        }
    }

    /// The real engine, headless, through RAD's own calls:
    /// RAD_PHOTOCRAFT=/path/to/PhotoCraft.app cargo test -p rad-app --lib photocraft -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn headless_engine_end_to_end() {
        let bundle = PathBuf::from(std::env::var("RAD_PHOTOCRAFT").unwrap());
        let work = std::env::temp_dir().join("rad-photocraft-test");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let p = Photocraft::default();
        let call = |m: &'static str, v: Value| headless_call_with(&bundle, &work, &p, m, v);
        let cmds = call("engine.commands", json!({})).await.unwrap();
        let n = cmds.as_array().unwrap().len();
        println!("{n} commands, e.g. {}", cmds[0]);
        assert!(n > 100);
        let blur = call("engine.commands", json!({"filter": "gaussian"})).await.unwrap();
        println!("gaussian: {blur}");
        call("engine.execute", json!({"command": "file.new", "params": {"width": 320, "height": 200}})).await.unwrap();
        let doc = call("engine.execute", json!({"command": "document.inspect", "params": {}})).await.unwrap();
        println!("document: {}", doc.to_string().chars().take(600).collect::<String>());
        call("doc.render", json!({"path": "r.png", "maxSide": 320})).await.unwrap();
        assert_eq!(&std::fs::read(work.join("r.png")).unwrap()[1..4], b"PNG");
        call("doc.save", json!({"path": "out.png"})).await.unwrap();
        assert!(work.join("out.png").is_file());
        // Outside the folder is refused.
        assert!(call("doc.save", json!({"path": "../escape.png"})).await.is_err());
        assert!(call("engine.execute", json!({"command": "no.such.command"})).await.is_err());
        p.stop();
    }

    #[test]
    fn answers() {
        assert_eq!(answer(json!({"id": 1, "ok": true, "result": {"a": 1}})).unwrap(), json!({"a": 1}));
        assert_eq!(answer(json!({"id": 1, "ok": false, "error": "unknown tool `foo`"})).unwrap_err().message, "unknown tool `foo`");
    }
}
