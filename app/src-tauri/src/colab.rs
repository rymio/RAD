//! Colab (Development → Rust Colab and Python Colab): notebooks of code cells, like Google Colab
//! or Jupyter, run on this computer, made for maths, machine learning and LLM experiments.
//! Gilgamesh helps from the chat beside them (lib/colab.svelte.ts tells him what's in the
//! notebook), and he draws on the vault there like in any chat.
//!
//! Everything is kept in the app's data folder, `colab/`:
//! - `notebooks/<id>.ipynb` — the notebooks, in Jupyter's own format, so ones from Google Colab
//!   open here and these open there.
//! - `files/` — the notebooks' working folder (their data files, the models they save).
//! - `python/venv` — the Python the notebooks use, made from this computer's python3, and
//!   `python/wheels`, the local packages: whatever is installed is downloaded there first, so it
//!   can be installed again without the internet.
//! - `rust/cargo` — Colab's own CARGO_HOME: the crates the notebooks use (`:dep`), downloaded
//!   once and built from there offline; `rust/build/<id>` — each notebook's compiled crates, so a
//!   restart doesn't build them again.
//! - `packs.json` — the packs of packages and crates downloaded for offline work.
//!
//! **Kernels** are one process per notebook: `colab_kernel.py` for Python (written next to the
//! venv at every start) and `rad-rust-kernel` (colab/rad-rust-kernel: evcxr) for Rust. Both speak
//! JSON lines; a run's output is streamed to the UI on a channel.
//!
//! **Offline**: in "auto" mode Colab is offline when crates.io / PyPI can't be reached. Offline,
//! Rust builds with `--offline` from `rust/cargo`, and `%pip install` only installs from the
//! local wheels (the kernel looks for the `offline` flag file in `colab/`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use crate::client::{CmdError, CmdResult};

const KERNEL_PY: &str = include_str!("colab_kernel.py");
const RUST_KERNEL: &str = if cfg!(windows) { "rad-rust-kernel.exe" } else { "rad-rust-kernel" };

#[derive(Default)]
pub struct Colab {
    kernels: Mutex<HashMap<String, Arc<Kernel>>>,
    /// One kernel start at a time, so a notebook never gets two.
    starting: tokio::sync::Mutex<()>,
    /// The download going on for each language (pip / cargo), so it can be stopped.
    jobs: Mutex<HashMap<String, u32>>,
    /// Guards packs.json and settings.json.
    lock: Mutex<()>,
    /// The last check of the package sites: (when, reachable). Offline it takes seconds to fail,
    /// so it isn't repeated at every kernel start.
    reached: Mutex<Option<(std::time::Instant, bool)>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Python,
    Rust,
}

impl Lang {
    fn name(self) -> &'static str {
        match self {
            Lang::Python => "python",
            Lang::Rust => "rust",
        }
    }
}

struct Pending {
    events: Channel<Value>,
    done: oneshot::Sender<Value>,
}

struct Kernel {
    lang: Lang,
    pid: u32,
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    pending: Mutex<HashMap<u64, Pending>>,
    /// The run going on: what the kernel process itself writes to stderr goes to it.
    current: AtomicU64,
    next: AtomicU64,
    ready: Value,
}

impl Kernel {
    async fn request(&self, mut req: Value, events: Channel<Value>) -> CmdResult<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        req["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, Pending { events, done: tx });
        if req["op"] == "run" {
            self.current.store(id, Ordering::SeqCst);
        }
        let line = format!("{req}\n");
        let wrote = self.stdin.lock().await.write_all(line.as_bytes()).await;
        if let Err(e) = wrote {
            self.pending.lock().unwrap().remove(&id);
            return Err(CmdError::new("kernel", format!("The kernel has stopped: {e}")));
        }
        rx.await.map_err(|_| CmdError::new("kernel", "The kernel has stopped"))
    }

    /// Hands a line the kernel wrote to the run it belongs to.
    fn route(&self, msg: Value) {
        let id = msg["id"].as_u64().unwrap_or(0);
        let last = matches!(msg["type"].as_str(), Some("done" | "error" | "vars"));
        let mut pending = self.pending.lock().unwrap();
        if last {
            if let Some(p) = pending.remove(&id) {
                let _ = p.done.send(msg);
            }
        } else if let Some(p) = pending.get(&id) {
            let _ = p.events.send(msg);
        }
    }

    /// The kernel has gone: every run waiting on it ends.
    fn fail_all(&self, why: &str) {
        for (id, p) in self.pending.lock().unwrap().drain() {
            let _ = p.done.send(json!({ "id": id, "type": "error", "kind": "died", "message": why }));
        }
    }
}

// ---------------------------------------------------------------- places

fn root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(d) = std::env::var("RAD_COLAB_DIR") {
        return d.into();
    }
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("colab")
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn check_id(id: &str) -> CmdResult<()> {
    if id.is_empty() || id.len() > 60 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(CmdError::new("bad_request", "invalid notebook"));
    }
    Ok(())
}

