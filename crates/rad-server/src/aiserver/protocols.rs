//! The three dialects, translated to RAD's model calls (rad-llm) and back. Requests are read
//! leniently (any content type, as `curl -d` sends); answers follow each API's own shapes,
//! streamed as server-sent events (OpenAI, Anthropic) or JSON lines (Ollama).

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use futures::{stream, Stream, StreamExt};
use rad_llm::{ChatMsg, ChatRequest, ImagePart, LlmError, StreamChunk};
use serde_json::{json, Value};

use super::{find_model, public_stats, shared_models, SharedModel, KEY_PREFIX};
use crate::state::Shared;

/// Output tokens when a request doesn't say.
const DEFAULT_MAX_TOKENS: u64 = 2048;
const MAX_TOKENS: u64 = 32_768;

pub fn router(st: Shared) -> Router {
    Router::new()
        .route("/", get(root).head(root))
        .route("/rad/stats", get(stats))
        // OpenAI (and Anthropic's model list, told apart by its `anthropic-version` header)
        .route("/v1/models", get(models))
        .route("/v1/models/{*id}", get(model))
        .route("/v1/chat/completions", post(openai_chat))
        .route("/v1/completions", post(openai_completion))
        .route("/v1/embeddings", post(openai_embeddings))
        // Anthropic
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/messages/count_tokens", post(anthropic_count))
        // Ollama
        .route("/api/version", get(ollama_version))
        .route("/api/tags", get(ollama_tags))
        .route("/api/ps", get(ollama_ps))
        .route("/api/show", post(ollama_show))
        .route("/api/chat", post(ollama_chat))
        .route("/api/generate", post(ollama_generate))
        .route("/api/embed", post(ollama_embed))
        .route("/api/embeddings", post(ollama_embeddings))
        .fallback(fallback)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        // Web pages may call it; without a key, only pages on this computer get answers (see Client).
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(st)
}

// ------------------------------------------------------------------ dialects and errors

#[derive(Clone, Copy, PartialEq)]
enum Api {
    OpenAi,
    Anthropic,
    Ollama,
}

impl Api {
    fn of(path: &str, headers: &HeaderMap) -> Api {
        if path.starts_with("/api/") {
            Api::Ollama
        } else if path.starts_with("/v1/messages") || headers.contains_key("anthropic-version") {
            Api::Anthropic
        } else {
            Api::OpenAi
        }
    }

    fn name(self) -> &'static str {
        match self {
            Api::OpenAi => "openai",
            Api::Anthropic => "anthropic",
            Api::Ollama => "ollama",
        }
    }
}

fn error(api: Api, status: StatusCode, msg: impl Into<String>) -> Response {
    let msg = msg.into();
    let kind = match status.as_u16() {
        400 | 413 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        503 => "overloaded_error",
        _ => "api_error",
    };
    let body = match api {
        Api::OpenAi => json!({"error": {"message": msg, "type": kind, "code": null}}),
        Api::Anthropic => json!({"type": "error", "error": {"type": kind, "message": msg}}),
        Api::Ollama => json!({"error": msg}),
    };
    (status, Json(body)).into_response()
}

fn bad(api: Api, msg: impl Into<String>) -> Response {
    error(api, StatusCode::BAD_REQUEST, msg)
}

fn llm_error(api: Api, e: LlmError) -> Response {
    let status = match &e {
        // The provider's own key or limits are RAD's business, not the caller's.
        LlmError::Http { status: 401 | 403, .. } => StatusCode::BAD_GATEWAY,
        LlmError::Http { status, .. } => StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY),
        LlmError::Unsupported(_) => StatusCode::BAD_REQUEST,
        LlmError::Local { retry: true, .. } => StatusCode::SERVICE_UNAVAILABLE,
        LlmError::Local { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_GATEWAY,
    };
    error(api, status, e.to_string())
}

async fn fallback(headers: HeaderMap, uri: axum::http::Uri) -> Response {
    error(Api::of(uri.path(), &headers), StatusCode::NOT_FOUND, format!("RAD's AI server has no {}", uri.path()))
}

// ------------------------------------------------------------------ who is asking

/// A program allowed to use the server; `key` is its API key's id.
pub struct Client {
    key: Option<String>,
}

