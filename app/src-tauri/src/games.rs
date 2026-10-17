//! Games: 2D games made with ggez (https://ggez.rs) in RAD's Games view, by the user and an AI
//! agent. Their files live in the encrypted "Games" database (see routes/db.rs); this module
//! builds and runs them on this Mac:
//!
//! - the files are written to a build folder, built with `cargo build --release` (one shared
//!   target folder, so ggez is compiled once), and the plain source is deleted afterwards;
//! - the game runs as its own program, in its own window; Stop ends it. Its assets (images,
//!   sounds, fonts — kept encrypted in the database too) are written to a `resources/` folder
//!   next to it for the run, which is where ggez looks for them (`"/player.png"` is
//!   `resources/player.png`), and deleted when it ends.
//!
//! A game is ordinary Rust code running as the user, like any program they build themselves.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::client::{CmdError, CmdResult};

#[derive(Default)]
pub struct Games {
    running: Mutex<HashMap<String, Arc<Mutex<Child>>>>,
    /// Builds in progress (one at a time per game).
    building: Mutex<HashMap<String, Arc<Mutex<Option<Child>>>>>,
}

#[derive(Deserialize)]
pub struct GameFile {
    path: String,
    content: String,
}

/// An asset for a run: its path in `resources/` and its bytes (base64). A path ending in `/`
/// is an (empty) folder.
#[derive(Deserialize)]
pub struct GameAsset {
    path: String,
    #[serde(default)]
    data: String,
}

/// Biggest asset RAD takes in (it's stored in the database and passed to each run).
pub const MAX_ASSET: u64 = 10 * 1024 * 1024;

fn root(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("games")
}

fn check_id(id: &str) -> CmdResult<()> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(CmdError::new("bad_request", "invalid game"));
    }
    Ok(())
}

/// A relative path inside the project (no `..`, no absolute paths).
fn safe_path(p: &str) -> CmdResult<PathBuf> {
    let p = Path::new(p.trim());
    let ok = !p.as_os_str().is_empty() && p.components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if !ok {
        return Err(CmdError::new("bad_request", format!("invalid file path {}", p.display())));
    }
    Ok(p.to_path_buf())
}

/// cargo: on PATH, or where rustup and Homebrew put it (apps don't get the shell's PATH).
fn cargo() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        c.push(PathBuf::from(h).join(".cargo/bin/cargo"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        c.extend(std::env::split_paths(&path).map(|d| d.join("cargo")));
    }
    c.extend(["/opt/homebrew/bin/cargo", "/usr/local/bin/cargo"].map(PathBuf::from));
    c.into_iter().find(|p| p.is_file())
}

