//! Oort (Development → Oort): the space-fleet programming game at oort.rs, played in RAD's
//! Chromium engine (`browser.rs`, engine "oort"), with Gilgamesh beside it as a tutor.
//!
//! This module downloads what he needs to help, onto this computer (`oort/docs/` in the app's
//! data folder), so it's there offline too:
//!   - the `oort_api` crate's source from crates.io (its doc comments are the API reference
//!     that docs.rs shows), at the version the game runs;
//!   - the tutorials and scenarios from the game's repository (rlane/oort3): what each one
//!     sets up, the starter code, the enemies, and the sample solutions;
//!   - the game's wiki.
//! They're turned into Markdown files in `docs/rag/`, which the view adds to the vault (project
//! "Oort") for RAG, and which `oort_docs_search` searches here for each question. The sample
//! solutions stay out of the vault (`docs/solutions.md`): Gilgamesh gets them only when the
//! owner asks for a hint or a solution.
//!
//! Oort is GPL-3.0 (© Rich Lane). Nothing of it ships with RAD; it's fetched on the owner's
//! computer when they ask for it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::client::{CmdError, CmdResult};

pub const HOME: &str = "https://oort.rs/";
const UA: &str = "RAD/0.1 (desktop app; downloads the Oort API docs for offline reading)";
const REPO_TREE: &str = "https://api.github.com/repos/rlane/oort3/git/trees/master?recursive=1";
const REPO_RAW: &str = "https://raw.githubusercontent.com/rlane/oort3/master/";
const WIKI: &str = "https://github.com/rlane/oort3/wiki";
const WIKI_RAW: &str = "https://raw.githubusercontent.com/wiki/rlane/oort3/";
/// The wiki's pages when its index can't be read.
const WIKI_PAGES: &[&str] = &["Home", "Advanced-Topics", "CPU-and-Memory-Limits", "Example-Code", "External-Editors", "Learning-Resources", "The-Physics-of-Oort", "Tournaments", "Vector-Math"];

/// The game's scenarios in the order of its menu, with their names there.
pub const SCENARIOS: &[(&str, &str)] = &[
    ("welcome", "Welcome"),
    ("tutorial_guns", "Tutorial 1: Guns"),
    ("tutorial_acceleration", "Tutorial 2: Acceleration"),
    ("tutorial_acceleration2", "Tutorial 3: Acceleration #2"),
    ("tutorial_rotation", "Tutorial 4: Rotation"),
    ("tutorial_lead", "Tutorial 5: Lead"),
    ("tutorial_deflection", "Tutorial 6: Deflection"),
    ("tutorial_radar", "Tutorial 7: Radar"),
    ("tutorial_search", "Tutorial 8: Search"),
    ("tutorial_radio", "Tutorial 9: Radio"),
    ("tutorial_missiles", "Tutorial 10: Missiles"),
    ("tutorial_squadron", "Tutorial 11: Squadron"),
    ("tutorial_frigate", "Tutorial 12: Frigate"),
    ("tutorial_cruiser", "Tutorial 13: Cruiser"),
    ("gunnery", "Gunnery"),
    ("cruiser_defense", "Cruiser Defense"),
    ("planetary_defense", "Planetary Defense"),
    ("race", "Asteroid Race"),
    ("avoidance", "Avoidance"),
    ("fighter_duel", "Fighter Duel"),
    ("mini_fleet", "Mini-Fleet"),
    ("frigate_duel", "Frigate Duel"),
    ("cruiser_duel", "Cruiser Duel"),
    ("asteroid_duel", "Asteroid Duel"),
    ("squadrons", "Squadrons"),
    ("fleet", "Fleet"),
    ("belt", "Belt"),
    ("orbit", "Orbit"),
];