fn notebook_path<R: Runtime>(app: &AppHandle<R>, id: &str) -> CmdResult<PathBuf> {
    check_id(id)?;
    Ok(root(app).join("notebooks").join(format!("{id}.ipynb")))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn new_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let h = blake3::hash(format!("{nanos}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)).as_bytes());
    h.to_hex()[..16].to_string()
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

/// A PATH with the toolchains on it: an app started from the Finder has only the system's.
fn tool_path() -> String {
    let mut parts: Vec<String> = vec![home().join(".cargo/bin").to_string_lossy().into(), "/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    parts.extend(std::env::var("PATH").unwrap_or_default().split(':').filter(|p| !p.is_empty()).map(String::from));
    for p in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        parts.push(p.into());
    }
    let mut seen = std::collections::HashSet::new();
    parts.retain(|p| seen.insert(p.clone()));
    parts.join(":")
}

/// This computer's Python 3 (3.9 or newer), to make the venv from.
fn system_python() -> Option<(PathBuf, String)> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_COLAB_PYTHON") {
        candidates.push(p.into());
    }
    for d in tool_path().split(':') {
        candidates.push(Path::new(d).join("python3"));
    }
    candidates.push("/Library/Frameworks/Python.framework/Versions/Current/bin/python3".into());
    for c in candidates {
        if !c.is_file() {
            continue;
        }
        let Ok(out) = std::process::Command::new(&c).args(["-c", "import sys; print('%d.%d.%d' % sys.version_info[:3]); assert sys.version_info >= (3, 9)"]).output() else { continue };
        if out.status.success() {
            return Some((c, String::from_utf8_lossy(&out.stdout).trim().to_string()));
        }
    }
    None
}

fn venv_python<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    root(app).join("python/venv/bin/python")
}

fn wheels<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    root(app).join("python/wheels")
}

fn cargo_home<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    root(app).join("rust/cargo")
}

fn cargo() -> Option<PathBuf> {
    tool_path().split(':').map(|d| Path::new(d).join("cargo")).find(|p| p.is_file())
}

/// The Rust kernel: next to the app (bundled sidecar), in its resources, or — in development —
/// in its own build folder. `RAD_COLAB_RUST_KERNEL` overrides these.
fn rust_kernel<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("RAD_COLAB_RUST_KERNEL") {
        candidates.push(p.into());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join(RUST_KERNEL));
    }
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join(RUST_KERNEL));
    }
    if cfg!(debug_assertions) {
        let t = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../colab/rad-rust-kernel/target");
        candidates.push(t.join("release").join(RUST_KERNEL));
        candidates.push(t.join("debug").join(RUST_KERNEL));
    }
    candidates.into_iter().find(|p| p.is_file())
}

async fn host_triple() -> Option<String> {
    let out = tokio::process::Command::new("rustc").arg("-vV").env("PATH", tool_path()).output().await.ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("host: ").map(String::from))
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

// ---------------------------------------------------------------- settings and offline

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    /// "auto" (offline when the package sites can't be reached), "offline" or "online".
    network: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { network: "auto".into() }
    }
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_json(p: &Path, v: &impl Serialize) -> CmdResult<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(io)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?).map_err(io)?;
    std::fs::rename(tmp, p).map_err(io)
}

/// Whether PyPI or crates.io answers (a TCP connection within a second and a half).
async fn reachable() -> bool {
    for host in ["index.crates.io:443", "pypi.org:443"] {
        if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(1500), tokio::net::TcpStream::connect(host)).await {
            return true;
        }
    }
    false
}

/// Works out whether Colab is offline now, and tells the kernels (and the next ones).
async fn update_offline<R: Runtime>(app: &AppHandle<R>, c: &Colab) -> (bool, bool) {
    let settings: Settings = read_json(&root(app).join("settings.json"));
    let cached = c.reached.lock().unwrap().filter(|(at, _)| at.elapsed() < Duration::from_secs(30)).map(|(_, ok)| ok);
    let online = match cached {
        Some(ok) => ok,
        None => {
            let ok = reachable().await;
            *c.reached.lock().unwrap() = Some((std::time::Instant::now(), ok));
            ok
        }
    };
    let offline = match settings.network.as_str() {
        "offline" => true,
        "online" => false,
        _ => !online,
    };
    let flag = root(app).join("offline");
    if offline {
        let _ = std::fs::create_dir_all(root(app));
        let _ = std::fs::write(&flag, b"1");
    } else {
        let _ = std::fs::remove_file(&flag);
    }
    let rust: Vec<Arc<Kernel>> = c.kernels.lock().unwrap().values().filter(|k| k.lang == Lang::Rust).cloned().collect();
    for k in rust {
        let _ = tokio::time::timeout(Duration::from_secs(5), k.request(json!({ "op": "offline", "on": offline }), Channel::new(|_| Ok(())))).await;
    }
    (offline, online)
}

// ---------------------------------------------------------------- packs

pub struct Pack {
    id: &'static str,
    name: &'static str,
    about: &'static str,
    /// pip requirements, or Cargo.toml dependency lines.
    items: &'static [&'static str],
    size: &'static str,
}

