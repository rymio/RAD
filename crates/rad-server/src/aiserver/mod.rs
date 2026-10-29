//! The AI API server: RAD's models, shared with other programs in the dialects they already
//! speak — OpenAI (`/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/models`),
//! Anthropic (`/v1/messages`) and Ollama (`/api/chat`, `/api/generate`, `/api/embed`,
//! `/api/tags`, `/api/ps`, `/api/show`) — plus `/rad/stats`: what is shared and how it's used.
//!
//! Switched on in Settings → AI API server. It listens on its own port (this computer only, or
//! the local network), apart from RAD's own API, and runs while the vault is unlocked: its
//! settings and keys are in the vault, as are the other providers' credentials. It shares the
//! downloaded built-in (llama.cpp) models and, if chosen, the models of the other providers set
//! up in RAD (their requests then use those accounts).
//!
//! Programs send `Authorization: Bearer radai_…` or `x-api-key: radai_…` (keys made in the
//! settings). Keys can be made optional for programs that can't send one (most Ollama
//! clients); browsers on other sites are then turned away.
//!
//! The RAD app manages it: GET/PUT /v1/ai-server, POST /v1/ai-server/keys,
//! DELETE /v1/ai-server/keys/{id}.

mod protocols;

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use rad_api::ProviderKind;
use rad_llm::ProviderConn;
use rad_vault::rusqlite::{params, OptionalExtension};
use rad_vault::{now_ms, repo, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::Authed;
use crate::error::{ApiErr, ApiResult};
use crate::state::{db_vault, Shared};

const CONFIG: &str = "ai_server";
const KEYS: &str = "ai_server_api_keys";
pub const KEY_PREFIX: &str = "radai_";
/// Other providers' model lists are asked for again after this long.
const CATALOG_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub enabled: bool,
    pub port: u16,
    /// Listen on every network interface (else only on this computer).
    pub lan: bool,
    pub require_key: bool,
    /// Also share the models of the other providers set up in RAD.
    pub share_providers: bool,
    /// Models not shared (their shared names).
    pub hidden: Vec<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig { enabled: false, port: 11435, lan: false, require_key: true, share_providers: false, hidden: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiKey {
    id: String,
    name: String,
    /// The first characters, to recognise it.
    prefix: String,
    /// blake3 of the whole key.
    hash: String,
    created_at: i64,
}

/// Use of one model (or by one key) since the server started.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Usage {
    pub requests: u64,
    pub errors: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Time spent answering, for tokens per second.
    pub busy_ms: u64,
    pub last_used: Option<i64>,
}

#[derive(Default)]
struct Stats {
    since: i64,
    requests: u64,
    active: u32,
    by_model: BTreeMap<String, Usage>,
    by_api: BTreeMap<&'static str, u64>,
    by_key: BTreeMap<String, Usage>,
}

struct Running {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

/// The server's state, kept in `AppState`.
#[derive(Default)]
pub struct AiServer {
    running: Mutex<Option<Running>>,
    /// Why it isn't running although switched on (e.g. the port is taken).
    error: Mutex<Option<String>>,
    cfg: RwLock<AiConfig>,
    keys: RwLock<Vec<AiKey>>,
    stats: Mutex<Stats>,
    catalog: tokio::sync::Mutex<Option<(Instant, Vec<SharedModel>)>>,
}

impl AiServer {
    /// Stops listening; the server finishes the answers under way on its own.
    pub fn stop(&self) {
        drop(self.halt());
    }

    fn halt(&self) -> Option<tokio::task::JoinHandle<()>> {
        let r = self.running.lock().unwrap().take()?;
        let _ = r.shutdown.send(());
        tracing::info!("AI API server on {} stopped", r.addr);
        Some(r.task)
    }

    /// Stopped because the vault locked: forgets the keys too.
    pub fn lock(&self) {
        self.stop();
        self.keys.write().unwrap().clear();
    }

    fn config(&self) -> AiConfig {
        self.cfg.read().unwrap().clone()
    }

    /// The id of the key with this token.
    fn key_for(&self, token: &str) -> Option<(String, String)> {
        let hash = blake3::hash(token.as_bytes()).to_hex().to_string();
        self.keys.read().unwrap().iter().find(|k| k.hash == hash).map(|k| (k.id.clone(), k.name.clone()))
    }

    fn begin(&self, api: &'static str) {
        let mut s = self.stats.lock().unwrap();
        s.requests += 1;
        s.active += 1;
        *s.by_api.entry(api).or_default() += 1;
    }

    fn end(&self, model: &str, key: Option<&str>, ok: bool, tokens_in: u64, tokens_out: u64, ms: u64) {
        let mut s = self.stats.lock().unwrap();
        s.active = s.active.saturating_sub(1);
        let now = now_ms();
        let add = |u: &mut Usage| {
            u.requests += 1;
            u.errors += u64::from(!ok);
            u.tokens_in += tokens_in;
            u.tokens_out += tokens_out;
            u.busy_ms += ms;
            u.last_used = Some(now);
        };
        add(s.by_model.entry(model.to_string()).or_default());
        if let Some(k) = key {
            add(s.by_key.entry(k.to_string()).or_default());
        }
    }
}

// ------------------------------------------------------------------ settings in the vault

fn load<T: for<'de> Deserialize<'de>>(c: &Connection, key: &str) -> ApiResult<Option<T>> {
    let raw: Option<String> = c.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0)).optional().map_err(ApiErr::internal)?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

fn store<T: Serialize>(c: &Connection, key: &str, v: &T) -> ApiResult<()> {
    c.execute("INSERT OR REPLACE INTO settings(key, value) VALUES (?1, ?2)", params![key, serde_json::to_string(v).map_err(ApiErr::internal)?]).map_err(ApiErr::internal)?;
    Ok(())
}

// ------------------------------------------------------------------ starting and stopping

/// Puts the settings into effect: starts, restarts or stops the listener as needed.
async fn apply(st: &Shared, cfg: AiConfig, keys: Vec<AiKey>) {
    *st.ai.cfg.write().unwrap() = cfg.clone();
    *st.ai.keys.write().unwrap() = keys;
    *st.ai.catalog.lock().await = None;
    let want = cfg.enabled.then(|| SocketAddr::new(if cfg.lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) }, cfg.port));
    // Same address: the new settings apply as they are (they're read on every request).
    if st.ai.running.lock().unwrap().as_ref().map(|r| r.addr) == want {
        *st.ai.error.lock().unwrap() = None;
        return;
    }
    // Moving to another address (or the same port, now on the network): the old listener lets
    // go of its port first, after the answers under way (for a few seconds at most).
    if let Some(old) = st.ai.halt() {
        let _ = tokio::time::timeout(Duration::from_secs(5), old).await;
    }
    *st.ai.error.lock().unwrap() = None;
    let Some(addr) = want else { return };
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::AddrInUse { format!("port {} is already in use — choose another", cfg.port) } else { format!("can't listen on {addr}: {e}") };
            tracing::warn!("AI API server: {msg}");
            *st.ai.error.lock().unwrap() = Some(msg);
            return;
        }
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let router = protocols::router(st.clone());
    let task = tokio::spawn(async move {
        let served = axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = rx.await;
        });
        if let Err(e) = served.await {
            tracing::error!("AI API server stopped: {e}");
        }
    });
    {
        let mut s = st.ai.stats.lock().unwrap();
        *s = Stats { since: now_ms(), ..Default::default() };
    }
    *st.ai.running.lock().unwrap() = Some(Running { addr, shutdown: tx, task });
    tracing::info!("AI API server listening on {addr}");
}