#[derive(Default)]
pub struct Oort {
    /// One download at a time.
    busy: tokio::sync::Mutex<()>,
    /// The docs, cut into passages for searching (loaded on first search).
    index: Mutex<Option<Vec<Chunk>>>,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Manifest {
    /// The oort_api version the docs are for.
    pub version: String,
    pub fetched: i64,
    /// The Markdown files for RAG, in `rag/`.
    pub files: Vec<String>,
    pub tutorials: usize,
    pub scenarios: usize,
    pub wiki: usize,
    /// Parts that couldn't be fetched (the rest still works).
    #[serde(default)]
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Chunk {
    pub file: String,
    pub heading: String,
    pub text: String,
    /// A sample solution (only given when asked for).
    pub solution: bool,
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn net(what: &str, e: impl std::fmt::Display) -> CmdError {
    CmdError::new("network", format!("Couldn't download {what}: {e}"))
}

pub fn root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(d) = std::env::var("RAD_OORT_DIR") {
        return PathBuf::from(d);
    }
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("oort").join("docs")
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn read_manifest(dir: &Path) -> Option<Manifest> {
    std::fs::read(dir.join("manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

// ------------------------------------------------------------------ what was downloaded

/// Everything fetched, before it's written out as Markdown.
#[derive(Default)]
pub struct Sources {
    pub version: String,
    /// The crate's files: (path inside the crate, text).
    pub api: Vec<(String, String)>,
    /// Repository files: (path, text).
    pub repo: Vec<(String, String)>,
    /// Wiki pages: (title, Markdown).
    pub wiki: Vec<(String, String)>,
    pub missing: Vec<String>,
}

fn fence(path: &str, text: &str) -> String {
    let lang = if path.ends_with(".rs") { "rust" } else if path.ends_with(".toml") { "toml" } else { "" };
    format!("```{lang}\n{}\n```\n", text.trim_end())
}

fn title_of(id: &str) -> String {
    SCENARIOS.iter().find(|(s, _)| *s == id).map(|(_, t)| t.to_string()).unwrap_or_else(|| id.replace('_', " "))
}

fn order_of(id: &str) -> usize {
    SCENARIOS.iter().position(|(s, _)| *s == id).unwrap_or(usize::MAX)
}

/// The Markdown files: (file name, text) for RAG, and the sample solutions (kept apart).
pub fn build(src: &Sources) -> (Vec<(String, String)>, String) {
    let v = &src.version;
    let mut out = Vec::new();

    // The API reference: the README, then lib.rs (the guide and most of the API), then the rest.
    let mut api = format!("# Oort API reference (oort_api {v})\n\nThe Rust API for Oort, the space-fleet programming game at https://oort.rs. Player code starts with `use oort_api::prelude::*;` and defines a `Ship` type with `new()` and a `tick(&mut self)` method, called 60 times per second. This is the crate's source: its doc comments (`///` and `//!`) are the documentation shown on https://docs.rs/oort_api/{v}/oort_api/.\n\n");
    let mut files: Vec<&(String, String)> = src.api.iter().filter(|(p, _)| p.ends_with(".rs") || p.ends_with("README.md")).collect();
    files.sort_by_key(|(p, _)| (!p.ends_with("README.md"), !p.ends_with("lib.rs"), p.clone()));
    for (p, t) in files {
        if p.ends_with(".md") {
            api.push_str(&format!("## {p}\n\n{}\n\n", t.trim()));
        } else {
            api.push_str(&format!("## {p}\n\n{}\n", fence(p, t)));
        }
    }
    out.push((format!("Oort API reference (oort_api {v}).md"), api));

    // Tutorials, in the game's order: what the scenario sets up, the starter code, the enemy.
    let file = |path: &str| src.repo.iter().find(|(p, _)| p == path).map(|(_, t)| t.as_str());
    let scenario_ids: Vec<String> = {
        let mut ids: Vec<String> = src
            .repo
            .iter()
            .filter_map(|(p, _)| p.strip_prefix("shared/simulator/src/scenario/")?.strip_suffix(".rs").map(String::from))
            .filter(|id| !["mod", "test", "stress", "sandbox"].contains(&id.as_str()))
            .collect();
        ids.sort_by_key(|id| (order_of(id), id.clone()));
        ids
    };
    let mut tut = format!("# Oort tutorials (game version {v})\n\nThe tutorials teach Oort step by step. Each one sets up a scenario (the Rust code below that creates the ships and decides when it's won), and gives starter code in the editor with a comment describing the objective. Open one at https://oort.rs/scenario/<name>.\n\n");
    let mut sc = format!("# Oort scenarios (game version {v})\n\nThe scenarios after the tutorials, including the tournament ones. Each section is the scenario's code: the ships each side gets, where they start, and how it's won. Open one at https://oort.rs/scenario/<name>.\n\n");
    let mut solutions = format!("# Oort tutorial sample solutions (game version {v})\n\nThe game's own sample solutions. Use them for hints; don't hand them over unless the owner asks for the solution.\n\n");
    let (mut n_tut, mut n_sc) = (0, 0);
    for id in &scenario_ids {
        let title = title_of(id);
        let scen = file(&format!("shared/simulator/src/scenario/{id}.rs")).unwrap_or_default();
        if id.starts_with("tutorial_") {
            n_tut += 1;
            tut.push_str(&format!("## {title} (`{id}`)\n\nhttps://oort.rs/scenario/{id}\n\n"));
            let base = format!("shared/builtin_ai/src/tutorial/{id}");
            if let Some(t) = file(&format!("{base}_initial.rs")) {
                tut.push_str(&format!("### Starter code ({title})\n\n{}\n", fence(".rs", t)));
            }
            if let Some(t) = file(&format!("{base}_enemy.rs")) {
                tut.push_str(&format!("### The enemy's code ({title})\n\n{}\n", fence(".rs", t)));
            }
            if !scen.is_empty() {
                tut.push_str(&format!("### The scenario ({title})\n\n{}\n", fence(".rs", scen)));
            }
            if let Some(t) = file(&format!("{base}_solution.rs")) {
                solutions.push_str(&format!("## Sample solution: {title} (`{id}`)\n\n{}\n", fence(".rs", t)));
            }
        } else if !scen.is_empty() {
            n_sc += 1;
            sc.push_str(&format!("## {title} (`{id}`)\n\nhttps://oort.rs/scenario/{id}\n\n{}\n", fence(".rs", scen)));
        }
    }
    if n_tut > 0 {
        out.push(("Oort tutorials.md".into(), tut));
    }
    if n_sc > 0 {
        out.push(("Oort scenarios.md".into(), sc));
    }
    for (page, md) in &src.wiki {
        let name = page.replace(['-', '\u{2011}'], " ");
        out.push((format!("Oort wiki - {}.md", safe(&name)), format!("# Oort wiki: {name}\n\nFrom https://github.com/rlane/oort3/wiki/{page}\n\n{}\n", md.trim())));
    }
    (out, solutions)
}

fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect::<String>().trim().chars().take(80).collect()
}

// ------------------------------------------------------------------ fetching

fn client() -> CmdResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| net("the docs", e))
}

async fn get_bytes(c: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let r = c.get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("{} for {url}", r.status()));
    }
    r.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}

async fn get_text(c: &reqwest::Client, url: &str) -> Result<String, String> {
    get_bytes(c, url).await.map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The crate's files out of its `.crate` (a gzipped tar): (path inside the crate, text).
pub fn unpack_crate(bytes: &[u8]) -> CmdResult<Vec<(String, String)>> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut out = Vec::new();
    for e in ar.entries().map_err(io)? {
        let mut e = e.map_err(io)?;
        let path = e.path().map_err(io)?.to_string_lossy().into_owned();
        // "oort_api-0.89.0/src/lib.rs" → "src/lib.rs"
        let Some((_, inner)) = path.split_once('/') else { continue };
        if !(inner.ends_with(".rs") || inner == "README.md" || inner == "Cargo.toml") || e.size() > 4 << 20 {
            continue;
        }
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.push((inner.to_string(), s));
        }
    }
    Ok(out)
}

