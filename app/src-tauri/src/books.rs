//! Books (Media → Books): e-books the owner owns, kept on this computer in the app's data folder
//! (`books/<id>/<original file name>`, with `cover.jpg` and the library list in `books/index.json`).
//! They aren't in the vault unless the owner adds one there ("Add to RAG"), which uploads the
//! file like any other document.
//!
//! PDFs are shown by the reader itself (pdf.js); every other format is opened by
//! `rad_ingest::book` into chapters of HTML, which the reader sanitises and paginates.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::ipc::Response;
use tauri::{AppHandle, Manager, State};

use crate::client::{CmdError, CmdResult};

#[derive(Default)]
pub struct Books {
    /// Guards index.json.
    lock: Mutex<()>,
}

/// Biggest book RAD takes in.
const MAX_BOOK: u64 = 512 * 1024 * 1024;
const FORMATS: &[&str] = &["pdf", "epub", "mobi", "azw", "azw3", "prc", "kf8", "fb2", "txt", "html", "htm", "xhtml"];

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct BookMeta {
    id: String,
    title: String,
    #[serde(default)]
    author: Option<String>,
    format: String,
    /// The file's name in its folder (the original name, which is also its name in the vault).
    file: String,
    size: u64,
    hash: String,
    added: i64,
    #[serde(default)]
    opened: Option<i64>,
    #[serde(default)]
    cover: bool,
    /// Where the owner is in it (the reader's own shape).
    #[serde(default)]
    progress: Value,
    /// The vault document, once it's been added to RAG.
    #[serde(default)]
    vault: Option<i64>,
}

fn root(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("books")
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn check_id(id: &str) -> CmdResult<()> {
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(CmdError::new("bad_request", "invalid book"));
    }
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn load(app: &AppHandle) -> Vec<BookMeta> {
    std::fs::read(root(app).join("index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(app: &AppHandle, list: &[BookMeta]) -> CmdResult<()> {
    let r = root(app);
    std::fs::create_dir_all(&r).map_err(io)?;
    let tmp = r.join("index.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list).map_err(io)?).map_err(io)?;
    std::fs::rename(&tmp, r.join("index.json")).map_err(io)
}

fn find(app: &AppHandle, id: &str) -> CmdResult<BookMeta> {
    check_id(id)?;
    load(app).into_iter().find(|b| b.id == id).ok_or_else(|| CmdError::new("not_found", "That book isn't in the library any more"))
}

fn book_file(app: &AppHandle, b: &BookMeta) -> PathBuf {
    root(app).join(&b.id).join(&b.file)
}

/// A name that is safe as a file name and still reads like the original.
fn safe_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c == '/' || c == '\\' || c == ':' || c.is_control() { '_' } else { c }).collect();
    let n = n.trim().trim_start_matches('.').to_string();
    if n.is_empty() {
        "book".into()
    } else {
        n.chars().take(150).collect()
    }
}

/// A small JPEG of the cover (at most 360 px tall).
fn write_cover(dir: &Path, data: &[u8]) -> bool {
    let Ok(img) = image::load_from_memory(data) else { return false };
    let img = if img.height() > 360 { img.resize(10_000, 360, image::imageops::FilterType::Triangle) } else { img };
    let mut out = std::io::Cursor::new(Vec::new());
    if img.to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).is_err() {
        return false;
    }
    std::fs::write(dir.join("cover.jpg"), out.into_inner()).is_ok()
}

fn with_cover(app: &AppHandle, b: &BookMeta) -> Value {
    let mut v = serde_json::to_value(b).unwrap_or_default();
    let cover = b.cover.then(|| std::fs::read(root(app).join(&b.id).join("cover.jpg")).ok()).flatten();
    v["cover"] = cover.map(|c| Value::String(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(c)))).unwrap_or(Value::Null);
    v
}

#[tauri::command]
pub fn books_list(app: AppHandle, st: State<'_, Books>) -> Vec<Value> {
    let _g = st.lock.lock().unwrap();
    load(&app).iter().map(|b| with_cover(&app, b)).collect()
}

