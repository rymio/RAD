use std::io::Read;

use futures::StreamExt;
use rad_api::{HEADER_FILENAME, HEADER_SOURCE};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::client::{normalize_url, CmdError, CmdResult};
use crate::config::ClientConfig;
use crate::{events, secrets, App};

fn method(m: &str) -> CmdResult<Method> {
    Method::from_bytes(m.to_uppercase().as_bytes()).map_err(|_| CmdError::new("bad_request", "bad method"))
}

// ------------------------------------------------------------------ config

#[tauri::command]
pub fn get_config(st: State<'_, App>) -> ClientConfig {
    st.config()
}

#[tauri::command]
pub fn set_config(app: AppHandle, st: State<'_, App>, config: ClientConfig) -> ClientConfig {
    let before = st.config();
    let mut c = config;
    // The server binding is managed by connect/pair/forget, not by the settings form.
    c.server_url = before.server_url.clone();
    c.server_name = before.server_name.clone();
    c.mode = before.mode.clone();
    c.local_vault = before.local_vault.clone();
    c.recent_vaults = before.recent_vaults.clone();
    c.machine_id = before.machine_id.clone();
    st.save_config(c.clone());
    if before.hotkey_quick_ask != c.hotkey_quick_ask || before.hotkey_screenshot != c.hotkey_screenshot || before.hotkey_lock != c.hotkey_lock {
        crate::register_hotkeys(&app);
    }
    #[cfg(target_os = "macos")]
    if before.hide_dock_icon != c.hide_dock_icon {
        let _ = app.set_activation_policy(if c.hide_dock_icon { tauri::ActivationPolicy::Accessory } else { tauri::ActivationPolicy::Regular });
    }
    let _ = app.emit("rad-config", &c);
    c
}

#[tauri::command]
pub fn platform() -> &'static str {
    std::env::consts::OS
}

// ------------------------------------------------------------------ connection & pairing

async fn stop_embedded(st: &App) {
    if let Some(e) = st.embedded.lock().await.take() {
        e.stop().await;
    }
}

/// Standalone mode: open (or prepare to create) a vault file on this computer and serve it
/// in-process. Everything else in the app works exactly as with a remote server.
#[tauri::command]
pub async fn start_local(app: AppHandle, st: State<'_, App>, path: String) -> CmdResult<Value> {
    let mut path = std::path::PathBuf::from(path.trim());
    if path.extension().is_none_or(|e| e != "radvault") {
        path.set_extension("radvault");
    }
    events::stop(&app);
    stop_embedded(&st).await;
    serve_local(&app, &st, &path, None).await
}

/// Serves `path` in-process and makes it the current vault (locked). `replaces` is an old
/// location of the same vault, dropped from the recent list.
async fn serve_local(app: &AppHandle, st: &App, path: &std::path::Path, replaces: Option<&str>) -> CmdResult<Value> {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Vault".into());
    let (models_dir, bundled) = model_dirs(app);
    let cfg = st.config();
    let machine_name = Some(cfg.device_name).filter(|n| !n.trim().is_empty());
    let machine_id = Some(cfg.machine_id).filter(|n| !n.is_empty());
    let emb = rad_server::embedded::Embedded::start(path.to_path_buf(), &name, None, models_dir, bundled, machine_id, machine_name)
        .await
        .map_err(|e| CmdError::new("local", format!("Couldn't open the vault: {e:#}")))?;
    st.client.set_server(Some(emb.url()), Some(emb.device_token.clone()));
    write_api_info(app, &emb.url());
    *st.embedded.lock().await = Some(emb);
    let p = path.display().to_string();
    let mut c = st.config();
    c.mode = "local".into();
    c.local_vault = Some(p.clone());
    c.recent_vaults.retain(|r| r != &p && Some(r.as_str()) != replaces);
    c.recent_vaults.insert(0, p);
    c.recent_vaults.truncate(8);
    st.save_config(c.clone());
    let _ = app.emit("rad-config", &c);
    st.client.json(Method::GET, "/v1/status", None).await
}