const PY_PACKS: &[Pack] = &[
    Pack { id: "maths", name: "Maths & data", about: "NumPy, SciPy, SymPy, pandas, matplotlib, seaborn", items: &["numpy", "scipy", "sympy", "pandas", "matplotlib", "seaborn", "pillow", "tqdm"], size: "≈ 80 MB download" },
    Pack { id: "ml", name: "Machine learning", about: "scikit-learn and PyTorch", items: &["scikit-learn", "torch"], size: "≈ 150 MB download" },
    Pack { id: "llm", name: "LLMs", about: "Hugging Face transformers, tokenizers, safetensors, the Hub client, tiktoken", items: &["transformers", "tokenizers", "safetensors", "huggingface_hub", "tiktoken"], size: "≈ 30 MB download" },
];

const RUST_PACKS: &[Pack] = &[
    Pack {
        id: "maths",
        name: "Maths",
        about: "ndarray, nalgebra, num, rand, rand_distr, statrs, approx",
        items: &[r#"ndarray = "0.17""#, r#"nalgebra = "0.35""#, r#"num = "0.4""#, r#"rand = "0.10""#, r#"rand_distr = "0.6""#, r#"statrs = "0.19""#, r#"approx = "0.5""#],
        size: "≈ 55 MB on disk",
    },
    Pack {
        id: "ml",
        name: "Machine learning",
        about: "candle (tensors and neural networks), linfa, smartcore",
        items: &[r#"candle-core = "0.11""#, r#"candle-nn = "0.11""#, r#"linfa = "0.8""#, r#"smartcore = "0.7""#],
        size: "≈ 85 MB on disk",
    },
    Pack {
        id: "llm",
        name: "LLMs",
        about: "candle-transformers, tokenizers, safetensors, hf-hub",
        items: &[r#"candle-transformers = "0.11""#, r#"tokenizers = "0.23""#, r#"safetensors = "0.8""#, r#"hf-hub = "1""#],
        size: "≈ 290 MB on disk",
    },
    Pack {
        id: "tools",
        name: "Everyday crates",
        about: "serde, serde_json, itertools, rayon, csv, plotters (charts in the notebook)",
        items: &[r#"serde = { version = "1", features = ["derive"] }"#, r#"serde_json = "1""#, r#"itertools = "0.15""#, r#"rayon = "1""#, r#"csv = "1""#, r#"plotters = { version = "0.3", features = ["evcxr"] }"#],
        size: "≈ 40 MB on disk",
    },
];

fn packs(lang: Lang) -> &'static [Pack] {
    match lang {
        Lang::Python => PY_PACKS,
        Lang::Rust => RUST_PACKS,
    }
}

/// `{"python/maths": {"at": …}, …}`: the packs downloaded so far.
fn packs_done<R: Runtime>(app: &AppHandle<R>) -> serde_json::Map<String, Value> {
    read_json(&root(app).join("packs.json"))
}

// ---------------------------------------------------------------- status

#[tauri::command]
pub async fn colab_status(app: AppHandle, c: State<'_, Colab>) -> CmdResult<Value> {
    // Looked at afresh: the owner may have just come online.
    *c.reached.lock().unwrap() = None;
    let (offline, online) = update_offline(&app, &c).await;
    let settings: Settings = read_json(&root(&app).join("settings.json"));
    let done = packs_done(&app);
    let pack_list = |lang: Lang| -> Vec<Value> {
        packs(lang)
            .iter()
            .map(|p| json!({ "id": p.id, "name": p.name, "about": p.about, "items": p.items, "size": p.size, "downloaded": done.get(&format!("{}/{}", lang.name(), p.id)).and_then(|v| v["at"].as_i64()) }))
            .collect()
    };
    let r = root(&app);
    let (py, venv, wheels_dir, crates_dir) = (system_python(), venv_python(&app), wheels(&app), cargo_home(&app));
    let rustc = tokio::process::Command::new("rustc").arg("--version").env("PATH", tool_path()).output().await.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let (wheels_bytes, crates_bytes) = tokio::task::spawn_blocking(move || (dir_size(&wheels_dir), dir_size(&crates_dir.join("registry")))).await.unwrap_or((0, 0));
    let running: Vec<Value> = c.kernels.lock().unwrap().iter().map(|(id, k)| json!({ "id": id, "lang": k.lang })).collect();
    Ok(json!({
        "offline": offline,
        "online": online,
        "network": settings.network,
        "folder": r.join("files"),
        "running": running,
        "python": {
            "system": py.as_ref().map(|p| p.0.clone()),
            "version": py.map(|p| p.1),
            "ready": venv.is_file(),
            "wheels_mb": wheels_bytes / 1_048_576,
            "packs": pack_list(Lang::Python),
        },
        "rust": {
            "cargo": cargo().is_some(),
            "rustc": rustc,
            "kernel": rust_kernel(&app).is_some(),
            "ready": cargo().is_some() && rust_kernel(&app).is_some(),
            "crates_mb": crates_bytes / 1_048_576,
            "packs": pack_list(Lang::Rust),
        },
    }))
}

#[tauri::command]
pub async fn colab_set_network(app: AppHandle, c: State<'_, Colab>, network: String) -> CmdResult<Value> {
    if !["auto", "offline", "online"].contains(&network.as_str()) {
        return Err(CmdError::new("bad_request", "network is auto, offline or online"));
    }
    {
        let _g = c.lock.lock().unwrap();
        write_json(&root(&app).join("settings.json"), &Settings { network })?;
    }
    let (offline, online) = update_offline(&app, &c).await;
    Ok(json!({ "offline": offline, "online": online }))
}

// ---------------------------------------------------------------- setting up and downloading

/// Runs a setup command, streaming its output lines as `{type: "log", text}`.
async fn run_logged(c: &Colab, job: &str, mut cmd: tokio::process::Command, events: &Channel<Value>) -> CmdResult<()> {
    cmd.env("PATH", tool_path()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| CmdError::new("spawn", e.to_string()))?;
    if let Some(pid) = child.id() {
        c.jobs.lock().unwrap().insert(job.to_string(), pid);
    }
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let send = |stream: &'static str, ch: Channel<Value>| {
        move |text: String| {
            let _ = ch.send(json!({ "type": "log", "stream": stream, "text": text }));
        }
    };
    let (o, e) = (send("stdout", events.clone()), send("stderr", events.clone()));
    let a = tokio::spawn(async move {
        let mut l = BufReader::new(out).lines();
        while let Ok(Some(line)) = l.next_line().await {
            o(line);
        }
    });
    let b = tokio::spawn(async move {
        let mut l = BufReader::new(err).lines();
        let mut tail = Vec::new();
        while let Ok(Some(line)) = l.next_line().await {
            tail.push(line.clone());
            if tail.len() > 20 {
                tail.remove(0);
            }
            e(line);
        }
        tail
    });
    let status = child.wait().await.map_err(io)?;
    c.jobs.lock().unwrap().remove(job);
    let _ = a.await;
    let tail = b.await.unwrap_or_default();
    if !status.success() {
        let last = tail.iter().rev().find(|l| l.to_lowercase().contains("error")).or(tail.last()).cloned().unwrap_or_default();
        return Err(CmdError::new("failed", if status.code().is_none() { "Stopped".to_string() } else { last }));
    }
    Ok(())
}

async fn ensure_venv<R: Runtime>(app: &AppHandle<R>, c: &Colab, events: &Channel<Value>) -> CmdResult<()> {
    if venv_python(app).is_file() {
        return Ok(());
    }
    let (py, version) = system_python().ok_or_else(|| CmdError::new("missing", "Python 3 isn't installed on this computer. Install it (python.org, or `brew install python`), then try again."))?;
    let _ = events.send(json!({ "type": "log", "stream": "stdout", "text": format!("Making Colab's Python environment from Python {version}…") }));
    std::fs::create_dir_all(root(app).join("python")).map_err(io)?;
    let mut cmd = tokio::process::Command::new(py);
    cmd.args(["-m", "venv"]).arg(root(app).join("python/venv"));
    run_logged(c, "python", cmd, events).await
}

/// Downloads packs (and any extra packages) for offline work: Python wheels into the wheelhouse
/// and installed from it; Rust crates fetched into Colab's CARGO_HOME.
#[tauri::command]
pub async fn colab_download(app: AppHandle, c: State<'_, Colab>, lang: Lang, packs_wanted: Vec<String>, extra: Vec<String>, on_event: Channel<Value>) -> CmdResult<Value> {
    let chosen: Vec<&Pack> = packs(lang).iter().filter(|p| packs_wanted.iter().any(|w| w == p.id)).collect();
    let extra: Vec<String> = extra.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    for x in &extra {
        if x.starts_with('-') || x.chars().any(|ch| ch.is_control()) {
            return Err(CmdError::new("bad_request", format!("“{x}” isn't a package name")));
        }
    }
    let r = root(&app);
    match lang {
        Lang::Python => {
            ensure_venv(&app, &c, &on_event).await?;
            let mut items: Vec<String> = chosen.iter().flat_map(|p| p.items.iter().map(|s| s.to_string())).collect();
            items.extend(extra.iter().cloned());
            if items.is_empty() {
                return Ok(json!({ "ok": true }));
            }
            let w = wheels(&app);
            std::fs::create_dir_all(&w).map_err(io)?;
            let py = venv_python(&app);
            let _ = on_event.send(json!({ "type": "log", "stream": "stdout", "text": format!("Downloading {} into the local packages…", items.join(", ")) }));
            let mut dl = tokio::process::Command::new(&py);
            dl.args(["-m", "pip", "download", "--disable-pip-version-check", "--progress-bar", "off", "-d"]).arg(&w).args(&items);
            run_logged(&c, "python", dl, &on_event).await?;
            let _ = on_event.send(json!({ "type": "log", "stream": "stdout", "text": "Installing from the local packages…" }));
            let mut inst = tokio::process::Command::new(&py);
            inst.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-index", "--find-links"]).arg(&w).args(&items);
            run_logged(&c, "python", inst, &on_event).await?;
        }
        Lang::Rust => {
            let cargo = cargo().ok_or_else(|| CmdError::new("missing", "Rust isn't installed on this computer. Install it from rustup.rs, then try again."))?;
            let mut deps: Vec<String> = chosen.iter().flat_map(|p| p.items.iter().map(|s| s.to_string())).collect();
            // Extra crates: `name` or `name = "1.2"`.
            for x in &extra {
                deps.push(if x.contains('=') { x.clone() } else { format!("{x} = \"*\"") });
            }
            if deps.is_empty() {
                return Ok(json!({ "ok": true }));
            }
            // One small project per download: fetching it brings every crate it needs into
            // Colab's CARGO_HOME, where the notebooks' builds find them offline.
            let proj = r.join("rust/fetch");
            std::fs::create_dir_all(proj.join("src")).map_err(io)?;
            std::fs::write(proj.join("src/lib.rs"), "").map_err(io)?;
            std::fs::write(proj.join("Cargo.toml"), format!("[package]\nname = \"rad-colab-fetch\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\n{}\n", deps.join("\n"))).map_err(io)?;
            let _ = std::fs::remove_file(proj.join("Cargo.lock"));
            let _ = on_event.send(json!({ "type": "log", "stream": "stdout", "text": format!("Downloading {} crates and everything they use…", deps.len()) }));
            let mut cmd = tokio::process::Command::new(cargo);
            cmd.arg("fetch");
            // Only what this computer builds (not every platform's crates: half the download).
            if let Some(host) = host_triple().await {
                cmd.args(["--target", &host]);
            }
            cmd.current_dir(&proj).env("CARGO_HOME", cargo_home(&app)).env("CARGO_TERM_COLOR", "never");
            run_logged(&c, "rust", cmd, &on_event).await?;
        }
    }
    if !chosen.is_empty() {
        let _g = c.lock.lock().unwrap();
        let mut done = packs_done(&app);
        for p in &chosen {
            done.insert(format!("{}/{}", lang.name(), p.id), json!({ "at": now() }));
        }
        write_json(&r.join("packs.json"), &done)?;
    }
    Ok(json!({ "ok": true }))
}

#[tauri::command]
pub fn colab_download_stop(c: State<'_, Colab>, lang: Lang) {
    if let Some(pid) = c.jobs.lock().unwrap().get(lang.name()) {
        let _ = std::process::Command::new("/bin/kill").args(["-TERM", &pid.to_string()]).stderr(Stdio::null()).status();
    }
}

/// What's installed in Colab's Python: `[{name, version}]`.
#[tauri::command]
pub async fn colab_packages(app: AppHandle) -> CmdResult<Value> {
    let py = venv_python(&app);
    if !py.is_file() {
        return Ok(json!([]));
    }
    let out = tokio::process::Command::new(py).args(["-m", "pip", "list", "--format", "json", "--disable-pip-version-check"]).output().await.map_err(io)?;
    Ok(serde_json::from_slice(&out.stdout).unwrap_or(json!([])))
}

// ---------------------------------------------------------------- notebooks

fn language_of(nb: &Value) -> Lang {
    let names = [&nb["metadata"]["kernelspec"]["language"], &nb["metadata"]["language_info"]["name"], &nb["metadata"]["kernelspec"]["name"]];
    if names.iter().any(|v| v.as_str().is_some_and(|s| s.to_lowercase() == "rust")) {
        Lang::Rust
    } else {
        Lang::Python
    }
}

fn title_of(nb: &Value, id: &str) -> String {
    nb["metadata"]["rad"]["title"].as_str().or(nb["metadata"]["colab"]["name"].as_str()).map(|s| s.trim_end_matches(".ipynb").to_string()).unwrap_or_else(|| id.to_string())
}

#[derive(Serialize)]
struct Listed {
    id: String,
    title: String,
    lang: Lang,
    updated: i64,
    cells: usize,
}

#[tauri::command]
pub fn colab_list(app: AppHandle, lang: Lang) -> CmdResult<Vec<Value>> {
    let dir = root(&app).join("notebooks");
    let mut out: Vec<Listed> = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("ipynb") {
            continue;
        }
        let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let Ok(nb) = std::fs::read(&p).map_err(io).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(io)) else { continue };
        if language_of(&nb) != lang {
            continue;
        }
        let updated = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
        out.push(Listed { title: title_of(&nb, &id), id, lang, updated, cells: nb["cells"].as_array().map_or(0, |c| c.len()) });
    }
    out.sort_by_key(|l| std::cmp::Reverse(l.updated));
    Ok(out.into_iter().map(|l| serde_json::to_value(l).unwrap()).collect())
}