/// The wiki's page names from its front page.
fn wiki_pages(html: &str) -> Vec<String> {
    let re = regex::Regex::new(r##"href="/rlane/oort3/wiki/([^"#?/]+)""##).unwrap();
    let mut pages = vec!["Home".to_string()];
    for c in re.captures_iter(html) {
        let p = c[1].to_string();
        if !p.starts_with('_') && !pages.contains(&p) {
            pages.push(p);
        }
    }
    pages
}

async fn fetch<R: Runtime>(app: &AppHandle<R>) -> CmdResult<Sources> {
    let c = client()?;
    let step = |s: &str, done: usize, total: usize| {
        let _ = app.emit("oort-docs", json!({ "step": s, "done": done, "total": total }));
    };
    let mut src = Sources::default();

    // 1. The API, at the newest version (the one oort.rs runs).
    step("Finding the newest oort_api", 0, 0);
    let info: Value = serde_json::from_str(&get_text(&c, "https://crates.io/api/v1/crates/oort_api").await.map_err(|e| net("the oort_api version", e))?).map_err(|e| net("the oort_api version", e))?;
    let v = info["crate"]["max_stable_version"].as_str().or(info["crate"]["max_version"].as_str()).ok_or_else(|| net("the oort_api version", "crates.io didn't say"))?.to_string();
    step(&format!("Downloading oort_api {v}"), 0, 0);
    let krate = get_bytes(&c, &format!("https://static.crates.io/crates/oort_api/oort_api-{v}.crate")).await.map_err(|e| net("the oort_api crate", e))?;
    src.api = tauri::async_runtime::spawn_blocking(move || unpack_crate(&krate)).await.map_err(io)??;
    src.version = v;

    // 2. Tutorials and scenarios from the repository.
    match get_text(&c, REPO_TREE).await.and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string())) {
        Ok(tree) => {
            let paths: Vec<String> = tree["tree"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|x| x["type"] == "blob")
                .filter_map(|x| x["path"].as_str())
                .filter(|p| p.ends_with(".rs") && (p.starts_with("shared/simulator/src/scenario/") || p.starts_with("shared/builtin_ai/src/tutorial/")))
                .map(String::from)
                .collect();
            for (i, p) in paths.iter().enumerate() {
                step("Downloading the tutorials and scenarios", i, paths.len());
                match get_text(&c, &format!("{REPO_RAW}{p}")).await {
                    Ok(t) => src.repo.push((p.clone(), t)),
                    Err(e) => src.missing.push(format!("{p}: {e}")),
                }
            }
        }
        Err(e) => src.missing.push(format!("the tutorials and scenarios: {e}")),
    }

    // 3. The wiki.
    let pages = match get_text(&c, WIKI).await {
        Ok(html) => wiki_pages(&html),
        Err(_) => WIKI_PAGES.iter().map(|s| s.to_string()).collect(),
    };
    for (i, p) in pages.iter().enumerate() {
        step("Downloading the wiki", i, pages.len());
        match get_text(&c, &format!("{WIKI_RAW}{p}.md")).await {
            Ok(t) if !t.trim().is_empty() => {
                let title = percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned();
                src.wiki.push((title, t));
            }
            Ok(_) => {}
            Err(e) => src.missing.push(format!("wiki {p}: {e}")),
        }
    }
    Ok(src)
}