/// Where other programs on this computer find RAD's API (the in-process server has a new port
/// each time RAD starts): `<app data>/api.json` = {"url", "pid"}. They authenticate with an API
/// key made in the Database view.
fn api_info_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("api.json"))
}

fn write_api_info(app: &AppHandle, url: &str) {
    let Some(p) = api_info_file(app) else { return };
    let body = serde_json::to_vec_pretty(&json!({ "url": url, "pid": std::process::id(), "api": "RAD", "auth": "Authorization: Bearer <API key from RAD → Database → API keys>" })).unwrap_or_default();
    if std::fs::write(&p, body).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// The API's address for other programs, and the file that announces it.
#[tauri::command]
pub fn db_api_info(app: AppHandle, st: State<'_, App>) -> Value {
    json!({
        "url": st.client.base().ok(),
        "discovery_file": api_info_file(&app).map(|p| p.display().to_string()),
        "local": st.config().mode == "local",
    })
}

/// Moves this computer's vault file into `folder` and reopens it from there (locked, so the
/// next step is unlocking). Refuses while another computer has it open.
#[tauri::command]
pub async fn vault_relocate(app: AppHandle, st: State<'_, App>, folder: String) -> CmdResult<Value> {
    let cfg = st.config();
    let from = match (cfg.mode.as_str(), cfg.local_vault.as_deref()) {
        ("local", Some(p)) => std::path::PathBuf::from(p),
        _ => return Err(CmdError::new("local", "Only a vault stored by this app can be moved. A server's vault is moved on the server.")),
    };
    let dest_dir = std::path::PathBuf::from(folder.trim());
    if !dest_dir.is_dir() {
        return Err(CmdError::new("local", "That folder doesn't exist"));
    }
    let file_name = from.file_name().ok_or_else(|| CmdError::new("local", "The vault path is invalid"))?;
    let to = dest_dir.join(file_name);
    let same = |a: &std::path::Path, b: &std::path::Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    };
    if from.parent().is_some_and(|p| same(p, &dest_dir)) {
        return Err(CmdError::new("local", "The vault is already in that folder"));
    }
    if to.exists() {
        return Err(CmdError::new("local", format!("There's already a file named {} in that folder", file_name.to_string_lossy())));
    }
    if let Some(e) = st.embedded.lock().await.as_ref() {
        let others = e.others_using();
        if !others.is_empty() {
            return Err(CmdError::new("vault_in_use", format!("{} has this vault open. Lock RAD there before moving it.", others.join(", "))));
        }
    }

    // Lock and stop serving, so the file is closed and complete (WAL checkpointed).
    do_lock(&app, true).await;
    events::stop(&app);
    stop_embedded(&st).await;

    let moved = crate::files::move_vault(&app, &from, &to).await;
    let open_at = if moved.is_ok() { &to } else { &from };
    // Serve the vault again from wherever it now is, even if the move failed.
    let status = serve_local(&app, &st, open_at, moved.is_ok().then(|| from.to_str()).flatten()).await;
    moved?;
    status
}

/// Built-in models: downloads go to the app's data folder; the copy bundled with the app (and,
/// in dev builds, the repo's `models/` folder) is used first so nothing needs downloading.
fn model_dirs(app: &AppHandle) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
    let dir = app.path().app_data_dir().map(|d| d.join("models")).unwrap_or_else(|_| "models".into());
    let mut bundled: Vec<std::path::PathBuf> = app.path().resource_dir().map(|d| d.join("models")).into_iter().collect();
    if cfg!(debug_assertions) {
        bundled.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    }
    (dir, bundled)
}