#[tauri::command]
pub fn colab_open(app: AppHandle, id: String) -> CmdResult<Value> {
    let b = std::fs::read(notebook_path(&app, &id)?).map_err(io)?;
    serde_json::from_slice(&b).map_err(|e| CmdError::new("bad_notebook", format!("Not a notebook: {e}")))
}

/// Saves a notebook (creating it when `id` is null); returns its id.
#[tauri::command]
pub fn colab_save(app: AppHandle, id: Option<String>, notebook: Value) -> CmdResult<String> {
    if !notebook["cells"].is_array() {
        return Err(CmdError::new("bad_request", "a notebook needs cells"));
    }
    let id = id.unwrap_or_else(new_id);
    write_json(&notebook_path(&app, &id)?, &notebook)?;
    Ok(id)
}

#[tauri::command]
pub async fn colab_delete(app: AppHandle, c: State<'_, Colab>, id: String) -> CmdResult<()> {
    shutdown(&c, &id);
    let _ = std::fs::remove_dir_all(root(&app).join("rust/build").join(&id));
    std::fs::remove_file(notebook_path(&app, &id)?).map_err(io)
}

/// Copies .ipynb files in (from Google Colab, Jupyter…): `[{id, lang, title} | {error}]`.
#[tauri::command]
pub fn colab_import(app: AppHandle, paths: Vec<String>) -> Vec<Value> {
    paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            let read = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|e| format!("not a notebook ({e})")));
            match read {
                Ok(mut nb) if nb["cells"].is_array() => {
                    let title = nb["metadata"]["rad"]["title"].as_str().map(String::from).unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("Notebook").to_string());
                    if !nb["metadata"].is_object() {
                        nb["metadata"] = json!({});
                    }
                    nb["metadata"]["rad"] = json!({ "title": title, "imported": now() });
                    let lang = language_of(&nb);
                    match colab_save(app.clone(), None, nb) {
                        Ok(id) => json!({ "id": id, "lang": lang, "title": title }),
                        Err(e) => json!({ "error": format!("{p}: {}", e.message) }),
                    }
                }
                Ok(_) => json!({ "error": format!("{p}: not a notebook (no cells)") }),
                Err(e) => json!({ "error": format!("{p}: {e}") }),
            }
        })
        .collect()
}