fn local_origin(origin: &str) -> bool {
    let host = origin.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    let host = host.rsplit_once(':').filter(|(h, p)| !h.is_empty() && p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

impl FromRequestParts<Shared> for Client {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, st: &Shared) -> Result<Self, Response> {
        let api = Api::of(parts.uri.path(), &parts.headers);
        let h = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
        let token = crate::auth::bearer(parts).or_else(|| h("x-api-key")).or_else(|| h("api-key"));
        if let Some(t) = token.filter(|t| t.starts_with(KEY_PREFIX)) {
            return match st.ai.key_for(t) {
                Some((id, _)) => Ok(Client { key: Some(id) }),
                None => Err(error(api, StatusCode::UNAUTHORIZED, "unknown or deleted API key")),
            };
        }
        if st.ai.config().require_key {
            return Err(error(api, StatusCode::UNAUTHORIZED, "this RAD AI server needs an API key: send “Authorization: Bearer radai_…” (keys are made in RAD → Settings → AI API server)"));
        }
        // Without keys, only programs — not web pages on other sites, which anyone could make.
        if h("origin").is_some_and(|o| !local_origin(o)) {
            return Err(error(api, StatusCode::FORBIDDEN, "web pages need an API key to use this server"));
        }
        Ok(Client { key: None })
    }
}

fn body(api: Api, b: &Bytes) -> Result<Value, Response> {
    if b.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(b).map_err(|e| bad(api, format!("the request isn't valid JSON: {e}")))
}

async fn model_for(st: &Shared, api: Api, v: &Value, embedding: bool) -> Result<SharedModel, Response> {
    let name = v["model"].as_str().or(v["name"].as_str()).unwrap_or("").trim();
    if name.is_empty() {
        return Err(bad(api, "say which model to use (\"model\"); GET /v1/models or /api/tags lists them"));
    }
    let m = find_model(st, name).await.ok_or_else(|| error(api, StatusCode::NOT_FOUND, format!("model “{name}” isn't shared by this RAD server")))?;
    if m.embedding != embedding && m.builtin {
        return Err(bad(api, if embedding { format!("“{}” is a chat model, not an embedding model", m.name) } else { format!("“{}” is an embedding model; it can't chat", m.name) }));
    }
    Ok(m)
}

// ------------------------------------------------------------------ running a model

#[derive(Default)]
struct Tally {
    out_chars: usize,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    failed: bool,
}

impl Tally {
    /// (input, output) tokens: as the model counted them, else estimated (~4 characters each).
    fn usage(&self, prompt_chars: usize) -> (u64, u64) {
        let est = |chars: usize| chars.div_ceil(4) as u64;
        (self.tokens_in.map_or(est(prompt_chars), |t| t.max(0) as u64), self.tokens_out.map_or(est(self.out_chars), |t| t.max(0) as u64))
    }
}

/// Counts one request in the statistics when it's over (finished, failed or abandoned).
struct Guard {
    st: Shared,
    model: String,
    key: Option<String>,
    started: Instant,
    prompt_chars: usize,
    tally: Arc<Mutex<Tally>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let t = self.tally.lock().unwrap();
        let (i, o) = t.usage(self.prompt_chars);
        self.st.ai.end(&self.model, self.key.as_deref(), !t.failed, i, o, self.started.elapsed().as_millis() as u64);
    }
}

type Chunks = Pin<Box<dyn Stream<Item = Result<StreamChunk, LlmError>> + Send>>;

struct Run {
    chunks: Chunks,
    tally: Arc<Mutex<Tally>>,
    prompt_chars: usize,
    started: Instant,
}

fn prompt_chars(req: &ChatRequest) -> usize {
    req.system.as_ref().map_or(0, String::len) + req.messages.iter().map(|m| m.content.len()).sum::<usize>()
}

async fn run(st: &Shared, client: &Client, api: Api, m: &SharedModel, req: ChatRequest) -> Result<Run, Response> {
    if req.messages.is_empty() {
        return Err(bad(api, "there are no messages to answer"));
    }
    st.ai.begin(api.name());
    st.touch();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let pc = prompt_chars(&req);
    let started = Instant::now();
    let guard = Guard { st: st.clone(), model: m.name.clone(), key: client.key.clone(), started, prompt_chars: pc, tally: tally.clone() };
    match st.llm.chat_stream(&m.conn, &m.model, &req).await {
        Err(e) => {
            guard.tally.lock().unwrap().failed = true;
            Err(llm_error(api, e))
        }
        Ok(s) => {
            let t = tally.clone();
            let chunks = s.map(move |c| {
                let _counted = &guard;
                let mut t = t.lock().unwrap();
                match &c {
                    Ok(c) => {
                        t.out_chars += c.text.len();
                        t.tokens_in = c.tokens_in.or(t.tokens_in);
                        t.tokens_out = c.tokens_out.or(t.tokens_out);
                    }
                    Err(_) => t.failed = true,
                }
                c
            });
            Ok(Run { chunks: Box::pin(chunks), tally, prompt_chars: pc, started })
        }
    }
}