#[tauri::command]
pub async fn connect(st: State<'_, App>, url: String) -> CmdResult<Value> {
    stop_embedded(&st).await;
    let url = normalize_url(&url);
    let token = secrets::get(&st.cfg_dir, &url);
    st.client.set_server(Some(url.clone()), token);
    let status = st.client.json(Method::GET, "/v1/status", None).await?;
    if status["api_version"].as_u64().is_none() {
        return Err(CmdError::new("protocol", "That address doesn't look like a RAD server"));
    }
    let mut c = st.config();
    c.mode = "server".into();
    c.server_url = Some(url);
    c.server_name = status["name"].as_str().map(String::from);
    st.save_config(c);
    Ok(status)
}

#[tauri::command]
pub async fn status(st: State<'_, App>) -> CmdResult<Value> {
    st.client.json(Method::GET, "/v1/status", None).await
}

#[tauri::command]
pub async fn pair(st: State<'_, App>, code: String, device_name: Option<String>) -> CmdResult<Value> {
    let cfg = st.config();
    let name = device_name.filter(|n| !n.trim().is_empty()).unwrap_or(cfg.device_name.clone());
    let resp = st
        .client
        .json(Method::POST, "/v1/pair", Some(json!({"code": code.trim(), "device_name": name, "os": std::env::consts::OS})))
        .await?;
    let token = resp["device_token"].as_str().ok_or_else(|| CmdError::new("protocol", "no token"))?.to_string();
    let url = st.client.base()?;
    secrets::set(&st.cfg_dir, &url, &token)?;
    st.client.set_token(Some(token));
    let mut c = st.config();
    c.device_name = name;
    st.save_config(c);
    st.client.json(Method::GET, "/v1/status", None).await
}

#[tauri::command]
pub async fn forget_server(app: AppHandle, st: State<'_, App>) -> CmdResult<()> {
    do_lock(&app, false).await;
    let cfg = st.config();
    if cfg.mode == "local" {
        stop_embedded(&st).await;
    } else if let Ok(url) = st.client.base() {
        secrets::delete(&st.cfg_dir, &url);
    }
    st.client.set_server(None, None);
    let mut c = st.config();
    c.mode = "server".into();
    c.local_vault = None;
    c.server_url = None;
    c.server_name = None;
    st.save_config(c);
    Ok(())
}

// ------------------------------------------------------------------ vault lock state

fn after_unlock(app: &AppHandle, session: &str) {
    let st = app.state::<App>();
    st.client.set_session(Some(session.to_string()));
    st.touch();
    events::start(app);
    let _ = app.emit("rad-unlocked", ());
}

#[tauri::command]
pub async fn init_vault(app: AppHandle, st: State<'_, App>, passphrase: String) -> CmdResult<String> {
    let r = st.client.json(Method::POST, "/v1/vault/init", Some(json!({ "passphrase": passphrase }))).await?;
    after_unlock(&app, r["session"].as_str().unwrap_or_default());
    Ok(r["recovery_key"].as_str().unwrap_or_default().to_string())
}

#[tauri::command]
pub async fn unlock(app: AppHandle, st: State<'_, App>, passphrase: String) -> CmdResult<()> {
    let r = st.client.json(Method::POST, "/v1/unlock", Some(json!({ "passphrase": passphrase }))).await?;
    after_unlock(&app, r["session"].as_str().unwrap_or_default());
    Ok(())
}

#[tauri::command]
pub async fn unlock_recovery(app: AppHandle, st: State<'_, App>, recovery_key: String, new_passphrase: String) -> CmdResult<String> {
    let r = st
        .client
        .json(Method::POST, "/v1/unlock/recovery", Some(json!({ "recovery_key": recovery_key, "new_passphrase": new_passphrase })))
        .await?;
    after_unlock(&app, r["session"].as_str().unwrap_or_default());
    Ok(r["recovery_key"].as_str().unwrap_or_default().to_string())
}