#[tauri::command]
pub fn colab_export(app: AppHandle, id: String, path: String) -> CmdResult<()> {
    std::fs::copy(notebook_path(&app, &id)?, path).map_err(io).map(|_| ())
}

/// The notebooks' working folder (opened in Files, or the Finder).
#[tauri::command]
pub fn colab_folder(app: AppHandle) -> CmdResult<String> {
    let d = root(&app).join("files");
    std::fs::create_dir_all(&d).map_err(io)?;
    Ok(d.to_string_lossy().into())
}

// ---------------------------------------------------------------- kernels

async fn start<R: Runtime>(app: &AppHandle<R>, c: &Colab, id: &str, lang: Lang) -> CmdResult<Arc<Kernel>> {
    let _one = c.starting.lock().await;
    check_id(id)?;
    if let Some(k) = c.kernels.lock().unwrap().get(id).cloned() {
        if k.lang == lang {
            return Ok(k);
        }
    }
    shutdown(c, id);
    let (offline, _) = update_offline(app, c).await;
    let r = root(app);
    let files = r.join("files");
    std::fs::create_dir_all(&files).map_err(io)?;
    let mut cmd = match lang {
        Lang::Python => {
            let py = venv_python(app);
            if !py.is_file() {
                return Err(CmdError::new("setup", "Colab's Python isn't set up yet."));
            }
            let script = r.join("python/rad_kernel.py");
            std::fs::write(&script, KERNEL_PY).map_err(io)?;
            let mut cmd = tokio::process::Command::new(py);
            cmd.arg("-u").arg(script).env("RAD_COLAB_WHEELS", wheels(app)).env("RAD_COLAB_OFFLINE_FLAG", r.join("offline")).env("MPLBACKEND", "Agg").env("PYTHONUNBUFFERED", "1");
            cmd
        }
        Lang::Rust => {
            let bin = rust_kernel(app).ok_or_else(|| CmdError::new("setup", "The Rust kernel isn't built. Run ./scripts/build-colab.sh"))?;
            if cargo().is_none() {
                return Err(CmdError::new("setup", "Rust isn't installed on this computer. Install it from rustup.rs, then try again."));
            }
            let build = r.join("rust/build").join(id);
            std::fs::create_dir_all(&build).map_err(io)?;
            std::fs::create_dir_all(cargo_home(app)).map_err(io)?;
            let mut cmd = tokio::process::Command::new(bin);
            cmd.env("CARGO_HOME", cargo_home(app)).env("EVCXR_TMPDIR", build).env("CARGO_TERM_COLOR", "never");
            if offline {
                cmd.env("CARGO_NET_OFFLINE", "true");
            }
            cmd
        }
    };
    cmd.current_dir(&files).env("PATH", tool_path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| CmdError::new("spawn", format!("Couldn't start the kernel: {e}")))?;
    let pid = child.id().unwrap_or(0);
    let stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let err = child.stderr.take().unwrap();

    // The first line says it's ready (the Rust kernel builds a first cell to get there).
    let first = tokio::time::timeout(Duration::from_secs(180), out.next_line()).await;
    let ready: Value = match first {
        Ok(Ok(Some(line))) => serde_json::from_str(&line).unwrap_or(json!({})),
        _ => {
            let _ = child.kill().await;
            return Err(CmdError::new("kernel", "The kernel didn't start"));
        }
    };
    if ready["type"] != "ready" {
        let _ = child.kill().await;
        return Err(CmdError::new("kernel", ready["message"].as_str().unwrap_or("The kernel didn't start").to_string()));
    }
    if lang == Lang::Rust && offline {
        // Set in the kernel too (CARGO_NET_OFFLINE covers cargo; this covers evcxr's own checks).
        let mut s = stdin;
        s.write_all(b"{\"id\":0,\"op\":\"offline\",\"on\":true}\n").await.map_err(io)?;
        let _ = out.next_line().await;
        return finish_start(c, id, lang, pid, s, out, err, child, ready);
    }
    finish_start(c, id, lang, pid, stdin, out, err, child, ready)
}