impl Run {
    /// The whole answer (not streaming).
    async fn text(mut self) -> Result<(String, u64, u64), LlmError> {
        let mut out = String::new();
        while let Some(c) = self.chunks.next().await {
            out.push_str(&c?.text);
        }
        drop(self.chunks);
        let (i, o) = self.tally.lock().unwrap().usage(self.prompt_chars);
        Ok((out, i, o))
    }
}

async fn embed(st: &Shared, client: &Client, api: Api, m: &SharedModel, texts: Vec<String>) -> Result<Vec<Vec<f32>>, Response> {
    if texts.is_empty() {
        return Err(bad(api, "there's no input to embed"));
    }
    st.ai.begin(api.name());
    st.touch();
    let tally = Arc::new(Mutex::new(Tally::default()));
    let guard = Guard { st: st.clone(), model: m.name.clone(), key: client.key.clone(), started: Instant::now(), prompt_chars: texts.iter().map(String::len).sum(), tally };
    let r = st.llm.embed(&m.conn, &m.model, &texts).await;
    if r.is_err() {
        guard.tally.lock().unwrap().failed = true;
    }
    r.map_err(|e| llm_error(api, e))
}

fn streaming(content_type: &'static str, s: impl Stream<Item = Bytes> + Send + 'static) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(s.map(Ok::<_, Infallible>)))
        .unwrap()
}

fn sse(data: &Value) -> Bytes {
    Bytes::from(format!("data: {data}\n\n"))
}

fn sse_event(name: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {data}\n\n"))
}

fn line(data: &Value) -> Bytes {
    Bytes::from(format!("{data}\n"))
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", crate::devices::random_hex(12))
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn now_rfc3339() -> String {
    rfc3339(chrono::Utc::now().timestamp_millis())
}

fn max_tokens(v: Option<u64>) -> u32 {
    v.filter(|n| *n > 0).unwrap_or(DEFAULT_MAX_TOKENS).min(MAX_TOKENS) as u32
}

fn temperature(v: &Value) -> Option<f32> {
    v.as_f64().map(|t| t as f32)
}

/// An image from a `data:` URL.
fn data_url(url: &str) -> Option<ImagePart> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?.to_string();
    Some(ImagePart { mime, data: base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()? })
}

fn texts(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------ shared answers

async fn root() -> &'static str {
    "RAD AI server is running (OpenAI, Anthropic and Ollama APIs)"
}

async fn stats(State(st): State<Shared>, _c: Client) -> Json<Value> {
    Json(public_stats(&st).await)
}

fn since_ms(st: &Shared) -> i64 {
    st.ai.stats.lock().unwrap().since
}

async fn models(State(st): State<Shared>, _c: Client, headers: HeaderMap) -> Json<Value> {
    let list = shared_models(&st).await;
    let since = since_ms(&st);
    if headers.contains_key("anthropic-version") {
        let data: Vec<Value> = list.iter().filter(|m| !m.embedding).map(|m| json!({"type": "model", "id": m.name, "display_name": m.name, "created_at": rfc3339(since)})).collect();
        let (first, last) = (data.first().map(|m| m["id"].clone()), data.last().map(|m| m["id"].clone()));
        return Json(json!({"data": data, "has_more": false, "first_id": first, "last_id": last}));
    }
    Json(json!({"object": "list", "data": list.iter().map(|m| openai_model(m, since)).collect::<Vec<_>>()}))
}

fn openai_model(m: &SharedModel, since: i64) -> Value {
    json!({"id": m.name, "object": "model", "created": since / 1000, "owned_by": m.owner})
}

async fn model(State(st): State<Shared>, _c: Client, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let api = Api::of("/v1/models", &headers);
    match find_model(&st, &id).await {
        Some(m) if api == Api::Anthropic => Json(json!({"type": "model", "id": m.name, "display_name": m.name, "created_at": rfc3339(since_ms(&st))})).into_response(),
        Some(m) => Json(openai_model(&m, since_ms(&st))).into_response(),
        None => error(api, StatusCode::NOT_FOUND, format!("model “{id}” isn't shared by this RAD server")),
    }
}

// ------------------------------------------------------------------ OpenAI

