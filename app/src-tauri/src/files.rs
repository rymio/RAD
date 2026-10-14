//! The Files view: browse this computer's files to preview them, add them to the vault, or
//! hand them to a chat as context. Nothing is copied anywhere until the user asks.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};

use crate::client::{CmdError, CmdResult};

const PREVIEW_IMAGE_MAX: u64 = 25 * 1024 * 1024;
const PREVIEW_TEXT_MAX: usize = 256 * 1024;

#[derive(Serialize)]
pub struct Entry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: i64,
    hidden: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(Into::into)
}

fn io(e: std::io::Error) -> CmdError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => CmdError::new("io", "RAD isn't allowed to read this folder. On macOS, allow it under System Settings → Privacy & Security → Files and Folders."),
        std::io::ErrorKind::NotFound => CmdError::new("io", "That folder doesn't exist any more"),
        _ => CmdError::new("io", e.to_string()),
    }
}

/// Shortcuts for the sidebar: home, the usual folders, and mounted drives.
#[tauri::command]
pub fn fs_places() -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(h) = home() {
        out.push(json!({"name": "Home", "path": h, "icon": "home"}));
        for (name, icon) in [("Desktop", "desktop"), ("Documents", "docs"), ("Downloads", "downloads"), ("Pictures", "pictures")] {
            let p = h.join(name);
            if p.is_dir() {
                out.push(json!({"name": name, "path": p, "icon": icon}));
            }
        }
    }
    if cfg!(target_os = "macos") {
        if let Ok(rd) = std::fs::read_dir("/Volumes") {
            for e in rd.flatten() {
                out.push(json!({"name": e.file_name().to_string_lossy(), "path": e.path(), "icon": "drive"}));
            }
        }
    } else if cfg!(windows) {
        for l in b'C'..=b'Z' {
            let p = format!("{}:\\", l as char);
            if Path::new(&p).is_dir() {
                out.push(json!({"name": format!("{}:", l as char), "path": p, "icon": "drive"}));
            }
        }
    } else {
        out.push(json!({"name": "Computer", "path": "/", "icon": "drive"}));
        for base in ["/media", "/mnt"] {
            if let Ok(rd) = std::fs::read_dir(base) {
                for e in rd.flatten().filter(|e| e.path().is_dir()) {
                    out.push(json!({"name": e.file_name().to_string_lossy(), "path": e.path(), "icon": "drive"}));
                }
            }
        }
    }
    out
}

#[tauri::command]
pub async fn fs_list(path: Option<String>) -> CmdResult<Value> {
    let dir = match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => home().ok_or_else(|| CmdError::new("io", "No home folder"))?,
    };
    tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        for e in std::fs::read_dir(&dir).map_err(io)?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Follow symlinks for the type and size, like Finder does.
            let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata());
            let Ok(meta) = meta else { continue };
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
            entries.push(Entry { hidden: name.starts_with('.'), path: e.path().display().to_string(), is_dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, modified, name });
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(json!({
            "path": dir.display().to_string(),
            "parent": dir.parent().map(|p| p.display().to_string()),
            "entries": entries,
        }))
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

fn image_mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// A quick look at a file: images as data, text as text, anything else as facts.
#[tauri::command]
pub async fn fs_preview(path: String) -> CmdResult<Value> {
    tokio::task::spawn_blocking(move || {
        let p = PathBuf::from(&path);
        let meta = std::fs::metadata(&p).map_err(io)?;
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if let Some(mime) = image_mime(&ext).filter(|_| meta.len() <= PREVIEW_IMAGE_MAX) {
            let data = std::fs::read(&p).map_err(io)?;
            return Ok(json!({"kind": "image", "mime": mime, "data": base64::engine::general_purpose::STANDARD.encode(data)}));
        }
        let mut f = std::fs::File::open(&p).map_err(io)?;
        let mut head = vec![0u8; PREVIEW_TEXT_MAX];
        let n = std::io::Read::read(&mut f, &mut head).map_err(io)?;
        head.truncate(n);
        // UTF-8 without NUL bytes (allowing a character cut off at the end of the sample).
        let utf8 = match std::str::from_utf8(&head) {
            Ok(_) => true,
            Err(e) => e.valid_up_to() + 4 >= head.len(),
        };
        if utf8 && !head.contains(&0) {
            return Ok(json!({"kind": "text", "text": String::from_utf8_lossy(&head), "truncated": meta.len() as usize > n}));
        }
        Ok(json!({"kind": "other", "size": meta.len()}))
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

/// Opens a file or folder with the system's default app.
#[tauri::command]
pub fn fs_open(path: String) -> CmdResult<()> {
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| CmdError::new("io", e.to_string()))
}

#[tauri::command]
pub fn fs_reveal(path: String) -> CmdResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| CmdError::new("io", e.to_string()))
}