/// The package name from Cargo.toml (the program cargo builds).
fn package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_package = l == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = l.strip_prefix("name") {
                let v = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"').to_string();
                if !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Builds a game; streams cargo's output as {"o": line}. Returns (ok, the program, cargo's
/// output). The plain source is deleted when it's done.
fn build(app: &AppHandle, g: &Games, id: &str, files: &[GameFile], on: &dyn Fn(Value)) -> CmdResult<(bool, Option<PathBuf>, String)> {
    check_id(id)?;
    let cargo = cargo().ok_or_else(|| CmdError::new("no_rust", "Rust isn't installed on this Mac. Install it from https://rustup.rs, then try again."))?;
    let toml = files.iter().find(|f| f.path == "Cargo.toml").ok_or_else(|| CmdError::new("bad_request", "The game has no Cargo.toml"))?;
    let name = package_name(&toml.content).ok_or_else(|| CmdError::new("bad_request", "Cargo.toml needs [package] with a name"))?;
    // One build per game at a time (they share its build folder).
    let slot = Arc::new(Mutex::new(None));
    {
        let mut b = g.building.lock().unwrap();
        if b.contains_key(id) {
            return Err(CmdError::new("busy", "The game is already being built"));
        }
        b.insert(id.to_string(), slot.clone());
    }
    let r = build_in(app, &cargo, id, files, &name, &slot, on);
    g.building.lock().unwrap().remove(id);
    r
}

fn build_in(app: &AppHandle, cargo: &Path, id: &str, files: &[GameFile], name: &str, slot: &Arc<Mutex<Option<Child>>>, on: &dyn Fn(Value)) -> CmdResult<(bool, Option<PathBuf>, String)> {
    let r = root(app);
    let work = r.join("work").join(id);
    let target = r.join("target");
    let _ = std::fs::remove_dir_all(&work);
    for f in files {
        let p = work.join(safe_path(&f.path)?);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| CmdError::new("io", e.to_string()))?;
        }
        std::fs::write(&p, &f.content).map_err(|e| CmdError::new("io", e.to_string()))?;
    }
    on(json!({"phase": "building"}));
    let child = Command::new(cargo)
        .args(["build", "--release", "--color", "never"])
        .current_dir(&work)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("build", format!("Couldn't start cargo: {e}")))?;
    *slot.lock().unwrap() = Some(child);
    let stderr = slot.lock().unwrap().as_mut().and_then(|c| c.stderr.take());
    let mut out = String::new();
    if let Some(e) = stderr {
        for line in BufReader::new(e).lines().map_while(Result::ok) {
            on(json!({"o": format!("{line}\n")}));
            out.push_str(&line);
            out.push('\n');
        }
    }
    let status = slot.lock().unwrap().take().map(|mut c| c.wait());
    // The source of truth is the encrypted database: don't leave plain copies around.
    let _ = std::fs::remove_dir_all(&work);
    let ok = matches!(status, Some(Ok(s)) if s.success());
    let exe = target.join("release").join(name);
    Ok((ok && exe.is_file(), ok.then_some(exe), out))
}

/// Builds without running (what the agent uses to check its code). Returns {ok, output}
/// with the errors (and warnings) cargo printed.
#[tauri::command]
pub async fn game_build(app: AppHandle, id: String, files: Vec<GameFile>) -> CmdResult<Value> {
    tauri::async_runtime::spawn_blocking(move || {
        let g = app.state::<Games>();
        let (ok, _, out) = build(&app, &g, &id, &files, &|_| {})?;
        // The errors and their context, not the "Compiling …" noise.
        let useful: String = out.lines().filter(|l| !l.trim_start().starts_with("Compiling") && !l.trim_start().starts_with("Finished")).collect::<Vec<_>>().join("\n");
        let tail: String = useful.chars().rev().take(12_000).collect::<Vec<_>>().into_iter().rev().collect();
        Ok(json!({"ok": ok, "output": tail}))
    })
    .await
    .map_err(|e| CmdError::new("build", e.to_string()))?
}

/// Builds and runs a game. Events: {"phase": "building" | "running" | "exited", "code"?},
/// {"o": text} for cargo's and the game's output. Resolves when the game has started (or the
/// build failed: {"ok": false}).
#[tauri::command]
pub async fn game_run(app: AppHandle, id: String, files: Vec<GameFile>, assets: Option<Vec<GameAsset>>, on_event: Channel<Value>) -> CmdResult<Value> {
    {
        let g = app.state::<Games>();
        if g.running.lock().unwrap().contains_key(&id) {
            return Err(CmdError::new("busy", "The game is already running — stop it first"));
        }
        if g.building.lock().unwrap().contains_key(&id) {
            return Err(CmdError::new("busy", "The game is being built"));
        }
    }
    let ch = on_event.clone();
    let a = app.clone();
    let id2 = id.clone();
    let built = tauri::async_runtime::spawn_blocking(move || {
        let g = a.state::<Games>();
        build(&a, &g, &id2, &files, &|v| {
            let _ = ch.send(v);
        })
    })
    .await
    .map_err(|e| CmdError::new("build", e.to_string()))??;
    let (ok, exe, _) = built;
    let Some(exe) = exe.filter(|_| ok) else {
        let _ = on_event.send(json!({"phase": "exited", "code": null, "build_failed": true}));
        return Ok(json!({"ok": false}));
    };

    // Its own folder: the program (an APFS clone of the build, so instant) with `resources/`
    // next to it, where ggez finds the assets.
    let run_dir = root(&app).join("run").join(&id);
    let run_exe = prepare_run(&run_dir, &exe, assets.as_deref().unwrap_or_default())?;
    let mut child = Command::new(&run_exe)
        .current_dir(&run_dir)
        .env("RUST_BACKTRACE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CmdError::new("run", format!("Couldn't start the game: {e}")))?;
    let _ = on_event.send(json!({"phase": "running"}));
    let (so, se) = (child.stdout.take(), child.stderr.take());
    let child = Arc::new(Mutex::new(child));
    app.state::<Games>().running.lock().unwrap().insert(id.clone(), child.clone());
    let _ = app.emit("games-changed", ());
    for r in [so.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), se.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        let ch = on_event.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                let _ = ch.send(json!({"o": format!("{line}\n")}));
            }
        });
    }
    // Wait for it to end.
    let a = app.clone();
    std::thread::spawn(move || {
        let code = loop {
            if let Ok(Some(st)) = child.lock().unwrap().try_wait() {
                break st.code();
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        };
        a.state::<Games>().running.lock().unwrap().remove(&id);
        // The assets' plain copies go (the database keeps them, encrypted).
        let _ = std::fs::remove_dir_all(root(&a).join("run").join(&id));
        let _ = on_event.send(json!({"phase": "exited", "code": code}));
        let _ = a.emit("games-changed", ());
    });
    Ok(json!({"ok": true}))
}

