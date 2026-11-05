//! Collections: one place on this computer for folders and files the owner drops on RAD — PDFs,
//! music (MP3, FLAC, M4A) and films (MP4). It is a plain folder tree in the app's data folder
//! (`collections/`), so dropped folders keep their shape. Files are opened, watched or listened
//! to right here (served by `rad://localhost/collections/<path>`, with Range for seeking), and can
//! be moved on to Media: PDFs into Books, music and films into RAD's media folder (`media/music`,
//! `media/video`) for the Music and Video players.
//!
//! Paths given to the commands are relative to the collection root, `/`-separated; "" is the root.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::client::{CmdError, CmdResult};

/// How deep a dropped folder is followed.
const MAX_DEPTH: usize = 24;
/// The most one Range answer carries when the request leaves the end open.
const CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Default)]
pub struct Collections {
    /// One import or move at a time (they rename and copy within the same tree).
    busy: Mutex<()>,
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn data_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(d) = std::env::var("RAD_COLLECTIONS_DATA") {
        return PathBuf::from(d);
    }
    app.path().app_data_dir().unwrap_or_else(|_| ".".into())
}

pub fn root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    data_dir(app).join("collections")
}

fn media_root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    data_dir(app).join("media")
}

pub fn ext(name: &str) -> String {
    Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Pdf,
    Audio,
    Video,
}

pub fn kind_of(name: &str) -> Option<Kind> {
    match ext(name).as_str() {
        "pdf" => Some(Kind::Pdf),
        "mp3" | "flac" | "m4a" => Some(Kind::Audio),
        "mp4" => Some(Kind::Video),
        _ => None,
    }
}

fn mime(name: &str) -> &'static str {
    match ext(name).as_str() {
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// A relative path inside the collection, checked: no `..`, no absolute parts, nothing hidden.
pub fn rel(path: &str) -> CmdResult<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        if part == "." || part == ".." || part.starts_with('.') || part.contains('\\') || part.contains(':') || part.chars().any(char::is_control) {
            return Err(CmdError::new("bad_request", "invalid path"));
        }
        out.push(part);
    }
    if out.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(CmdError::new("bad_request", "invalid path"));
    }
    Ok(out)
}

fn rel_string(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// A name that is safe in a folder and still reads like the original.
fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c == '/' || c == '\\' || c == ':' || c.is_control() { '_' } else { c }).collect();
    let n = n.trim().trim_start_matches('.').trim().to_string();
    if n.is_empty() {
        "Untitled".into()
    } else {
        n.chars().take(150).collect()
    }
}

/// `name`, or `name (2)`, `name (3)`… — whichever isn't taken in `dir`.
fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let p = Path::new(name);
    let (stem, dot_ext) = match p.extension() {
        Some(e) if !p.file_stem().unwrap_or_default().is_empty() => (p.file_stem().unwrap().to_string_lossy().into_owned(), format!(".{}", e.to_string_lossy())),
        _ => (name.to_string(), String::new()),
    };
    (2..)
        .map(|i| format!("{stem} ({i}){dot_ext}"))
        .find(|n| !dir.join(n).exists())
        .unwrap()
}

fn modified(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Files Collections takes, under `dir` (itself included if it's a file), by relative path.
fn walk(dir: &Path, base: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(md) = std::fs::symlink_metadata(dir) else { return };
    if md.is_file() {
        if kind_of(&dir.to_string_lossy()).is_some() {
            out.push((dir.strip_prefix(base).unwrap_or(dir).to_path_buf(), md.len()));
        }
        return;
    }
    if !md.is_dir() || depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).collect();
    entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
    for e in entries {
        walk(&e.path(), base, depth + 1, out);
    }
}

#[derive(Serialize)]
pub struct Entry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: i64,
    kind: Option<Kind>,
    /// Folders: the files in it, subfolders included.
    count: u64,
    /// Folders: what's in it, by kind.
    pdf: u64,
    audio: u64,
    video: u64,
}