/// Text and images of an OpenAI message's content (a string or a list of parts).
fn openai_content(api: Api, v: &Value) -> Result<(String, Vec<ImagePart>), Response> {
    let mut text = String::new();
    let mut images = Vec::new();
    match v {
        Value::String(s) => text.push_str(s),
        Value::Array(parts) => {
            for p in parts {
                match p["type"].as_str().unwrap_or("text") {
                    "text" | "input_text" => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(p["text"].as_str().unwrap_or(""));
                    }
                    "image_url" => {
                        let url = p["image_url"]["url"].as_str().or(p["image_url"].as_str()).unwrap_or("");
                        images.push(data_url(url).ok_or_else(|| bad(api, "images must be sent as data: URLs (base64)"))?);
                    }
                    other => return Err(bad(api, format!("message parts of type “{other}” aren't supported"))),
                }
            }
        }
        _ => {}
    }
    Ok((text, images))
}

fn openai_request(api: Api, v: &Value) -> Result<ChatRequest, Response> {
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for m in v["messages"].as_array().ok_or_else(|| bad(api, "\"messages\" must be a list"))? {
        let (text, images) = openai_content(api, &m["content"])?;
        match m["role"].as_str().unwrap_or("user") {
            "system" | "developer" => system.push(text),
            "assistant" => messages.push(ChatMsg::new("assistant", text)),
            "tool" | "function" => messages.push(ChatMsg::new("user", format!("Tool result:\n{text}"))),
            _ => messages.push(ChatMsg { role: "user".into(), content: text, images }),
        }
    }
    Ok(ChatRequest {
        system: (!system.is_empty()).then(|| system.join("\n\n")),
        messages,
        temperature: temperature(&v["temperature"]),
        max_tokens: max_tokens(v["max_completion_tokens"].as_u64().or(v["max_tokens"].as_u64())),
    })
}

fn openai_usage(i: u64, o: u64) -> Value {
    json!({"prompt_tokens": i, "completion_tokens": o, "total_tokens": i + o})
}

fn finish(o: u64, max: u32) -> &'static str {
    if o >= max as u64 {
        "length"
    } else {
        "stop"
    }
}

async fn openai_chat(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::OpenAi;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, false).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let req = match openai_request(api, &v) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let max = req.max_tokens;
    let r = match run(&st, &client, api, &m, req).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let (id, created, name) = (new_id("chatcmpl-"), unix_now(), m.name.clone());
    if v["stream"].as_bool() != Some(true) {
        return match r.text().await {
            Ok((text, i, o)) => Json(json!({
                "id": id, "object": "chat.completion", "created": created, "model": name,
                "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "logprobs": null, "finish_reason": finish(o, max)}],
                "usage": openai_usage(i, o), "system_fingerprint": null,
            }))
            .into_response(),
            Err(e) => llm_error(api, e),
        };
    }
    let include_usage = v["stream_options"]["include_usage"].as_bool() == Some(true);
    let chunk = {
        let (id, name) = (id.clone(), name.clone());
        move |delta: Value, reason: Option<&str>| json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": name, "choices": [{"index": 0, "delta": delta, "logprobs": null, "finish_reason": reason}]})
    };
    let head = stream::iter([sse(&chunk(json!({"role": "assistant", "content": ""}), None))]);
    let c2 = chunk.clone();
    let failed = Arc::new(Mutex::new(false));
    let f2 = failed.clone();
    let middle = r.chunks.filter_map(move |c| {
        let out = match c {
            Ok(c) if c.text.is_empty() => None,
            Ok(c) => Some(sse(&c2(json!({"content": c.text}), None))),
            Err(e) => {
                *f2.lock().unwrap() = true;
                Some(sse(&json!({"error": {"message": e.to_string(), "type": "api_error"}})))
            }
        };
        std::future::ready(out)
    });
    let (tally, pc) = (r.tally, r.prompt_chars);
    let tail = stream::once(async move {
        let (i, o) = tally.lock().unwrap().usage(pc);
        let mut out = String::new();
        if !*failed.lock().unwrap() {
            out.push_str(&String::from_utf8_lossy(&sse(&chunk(json!({}), Some(finish(o, max))))));
            if include_usage {
                out.push_str(&String::from_utf8_lossy(&sse(&json!({"id": id, "object": "chat.completion.chunk", "created": created, "model": name, "choices": [], "usage": openai_usage(i, o)}))));
            }
        }
        out.push_str("data: [DONE]\n\n");
        Bytes::from(out)
    });
    streaming("text/event-stream", head.chain(middle).chain(tail))
}

