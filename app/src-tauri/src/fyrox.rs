//! Fyrox (Development → Fyrox): the Rust game engine (https://fyrox.rs, MIT) and its editor,
//! and Gilgamesh's world, a 2D Fyrox project in RAD (`fyrox/world`) where he lives and the owner
//! visits him as a character of their own.
//!
//! This part keeps the Fyrox docs: the Fyrox book (https://fyrox-book.github.io, its Markdown
//! with the code examples it includes), and the engine's README and architecture notes. They're
//! downloaded onto this computer (`fyrox/docs/` in the app's data folder), one Markdown file per
//! chapter in `docs/rag/`, which the view adds to the vault (project "Fyrox") for RAG, and which
//! `fyrox_docs_search` searches for Gilgamesh's answers, offline too.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::client::{CmdError, CmdResult};
use crate::oort::{chunk, search, Chunk};

pub const ENGINE_VERSION: &str = "1.0.1";
const UA: &str = "RAD/0.1 (desktop app; downloads the Fyrox docs for offline reading)";
const BOOK_TARBALL: &str = "https://codeload.github.com/fyrox-book/fyrox-book.github.io/tar.gz/refs/heads/main";
const ENGINE_RAW: &str = "https://raw.githubusercontent.com/FyroxEngine/Fyrox/v";

#[derive(Default)]
pub struct Fyrox {
    busy: tokio::sync::Mutex<()>,
    index: Mutex<Option<Vec<Chunk>>>,
    pub(crate) world: crate::fyrox_world::World,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Manifest {
    pub engine: String,
    pub fetched: i64,
    pub files: Vec<String>,
    pub pages: usize,
    #[serde(default)]
    pub missing: Vec<String>,
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn net(what: &str, e: impl std::fmt::Display) -> CmdError {
    CmdError::new("network", format!("Couldn't download {what}: {e}"))
}

pub fn data_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(d) = std::env::var("RAD_FYROX_DIR") {
        return PathBuf::from(d);
    }
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("fyrox")
}

fn docs_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    data_dir(app).join("docs")
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    std::fs::read(dir.join("manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

// ------------------------------------------------------------------ the book

/// The book's files out of the repository's tarball: (path from the repo root, text), for the
/// Markdown and the Rust examples it includes.
pub fn unpack_book(bytes: &[u8]) -> CmdResult<HashMap<String, String>> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut out = HashMap::new();
    for e in ar.entries().map_err(io)? {
        let mut e = e.map_err(io)?;
        let path = e.path().map_err(io)?.to_string_lossy().into_owned();
        let Some((_, inner)) = path.split_once('/') else { continue };
        if !inner.starts_with("src/") || !(inner.ends_with(".md") || inner.ends_with(".rs")) || e.size() > 1 << 20 {
            continue;
        }
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.insert(inner.to_string(), s);
        }
    }
    Ok(out)
}

/// `a/b/../c` → `a/c`.
fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    parts.join("/")
}