/// After unlocking: starts the server if it's switched on.
pub async fn on_unlock(st: Shared) {
    let Ok(v) = st.vault() else { return };
    let loaded = db_vault(v, &st, |_, c| Ok((load::<AiConfig>(c, CONFIG)?.unwrap_or_default(), load::<Vec<AiKey>>(c, KEYS)?.unwrap_or_default()))).await;
    match loaded {
        Ok((cfg, keys)) => apply(&st, cfg, keys).await,
        Err(e) => tracing::warn!("AI API server settings: {}", e.message),
    }
}

// ------------------------------------------------------------------ what is shared

#[derive(Debug, Clone)]
pub struct SharedModel {
    /// The name programs ask for.
    pub name: String,
    pub conn: ProviderConn,
    /// The model's id at its provider.
    pub model: String,
    pub embedding: bool,
    /// "rad" for built-in models, else the provider's name.
    pub owner: String,
    pub builtin: bool,
    pub size: Option<u64>,
    pub family: Option<String>,
    pub parameters: Option<String>,
    pub ctx: Option<u32>,
    pub quantization: Option<String>,
    pub digest: String,
}

fn slug(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c.to_ascii_lowercase() } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// "Q4_K_M" from "Qwen3-4B-Q4_K_M.gguf".
fn quantization(file: &str) -> Option<String> {
    let stem = file.trim_end_matches(".gguf");
    stem.split(['-', '.']).rev().find(|p| p.len() >= 2 && (p.starts_with('Q') || p.starts_with("IQ") || *p == "F16" || *p == "BF16")).map(|p| p.to_string())
}