/// The legacy completions API: the prompt is answered as one user message.
async fn openai_completion(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::OpenAi;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, false).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let prompt = texts(&v["prompt"]).join("\n");
    let req = ChatRequest {
        system: None,
        messages: if prompt.is_empty() { Vec::new() } else { vec![ChatMsg::new("user", prompt)] },
        temperature: temperature(&v["temperature"]),
        max_tokens: max_tokens(v["max_tokens"].as_u64()),
    };
    let max = req.max_tokens;
    let r = match run(&st, &client, api, &m, req).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let (id, created, name) = (new_id("cmpl-"), unix_now(), m.name.clone());
    if v["stream"].as_bool() != Some(true) {
        return match r.text().await {
            Ok((text, i, o)) => Json(json!({
                "id": id, "object": "text_completion", "created": created, "model": name,
                "choices": [{"text": text, "index": 0, "logprobs": null, "finish_reason": finish(o, max)}],
                "usage": openai_usage(i, o),
            }))
            .into_response(),
            Err(e) => llm_error(api, e),
        };
    }
    let chunk = move |text: &str, reason: Option<&str>| json!({"id": id, "object": "text_completion", "created": created, "model": name, "choices": [{"text": text, "index": 0, "logprobs": null, "finish_reason": reason}]});
    let c2 = chunk.clone();
    let middle = r.chunks.filter_map(move |c| {
        std::future::ready(match c {
            Ok(c) if c.text.is_empty() => None,
            Ok(c) => Some(sse(&c2(&c.text, None))),
            Err(e) => Some(sse(&json!({"error": {"message": e.to_string(), "type": "api_error"}}))),
        })
    });
    let (tally, pc) = (r.tally, r.prompt_chars);
    let tail = stream::once(async move {
        let (_, o) = tally.lock().unwrap().usage(pc);
        Bytes::from(format!("{}data: [DONE]\n\n", String::from_utf8_lossy(&sse(&chunk("", Some(finish(o, max)))))))
    });
    streaming("text/event-stream", middle.chain(tail))
}

async fn openai_embeddings(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::OpenAi;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, true).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let input = texts(&v["input"]);
    let chars: usize = input.iter().map(String::len).sum();
    match embed(&st, &client, api, &m, input).await {
        Ok(vecs) => {
            let base64 = v["encoding_format"].as_str() == Some("base64");
            let data: Vec<Value> = vecs
                .into_iter()
                .enumerate()
                .map(|(i, e)| {
                    let embedding = if base64 {
                        Value::String(base64::engine::general_purpose::STANDARD.encode(e.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()))
                    } else {
                        json!(e)
                    };
                    json!({"object": "embedding", "index": i, "embedding": embedding})
                })
                .collect();
            let t = chars.div_ceil(4);
            Json(json!({"object": "list", "data": data, "model": m.name, "usage": {"prompt_tokens": t, "total_tokens": t}})).into_response()
        }
        Err(r) => r,
    }
}

// ------------------------------------------------------------------ Anthropic