#[allow(clippy::too_many_arguments)]
fn finish_start(
    c: &Colab,
    id: &str,
    lang: Lang,
    pid: u32,
    stdin: tokio::process::ChildStdin,
    mut out: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    err: tokio::process::ChildStderr,
    mut child: tokio::process::Child,
    ready: Value,
) -> CmdResult<Arc<Kernel>> {
    let k = Arc::new(Kernel { lang, pid, stdin: tokio::sync::Mutex::new(stdin), pending: Mutex::new(HashMap::new()), current: AtomicU64::new(0), next: AtomicU64::new(1), ready });
    c.kernels.lock().unwrap().insert(id.to_string(), k.clone());

    // What the kernel process writes on stderr: cargo building the crates (Rust), or C code
    // printing (Python). It goes to the run going on.
    let kk = k.clone();
    tokio::spawn(async move {
        let mut l = BufReader::new(err).lines();
        while let Ok(Some(line)) = l.next_line().await {
            let id = kk.current.load(Ordering::SeqCst);
            let kind = if kk.lang == Lang::Rust && is_cargo_line(&line) { "build" } else { "stderr" };
            kk.route(json!({ "id": id, "type": kind, "text": format!("{line}\n") }));
        }
    });
    let kk = k.clone();
    tokio::spawn(async move {
        while let Ok(Some(line)) = out.next_line().await {
            if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                kk.route(msg);
            }
        }
        let status = child.wait().await.ok();
        let why = match status.and_then(|s| s.code()) {
            Some(code) => format!("The kernel stopped (exit code {code}). Restart it to go on."),
            None => "The kernel stopped. Restart it to go on.".to_string(),
        };
        kk.fail_all(&why);
    });
    Ok(k)
}