/// Writes the Markdown out (replacing what was there) and returns the manifest.
pub fn write(dir: &Path, src: &Sources) -> CmdResult<Manifest> {
    let (files, solutions) = build(src);
    let tmp = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("rag")).map_err(io)?;
    std::fs::create_dir_all(tmp.join("source")).map_err(io)?;
    for (name, text) in &files {
        std::fs::write(tmp.join("rag").join(name), text).map_err(io)?;
    }
    std::fs::write(tmp.join("solutions.md"), solutions).map_err(io)?;
    // The raw files too, for anyone who wants to read them.
    for (p, t) in src.api.iter().map(|(p, t)| (format!("oort_api/{p}"), t)).chain(src.repo.iter().map(|(p, t)| (p.clone(), t))) {
        let path = tmp.join("source").join(&p);
        if p.split('/').any(|c| c == ".." || c.is_empty()) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(path, t).map_err(io)?;
    }
    let m = Manifest {
        version: src.version.clone(),
        fetched: now(),
        files: files.iter().map(|(n, _)| n.clone()).collect(),
        tutorials: src.repo.iter().filter(|(p, _)| p.contains("/scenario/tutorial_")).count(),
        scenarios: src.repo.iter().filter(|(p, _)| p.contains("/scenario/") && !p.contains("/scenario/tutorial_")).count(),
        wiki: src.wiki.len(),
        missing: src.missing.clone(),
    };
    std::fs::write(tmp.join("manifest.json"), serde_json::to_vec_pretty(&m).map_err(io)?).map_err(io)?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(&tmp, dir).map_err(io)?;
    Ok(m)
}