/// Every model the server can share (hidden ones too); other providers' lists are cached.
pub async fn all_models(st: &Shared) -> Vec<SharedModel> {
    let cfg = st.ai.config();
    let providers = match st.vault() {
        Ok(v) => db_vault(v, st, |_, c| Ok(repo::providers(c)?)).await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let mut out = Vec::new();
    if let Some(p) = providers.iter().find(|p| p.kind == ProviderKind::Builtin && p.enabled) {
        let conn = crate::models::conn(p);
        for m in st.library.status().into_iter().filter(|m| m.path.is_some()) {
            let s = m.spec;
            out.push(SharedModel {
                name: s.id.to_string(),
                conn: conn.clone(),
                model: s.id.to_string(),
                embedding: s.kind == rad_local::ModelKind::Embedding,
                owner: "rad".into(),
                builtin: true,
                size: Some(s.bytes),
                family: Some(s.family.to_string()),
                parameters: Some(s.parameters.to_string()),
                ctx: Some(s.ctx),
                quantization: quantization(s.file),
                digest: s.sha256.to_string(),
            });
        }
    }
    if !cfg.share_providers {
        return out;
    }
    let mut cache = st.ai.catalog.lock().await;
    if let Some((_, list)) = cache.as_ref().filter(|(at, _)| at.elapsed() < CATALOG_TTL) {
        out.extend(list.iter().cloned());
        return out;
    }
    let mut others = Vec::new();
    for p in providers.iter().filter(|p| p.kind != ProviderKind::Builtin && p.enabled) {
        let conn = crate::models::conn(p);
        let models = match tokio::time::timeout(Duration::from_secs(8), st.llm.list_models(&conn)).await {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                tracing::debug!("AI API server: {} models: {e}", p.name);
                continue;
            }
            Err(_) => continue,
        };
        let owner = slug(&p.name);
        for m in models {
            let name = format!("{owner}/{}", m.id);
            others.push(SharedModel {
                digest: blake3::hash(name.as_bytes()).to_hex().to_string(),
                name,
                conn: conn.clone(),
                embedding: m.id.to_ascii_lowercase().contains("embed"),
                model: m.id,
                owner: p.name.clone(),
                builtin: false,
                size: m.size,
                family: m.family,
                parameters: m.parameter_size,
                ctx: None,
                quantization: None,
            });
        }
    }
    out.extend(others.iter().cloned());
    *cache = Some((Instant::now(), others));
    out
}