/// mdBook's `{{#include path}}`, `{{#include path:anchor}}`, `{{#include path:10:20}}` and
/// `{{#rustdoc_include …}}`, filled in from the book's files (anchor marker lines dropped).
pub fn resolve_includes(md: &str, md_path: &str, files: &HashMap<String, String>) -> String {
    let re = regex::Regex::new(r"\{\{#(?:rustdoc_)?include\s+([^}\s]+)\s*\}\}").unwrap();
    let dir = md_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    re.replace_all(md, |c: &regex::Captures| {
        let spec = &c[1];
        let mut it = spec.splitn(3, ':');
        let file = it.next().unwrap_or("");
        let a = it.next();
        let b = it.next();
        let Some(text) = files.get(&normalize(&format!("{dir}/{file}"))) else {
            return format!("(example: {file})");
        };
        let lines: Vec<&str> = text.lines().collect();
        let picked: Vec<&str> = match (a, b) {
            (Some(a), b) if a.chars().all(|ch| ch.is_ascii_digit()) && !a.is_empty() => {
                let from = a.parse::<usize>().unwrap_or(1).max(1) - 1;
                let to = b.and_then(|b| b.parse::<usize>().ok()).unwrap_or(lines.len()).min(lines.len());
                lines.get(from..to.max(from)).map(|s| s.to_vec()).unwrap_or_default()
            }
            (Some(anchor), _) if !anchor.is_empty() => {
                let start = format!("ANCHOR: {anchor}");
                let end = format!("ANCHOR_END: {anchor}");
                let mut on = false;
                let mut v = Vec::new();
                for l in &lines {
                    let t = l.trim();
                    if t.ends_with(&start) && t.starts_with("//") {
                        on = true;
                    } else if t.ends_with(&end) && t.starts_with("//") {
                        on = false;
                    } else if on {
                        v.push(*l);
                    }
                }
                v
            }
            _ => lines.clone(),
        };
        let kept: Vec<&str> = picked.into_iter().filter(|l| !l.trim_start().starts_with("// ANCHOR")).collect();
        dedent(&kept)
    })
    .into_owned()
}

fn dedent(lines: &[&str]) -> String {
    let n = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|l| if l.len() >= n { &l[n..] } else { l.trim_start() }).collect::<Vec<_>>().join("\n")
}

/// The pages in the book's order (SUMMARY.md): (title, path from the repo root, depth).
pub fn summary(md: &str) -> Vec<(String, String, usize)> {
    let re = regex::Regex::new(r"^(\s*)[-*]\s*\[([^\]]+)\]\(([^)]+\.md)\)").unwrap();
    md.lines()
        .filter_map(|l| {
            let c = re.captures(l)?;
            Some((c[2].to_string(), normalize(&format!("src/{}", &c[3])), c[1].len() / 4))
        })
        .collect()
}

/// One Markdown file per top-level chapter of the book: (file name, text).
pub fn build_book(files: &HashMap<String, String>) -> (Vec<(String, String)>, usize) {
    let order = files.get("src/SUMMARY.md").map(|s| summary(s)).unwrap_or_default();
    let mut chapters: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut pages = 0;
    for (title, path, depth) in &order {
        let Some(md) = files.get(path) else { continue };
        if !seen.insert(path.clone()) {
            continue;
        }
        let body = resolve_includes(md, path, files);
        pages += 1;
        if *depth == 0 || chapters.is_empty() {
            chapters.push((title.clone(), format!("# Fyrox book: {title}\n\nFrom the Fyrox book (https://fyrox-book.github.io), for Fyrox {ENGINE_VERSION}.\n\n")));
        }
        let ch = chapters.last_mut().unwrap();
        // Each page's own headings go one level down, under the page's title.
        let body = demote(&body);
        ch.1.push_str(&format!("## {title}\n\n{}\n\n", body.trim()));
    }
    let out = chapters.into_iter().filter(|(_, t)| t.len() > 300).map(|(title, text)| (format!("Fyrox book - {}.md", safe(&title)), text)).collect();
    (out, pages)
}