// ------------------------------------------------------------------ searching

/// Cuts Markdown into passages at its `##`/`###` headings, long ones into pieces of about
/// `max` characters (at line ends, so code stays readable).
pub fn chunk(file: &str, md: &str, solution: bool, max: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut heading = String::new();
    let mut buf = String::new();
    let mut in_code = false;
    let flush = |heading: &str, buf: &mut String, out: &mut Vec<Chunk>| {
        if !buf.trim().is_empty() {
            out.push(Chunk { file: file.to_string(), heading: heading.to_string(), text: buf.trim().to_string(), solution });
        }
        buf.clear();
    };
    for line in md.lines() {
        if line.starts_with("```") {
            in_code = !in_code;
        }
        if !in_code && (line.starts_with("## ") || line.starts_with("### ")) {
            flush(&heading, &mut buf, &mut out);
            heading = line.trim_start_matches('#').trim().to_string();
            continue;
        }
        if buf.len() + line.len() > max && !buf.is_empty() {
            // Keep a fence open across the cut.
            let open = buf.matches("```").count() % 2 == 1;
            if open {
                buf.push_str("```\n");
            }
            flush(&heading, &mut buf, &mut out);
            if open {
                buf.push_str("```rust\n");
            }
        }
        buf.push_str(line);
        buf.push('\n');
    }
    flush(&heading, &mut buf, &mut out);
    out
}

const STOP: &[&str] = &["the", "and", "for", "how", "what", "why", "does", "with", "this", "that", "can", "you", "are", "not", "but", "use", "get", "my", "its", "it's", "into", "from", "when", "should", "would", "could", "about", "there", "have", "has", "was", "will", "make", "doesn", "don", "isn", "just", "like", "need", "want", "help", "please", "tell", "explain", "code", "oort"];

fn words(q: &str) -> Vec<String> {
    let mut w: Vec<String> = q
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !STOP.contains(&w.as_str()))
        .collect();
    w.sort();
    w.dedup();
    w
}

/// The passages that best match the words of `query`. Matches in the heading and whole-word
/// matches of API names (`set_radar_heading`) count most.
pub fn search(index: &[Chunk], query: &str, limit: usize, solutions: bool) -> Vec<Chunk> {
    let q = words(query);
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f64, &Chunk)> = index
        .iter()
        .filter(|c| solutions || !c.solution)
        .filter_map(|c| {
            let text = c.text.to_lowercase();
            let head = c.heading.to_lowercase();
            let mut s = 0.0;
            let mut hits = 0;
            for w in &q {
                let n = text.matches(w.as_str()).count();
                if n > 0 {
                    hits += 1;
                    s += 1.0 + (n as f64).ln();
                    if w.contains('_') {
                        s += 2.0;
                    }
                }
                if head.contains(w.as_str()) {
                    hits += 1;
                    s += 3.0;
                }
            }
            (hits > 0).then(|| (s * (hits as f64 / q.len() as f64), c))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, c)| c.clone()).collect()
}