fn anthropic_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn anthropic_request(api: Api, v: &Value) -> Result<ChatRequest, Response> {
    let mut messages = Vec::new();
    for m in v["messages"].as_array().ok_or_else(|| bad(api, "\"messages\" must be a list"))? {
        let role = if m["role"] == "assistant" { "assistant" } else { "user" };
        let mut text = Vec::new();
        let mut images = Vec::new();
        match &m["content"] {
            Value::String(s) => text.push(s.clone()),
            Value::Array(blocks) => {
                for b in blocks {
                    match b["type"].as_str().unwrap_or("") {
                        "text" => text.push(b["text"].as_str().unwrap_or("").to_string()),
                        "image" => {
                            let src = &b["source"];
                            if src["type"] != "base64" {
                                return Err(bad(api, "images must be sent as base64"));
                            }
                            let data = base64::engine::general_purpose::STANDARD.decode(src["data"].as_str().unwrap_or("")).map_err(|_| bad(api, "an image's base64 data is invalid"))?;
                            images.push(ImagePart { mime: src["media_type"].as_str().unwrap_or("image/png").to_string(), data });
                        }
                        "tool_result" => text.push(format!("Tool result:\n{}", anthropic_text(&b["content"]))),
                        // Thinking, tool calls and documents are left out.
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        messages.push(ChatMsg { role: role.into(), content: text.join("\n"), images });
    }
    let system = anthropic_text(&v["system"]);
    Ok(ChatRequest {
        system: (!system.is_empty()).then_some(system),
        messages,
        temperature: temperature(&v["temperature"]),
        max_tokens: max_tokens(v["max_tokens"].as_u64()),
    })
}

fn stop_reason(o: u64, max: u32) -> &'static str {
    if o >= max as u64 {
        "max_tokens"
    } else {
        "end_turn"
    }
}

async fn anthropic_messages(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::Anthropic;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, false).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let req = match anthropic_request(api, &v) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let max = req.max_tokens;
    let estimate_in = prompt_chars(&req).div_ceil(4);
    let r = match run(&st, &client, api, &m, req).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let (id, name) = (new_id("msg_"), m.name.clone());
    if v["stream"].as_bool() != Some(true) {
        return match r.text().await {
            Ok((text, i, o)) => Json(json!({
                "id": id, "type": "message", "role": "assistant", "model": name,
                "content": [{"type": "text", "text": text}],
                "stop_reason": stop_reason(o, max), "stop_sequence": null,
                "usage": {"input_tokens": i, "output_tokens": o},
            }))
            .into_response(),
            Err(e) => llm_error(api, e),
        };
    }
    let head = stream::iter([
        sse_event("message_start", &json!({"type": "message_start", "message": {"id": id, "type": "message", "role": "assistant", "model": name, "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": estimate_in, "output_tokens": 1}}})),
        sse_event("content_block_start", &json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        sse_event("ping", &json!({"type": "ping"})),
    ]);
    let failed = Arc::new(Mutex::new(false));
    let f2 = failed.clone();
    let middle = r.chunks.filter_map(move |c| {
        std::future::ready(match c {
            Ok(c) if c.text.is_empty() => None,
            Ok(c) => Some(sse_event("content_block_delta", &json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": c.text}}))),
            Err(e) => {
                *f2.lock().unwrap() = true;
                Some(sse_event("error", &json!({"type": "error", "error": {"type": "api_error", "message": e.to_string()}})))
            }
        })
    });
    let (tally, pc) = (r.tally, r.prompt_chars);
    let tail = stream::once(async move {
        if *failed.lock().unwrap() {
            return Bytes::new();
        }
        let (_, o) = tally.lock().unwrap().usage(pc);
        let mut out = Vec::new();
        for b in [
            sse_event("content_block_stop", &json!({"type": "content_block_stop", "index": 0})),
            sse_event("message_delta", &json!({"type": "message_delta", "delta": {"stop_reason": stop_reason(o, max), "stop_sequence": null}, "usage": {"output_tokens": o}})),
            sse_event("message_stop", &json!({"type": "message_stop"})),
        ] {
            out.extend_from_slice(&b);
        }
        Bytes::from(out)
    });
    streaming("text/event-stream", head.chain(middle).chain(tail))
}

/// An estimate (~4 characters a token): the models' own tokenizers aren't at hand here.
async fn anthropic_count(_c: Client, b: Bytes) -> Response {
    let api = Api::Anthropic;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match anthropic_request(api, &v) {
        Ok(req) => Json(json!({"input_tokens": prompt_chars(&req).div_ceil(4)})).into_response(),
        Err(r) => r,
    }
}

// ------------------------------------------------------------------ Ollama

async fn ollama_version() -> Json<Value> {
    // Clients check for a recent Ollama; this is the API level answered here.
    Json(json!({"version": "0.9.0"}))
}

fn ollama_details(m: &SharedModel) -> Value {
    json!({
        "parent_model": "",
        "format": if m.builtin { "gguf" } else { "" },
        "family": m.family.clone().unwrap_or_default(),
        "families": m.family.iter().collect::<Vec<_>>(),
        "parameter_size": m.parameters.clone().unwrap_or_default(),
        "quantization_level": m.quantization.clone().unwrap_or_default(),
    })
}

async fn ollama_tags(State(st): State<Shared>, _c: Client) -> Json<Value> {
    let since = rfc3339(since_ms(&st));
    let models: Vec<Value> = shared_models(&st)
        .await
        .iter()
        .map(|m| json!({"name": m.name, "model": m.name, "modified_at": since, "size": m.size.unwrap_or(0), "digest": m.digest, "details": ollama_details(m)}))
        .collect();
    Json(json!({"models": models}))
}

async fn ollama_ps(State(st): State<Shared>, _c: Client) -> Json<Value> {
    let loaded = rad_local::loaded();
    let models: Vec<Value> = shared_models(&st)
        .await
        .iter()
        .filter(|m| m.builtin && loaded.contains(&m.model.as_str()))
        .map(|m| json!({"name": m.name, "model": m.name, "size": m.size.unwrap_or(0), "digest": m.digest, "details": ollama_details(m), "expires_at": now_rfc3339(), "size_vram": 0}))
        .collect();
    Json(json!({"models": models}))
}

async fn ollama_show(State(st): State<Shared>, _c: Client, b: Bytes) -> Response {
    let api = Api::Ollama;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let name = v["model"].as_str().or(v["name"].as_str()).unwrap_or("");
    let Some(m) = find_model(&st, name).await else {
        return error(api, StatusCode::NOT_FOUND, format!("model “{name}” isn't shared by this RAD server"));
    };
    let family = m.family.clone().unwrap_or_default();
    let mut info = serde_json::Map::new();
    info.insert("general.architecture".into(), json!(family));
    if let Some(ctx) = m.ctx {
        info.insert(format!("{family}.context_length"), json!(ctx));
    }
    Json(json!({
        "modelfile": "", "parameters": "", "template": "",
        "details": ollama_details(&m),
        "model_info": info,
        "capabilities": if m.embedding { json!(["embedding"]) } else { json!(["completion"]) },
        "modified_at": rfc3339(since_ms(&st)),
    }))
    .into_response()
}

fn ollama_options(v: &Value) -> (Option<f32>, u32) {
    let o = &v["options"];
    let n = o["num_predict"].as_i64().filter(|n| *n > 0).map(|n| n as u64);
    (temperature(&o["temperature"]), max_tokens(n))
}

/// The closing line of an Ollama answer, with its counts and timings (nanoseconds).
fn ollama_done(name: &str, started: Instant, i: u64, o: u64, last: Value) -> Value {
    let ns = started.elapsed().as_nanos() as u64;
    let mut v = json!({
        "model": name, "created_at": now_rfc3339(), "done": true, "done_reason": "stop",
        "total_duration": ns, "load_duration": 0, "prompt_eval_count": i, "prompt_eval_duration": 0, "eval_count": o, "eval_duration": ns,
    });
    if let (Some(obj), Value::Object(extra)) = (v.as_object_mut(), last) {
        obj.extend(extra);
    }
    v
}

/// Answers an Ollama chat or generate request; `wrap` puts a piece of text in its field.
async fn ollama_answer(st: Shared, client: Client, m: SharedModel, req: ChatRequest, stream_it: bool, wrap: fn(&str) -> Value) -> Response {
    let api = Api::Ollama;
    let r = match run(&st, &client, api, &m, req).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let (name, started) = (m.name.clone(), r.started);
    if !stream_it {
        return match r.text().await {
            Ok((text, i, o)) => Json(ollama_done(&name, started, i, o, wrap(&text))).into_response(),
            Err(e) => llm_error(api, e),
        };
    }
    let n2 = name.clone();
    let middle = r.chunks.filter_map(move |c| {
        std::future::ready(match c {
            Ok(c) if c.text.is_empty() => None,
            Ok(c) => {
                let mut v = json!({"model": n2, "created_at": now_rfc3339(), "done": false});
                if let (Some(obj), Value::Object(extra)) = (v.as_object_mut(), wrap(&c.text)) {
                    obj.extend(extra);
                }
                Some(line(&v))
            }
            Err(e) => Some(line(&json!({"error": e.to_string()}))),
        })
    });
    let (tally, pc) = (r.tally, r.prompt_chars);
    let tail = stream::once(async move {
        let (i, o) = tally.lock().unwrap().usage(pc);
        line(&ollama_done(&name, started, i, o, wrap("")))
    });
    streaming("application/x-ndjson", middle.chain(tail))
}

fn chat_field(t: &str) -> Value {
    json!({"message": {"role": "assistant", "content": t}})
}

fn generate_field(t: &str) -> Value {
    json!({"response": t})
}

fn ollama_images(v: &Value) -> Result<Vec<ImagePart>, Response> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    let data = base64::engine::general_purpose::STANDARD.decode(x.as_str().unwrap_or("")).map_err(|_| bad(Api::Ollama, "an image's base64 data is invalid"))?;
                    Ok(ImagePart { mime: "image/png".into(), data })
                })
                .collect()
        })
        .unwrap_or(Ok(Vec::new()))
}