/// The models programs can use.
pub async fn shared_models(st: &Shared) -> Vec<SharedModel> {
    let hidden = st.ai.config().hidden;
    all_models(st).await.into_iter().filter(|m| !hidden.contains(&m.name)).collect()
}

/// The shared model a program asked for ("qwen3-4b", "qwen3-4b:latest", "ollama/llama3.2"…).
pub async fn find_model(st: &Shared, name: &str) -> Option<SharedModel> {
    let models = shared_models(st).await;
    let n = name.trim();
    let bare = n.strip_suffix(":latest").unwrap_or(n);
    models
        .iter()
        .find(|m| m.name == n)
        .or_else(|| models.iter().find(|m| m.name == bare || m.name.eq_ignore_ascii_case(bare)))
        // A provider's model asked for without the provider ("gpt-4o-mini").
        .or_else(|| models.iter().find(|m| !m.builtin && m.model == bare))
        .cloned()
}

// ------------------------------------------------------------------ statistics

fn tokens_per_second(u: &Usage) -> Option<f64> {
    (u.busy_ms > 0 && u.tokens_out > 0).then(|| (u.tokens_out as f64 * 1000.0 / u.busy_ms as f64 * 10.0).round() / 10.0)
}

fn usage_json(u: &Usage) -> Value {
    json!({"requests": u.requests, "errors": u.errors, "tokens_in": u.tokens_in, "tokens_out": u.tokens_out, "tokens_per_second": tokens_per_second(u), "last_used": u.last_used})
}