pub fn list_in(root: &Path, dir: &str) -> CmdResult<Vec<Entry>> {
    let r = rel(dir)?;
    let full = root.join(&r);
    std::fs::create_dir_all(root).map_err(io)?;
    let rd = std::fs::read_dir(&full).map_err(|_| CmdError::new("not_found", "That folder isn't in Collections any more"))?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        let path = rel_string(&r.join(&name));
        if md.is_dir() {
            let mut files = Vec::new();
            walk(&e.path(), &e.path(), 0, &mut files);
            let by = |k: Kind| files.iter().filter(|(p, _)| kind_of(&p.to_string_lossy()) == Some(k)).count() as u64;
            out.push(Entry { pdf: by(Kind::Pdf), audio: by(Kind::Audio), video: by(Kind::Video), count: files.len() as u64, size: files.iter().map(|f| f.1).sum(), name, path, is_dir: true, modified: modified(&md), kind: None });
        } else if md.is_file() {
            let kind = kind_of(&name);
            if kind.is_none() {
                continue;
            }
            out.push(Entry { name, path, is_dir: false, size: md.len(), modified: modified(&md), kind, count: 1, pdf: 0, audio: 0, video: 0 });
        }
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// Copies one dropped file or folder into `into` (inside `root`). Folders keep their shape and
/// only the files Collections takes; a name that's taken gets " (2)". Returns (added, skipped).
pub fn import_in(root: &Path, src: &Path, into: &Path, mut progress: impl FnMut(&str)) -> CmdResult<(u64, u64)> {
    let md = std::fs::metadata(src).map_err(|e| CmdError::new("io", format!("{}: {e}", src.display())))?;
    let dest_dir = root.join(into);
    std::fs::create_dir_all(&dest_dir).map_err(io)?;
    let name = safe_name(&src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    // Not into itself (a collection folder dropped back on Collections).
    if let (Ok(s), Ok(d)) = (src.canonicalize(), dest_dir.canonicalize()) {
        if d.starts_with(&s) {
            return Err(CmdError::new("bad_request", "A folder can't be copied into itself"));
        }
    }
    if md.is_file() {
        if kind_of(&name).is_none() {
            return Ok((0, 1));
        }
        let target = dest_dir.join(free_name(&dest_dir, &name));
        progress(&name);
        std::fs::copy(src, &target).map_err(|e| CmdError::new("io", format!("{name}: {e}")))?;
        return Ok((1, 0));
    }
    let mut files = Vec::new();
    walk(src, src, 0, &mut files);
    let mut all = 0u64;
    count_files(src, 0, &mut all);
    if files.is_empty() {
        return Ok((0, all));
    }
    let top = dest_dir.join(free_name(&dest_dir, &name));
    let mut added = 0;
    for (r, _) in &files {
        let target = top.join(r);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        progress(&r.to_string_lossy());
        std::fs::copy(src.join(r), &target).map_err(|e| CmdError::new("io", format!("{}: {e}", r.display())))?;
        added += 1;
    }
    Ok((added, all.saturating_sub(added)))
}

fn count_files(dir: &Path, depth: usize, n: &mut u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() && depth < MAX_DEPTH => count_files(&e.path(), depth + 1, n),
            Ok(t) if t.is_file() => *n += 1,
            _ => {}
        }
    }
}

/// Removes folders left empty under (and including) `dir`, but never `stop`.
fn prune(dir: &Path, stop: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                prune(&e.path(), stop);
            }
        }
    }
    if dir != stop {
        // Only succeeds when empty (a stray .DS_Store goes too).
        let _ = std::fs::remove_file(dir.join(".DS_Store"));
        let _ = std::fs::remove_dir(dir);
    }
}

/// Moves a file, falling back to copy + delete across volumes.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// What moving to Media did: the new homes of music and films (for the players' playlists), and
/// the PDFs still to go to Books (relative paths, absolute paths).
#[derive(Default, Serialize)]
pub struct Moved {
    music: Vec<String>,
    video: Vec<String>,
    #[serde(skip)]
    pdfs: Vec<(PathBuf, PathBuf)>,
}