async fn ollama_chat(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::Ollama;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, false).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let msgs = v["messages"].as_array().cloned().unwrap_or_default();
    // No messages: a request to load the model, which RAD does on first use.
    if msgs.is_empty() {
        return Json(json!({"model": m.name, "created_at": now_rfc3339(), "message": {"role": "assistant", "content": ""}, "done_reason": "load", "done": true})).into_response();
    }
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for x in &msgs {
        let content = x["content"].as_str().unwrap_or("").to_string();
        match x["role"].as_str().unwrap_or("user") {
            "system" => system.push(content),
            "assistant" => messages.push(ChatMsg::new("assistant", content)),
            "tool" => messages.push(ChatMsg::new("user", format!("Tool result:\n{content}"))),
            _ => match ollama_images(&x["images"]) {
                Ok(images) => messages.push(ChatMsg { role: "user".into(), content, images }),
                Err(r) => return r,
            },
        }
    }
    let (temperature, max_tokens) = ollama_options(&v);
    let req = ChatRequest { system: (!system.is_empty()).then(|| system.join("\n\n")), messages, temperature, max_tokens };
    ollama_answer(st, client, m, req, v["stream"].as_bool() != Some(false), chat_field).await
}

async fn ollama_generate(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::Ollama;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, false).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let prompt = v["prompt"].as_str().unwrap_or("").to_string();
    if prompt.is_empty() {
        return Json(json!({"model": m.name, "created_at": now_rfc3339(), "response": "", "done": true, "done_reason": "load"})).into_response();
    }
    let images = match ollama_images(&v["images"]) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let (temperature, max_tokens) = ollama_options(&v);
    let system = v["system"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let req = ChatRequest { system, messages: vec![ChatMsg { role: "user".into(), content: prompt, images }], temperature, max_tokens };
    ollama_answer(st, client, m, req, v["stream"].as_bool() != Some(false), generate_field).await
}

