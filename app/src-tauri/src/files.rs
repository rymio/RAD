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

// ------------------------------------------------------------------ moving the vault

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", p.display()))
}

/// Moves a closed vault file to `to`, with any journal file SQLite left next to it. On the
/// same volume this is a rename; otherwise a copy that is read back and compared before the
/// original is deleted. Progress goes out as `rad-vault-move` events.
pub async fn move_vault(app: &tauri::AppHandle, from: &Path, to: &Path) -> CmdResult<()> {
    use tauri::Emitter;
    let (app, from, to) = (app.clone(), from.to_path_buf(), to.to_path_buf());
    tokio::task::spawn_blocking(move || {
        move_files(&from, &to, true, &|done, total| {
            let _ = app.emit("rad-vault-move", json!({"done": done, "total": total}));
        })
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

/// `rename` false forces the copy path (tests).
fn move_files(from: &Path, to: &Path, rename: bool, progress: &dyn Fn(u64, u64)) -> CmdResult<()> {
    // The shared-memory index is rebuilt on open; empty journals are leftovers.
    let _ = std::fs::remove_file(with_suffix(from, "-shm"));
    let mut files = vec![(from.to_path_buf(), to.to_path_buf())];
    for s in ["-wal", "-journal"] {
        let f = with_suffix(from, s);
        match std::fs::metadata(&f) {
            Ok(m) if m.len() > 0 => files.push((f, with_suffix(to, s))),
            Ok(_) => {
                let _ = std::fs::remove_file(&f);
            }
            Err(_) => {}
        }
    }
    // The vault's databases travel with it (their keys are in it).
    let (db_from, db_to) = (rad_server::databases_dir(from), rad_server::databases_dir(to));
    if db_from.is_dir() {
        // Only inside a folder that exists (never create the destination itself).
        if !to.parent().is_some_and(|p| p.is_dir()) {
            return Err(CmdError::new("io", "The destination folder doesn't exist"));
        }
        std::fs::create_dir_all(&db_to).map_err(io)?;
        for e in std::fs::read_dir(&db_from).map_err(io)?.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().ends_with("-shm") {
                let _ = std::fs::remove_file(e.path());
            } else if e.path().is_file() {
                files.push((e.path(), db_to.join(name)));
            }
        }
    }
    let total: u64 = files.iter().filter_map(|(f, _)| std::fs::metadata(f).ok()).map(|m| m.len()).sum();

    if rename && std::fs::rename(from, to).is_ok() {
        for (a, b) in &files[1..] {
            std::fs::rename(a, b).or_else(|_| std::fs::copy(a, b).map(|_| ()).and_then(|_| std::fs::remove_file(a))).map_err(io)?;
        }
    } else {
        // Another volume (an external disk, a network share): copy, verify, then delete.
        let mut done = 0u64;
        let mut copied: Vec<PathBuf> = Vec::new();
        for (a, b) in &files {
            if let Err(e) = copy_verified(a, b, &mut done, total, progress) {
                for c in &copied {
                    let _ = std::fs::remove_file(c);
                }
                let _ = std::fs::remove_dir(&db_to);
                return Err(e);
            }
            copied.push(b.clone());
        }
        for (a, _) in &files {
            let _ = std::fs::remove_file(a);
        }
    }
    let _ = std::fs::remove_dir(&db_from);
    // This computer's (already withdrawn) session folder stays behind otherwise.
    let _ = std::fs::remove_dir_all(rad_server::vault_presence::dir(from));
    progress(total, total);
    Ok(())
}

fn copy_verified(from: &Path, to: &Path, done: &mut u64, total: u64, progress: &dyn Fn(u64, u64)) -> CmdResult<()> {
    use std::io::{Read, Write};
    let before = std::fs::metadata(from).map_err(io)?;
    let tmp = with_suffix(to, ".moving");
    let mut src = std::fs::File::open(from).map_err(io)?;
    let mut dst = std::fs::File::create(&tmp).map_err(io)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut last = 0u64;
    let result = (|| -> CmdResult<()> {
        loop {
            let n = src.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            dst.write_all(&buf[..n]).map_err(io)?;
            *done += n as u64;
            if *done - last >= 16 << 20 {
                last = *done;
                progress(*done, total);
            }
        }
        dst.sync_all().map_err(io)?;
        // Read the copy back: it must be byte-for-byte the original.
        let mut check = blake3::Hasher::new();
        let mut f = std::fs::File::open(&tmp).map_err(io)?;
        loop {
            let n = f.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            check.update(&buf[..n]);
        }
        if check.finalize() != hasher.finalize() {
            return Err(CmdError::new("io", "The copy didn't match the original, so nothing was moved. Check the destination disk and try again."));
        }
        let after = std::fs::metadata(from).map_err(io)?;
        if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
            return Err(CmdError::new("io", "The vault changed while it was being copied, so nothing was moved. Try again."));
        }
        std::fs::rename(&tmp, to).map_err(io)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with_journal(dir: &Path) -> (PathBuf, Vec<u8>, Vec<u8>) {
        let v = dir.join("Personal.radvault");
        let body: Vec<u8> = (0..(9 << 20)).map(|i| (i * 31 % 251) as u8).collect();
        let wal = b"leftover wal".to_vec();
        std::fs::write(&v, &body).unwrap();
        std::fs::write(with_suffix(&v, "-wal"), &wal).unwrap();
        std::fs::write(with_suffix(&v, "-shm"), b"index").unwrap();
        std::fs::create_dir_all(rad_server::vault_presence::dir(&v)).unwrap();
        let dbs = rad_server::databases_dir(&v);
        std::fs::create_dir_all(&dbs).unwrap();
        std::fs::write(dbs.join("db1.db"), b"encrypted db").unwrap();
        std::fs::write(dbs.join("db1.db-shm"), b"index").unwrap();
        (v, body, wal)
    }

    #[test]
    fn move_by_rename_and_by_verified_copy() {
        for rename in [true, false] {
            let td = tempfile::tempdir().unwrap();
            let (src, dst) = (td.path().join("icloud"), td.path().join("nas"));
            std::fs::create_dir_all(&src).unwrap();
            std::fs::create_dir_all(&dst).unwrap();
            let (from, body, wal) = vault_with_journal(&src);
            let to = dst.join("Personal.radvault");
            let seen = std::sync::Mutex::new(Vec::new());
            move_files(&from, &to, rename, &|d, t| seen.lock().unwrap().push((d, t))).unwrap();
            assert_eq!(std::fs::read(&to).unwrap(), body);
            assert_eq!(std::fs::read(with_suffix(&to, "-wal")).unwrap(), wal, "a leftover WAL travels with the vault");
            assert!(!from.exists() && !with_suffix(&from, "-wal").exists() && !with_suffix(&from, "-shm").exists());
            assert!(!rad_server::vault_presence::dir(&from).exists());
            assert!(!with_suffix(&to, ".moving").exists());
            assert_eq!(std::fs::read(rad_server::databases_dir(&to).join("db1.db")).unwrap(), b"encrypted db", "the databases travel with the vault");
            assert!(!rad_server::databases_dir(&from).exists());
            let total = body.len() as u64 + wal.len() as u64 + 12;
            assert_eq!(seen.lock().unwrap().last().copied(), Some((total, total)));
        }
    }

    #[test]
    fn failed_copy_keeps_the_original() {
        let td = tempfile::tempdir().unwrap();
        let (from, body, _) = vault_with_journal(td.path());
        let to = td.path().join("missing-folder").join("Personal.radvault");
        assert!(move_files(&from, &to, false, &|_, _| {}).is_err());
        assert_eq!(std::fs::read(&from).unwrap(), body);
    }
}