/// The run folder: a copy of the program and `resources/` with the assets. Returns the program.
fn prepare_run(run_dir: &Path, exe: &Path, assets: &[GameAsset]) -> CmdResult<PathBuf> {
    use base64::Engine as _;
    let io = |e: std::io::Error| CmdError::new("io", e.to_string());
    let _ = std::fs::remove_dir_all(run_dir);
    let res = run_dir.join("resources");
    std::fs::create_dir_all(&res).map_err(io)?;
    for a in assets {
        let folder = a.path.ends_with('/');
        let p = res.join(safe_path(a.path.trim_end_matches('/'))?);
        if folder {
            std::fs::create_dir_all(&p).map_err(io)?;
            continue;
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(a.data.as_bytes()).map_err(|_| CmdError::new("bad_request", format!("{} isn't valid data", a.path)))?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
        std::fs::write(&p, bytes).map_err(io)?;
    }
    let run_exe = run_dir.join(exe.file_name().unwrap_or_else(|| std::ffi::OsStr::new("game")));
    std::fs::copy(exe, &run_exe).map_err(io)?;
    Ok(run_exe)
}

/// Reads files the user picked or dropped (to add them as assets): name, size and bytes
/// (base64). Folders are skipped; files over 10 MB are refused.
#[tauri::command]
pub async fn game_read_files(paths: Vec<String>) -> CmdResult<Vec<Value>> {
    use base64::Engine as _;
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for p in paths {
            let p = PathBuf::from(p);
            let Ok(m) = std::fs::metadata(&p) else { continue };
            if !m.is_file() {
                continue;
            }
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if m.len() > MAX_ASSET {
                out.push(json!({"name": name, "size": m.len(), "error": "bigger than 10 MB"}));
                continue;
            }
            let bytes = std::fs::read(&p).map_err(|e| CmdError::new("io", format!("{name}: {e}")))?;
            out.push(json!({"name": name, "size": bytes.len(), "data": base64::engine::general_purpose::STANDARD.encode(bytes)}));
        }
        Ok(out)
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?
}

/// Stops a running game (or its build).
#[tauri::command]
pub fn game_stop(g: State<'_, Games>, id: String) {
    if let Some(c) = g.running.lock().unwrap().get(&id) {
        let _ = c.lock().unwrap().kill();
    }
    if let Some(b) = g.building.lock().unwrap().get(&id) {
        if let Some(c) = b.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
}

#[tauri::command]
pub fn game_status(g: State<'_, Games>) -> Value {
    json!({
        "running": g.running.lock().unwrap().keys().cloned().collect::<Vec<_>>(),
        "building": g.building.lock().unwrap().keys().cloned().collect::<Vec<_>>(),
        "rust": cargo().is_some(),
    })
}

impl Games {
    /// Ends every game (RAD is quitting).
    pub fn stop_all(&self) {
        for c in self.running.lock().unwrap().values() {
            let _ = c.lock().unwrap().kill();
        }
        for b in self.building.lock().unwrap().values() {
            if let Some(c) = b.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_folder_has_the_program_and_its_resources() {
        use base64::Engine as _;
        let td = tempfile::tempdir().unwrap();
        let exe = td.path().join("build/mygame");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"program").unwrap();
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let assets = vec![
            GameAsset { path: "player.png".into(), data: b64(b"png") },
            GameAsset { path: "sfx/jump.ogg".into(), data: b64(b"ogg") },
            GameAsset { path: "music/".into(), data: String::new() },
        ];
        let run = td.path().join("run/g1");
        let got = prepare_run(&run, &exe, &assets).unwrap();
        assert_eq!(got, run.join("mygame"));
        assert_eq!(std::fs::read(&got).unwrap(), b"program");
        assert_eq!(std::fs::read(run.join("resources/player.png")).unwrap(), b"png");
        assert_eq!(std::fs::read(run.join("resources/sfx/jump.ogg")).unwrap(), b"ogg");
        assert!(run.join("resources/music").is_dir());
        // A fresh folder each run; nothing escapes it.
        let bad = vec![GameAsset { path: "../evil".into(), data: b64(b"x") }];
        assert!(prepare_run(&run, &exe, &bad).is_err());
        assert!(!td.path().join("run/evil").exists());
    }

    #[test]
    fn reads_package_names_and_refuses_escaping_paths() {
        assert_eq!(package_name("[package]\nname = \"dodge\"\nversion = \"0.1.0\"\n\n[dependencies]\nname = \"x\"").as_deref(), Some("dodge"));
        assert_eq!(package_name("[dependencies]\nname = \"x\""), None);
        assert!(safe_path("src/main.rs").is_ok());
        for bad in ["../x.rs", "/etc/passwd", "src/../../x", ""] {
            assert!(safe_path(bad).is_err(), "{bad}");
        }
    }
}

// ------------------------------------------------------------------ ggez documentation (local)

/// One searchable piece of the ggez docs (assets/ggez-docs, from scripts/fetch-ggez-docs.sh).
struct DocChunk {
    file: String,
    line: usize,
    text: String,
    terms: HashMap<String, u32>,
    len: usize,
}

fn docs_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    if let Ok(d) = app.path().resource_dir() {
        c.push(d.join("ggez-docs"));
    }
    if cfg!(debug_assertions) {
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ggez-docs"));
    }
    c.into_iter().find(|d| d.join("VERSION").is_file())
}

/// Words for searching: lowercase identifiers, plus the parts of snake_case and CamelCase names.
fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in text.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() > 1) {
        let lw = w.to_lowercase();
        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        for ch in w.chars() {
            if ch == '_' || (ch.is_uppercase() && !cur.is_empty() && !cur.chars().last().unwrap().is_uppercase()) {
                if cur.len() > 1 {
                    parts.push(cur.to_lowercase());
                }
                cur.clear();
                if ch == '_' {
                    continue;
                }
            }
            cur.push(ch);
        }
        if cur.len() > 1 {
            parts.push(cur.to_lowercase());
        }
        if parts.len() > 1 {
            out.extend(parts);
        }
        out.push(lw);
    }
    out
}