async fn ollama_embed(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::Ollama;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, true).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let input = texts(&v["input"]);
    let count = input.iter().map(String::len).sum::<usize>().div_ceil(4);
    let started = Instant::now();
    match embed(&st, &client, api, &m, input).await {
        Ok(e) => Json(json!({"model": m.name, "embeddings": e, "total_duration": started.elapsed().as_nanos() as u64, "load_duration": 0, "prompt_eval_count": count})).into_response(),
        Err(r) => r,
    }
}

/// The older single-text endpoint.
async fn ollama_embeddings(State(st): State<Shared>, client: Client, b: Bytes) -> Response {
    let api = Api::Ollama;
    let v = match body(api, &b) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let m = match model_for(&st, api, &v, true).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let prompt = v["prompt"].as_str().unwrap_or("").to_string();
    match embed(&st, &client, api, &m, vec![prompt]).await {
        Ok(mut e) => Json(json!({"embedding": e.pop().unwrap_or_default()})).into_response(),
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert!(local_origin("http://localhost:3000"));
        assert!(local_origin("http://127.0.0.1"));
        assert!(local_origin("http://[::1]:8080"));
        assert!(!local_origin("https://example.com"));
        assert!(!local_origin("http://localhost.example.com"));
        assert!(!local_origin("null"));
    }

    #[test]
    fn requests() {
        let v = json!({"model": "m", "max_tokens": 50, "messages": [
            {"role": "system", "content": "Be brief."},
            {"role": "user", "content": [{"type": "text", "text": "Hi"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBO"}}]},
            {"role": "assistant", "content": "Hello"},
        ]});
        let r = openai_request(Api::OpenAi, &v).ok().unwrap();
        assert_eq!(r.system.as_deref(), Some("Be brief."));
        assert_eq!(r.messages.len(), 2);
        assert_eq!(r.messages[0].images.len(), 1);
        assert_eq!(r.max_tokens, 50);

        let v = json!({"model": "m", "max_tokens": 10, "system": [{"type": "text", "text": "S"}], "messages": [{"role": "user", "content": [{"type": "text", "text": "A"}, {"type": "text", "text": "B"}]}]});
        let r = anthropic_request(Api::Anthropic, &v).ok().unwrap();
        assert_eq!(r.system.as_deref(), Some("S"));
        assert_eq!(r.messages[0].content, "A\nB");
    }

    #[test]
    fn usage_estimates() {
        let t = Tally { out_chars: 9, ..Default::default() };
        assert_eq!(t.usage(8), (2, 3));
        let t = Tally { out_chars: 9, tokens_in: Some(5), tokens_out: Some(7), failed: false };
        assert_eq!(t.usage(8), (5, 7));
    }
}