/// Music and films under `paths` go to `media/music` and `media/video`, keeping the folder they
/// were in (an album stays an album). PDFs are only gathered: Books copies them in itself.
pub fn to_media_in(root: &Path, media: &Path, paths: &[String]) -> CmdResult<Moved> {
    let mut out = Moved::default();
    for p in paths {
        let r = rel(p)?;
        if r.as_os_str().is_empty() {
            continue;
        }
        let full = root.join(&r);
        let mut files = Vec::new();
        walk(&full, root, 0, &mut files);
        for (f, _) in files {
            let src = root.join(&f);
            let name = f.to_string_lossy().into_owned();
            match kind_of(&name) {
                Some(Kind::Pdf) => out.pdfs.push((f.clone(), src)),
                Some(k) => {
                    let dir = media.join(if k == Kind::Audio { "music" } else { "video" });
                    // Keep the folder it was in, relative to what was chosen (or its parent).
                    let sub = f.strip_prefix(r.parent().unwrap_or(Path::new(""))).unwrap_or(&f);
                    let dest_parent = dir.join(sub.parent().unwrap_or(Path::new("")));
                    let fname = sub.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let target = dest_parent.join(free_name(&dest_parent, &fname));
                    move_file(&src, &target).map_err(|e| CmdError::new("io", format!("{fname}: {e}")))?;
                    let s = target.to_string_lossy().into_owned();
                    if k == Kind::Audio { out.music.push(s) } else { out.video.push(s) }
                }
                None => {}
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn collections_list(app: AppHandle, dir: String) -> CmdResult<Value> {
    let entries = list_in(&root(&app), &dir)?;
    Ok(json!({ "dir": rel_string(&rel(&dir)?), "entries": entries }))
}

/// Copies dropped files and folders into `into`; reports each file as `collections-progress`.
#[tauri::command]
pub async fn collections_import(app: AppHandle, paths: Vec<String>, into: String) -> CmdResult<Value> {
    let into = rel(&into)?;
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Collections>();
        let _g = st.busy.lock().unwrap();
        let r = root(&app);
        let (mut added, mut skipped, mut errors) = (0u64, 0u64, Vec::<String>::new());
        for p in &paths {
            let mut n = 0u64;
            let res = import_in(&r, Path::new(p), &into, |name| {
                n += 1;
                let _ = app.emit_to("main", "collections-progress", json!({ "name": name, "done": added + n }));
            });
            match res {
                Ok((a, s)) => {
                    added += a;
                    skipped += s;
                }
                Err(e) => {
                    added += n.saturating_sub(1);
                    errors.push(e.message);
                }
            }
        }
        Ok(json!({ "added": added, "skipped": skipped, "errors": errors }))
    })
    .await
    .map_err(io)?
}

/// Every file in a folder and its subfolders, in name order: [{path, name, kind, size}].
#[tauri::command]
pub fn collections_files(app: AppHandle, dir: String) -> CmdResult<Vec<Value>> {
    let root = root(&app);
    let mut files = Vec::new();
    walk(&root.join(rel(&dir)?), &root, 0, &mut files);
    Ok(files
        .into_iter()
        .map(|(p, size)| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            json!({ "path": rel_string(&p), "kind": kind_of(&name), "name": name, "size": size })
        })
        .collect())
}

#[tauri::command]
pub fn collections_mkdir(app: AppHandle, dir: String, name: String) -> CmdResult<String> {
    let parent = root(&app).join(rel(&dir)?);
    let name = free_name(&parent, &safe_name(&name));
    std::fs::create_dir_all(parent.join(&name)).map_err(io)?;
    Ok(rel_string(&rel(&dir)?.join(name)))
}

#[tauri::command]
pub fn collections_rename(app: AppHandle, path: String, name: String) -> CmdResult<String> {
    let r = rel(&path)?;
    if r.as_os_str().is_empty() {
        return Err(CmdError::new("bad_request", "invalid path"));
    }
    let root = root(&app);
    let from = root.join(&r);
    let old = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut name = safe_name(&name);
    // Files keep their type: "Song" for "Song.mp3" stays an MP3.
    if from.is_file() && ext(&name) != ext(&old) {
        name = format!("{name}.{}", ext(&old));
    }
    if name == old {
        return Ok(path);
    }
    let parent = from.parent().unwrap_or(&root).to_path_buf();
    let name = if name.to_lowercase() == old.to_lowercase() { name } else { free_name(&parent, &name) };
    std::fs::rename(&from, parent.join(&name)).map_err(io)?;
    Ok(rel_string(&r.with_file_name(name)))
}

#[tauri::command]
pub fn collections_delete(app: AppHandle, paths: Vec<String>) -> CmdResult<()> {
    let root = root(&app);
    for p in paths {
        let r = rel(&p)?;
        if r.as_os_str().is_empty() {
            continue;
        }
        let full = root.join(r);
        let res = if full.is_dir() { std::fs::remove_dir_all(&full) } else { std::fs::remove_file(&full) };
        res.map_err(io)?;
    }
    Ok(())
}

/// Moves files (and everything in folders) to Media: PDFs into Books, music and films into RAD's
/// media folder. Returns {music: [path], video: [path], books: n, errors: [..]}.
#[tauri::command]
pub async fn collections_to_media(app: AppHandle, paths: Vec<String>) -> CmdResult<Value> {
    let (moved, root) = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let st = app.state::<Collections>();
            let _g = st.busy.lock().unwrap();
            let root = root(&app);
            to_media_in(&root, &media_root(&app), &paths).map(|m| (m, root))
        })
        .await
        .map_err(io)??
    };
    let mut errors = Vec::new();
    let mut books = 0;
    if !moved.pdfs.is_empty() {
        let abs: Vec<String> = moved.pdfs.iter().map(|(_, a)| a.to_string_lossy().into_owned()).collect();
        let res = crate::books::books_import(app.clone(), abs).await?;
        for ((_, a), r) in moved.pdfs.iter().zip(res) {
            if let Some(e) = r.get("error").and_then(Value::as_str) {
                errors.push(e.to_string());
            } else {
                books += 1;
                let _ = std::fs::remove_file(a);
            }
        }
    }
    // Folders emptied by the move go too.
    prune(&root, &root);
    Ok(json!({ "music": moved.music, "video": moved.video, "books": books, "errors": errors }))
}