/// Forget the session locally and tell every window.
pub fn handle_locked(app: &AppHandle) {
    let st = app.state::<App>();
    st.client.set_session(None);
    app.state::<crate::mail::Mail>().disconnect();
    for (_, t) in st.chat_tasks.lock().unwrap().drain() {
        t.abort();
    }
    let _ = app.emit("rad-locked", ());
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
}

pub async fn do_lock(app: &AppHandle, server: bool) {
    let st = app.state::<App>();
    // Standalone: this app is the only user, so locking always removes the key from memory.
    let server = server || st.config().mode == "local";
    if st.client.has_session() {
        let path = if server { "/v1/vault/lock" } else { "/v1/session/end" };
        let _ = st.client.json(Method::POST, path, None).await;
    }
    events::stop(app);
    handle_locked(app);
}

#[tauri::command]
pub async fn lock(app: AppHandle, server: Option<bool>) -> CmdResult<()> {
    do_lock(&app, server.unwrap_or(false)).await;
    Ok(())
}

#[tauri::command]
pub fn activity(st: State<'_, App>) {
    st.touch();
}

// ------------------------------------------------------------------ generic API

#[tauri::command]
pub async fn api(app: AppHandle, st: State<'_, App>, method: String, path: String, body: Option<Value>) -> CmdResult<Value> {
    if !path.starts_with("/v1/") {
        return Err(CmdError::new("bad_request", "invalid path"));
    }
    let r = st.client.json(self::method(&method)?, &path, body).await;
    if let Err(e) = &r {
        if e.code == "session_required" || e.code == "vault_locked" {
            handle_locked(&app);
        }
    }
    r
}

// ------------------------------------------------------------------ streaming