/// Splits a file into pieces of about 50 lines, at blank lines (markdown: at headings).
fn chunk_file(file: &str, text: &str) -> Vec<DocChunk> {
    let lines: Vec<&str> = text.lines().collect();
    let md = file.ends_with(".md");
    let mut out = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = (start + 50).min(lines.len());
        // Prefer to stop at a heading (markdown) or a blank line, within 20 more lines.
        let limit = (end + 20).min(lines.len());
        while end < limit && !(if md { lines[end].starts_with('#') } else { lines[end].trim().is_empty() }) {
            end += 1;
        }
        let body = lines[start..end].join("\n");
        if body.trim().len() > 40 {
            let t = terms(&format!("{file} {body}"));
            let mut m: HashMap<String, u32> = HashMap::new();
            for x in &t {
                *m.entry(x.clone()).or_default() += 1;
            }
            out.push(DocChunk { file: file.to_string(), line: start + 1, len: t.len().max(1), text: body, terms: m });
        }
        start = end;
    }
    out
}

fn docs_index(app: &AppHandle) -> &'static [DocChunk] {
    static INDEX: std::sync::OnceLock<Vec<DocChunk>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        let Some(dir) = docs_dir(app) else { return Vec::new() };
        let mut chunks = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs" || x == "md") {
                    let rel = p.strip_prefix(&dir).unwrap_or(&p).display().to_string();
                    if rel == "CHANGELOG.md" {
                        continue;
                    }
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        chunks.extend(chunk_file(&rel, &t));
                    }
                }
            }
        }
        chunks
    })
}