fn load_index(dir: &Path) -> Vec<Chunk> {
    let mut out = Vec::new();
    let Some(m) = read_manifest(dir) else { return out };
    for f in &m.files {
        if let Ok(t) = std::fs::read_to_string(dir.join("rag").join(f)) {
            out.extend(chunk(f, &t, false, 1800));
        }
    }
    if let Ok(t) = std::fs::read_to_string(dir.join("solutions.md")) {
        out.extend(chunk("solutions.md", &t, true, 4000));
    }
    out
}

// ------------------------------------------------------------------ commands

/// What's been downloaded: {version, fetched, files, tutorials, scenarios, wiki, missing, rag: [paths]} or null.
#[tauri::command]
pub fn oort_docs_status(app: AppHandle) -> Value {
    let dir = root(&app);
    match read_manifest(&dir) {
        Some(m) => {
            let mut v = serde_json::to_value(&m).unwrap_or_default();
            v["rag"] = json!(m.files.iter().map(|f| dir.join("rag").join(f).to_string_lossy().into_owned()).collect::<Vec<_>>());
            v["dir"] = json!(dir.to_string_lossy());
            v
        }
        None => Value::Null,
    }
}

/// Downloads (or updates) the docs; reports `oort-docs` progress events.
#[tauri::command]
pub async fn oort_docs_download(app: AppHandle, st: State<'_, Oort>) -> CmdResult<Value> {
    let _g = st.busy.try_lock().map_err(|_| CmdError::new("busy", "The Oort docs are already being downloaded"))?;
    let src = fetch(&app).await?;
    let dir = root(&app);
    tauri::async_runtime::spawn_blocking(move || write(&dir, &src)).await.map_err(io)??;
    *st.index.lock().unwrap() = None;
    let _ = app.emit("oort-docs", json!({ "step": "done", "done": 1, "total": 1 }));
    Ok(oort_docs_status(app))
}

/// The passages of the docs that best match a question: [{file, heading, text, solution}].
#[tauri::command]
pub fn oort_docs_search(app: AppHandle, st: State<'_, Oort>, query: String, limit: Option<usize>, solutions: Option<bool>) -> Vec<Chunk> {
    let mut g = st.index.lock().unwrap();
    let index = g.get_or_insert_with(|| load_index(&root(&app)));
    search(index, &query, limit.unwrap_or(6).min(20), solutions.unwrap_or(false))
}