async fn stream_sse(st: &App, path: &str, body: Value, on_event: &Channel<Value>) -> CmdResult<()> {
    let rb = st.client.request(&st.client.stream_http, Method::POST, path)?.json(&body);
    let resp = st.client.send(rb).await?;
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::<u8>::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(CmdError::offline)?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(data) = line.trim_end().strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                    let _ = on_event.send(v);
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn chat_send(app: AppHandle, chat_id: i64, request: Value, on_event: Channel<Value>) -> CmdResult<()> {
    let handle = app.clone();
    let ch = on_event.clone();
    let task = tokio::spawn(async move {
        let st = handle.state::<App>();
        stream_sse(&st, &format!("/v1/chats/{chat_id}/messages"), request, &ch).await
    });
    app.state::<App>().chat_tasks.lock().unwrap().insert(chat_id, task.abort_handle());
    let res = task.await;
    app.state::<App>().chat_tasks.lock().unwrap().remove(&chat_id);
    match res {
        Ok(r) => r,
        Err(e) if e.is_cancelled() => {
            let _ = on_event.send(json!({"type": "stopped"}));
            Ok(())
        }
        Err(e) => Err(CmdError::new("internal", e.to_string())),
    }
}

#[tauri::command]
pub fn chat_stop(st: State<'_, App>, chat_id: i64) {
    if let Some(t) = st.chat_tasks.lock().unwrap().remove(&chat_id) {
        t.abort();
    }
}

#[tauri::command]
pub async fn pull_model(st: State<'_, App>, provider_id: i64, model: String, on_event: Channel<Value>) -> CmdResult<()> {
    stream_sse(&st, &format!("/v1/providers/{provider_id}/pull"), json!({ "model": model }), &on_event).await
}

// ------------------------------------------------------------------ uploads

fn enc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

pub async fn upload_bytes(app: &AppHandle, project_id: i64, name: &str, source: &str, data: Vec<u8>) -> CmdResult<Value> {
    let st = app.state::<App>();
    let rb = st
        .client
        .request(&st.client.stream_http, Method::POST, &format!("/v1/projects/{project_id}/documents"))?
        .header(HEADER_FILENAME, enc(name))
        .header(HEADER_SOURCE, source)
        .body(data);
    let resp = st.client.send(rb).await?;
    resp.json().await.map_err(|e| CmdError::new("protocol", e.to_string()))
}

#[derive(Deserialize)]
pub struct UploadPathsArgs {
    project_id: i64,
    paths: Vec<String>,
    source: Option<String>,
}

fn hash_file(path: &std::path::Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((h.finalize().to_hex().to_string(), total))
}

#[tauri::command]
pub async fn upload_paths(app: AppHandle, args: UploadPathsArgs) -> CmdResult<Vec<Value>> {
    let st = app.state::<App>();
    let source = args.source.unwrap_or_else(|| "upload".into());
    let mut out = Vec::new();
    let total_files = args.paths.len();
    for (i, p) in args.paths.iter().enumerate() {
        let path = std::path::PathBuf::from(p);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        if path.is_dir() {
            out.push(json!({"error": format!("{name} is a folder — zip it or drop the files inside")}));
            continue;
        }
        let _ = app.emit("rad-upload-progress", json!({"name": name, "index": i, "count": total_files, "phase": "hashing"}));
        let p2 = path.clone();
        let (hash, size) = match tokio::task::spawn_blocking(move || hash_file(&p2)).await {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => {
                out.push(json!({"error": format!("{name}: {e}")}));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        // Already in the vault? Link instead of re-uploading.
        let exists = st.client.request(&st.client.http, Method::HEAD, &format!("/v1/blobs/{hash}"))?.send().await.map(|r| r.status().is_success()).unwrap_or(false);
        let res = if exists {
            st.client
                .json(Method::POST, &format!("/v1/projects/{}/documents/from-blob", args.project_id), Some(json!({"hash": hash, "filename": name, "source_kind": source})))
                .await
        } else {
            let file = tokio::fs::File::open(&path).await.map_err(|e| CmdError::new("io", e.to_string()))?;
            let app2 = app.clone();
            let name2 = name.clone();
            let stream = futures::stream::unfold((file, 0u64), move |(mut f, sent)| {
                let app2 = app2.clone();
                let name2 = name2.clone();
                async move {
                    use tokio::io::AsyncReadExt;
                    let mut buf = vec![0u8; 1 << 20];
                    match f.read(&mut buf).await {
                        Ok(0) => None,
                        Ok(n) => {
                            buf.truncate(n);
                            let sent = sent + n as u64;
                            let _ = app2.emit("rad-upload-progress", json!({"name": name2, "index": i, "count": total_files, "phase": "uploading", "sent": sent, "total": size}));
                            Some((Ok::<_, std::io::Error>(buf), (f, sent)))
                        }
                        Err(e) => Some((Err(e), (f, sent))),
                    }
                }
            });
            let rb = st
                .client
                .request(&st.client.stream_http, Method::POST, &format!("/v1/projects/{}/documents", args.project_id))?
                .header(HEADER_FILENAME, enc(&name))
                .header(HEADER_SOURCE, &source)
                .header(reqwest::header::CONTENT_LENGTH, size)
                .body(reqwest::Body::wrap_stream(stream));
            match st.client.send(rb).await {
                Ok(r) => r.json::<Value>().await.map_err(|e| CmdError::new("protocol", e.to_string())),
                Err(e) => Err(e),
            }
        };
        match res {
            Ok(v) => out.push(v),
            Err(e) => out.push(json!({"error": format!("{name}: {}", e.message)})),
        }
    }
    let _ = app.emit("rad-upload-progress", json!({"phase": "done", "count": total_files}));
    if !app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false) && total_files > 0 {
        crate::notify(&app, "Upload finished", &format!("{total_files} file(s) added to your vault"));
    }
    Ok(out)
}

/// Raw binary upload from the webview (paste, drag of in-memory data, recordings).
/// Headers: x-project-id, x-filename (percent-encoded), x-source.
#[tauri::command]
pub async fn upload_raw(app: AppHandle, request: Request<'_>) -> CmdResult<Value> {
    let h = request.headers();
    let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(String::from);
    let project_id: i64 = get("x-project-id").and_then(|v| v.parse().ok()).ok_or_else(|| CmdError::new("bad_request", "missing project"))?;
    let name = get("x-filename").map(|n| percent_encoding::percent_decode_str(&n).decode_utf8_lossy().into_owned()).unwrap_or_else(|| "pasted".into());
    let source = get("x-source").unwrap_or_else(|| "paste".into());
    let InvokeBody::Raw(data) = request.body() else {
        return Err(CmdError::new("bad_request", "expected binary body"));
    };
    upload_bytes(&app, project_id, &name, &source, data.clone()).await
}

#[tauri::command]
pub async fn capture_screenshot(app: AppHandle, project_id: Option<i64>) -> CmdResult<Option<Value>> {
    // Hide our windows so they aren't in the shot.
    let main_visible = app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false);
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
    if main_visible {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let r = crate::capture::capture_and_upload(&app, project_id).await;
    if main_visible {
        crate::show_main(&app);
    }
    r
}

/// Saves a decrypted copy of a document where the user chooses (explicit export).
#[tauri::command]
pub async fn export_document(st: State<'_, App>, document_id: i64, dest: String) -> CmdResult<()> {
    let rb = st.client.request(&st.client.stream_http, Method::GET, &format!("/v1/documents/{document_id}/content"))?;
    let resp = st.client.send(rb).await?;
    let mut file = tokio::fs::File::create(&dest).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    let mut s = resp.bytes_stream();
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = s.next().await {
        let chunk = chunk.map_err(CmdError::offline)?;
        file.write_all(&chunk).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    }
    file.flush().await.map_err(|e| CmdError::new("io", e.to_string()))?;
    Ok(())
}

/// Writes text the user explicitly chose to save (recovery kit, chat export).
#[tauri::command]
pub async fn save_text_file(dest: String, content: String) -> CmdResult<()> {
    tokio::fs::write(&dest, content).await.map_err(|e| CmdError::new("io", e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ------------------------------------------------------------------ windows

#[tauri::command]
pub fn open_main(app: AppHandle, route: Option<Value>) {
    crate::show_main(&app);
    if let Some(r) = route {
        let _ = app.emit_to("main", "rad-navigate", r);
    }
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
}

#[tauri::command]
pub fn hide_quick(app: AppHandle) {
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
}

#[tauri::command]
pub fn toggle_quick_cmd(app: AppHandle) {
    crate::toggle_quick(&app);
}

/// Gilgamesh's own window (just his picture, e.g. full screen on a second monitor; the main
/// window drives it): `open` true opens or shows it, false closes it, null only asks. Returns
/// whether it's open. It reopens where it was last (the window-state plugin).
#[tauri::command]
pub fn gilgamesh_window(app: AppHandle, open: Option<bool>) -> CmdResult<bool> {
    let w = app.get_webview_window("gilgamesh");
    match (open, w) {
        (None, w) => Ok(w.is_some()),
        (Some(false), w) => {
            if let Some(w) = w {
                let _ = w.close();
            }
            Ok(false)
        }
        (Some(true), Some(w)) => {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            Ok(true)
        }
        (Some(true), None) => {
            let w = tauri::WebviewWindowBuilder::new(&app, "gilgamesh", tauri::WebviewUrl::App("index.html?view=gilgamesh".into()))
                .title("Gilgamesh")
                .inner_size(720.0, 900.0)
                .min_inner_size(240.0, 300.0)
                .background_color(tauri::window::Color(0, 0, 0, 255))
                .build()
                .map_err(|e| CmdError::new("window", e.to_string()))?;
            // The main window stops sending to it once it's gone.
            let a = app.clone();
            w.on_window_event(move |ev| {
                if let tauri::WindowEvent::Destroyed = ev {
                    let _ = a.emit_to("main", "gil-stage-closed", ());
                }
            });
            Ok(true)
        }
    }
}