/// The most relevant pieces of the ggez 0.10 docs for `query` (BM25 ranking), at most
/// `max_chars` of text. Examples and guides come before library internals on ties.
fn search_docs(index: &[DocChunk], query: &str, limit: usize, max_chars: usize) -> Vec<Value> {
    let q: Vec<String> = {
        let mut t = terms(query);
        t.sort();
        t.dedup();
        t
    };
    if q.is_empty() || index.is_empty() {
        return Vec::new();
    }
    let n = index.len() as f64;
    let avg = index.iter().map(|c| c.len).sum::<usize>() as f64 / n;
    let idf: Vec<f64> = q
        .iter()
        .map(|t| {
            let df = index.iter().filter(|c| c.terms.contains_key(t)).count() as f64;
            ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
        })
        .collect();
    let mut scored: Vec<(f64, &DocChunk)> = index
        .iter()
        .map(|c| {
            let mut s = 0.0;
            for (t, w) in q.iter().zip(&idf) {
                let f = *c.terms.get(t).unwrap_or(&0) as f64;
                s += w * f * 2.2 / (f + 1.2 * (0.25 + 0.75 * c.len as f64 / avg));
            }
            if c.file.starts_with("examples/") || c.file.starts_with("docs/") {
                s *= 1.25;
            }
            (s, c)
        })
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    let mut used = 0;
    for (_, c) in scored.into_iter().take(limit * 3) {
        if out.len() >= limit || used + c.text.len() > max_chars {
            continue;
        }
        used += c.text.len();
        out.push(json!({"file": c.file, "line": c.line, "text": c.text}));
    }
    out
}

/// Searches the local ggez documentation (what the game agent reads).
#[tauri::command]
pub fn game_docs(app: AppHandle, query: String, limit: Option<usize>) -> Value {
    let index = docs_index(&app);
    json!({
        "available": !index.is_empty(),
        "results": search_docs(index, &query, limit.unwrap_or(4).clamp(1, 10), 9000),
    })
}

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn finds_the_right_docs() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ggez-docs");
        let mut index = Vec::new();
        for f in ["examples/04_snake.rs", "examples/02_hello_world.rs", "docs/FAQ.md", "src/graphics/text.rs", "src/input/keyboard.rs", "src/audio.rs"] {
            if let Ok(t) = std::fs::read_to_string(dir.join(f)) {
                index.extend(chunk_file(f, &t));
            }
        }
        assert!(!index.is_empty(), "run ./scripts/fetch-ggez-docs.sh");
        let top = |q: &str| search_docs(&index, q, 1, 9000)[0]["file"].as_str().unwrap().to_string();
        assert_eq!(top("is_logical_key_pressed arrow keys held down"), "src/input/keyboard.rs");
        // The Text API itself, or the example that draws scaled text.
        assert!(["src/graphics/text.rs", "examples/02_hello_world.rs"].contains(&top("Text set_scale font draw text").as_str()));
        assert_eq!(top("snake food grid"), "examples/04_snake.rs");
        assert_eq!(terms("isLogicalKey snake_case"), ["is", "logical", "key", "islogicalkey", "snake", "case", "snake_case"]);
    }
}