/// One tutorial's or scenario's own section (starter code, scenario) — what the owner is playing.
#[tauri::command]
pub fn oort_docs_scenario(app: AppHandle, st: State<'_, Oort>, id: String, solution: Option<bool>) -> Vec<Chunk> {
    let mut g = st.index.lock().unwrap();
    let index = g.get_or_insert_with(|| load_index(&root(&app)));
    let tag = format!("(`{id}`)");
    let title = title_of(&id);
    index
        .iter()
        .filter(|c| (c.heading.contains(&tag) || c.heading.ends_with(&format!("({title})"))) && (solution.unwrap_or(false) || !c.solution))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Sources {
        Sources {
            version: "0.89.0".into(),
            api: vec![
                ("src/vec.rs".into(), "/// A 2D vector.\npub struct Vec2 { pub x: f64, pub y: f64 }\n".into()),
                ("src/lib.rs".into(), "//! Oort expects your code to have a `Ship` type with a `tick` method.\n/// Sets the radar heading.\npub fn set_radar_heading(heading: f64) {}\n".into()),
                ("README.md".into(), "Oort API".into()),
                ("Cargo.toml".into(), "[package]".into()),
            ],
            repo: vec![
                ("shared/simulator/src/scenario/tutorial_guns.rs".into(), "// Destroy the asteroid.\nfn init() {}".into()),
                ("shared/simulator/src/scenario/fighter_duel.rs".into(), "fn init() { /* two fighters */ }".into()),
                ("shared/simulator/src/scenario/mod.rs".into(), "mod x;".into()),
                ("shared/builtin_ai/src/tutorial/tutorial_guns_initial.rs".into(), "pub fn tick(&mut self) {\n    // fire(0);\n}".into()),
                ("shared/builtin_ai/src/tutorial/tutorial_guns_solution.rs".into(), "pub fn tick(&mut self) { fire(0); }".into()),
            ],
            wiki: vec![("The-Physics-of-Oort".into(), "Ships have mass.".into())],
            missing: vec![],
        }
    }

    #[test]
    fn builds_markdown() {
        let (files, solutions) = build(&sample());
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Oort API reference (oort_api 0.89.0).md", "Oort tutorials.md", "Oort scenarios.md", "Oort wiki - The Physics of Oort.md"]);
        let api = &files[0].1;
        // README first, then lib.rs, then the rest; Cargo.toml left out.
        assert!(api.find("## README.md").unwrap() < api.find("## src/lib.rs").unwrap());
        assert!(api.find("## src/lib.rs").unwrap() < api.find("## src/vec.rs").unwrap());
        assert!(!api.contains("[package]"));
        let tut = &files[1].1;
        assert!(tut.contains("## Tutorial 1: Guns (`tutorial_guns`)"));
        assert!(tut.contains("// fire(0);"));
        assert!(!tut.contains("{ fire(0); }"), "solutions stay out of the RAG files");
        assert!(solutions.contains("{ fire(0); }"));
        assert!(files[2].1.contains("## Fighter Duel (`fighter_duel`)"));
        assert!(!files[2].1.contains("mod x;"));
    }

    #[test]
    fn writes_and_searches() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("docs");
        let m = write(&dir, &sample()).unwrap();
        assert_eq!((m.version.as_str(), m.tutorials, m.scenarios, m.wiki), ("0.89.0", 1, 2, 1));
        assert!(dir.join("source/oort_api/src/lib.rs").exists());
        let index = load_index(&dir);
        let hits = search(&index, "How do I use set_radar_heading?", 3, false);
        assert!(hits[0].text.contains("pub fn set_radar_heading"), "{hits:?}");
        assert!(search(&index, "solution for guns", 10, false).iter().all(|c| !c.solution));
        assert!(search(&index, "sample solution guns", 10, true).iter().any(|c| c.solution));
        assert!(search(&index, "the and how", 3, false).is_empty());
        // Writing again replaces it.
        write(&dir, &sample()).unwrap();
        assert!(!dir.with_extension("new").exists());
    }

    #[test]
    fn chunks_keep_code_fenced() {
        let md = format!("## A\n\n```rust\n{}```\n## B\ntext\n", "let x = 1;\n".repeat(200));
        let c = chunk("f.md", &md, false, 500);
        assert!(c.len() > 3);
        for x in &c {
            assert_eq!(x.text.matches("```").count() % 2, 0, "{}", x.text);
        }
        assert_eq!(c.last().unwrap().heading, "B");
    }

    #[test]
    fn wiki_index() {
        let html = r##"<a href="/rlane/oort3/wiki/Vector-Math">x</a><a href="/rlane/oort3/wiki/_new">n</a><a href="/rlane/oort3/wiki/Home/_edit">e</a><a href="/rlane/oort3/wiki/Vector-Math">again</a>"##;
        assert_eq!(wiki_pages(html), ["Home", "Vector-Math"]);
    }

    /// Downloads the real docs: `cargo test -p rad-app --lib oort::tests::real -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real() {
        let d = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("RAD_OORT_DIR", d.path().join("docs")) };
        let app = tauri::test::mock_app();
        let src = tauri::async_runtime::block_on(fetch(app.handle())).unwrap();
        let m = write(&root(app.handle()), &src).unwrap();
        println!("{m:#?}");
        let index = load_index(&root(app.handle()));
        for q in ["how do I aim the radar at an enemy", "lead the target with bullet speed", "send a radio message", "missile"] {
            let h = search(&index, q, 2, false);
            println!("--- {q}: {:?}", h.iter().map(|c| format!("{} / {}", c.file, c.heading)).collect::<Vec<_>>());
            assert!(!h.is_empty());
        }
        assert!(m.tutorials >= 10 && m.wiki >= 3 && m.missing.is_empty(), "{m:?}");
    }
}