/// The models and their use, as `/rad/stats` shows them to programs.
pub async fn public_stats(st: &Shared) -> Value {
    let models = shared_models(st).await;
    let loaded = rad_local::loaded();
    let s = st.ai.stats.lock().unwrap();
    let empty = Usage::default();
    json!({
        "server": "RAD",
        "name": st.cfg.name,
        "version": env!("CARGO_PKG_VERSION"),
        "since": s.since,
        "uptime_seconds": (now_ms() - s.since).max(0) / 1000,
        "requests": s.requests,
        "active": s.active,
        "apis": s.by_api,
        "models": models.iter().map(|m| {
            let u = s.by_model.get(&m.name).unwrap_or(&empty);
            json!({
                "name": m.name,
                "kind": if m.embedding { "embedding" } else { "chat" },
                "owned_by": m.owner,
                "loaded": m.builtin && loaded.contains(&m.model.as_str()),
                "parameters": m.parameters,
                "context": m.ctx,
                "usage": usage_json(u),
            })
        }).collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------------------ the RAD app's settings

/// Addresses on this computer that other computers on the network can reach.
fn lan_address() -> Option<IpAddr> {
    // No packet is sent: connecting a UDP socket only picks the outgoing interface.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:80").ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

async fn status(st: &Shared) -> Value {
    let cfg = st.ai.config();
    let addr = st.ai.running.lock().unwrap().as_ref().map(|r| r.addr);
    let error = st.ai.error.lock().unwrap().clone();
    let models = all_models(st).await;
    let loaded = rad_local::loaded();
    let keys = st.ai.keys.read().unwrap().clone();
    let s = st.ai.stats.lock().unwrap();
    let empty = Usage::default();
    let mut urls = Vec::new();
    if addr.is_some() {
        urls.push(format!("http://127.0.0.1:{}", cfg.port));
        if cfg.lan {
            if let Some(ip) = lan_address() {
                urls.push(format!("http://{ip}:{}", cfg.port));
            }
        }
    }
    let total = s.by_model.values().fold(Usage::default(), |mut a, u| {
        a.tokens_in += u.tokens_in;
        a.tokens_out += u.tokens_out;
        a.errors += u.errors;
        a.busy_ms += u.busy_ms;
        a.requests += u.requests;
        a
    });
    json!({
        "config": cfg,
        "running": addr.is_some(),
        "error": error,
        "urls": urls,
        "since": addr.map(|_| s.since),
        "requests": s.requests,
        "active": s.active,
        "apis": s.by_api,
        "totals": usage_json(&total),
        "keys": keys.iter().map(|k| json!({"id": k.id, "name": k.name, "prefix": k.prefix, "created_at": k.created_at, "usage": usage_json(s.by_key.get(&k.id).unwrap_or(&empty))})).collect::<Vec<_>>(),
        "models": models.iter().map(|m| json!({
            "name": m.name,
            "kind": if m.embedding { "embedding" } else { "chat" },
            "owned_by": m.owner,
            "builtin": m.builtin,
            "shared": !cfg.hidden.contains(&m.name),
            "loaded": m.builtin && loaded.contains(&m.model.as_str()),
            "size": m.size,
            "parameters": m.parameters,
            "usage": usage_json(s.by_model.get(&m.name).unwrap_or(&empty)),
        })).collect::<Vec<_>>(),
    })
}

pub async fn get_status(State(st): State<Shared>, _a: Authed) -> ApiResult<Json<Value>> {
    Ok(Json(status(&st).await))
}

pub async fn put_config(State(st): State<Shared>, a: Authed, Json(cfg): Json<AiConfig>) -> ApiResult<Json<Value>> {
    if cfg.port < 1024 {
        return Err(ApiErr::bad_request("choose a port from 1024 to 65535"));
    }
    let save = cfg.clone();
    let keys = db_vault(a.vault, &st, move |_, c| {
        store(c, CONFIG, &save)?;
        Ok(load::<Vec<AiKey>>(c, KEYS)?.unwrap_or_default())
    })
    .await?;
    apply(&st, cfg, keys).await;
    Ok(Json(status(&st).await))
}

#[derive(Deserialize)]
pub struct KeyInput {
    name: String,
}

/// Makes an API key. The key itself is shown once, in this answer.
pub async fn create_key(State(st): State<Shared>, a: Authed, Json(input): Json<KeyInput>) -> ApiResult<Json<Value>> {
    let name = input.name.trim().chars().take(60).collect::<String>();
    if name.is_empty() {
        return Err(ApiErr::bad_request("name the key after the program or person that will use it"));
    }
    let token = format!("{KEY_PREFIX}{}", crate::devices::random_hex(32));
    let k = AiKey {
        id: format!("k{}", now_ms()),
        name,
        prefix: token.chars().take(KEY_PREFIX.len() + 6).collect(),
        hash: blake3::hash(token.as_bytes()).to_hex().to_string(),
        created_at: now_ms(),
    };
    let keys = db_vault(a.vault, &st, move |_, c| {
        let mut keys: Vec<AiKey> = load(c, KEYS)?.unwrap_or_default();
        keys.push(k);
        store(c, KEYS, &keys)?;
        Ok(keys)
    })
    .await?;
    *st.ai.keys.write().unwrap() = keys;
    Ok(Json(json!({ "key": token })))
}

pub async fn delete_key(State(st): State<Shared>, a: Authed, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let keys = db_vault(a.vault, &st, move |_, c| {
        let mut keys: Vec<AiKey> = load(c, KEYS)?.unwrap_or_default();
        let before = keys.len();
        keys.retain(|k| k.id != id);
        if keys.len() == before {
            return Err(ApiErr::not_found());
        }
        store(c, KEYS, &keys)?;
        Ok(keys)
    })
    .await?;
    *st.ai.keys.write().unwrap() = keys;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(slug("My Ollama (laptop)"), "my-ollama-laptop");
        assert_eq!(quantization("Qwen3-4B-Q4_K_M.gguf").as_deref(), Some("Q4_K_M"));
        assert_eq!(quantization("nomic-embed-text-v1.5.Q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(quantization("model.gguf"), None);
    }
}