/// `# A` → `### A` (outside code), so the page title (##) stays above its sections.
fn demote(md: &str) -> String {
    let mut in_code = false;
    md.lines()
        .map(|l| {
            if l.trim_start().starts_with("```") {
                in_code = !in_code;
            }
            if !in_code && l.starts_with('#') {
                let n = l.chars().take_while(|c| *c == '#').count();
                format!("{}{}", "#".repeat((n + 2).min(6)), &l[n..])
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect::<String>().trim().chars().take(80).collect()
}

async fn fetch<R: Runtime>(app: &AppHandle<R>) -> CmdResult<(Vec<(String, String)>, usize, Vec<String>)> {
    let c = reqwest::Client::builder().user_agent(UA).timeout(Duration::from_secs(120)).connect_timeout(Duration::from_secs(15)).build().map_err(|e| net("the docs", e))?;
    let step = |s: &str| {
        let _ = app.emit("fyrox-docs", json!({ "step": s }));
    };
    step("Downloading the Fyrox book");
    let r = c.get(BOOK_TARBALL).send().await.map_err(|e| net("the Fyrox book", e))?;
    if !r.status().is_success() {
        return Err(net("the Fyrox book", r.status()));
    }
    let bytes = r.bytes().await.map_err(|e| net("the Fyrox book", e))?.to_vec();
    step("Reading the book");
    let (mut files, pages) = tauri::async_runtime::spawn_blocking(move || unpack_book(&bytes).map(|f| build_book(&f))).await.map_err(io)??;
    let mut missing = Vec::new();
    step("Downloading the engine's notes");
    let mut engine = format!("# Fyrox engine {ENGINE_VERSION}: README and architecture\n\nFyrox is a Rust game engine with a scene editor (https://fyrox.rs, https://github.com/FyroxEngine/Fyrox, MIT). API reference: https://docs.rs/fyrox/{ENGINE_VERSION}/fyrox/.\n\n");
    for f in ["README.md", "ARCHITECTURE.md"] {
        match c.get(format!("{ENGINE_RAW}{ENGINE_VERSION}/{f}")).send().await {
            Ok(r) if r.status().is_success() => engine.push_str(&format!("## {f}\n\n{}\n\n", demote(&r.text().await.unwrap_or_default()).trim())),
            Ok(r) => missing.push(format!("{f}: {}", r.status())),
            Err(e) => missing.push(format!("{f}: {e}")),
        }
    }
    files.insert(0, (format!("Fyrox {ENGINE_VERSION} - README and architecture.md"), engine));
    Ok((files, pages, missing))
}

fn write(dir: &Path, files: &[(String, String)], pages: usize, missing: Vec<String>) -> CmdResult<Manifest> {
    let tmp = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("rag")).map_err(io)?;
    for (n, t) in files {
        std::fs::write(tmp.join("rag").join(n), t).map_err(io)?;
    }
    let m = Manifest { engine: ENGINE_VERSION.into(), fetched: now(), files: files.iter().map(|(n, _)| n.clone()).collect(), pages, missing };
    std::fs::write(tmp.join("manifest.json"), serde_json::to_vec_pretty(&m).map_err(io)?).map_err(io)?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir).map_err(io)?;
    Ok(m)
}

fn load_index(dir: &Path) -> Vec<Chunk> {
    let Some(m) = read_manifest(dir) else { return Vec::new() };
    m.files.iter().filter_map(|f| std::fs::read_to_string(dir.join("rag").join(f)).ok().map(|t| chunk(f, &t, false, 1800))).flatten().collect()
}

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn fyrox_docs_status(app: AppHandle) -> Value {
    let dir = docs_dir(&app);
    match read_manifest(&dir) {
        Some(m) => {
            let mut v = serde_json::to_value(&m).unwrap_or_default();
            v["rag"] = json!(m.files.iter().map(|f| dir.join("rag").join(f).to_string_lossy().into_owned()).collect::<Vec<_>>());
            v
        }
        None => Value::Null,
    }
}

#[tauri::command]
pub async fn fyrox_docs_download(app: AppHandle, st: State<'_, Fyrox>) -> CmdResult<Value> {
    let _g = st.busy.try_lock().map_err(|_| CmdError::new("busy", "The Fyrox docs are already being downloaded"))?;
    let (files, pages, missing) = fetch(&app).await?;
    let dir = docs_dir(&app);
    tauri::async_runtime::spawn_blocking(move || write(&dir, &files, pages, missing)).await.map_err(io)??;
    *st.index.lock().unwrap() = None;
    let _ = app.emit("fyrox-docs", json!({ "step": "done" }));
    Ok(fyrox_docs_status(app))
}

#[tauri::command]
pub fn fyrox_docs_search(app: AppHandle, st: State<'_, Fyrox>, query: String, limit: Option<usize>) -> Vec<Chunk> {
    let mut g = st.index.lock().unwrap();
    let index = g.get_or_insert_with(|| load_index(&docs_dir(&app)));
    search(index, &query, limit.unwrap_or(6).min(20), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> HashMap<String, String> {
        let mut f = HashMap::new();
        f.insert("src/SUMMARY.md".into(), "# Summary\n\n- [Introduction](./introduction/index.md)\n    - [Basic concepts](./introduction/basic_concepts.md)\n- [Scene](./scene/scene.md)\n    - [Sprite](./scene/sprite.md)\n".into());
        f.insert("src/introduction/index.md".into(), "# Introduction\n\nFyrox is a game engine. ".repeat(20));
        f.insert("src/introduction/basic_concepts.md".into(), "# Basic concepts\n\n## Plugins\n\nGames are plugins.".into());
        f.insert("src/scene/scene.md".into(), "# Scene\n\nA scene is a graph of nodes. ".repeat(20));
        f.insert("src/scene/sprite.md".into(), "# Sprite\n\n```rust,no_run\n{{#include ../code/snippets/src/scene/sprite.rs:create_sprite}}\n```\n\nWhole: {{#include ../code/snippets/src/scene/sprite.rs:3:3}}\n\nMissing: {{#include ../code/nope.rs}}".into());
        f.insert("src/code/snippets/src/scene/sprite.rs".into(), "use fyrox::scene;\n// ANCHOR: create_sprite\n    fn create_sprite() {\n        RectangleBuilder::new();\n    }\n// ANCHOR_END: create_sprite\n".into());
        f
    }

    #[test]
    fn includes() {
        let f = book();
        let out = resolve_includes(&f["src/scene/sprite.md"], "src/scene/sprite.md", &f);
        assert!(out.contains("fn create_sprite() {\n    RectangleBuilder::new();\n}"), "{out}");
        assert!(!out.contains("ANCHOR"));
        assert!(out.contains("Whole: fn create_sprite() {"), "{out}");
        assert!(out.contains("(example: ../code/nope.rs)"));
    }

    #[test]
    fn chapters_in_order() {
        let (files, pages) = build_book(&book());
        assert_eq!(pages, 4);
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Fyrox book - Introduction.md", "Fyrox book - Scene.md"]);
        let intro = &files[0].1;
        assert!(intro.find("## Introduction").unwrap() < intro.find("## Basic concepts").unwrap());
        assert!(intro.contains("#### Plugins"), "page sections go under the page title: {intro}");
        assert!(files[1].1.contains("RectangleBuilder::new();"));
    }

    #[test]
    fn writes_and_searches() {
        let d = tempfile::tempdir().unwrap();
        let (files, pages) = build_book(&book());
        write(&d.path().join("docs"), &files, pages, vec![]).unwrap();
        let index = load_index(&d.path().join("docs"));
        let hits = search(&index, "how do I create a sprite with RectangleBuilder", 3, false);
        assert!(hits[0].text.contains("RectangleBuilder"), "{hits:?}");
    }

    /// `cargo test -p rad-app --lib fyrox::tests::real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real() {
        let d = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_app();
        let (files, pages, missing) = tauri::async_runtime::block_on(fetch(app.handle())).unwrap();
        let m = write(&d.path().join("docs"), &files, pages, missing).unwrap();
        println!("{} pages in {} files, missing {:?}", m.pages, m.files.len(), m.missing);
        for (n, t) in &files {
            println!("{n}: {} KB", t.len() / 1024);
        }
        let unresolved = files.iter().map(|(_, t)| t.matches("{{#").count()).sum::<usize>();
        let index = load_index(&d.path().join("docs"));
        for q in ["how do I make a 2D sprite", "write a script and register it in the plugin", "tile map", "save and load a scene"] {
            println!("--- {q}: {:?}", search(&index, q, 2, false).iter().map(|c| format!("{} / {}", c.file, c.heading)).collect::<Vec<_>>());
        }
        assert!(m.pages > 100 && m.missing.is_empty() && unresolved == 0, "unresolved includes: {unresolved}");
    }
}