/// A file's place on disk (to add it to the vault, or show it in the Finder).
#[tauri::command]
pub fn collections_path(app: AppHandle, path: String) -> CmdResult<String> {
    Ok(root(&app).join(rel(&path)?).to_string_lossy().into_owned())
}

// ------------------------------------------------------------------ rad://localhost/collections/

/// Serves `/collections/<path>`: (status, headers, body). Honours `Range: bytes=a-b`.
pub fn serve<R: Runtime>(app: &AppHandle<R>, path: &str, range: Option<&str>) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
    let fail = |s: u16| (s, Vec::new(), Vec::new());
    let raw = path.trim_start_matches("/collections/");
    let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() else { return fail(400) };
    let Ok(r) = rel(&decoded) else { return fail(403) };
    let root = root(app);
    let full = root.join(&r);
    // Nothing outside the collection, even through a link.
    match (full.canonicalize(), root.canonicalize()) {
        (Ok(f), Ok(rt)) if f.starts_with(&rt) => {}
        _ => return fail(404),
    }
    let name = r.to_string_lossy().into_owned();
    if kind_of(&name).is_none() {
        return fail(403);
    }
    let Ok(mut file) = std::fs::File::open(&full) else { return fail(404) };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut headers = vec![("Content-Type", mime(&name).to_string()), ("Accept-Ranges", "bytes".to_string())];
    let wanted = range.and_then(|h| parse_range(h, len));
    // Large media without a Range is answered as if it asked for the start.
    let wanted = wanted.or_else(|| (range.is_none() && len > 64 * CHUNK).then(|| (0, CHUNK.min(len) - 1)));
    let Some((start, end)) = wanted else {
        if range.is_some() && len > 0 {
            headers.push(("Content-Range", format!("bytes */{len}")));
            return (416, headers, Vec::new());
        }
        let mut body = Vec::with_capacity(len as usize);
        if file.read_to_end(&mut body).is_err() {
            return fail(500);
        }
        return (200, headers, body);
    };
    let mut body = vec![0u8; (end - start + 1) as usize];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut body).is_err() {
        return fail(500);
    }
    headers.push(("Content-Range", format!("bytes {start}-{end}/{len}")));
    (206, headers, body)
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` → inclusive (start, end), the open end capped at CHUNK.
fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { (s + CHUNK - 1).min(len - 1) } else { b.parse::<u64>().ok()?.min(len - 1) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside() {
        assert!(rel("../x").is_err());
        assert!(rel("a/../../x").is_err());
        assert!(rel("a/.hidden").is_err());
        assert!(rel("C:\\x").is_err());
        assert_eq!(rel("").unwrap(), PathBuf::new());
        assert_eq!(rel("/a//b/").unwrap(), PathBuf::from("a").join("b"));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-1", 10), Some((0, 1)));
        assert_eq!(parse_range("bytes=5-", 10), Some((5, 9)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=0-99", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=0-", 100 * CHUNK), Some((0, CHUNK - 1)));
    }

    #[test]
    fn names() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.mp3"), b"").unwrap();
        assert_eq!(free_name(d.path(), "a.mp3"), "a (2).mp3");
        assert_eq!(free_name(d.path(), "b.mp3"), "b.mp3");
        assert_eq!(safe_name("../x:y"), "_x_y");
    }

    #[test]
    fn import_list_and_move_to_media() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (root, media) = (data.path().join("collections"), data.path().join("media"));
        let album = src.path().join("Album");
        std::fs::create_dir_all(album.join("CD2")).unwrap();
        std::fs::create_dir_all(album.join("empty")).unwrap();
        std::fs::write(album.join("01 One.mp3"), b"one").unwrap();
        std::fs::write(album.join("CD2").join("02 Two.flac"), b"two").unwrap();
        std::fs::write(album.join("booklet.pdf"), b"%PDF").unwrap();
        std::fs::write(album.join("clip.mp4"), b"mp4").unwrap();
        std::fs::write(album.join("cover.jpg"), b"jpg").unwrap();
        std::fs::write(album.join(".DS_Store"), b"x").unwrap();
        std::fs::write(src.path().join("notes.txt"), b"x").unwrap();
        std::fs::write(src.path().join("talk.m4a"), b"m4a").unwrap();

        let mut seen = 0;
        assert_eq!(import_in(&root, &album, Path::new(""), |_| seen += 1).unwrap(), (4, 1));
        assert_eq!(seen, 4);
        assert_eq!(import_in(&root, &src.path().join("notes.txt"), Path::new(""), |_| {}).unwrap(), (0, 1));
        assert_eq!(import_in(&root, &src.path().join("talk.m4a"), Path::new("Album"), |_| {}).unwrap(), (1, 0));
        // The same folder again is a second copy beside the first.
        import_in(&root, &album, Path::new(""), |_| {}).unwrap();
        assert!(root.join("Album (2)").join("CD2").join("02 Two.flac").exists());
        assert!(!root.join("Album").join("empty").exists());
        assert!(!root.join("Album").join("cover.jpg").exists());

        let top = list_in(&root, "").unwrap();
        assert_eq!(top.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Album", "Album (2)"]);
        assert_eq!((top[0].count, top[0].pdf, top[0].audio, top[0].video), (5, 1, 3, 1));
        let inner = list_in(&root, "Album").unwrap();
        assert_eq!(inner.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["CD2", "01 One.mp3", "booklet.pdf", "clip.mp4", "talk.m4a"]);
        assert_eq!(inner[1].kind, Some(Kind::Audio));
        assert!(list_in(&root, "../").is_err());

        let m = to_media_in(&root, &media, &["Album".into()]).unwrap();
        assert_eq!(m.music.len(), 3);
        assert!(media.join("music").join("Album").join("CD2").join("02 Two.flac").exists());
        assert_eq!(m.video, [media.join("video").join("Album").join("clip.mp4").to_string_lossy()]);
        assert_eq!(m.pdfs.len(), 1);
        assert!(root.join("Album").join("booklet.pdf").exists());
        // A single file keeps no folder above it.
        let m = to_media_in(&root, &media, &["Album (2)/CD2/02 Two.flac".into()]).unwrap();
        assert_eq!(m.music, [media.join("music").join("02 Two.flac").to_string_lossy()]);
        prune(&root, &root);
        assert!(!root.join("Album").join("CD2").exists());
        assert!(root.join("Album").exists());
    }
}