fn is_cargo_line(line: &str) -> bool {
    let t = line.trim_start();
    ["Compiling ", "Downloading ", "Downloaded ", "Updating ", "Locking ", "Adding ", "Blocking ", "Building ", "Finished ", "Fresh "].iter().any(|p| t.starts_with(p))
}

fn shutdown(c: &Colab, id: &str) {
    if let Some(k) = c.kernels.lock().unwrap().remove(id) {
        kill_tree(k.pid, "-KILL");
        k.fail_all("The kernel was shut down.");
    }
}

/// Signals a kernel and the processes it started (evcxr's runtime, a `!command`).
fn kill_tree(pid: u32, signal: &str) {
    if pid == 0 {
        return;
    }
    if signal == "-KILL" {
        let _ = std::process::Command::new("/usr/bin/pkill").args(["-KILL", "-P", &pid.to_string()]).stderr(Stdio::null()).status();
    }
    let _ = std::process::Command::new("/bin/kill").args([signal, &pid.to_string()]).stderr(Stdio::null()).status();
}

/// Starts a notebook's kernel (if it isn't running): what it said when it was ready.
#[tauri::command]
pub async fn colab_kernel_start(app: AppHandle, c: State<'_, Colab>, id: String, lang: Lang) -> CmdResult<Value> {
    Ok(start(&app, &c, &id, lang).await?.ready.clone())
}

/// Runs a cell, streaming its output on `on_event`; the cell's end (`done` or `error`) is the answer.
#[tauri::command]
pub async fn colab_run(app: AppHandle, c: State<'_, Colab>, id: String, lang: Lang, code: String, on_event: Channel<Value>) -> CmdResult<Value> {
    let k = start(&app, &c, &id, lang).await?;
    k.request(json!({ "op": "run", "code": code }), on_event).await
}