fn import_one(app: &AppHandle, path: &Path) -> Result<(BookMeta, bool), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = rad_ingest::extension(&name);
    if !FORMATS.contains(&ext.as_str()) {
        return Err(format!("{name}: not an e-book RAD can read (PDF, EPUB, MOBI, AZW, AZW3, FB2, TXT or HTML)"));
    }
    let size = std::fs::metadata(path).map_err(|e| format!("{name}: {e}"))?.len();
    if size > MAX_BOOK {
        return Err(format!("{name}: too big (over 512 MB)"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    if let Some(b) = load(app).into_iter().find(|b| b.hash == hash) {
        return Ok((b, true));
    }
    let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
    let mut meta = BookMeta { id: hash[..16].to_string(), title: stem.trim().to_string(), format: ext.clone(), file: safe_name(&name), size, hash, added: now(), ..Default::default() };
    let dir = root(app).join(&meta.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{name}: {e}"))?;
    if ext != "pdf" {
        // It must open (and not be DRM-locked) before it goes in the library.
        let b = match rad_ingest::book::open(&bytes, &name) {
            Ok(b) => b,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("{name}: {e}"));
            }
        };
        if let Some(t) = b.title.filter(|t| t.len() > 1) {
            meta.title = t;
        }
        meta.author = b.author;
        if let Some(c) = b.cover.as_ref().and_then(|k| b.resources.get(k)) {
            meta.cover = write_cover(&dir, &c.data);
        }
    }
    std::fs::write(dir.join(&meta.file), &bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok((meta, false))
}

/// Copies books into the library: [{book, duplicate}] or [{error}], one per path.
#[tauri::command]
pub async fn books_import(app: AppHandle, paths: Vec<String>) -> CmdResult<Vec<Value>> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Books>();
        let mut out = Vec::new();
        for p in paths {
            let r = import_one(&app, Path::new(&p));
            let _g = st.lock.lock().unwrap();
            match r {
                Ok((b, dup)) => {
                    if !dup {
                        let mut list = load(&app);
                        list.push(b.clone());
                        if let Err(e) = save(&app, &list) {
                            out.push(json!({"error": e.message}));
                            continue;
                        }
                    }
                    out.push(json!({"book": with_cover(&app, &b), "duplicate": dup}));
                }
                Err(e) => out.push(json!({ "error": e })),
            }
        }
        Ok(out)
    })
    .await
    .map_err(io)?
}

/// A PDF's bytes (for pdf.js).
#[tauri::command]
pub async fn books_file(app: AppHandle, id: String) -> CmdResult<Response> {
    let b = find(&app, &id)?;
    let bytes = tokio::fs::read(book_file(&app, &b)).await.map_err(io)?;
    Ok(Response::new(bytes))
}

/// A reflowable book: {chapters: [{path, html}], toc: [{title, href, depth}], resources: {path: data URL}}.
#[tauri::command]
pub async fn books_content(app: AppHandle, id: String) -> CmdResult<Value> {
    let b = find(&app, &id)?;
    let path = book_file(&app, &b);
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = std::fs::read(&path).map_err(io)?;
        let book = rad_ingest::book::open(&bytes, &b.file).map_err(|e| CmdError::new("book", e.to_string()))?;
        let b64 = base64::engine::general_purpose::STANDARD;
        let resources: Map<String, Value> = book.resources.iter().map(|(k, r)| (k.clone(), Value::String(format!("data:{};base64,{}", r.mime, b64.encode(&r.data))))).collect();
        Ok(json!({
            "chapters": book.chapters.iter().map(|c| json!({"path": c.path, "html": c.html})).collect::<Vec<_>>(),
            "toc": book.toc.iter().map(|t| json!({"title": t.title, "href": t.href, "depth": t.depth})).collect::<Vec<_>>(),
            "resources": resources,
            "language": book.language,
        }))
    })
    .await
    .map_err(io)?
}

/// Changes a book's details: title, author, progress, opened, vault, or a cover (a data URL, from
/// the reader: a PDF's first page).
#[tauri::command]
pub fn books_update(app: AppHandle, st: State<'_, Books>, id: String, patch: Value) -> CmdResult<Value> {
    check_id(&id)?;
    let _g = st.lock.lock().unwrap();
    let mut list = load(&app);
    let b = list.iter_mut().find(|b| b.id == id).ok_or_else(|| CmdError::new("not_found", "That book isn't in the library any more"))?;
    if let Some(t) = patch.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
        b.title = t.chars().take(300).collect();
    }
    if let Some(a) = patch.get("author") {
        b.author = a.as_str().map(str::trim).filter(|a| !a.is_empty()).map(|a| a.chars().take(200).collect());
    }
    if let Some(p) = patch.get("progress") {
        b.progress = p.clone();
    }
    if let Some(o) = patch.get("opened").and_then(Value::as_i64) {
        b.opened = Some(o);
    }
    if let Some(v) = patch.get("vault") {
        b.vault = v.as_i64();
    }
    if let Some(c) = patch.get("cover").and_then(Value::as_str) {
        let data = c.split_once(',').map(|(_, d)| d).and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
        if let Some(d) = data {
            b.cover = write_cover(&root(&app).join(&b.id), &d);
        }
    }
    let out = with_cover(&app, b);
    save(&app, &list)?;
    Ok(out)
}

#[tauri::command]
pub fn books_delete(app: AppHandle, st: State<'_, Books>, id: String) -> CmdResult<()> {
    check_id(&id)?;
    let _g = st.lock.lock().unwrap();
    let mut list = load(&app);
    list.retain(|b| b.id != id);
    save(&app, &list)?;
    let _ = std::fs::remove_dir_all(root(&app).join(&id));
    Ok(())
}

/// The book's file on disk (to add it to the vault).
#[tauri::command]
pub fn books_path(app: AppHandle, id: String) -> CmdResult<String> {
    let b = find(&app, &id)?;
    Ok(book_file(&app, &b).to_string_lossy().into_owned())
}