#[tauri::command]
pub fn colab_interrupt(c: State<'_, Colab>, id: String) -> CmdResult<()> {
    let k = c.kernels.lock().unwrap().get(&id).cloned();
    let Some(k) = k else { return Ok(()) };
    match k.lang {
        // KeyboardInterrupt, as in a terminal: the variables stay.
        Lang::Python => kill_tree(k.pid, "-INT"),
        // evcxr stops a cell by ending its runtime process (the kernel itself does that).
        Lang::Rust => {
            let k = k.clone();
            tauri::async_runtime::spawn(async move {
                let _ = k.stdin.lock().await.write_all(b"{\"op\":\"interrupt\"}\n").await;
            });
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn colab_restart(app: AppHandle, c: State<'_, Colab>, id: String, lang: Lang) -> CmdResult<Value> {
    shutdown(&c, &id);
    Ok(start(&app, &c, &id, lang).await?.ready.clone())
}

#[tauri::command]
pub fn colab_shutdown(c: State<'_, Colab>, id: String) {
    shutdown(&c, &id);
}

/// The notebook's variables, if its kernel is running.
#[tauri::command]
pub async fn colab_vars(c: State<'_, Colab>, id: String) -> CmdResult<Value> {
    let k = c.kernels.lock().unwrap().get(&id).cloned();
    let Some(k) = k else { return Ok(json!([])) };
    let r = tokio::time::timeout(Duration::from_secs(10), k.request(json!({ "op": "vars" }), Channel::new(|_| Ok(())))).await.map_err(|_| CmdError::new("busy", "The kernel is busy"))??;
    Ok(r["vars"].clone())
}

impl Colab {
    /// When RAD quits: no kernel is left behind.
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.kernels.lock().unwrap().keys().cloned().collect();
        for id in ids {
            shutdown(self, &id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel that keeps what's sent on it.
    fn collector() -> (Channel<Value>, Arc<Mutex<Vec<Value>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let ch = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(s) = body {
                if let Ok(v) = serde_json::from_str(&s) {
                    g.lock().unwrap().push(v);
                }
            }
            Ok(())
        });
        (ch, got)
    }

    fn printed(got: &Mutex<Vec<Value>>) -> String {
        got.lock().unwrap().iter().filter(|m| m["type"] == "stdout").filter_map(|m| m["text"].as_str()).collect()
    }

    /// Both kernels, through the code RAD uses: state between cells, streamed output, errors,
    /// interrupts. The Rust half runs when rad-rust-kernel has been built.
    #[tokio::test(flavor = "multi_thread")]
    async fn kernels_run_cells() {
        if system_python().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the only test that reads RAD_COLAB_DIR.
        unsafe { std::env::set_var("RAD_COLAB_DIR", dir.path()) };
        let app = tauri::test::mock_app();
        let h = app.handle();
        let c = Colab::default();

        ensure_venv(h, &c, &collector().0).await.unwrap();
        let k = start(h, &c, "py1", Lang::Python).await.unwrap();
        let (ch, got) = collector();
        let r = k.request(json!({ "op": "run", "code": "x = 6\nprint('hi')\nx * 7" }), ch).await.unwrap();
        assert_eq!(r["type"], "done", "{r}");
        assert_eq!(r["data"]["text/plain"], "42");
        assert_eq!(printed(&got), "hi\n");
        let r = k.request(json!({ "op": "run", "code": "1/0" }), collector().0).await.unwrap();
        assert_eq!(r["ename"], "ZeroDivisionError");
        // An interrupt stops the cell and keeps the variables.
        let k2 = k.clone();
        let run = tokio::spawn(async move { k2.request(json!({ "op": "run", "code": "import time\nwhile True: time.sleep(0.05)" }), collector().0).await });
        tokio::time::sleep(Duration::from_millis(600)).await;
        kill_tree(k.pid, "-INT");
        let r = run.await.unwrap().unwrap();
        assert_eq!(r["kind"], "interrupted", "{r}");
        let r = k.request(json!({ "op": "run", "code": "x" }), collector().0).await.unwrap();
        assert_eq!(r["data"]["text/plain"], "6");
        // The same kernel comes back for the notebook; a shut down one fails what waits on it.
        assert_eq!(start(h, &c, "py1", Lang::Python).await.unwrap().pid, k.pid);
        let k2 = k.clone();
        let run = tokio::spawn(async move { k2.request(json!({ "op": "run", "code": "import time\ntime.sleep(30)" }), collector().0).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        shutdown(&c, "py1");
        assert_eq!(run.await.unwrap().unwrap()["type"], "error");

        if rust_kernel(h).is_none() || cargo().is_none() {
            return;
        }
        let k = start(h, &c, "rs1", Lang::Rust).await.unwrap();
        let (ch, got) = collector();
        let r = k.request(json!({ "op": "run", "code": "let a: i64 = 2;\nprintln!(\"a is {a}\");\na + 3" }), ch).await.unwrap();
        assert_eq!(r["type"], "done", "{r}");
        assert_eq!(r["data"]["text/plain"], "5");
        assert_eq!(printed(&got), "a is 2\n");
        let r = k.request(json!({ "op": "run", "code": "let s: i32 = \"no\";" }), collector().0).await.unwrap();
        assert_eq!(r["kind"], "compile", "{r}");
        assert_eq!(r["errors"][0]["line"], 1);
        let k2 = k.clone();
        let run = tokio::spawn(async move { k2.request(json!({ "op": "run", "code": "loop { std::thread::sleep(std::time::Duration::from_millis(50)); }" }), collector().0).await });
        tokio::time::sleep(Duration::from_secs(4)).await;
        k.stdin.lock().await.write_all(b"{\"op\":\"interrupt\"}\n").await.unwrap();
        assert_eq!(run.await.unwrap().unwrap()["kind"], "interrupted");
        c.stop_all();
        assert!(c.kernels.lock().unwrap().is_empty());
    }
}
