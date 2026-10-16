//! Model provider adapters: Ollama, any OpenAI-compatible API, Anthropic, and RAD's built-in
//! llama.cpp engine (`rad-local`).

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures::{Stream, StreamExt};
use rad_api::{ModelInfo, ProviderKind};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("cannot reach {0}: {1}")]
    Connect(String, String),
    #[error("{provider} returned HTTP {status}: {message}")]
    Http { provider: String, status: u16, message: String },
    #[error("unexpected response: {0}")]
    Protocol(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("built-in engine: {message}")]
    Local { message: String, retry: bool },
}

impl From<rad_local::LocalError> for LlmError {
    fn from(e: rad_local::LocalError) -> Self {
        LlmError::Local { retry: e.is_transient(), message: e.to_string() }
    }
}

pub type Result<T> = std::result::Result<T, LlmError>;
pub type TextStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

#[derive(Debug, Clone)]
pub struct ProviderConn {
    pub kind: ProviderKind,
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ImagePart {
    pub mime: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
    pub images: Vec<ImagePart>,
}

impl ChatMsg {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        ChatMsg { role: role.into(), content: content.into(), images: Vec::new() }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<ChatMsg>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Default)]
pub struct StreamChunk {
    pub text: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Transcript {
    pub text: String,
    /// (start seconds, end seconds, text)
    pub segments: Vec<(f64, f64, String)>,
}

#[derive(Clone)]
pub struct Llm {
    http: reqwest::Client,
    local: Option<Arc<rad_local::Library>>,
}

impl Default for Llm {
    fn default() -> Self {
        Self::new()
    }
}

impl Llm {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .proxy(rad_tunnel::route::reqwest_proxy())
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(concat!("RAD/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Llm { http, local: None }
    }

    /// Enables the built-in provider, running GGUF models from `library`.
    pub fn with_local(mut self, library: Arc<rad_local::Library>) -> Self {
        self.local = Some(library);
        self
    }

    fn local(&self) -> Result<&Arc<rad_local::Library>> {
        self.local.as_ref().ok_or_else(|| LlmError::Unsupported("the built-in engine is not available here".into()))
    }

    fn req(&self, p: &ProviderConn, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, url);
        match p.kind {
            ProviderKind::Anthropic => {
                if let Some(k) = &p.api_key {
                    r = r.header("x-api-key", k);
                }
                r = r.header("anthropic-version", "2023-06-01");
            }
            _ => {
                if let Some(k) = p.api_key.as_deref().filter(|k| !k.is_empty()) {
                    r = r.bearer_auth(k);
                }
            }
        }
        for (k, v) in &p.headers {
            r = r.header(k, v);
        }
        r
    }

    async fn send(&self, p: &ProviderConn, rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let resp = rb.send().await.map_err(|e| LlmError::Connect(p.base_url.clone(), root_cause(&e)))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.get("error"))
                    .or_else(|| v.get("message"))
                    .map(|m| m.as_str().map(String::from).unwrap_or_else(|| m.to_string()))
            })
            .unwrap_or_else(|| body.chars().take(400).collect());
        Err(LlmError::Http { provider: p.name.clone(), status: status.as_u16(), message })
    }

    // ------------------------------------------------------------ models & health

    pub async fn list_models(&self, p: &ProviderConn) -> Result<Vec<ModelInfo>> {
        let base = base(p);
        let mut models = match p.kind {
            ProviderKind::Ollama => {
                let v: Value = self
                    .send(p, self.req(p, reqwest::Method::GET, format!("{base}/api/tags")))
                    .await?
                    .json()
                    .await
                    .map_err(|e| LlmError::Protocol(e.to_string()))?;
                v["models"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| ModelInfo {
                        id: m["name"].as_str().unwrap_or_default().to_string(),
                        size: m["size"].as_u64(),
                        family: m.pointer("/details/family").and_then(|f| f.as_str()).map(String::from),
                        parameter_size: m.pointer("/details/parameter_size").and_then(|f| f.as_str()).map(String::from),
                    })
                    .collect::<Vec<_>>()
            }
            ProviderKind::Openai | ProviderKind::Anthropic => {
                let url = match p.kind {
                    ProviderKind::Anthropic => format!("{base}/v1/models?limit=1000"),
                    _ => format!("{base}/models"),
                };
                let v: Value = self
                    .send(p, self.req(p, reqwest::Method::GET, url))
                    .await?
                    .json()
                    .await
                    .map_err(|e| LlmError::Protocol(e.to_string()))?;
                v["data"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .map(|id| ModelInfo { id, size: None, family: None, parameter_size: None })
                    .collect()
            }
            ProviderKind::Builtin => {
                // Downloaded models, plus the default embedding model (fetched on demand).
                self.local()?
                    .status()
                    .into_iter()
                    .filter(|m| m.path.is_some() || m.spec.id == rad_local::DEFAULT_EMBEDDING.id)
                    .map(|m| ModelInfo {
                        id: m.spec.id.to_string(),
                        size: Some(m.spec.bytes),
                        family: Some(m.spec.family.to_string()),
                        parameter_size: Some(m.spec.parameters.to_string()),
                    })
                    .collect()
            }
        };
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }

    /// Cheap reachability probe. Returns round-trip latency.
    pub async fn health(&self, p: &ProviderConn) -> Result<Duration> {
        let start = Instant::now();
        let base = base(p);
        let url = match p.kind {
            ProviderKind::Ollama => format!("{base}/api/version"),
            ProviderKind::Openai => format!("{base}/models"),
            ProviderKind::Anthropic => format!("{base}/v1/models?limit=1"),
            ProviderKind::Builtin => {
                self.local()?;
                return Ok(start.elapsed());
            }
        };
        self.send(p, self.req(p, reqwest::Method::GET, url).timeout(Duration::from_secs(10))).await?;
        Ok(start.elapsed())
    }

    // ------------------------------------------------------------ chat

    pub async fn chat_stream(&self, p: &ProviderConn, model: &str, req: &ChatRequest) -> Result<TextStream> {
        match p.kind {
            ProviderKind::Ollama => self.ollama_chat(p, model, req).await,
            ProviderKind::Openai => self.openai_chat(p, model, req).await,
            ProviderKind::Anthropic => self.anthropic_chat(p, model, req).await,
            ProviderKind::Builtin => self.builtin_chat(model, req),
        }
    }

    /// Non-streaming convenience wrapper.
    pub async fn complete(&self, p: &ProviderConn, model: &str, req: &ChatRequest) -> Result<String> {
        let mut s = self.chat_stream(p, model, req).await?;
        let mut out = String::new();
        while let Some(c) = s.next().await {
            out.push_str(&c?.text);
        }
        Ok(out)
    }

    fn builtin_chat(&self, model: &str, req: &ChatRequest) -> Result<TextStream> {
        if req.messages.iter().any(|m| !m.images.is_empty()) {
            return Err(LlmError::Unsupported("built-in models can't read images — choose a vision model from another provider".into()));
        }
        let mut messages = Vec::new();
        if let Some(s) = &req.system {
            messages.push(rad_local::ChatMessage { role: "system".into(), content: s.clone() });
        }
        messages.extend(req.messages.iter().map(|m| rad_local::ChatMessage { role: m.role.clone(), content: m.content.clone() }));
        let opts = rad_local::ChatOptions { max_tokens: req.max_tokens, temperature: req.temperature };
        let rx = self.local()?.chat(model, messages, opts)?;
        let s = tokio_stream::wrappers::ReceiverStream::new(rx).map(|ev| {
            Ok(match ev? {
                rad_local::ChatEvent::Text(text) => StreamChunk { text, ..Default::default() },
                rad_local::ChatEvent::Done { prompt_tokens, completion_tokens } => StreamChunk {
                    text: String::new(),
                    tokens_in: Some(prompt_tokens as i64),
                    tokens_out: Some(completion_tokens as i64),
                },
            })
        });
        Ok(Box::pin(s))
    }

    async fn ollama_chat(&self, p: &ProviderConn, model: &str, req: &ChatRequest) -> Result<TextStream> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut messages = Vec::new();
        if let Some(s) = &req.system {
            messages.push(json!({"role": "system", "content": s}));
        }
        for m in &req.messages {
            let mut o = json!({"role": m.role, "content": m.content});
            if !m.images.is_empty() {
                o["images"] = m.images.iter().map(|i| Value::String(b64.encode(&i.data))).collect();
            }
            messages.push(o);
        }
        let mut options = json!({"num_predict": req.max_tokens});
        if let Some(t) = req.temperature {
            options["temperature"] = json!(t);
        }
        let body = json!({"model": model, "messages": messages, "stream": true, "options": options});
        let resp = self
            .send(p, self.req(p, reqwest::Method::POST, format!("{}/api/chat", base(p))).json(&body))
            .await?;
        let s = lines(resp).filter_map(|line| async move {
            let line = match line {
                Ok(l) => l,
                Err(e) => return Some(Err(e)),
            };
            if line.trim().is_empty() {
                return None;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => return Some(Err(LlmError::Protocol(format!("{e}: {line}")))),
            };
            if let Some(err) = v.get("error") {
                return Some(Err(LlmError::Protocol(err.as_str().unwrap_or("error").to_string())));
            }
            let text = v.pointer("/message/content").and_then(|c| c.as_str()).unwrap_or_default().to_string();
            let done = v["done"].as_bool().unwrap_or(false);
            Some(Ok(StreamChunk {
                text,
                tokens_in: if done { v["prompt_eval_count"].as_i64() } else { None },
                tokens_out: if done { v["eval_count"].as_i64() } else { None },
            }))
        });
        Ok(Box::pin(s))
    }

    async fn openai_chat(&self, p: &ProviderConn, model: &str, req: &ChatRequest) -> Result<TextStream> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut messages = Vec::new();
        if let Some(s) = &req.system {
            messages.push(json!({"role": "system", "content": s}));
        }
        for m in &req.messages {
            if m.images.is_empty() {
                messages.push(json!({"role": m.role, "content": m.content}));
            } else {
                let mut parts = vec![json!({"type": "text", "text": m.content})];
                for i in &m.images {
                    parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", i.mime, b64.encode(&i.data))}}));
                }
                messages.push(json!({"role": m.role, "content": parts}));
            }
        }
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": {"include_usage": true},
            "max_tokens": req.max_tokens,
        });
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        let resp = self
            .send(p, self.req(p, reqwest::Method::POST, format!("{}/chat/completions", base(p))).json(&body))
            .await?;
        let s = sse(resp).filter_map(|ev| async move {
            let data = match ev {
                Ok((_, d)) => d,
                Err(e) => return Some(Err(e)),
            };
            if data.trim() == "[DONE]" {
                return None;
            }
            let v: Value = serde_json::from_str(&data).ok()?;
            if let Some(err) = v.get("error") {
                return Some(Err(LlmError::Protocol(err.to_string())));
            }
            let text = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()).unwrap_or_default().to_string();
            let usage = v.get("usage").filter(|u| !u.is_null());
            Some(Ok(StreamChunk {
                text,
                tokens_in: usage.and_then(|u| u["prompt_tokens"].as_i64()),
                tokens_out: usage.and_then(|u| u["completion_tokens"].as_i64()),
            }))
        });
        Ok(Box::pin(s))
    }

    async fn anthropic_chat(&self, p: &ProviderConn, model: &str, req: &ChatRequest) -> Result<TextStream> {
        let b64 = base64::engine::general_purpose::STANDARD;
        // Anthropic requires alternating user/assistant turns starting with user.
        let mut messages: Vec<Value> = Vec::new();
        for m in req.messages.iter().filter(|m| m.role != "system") {
            let mut parts: Vec<Value> = m
                .images
                .iter()
                .map(|i| json!({"type": "image", "source": {"type": "base64", "media_type": i.mime, "data": b64.encode(&i.data)}}))
                .collect();
            if !m.content.is_empty() || parts.is_empty() {
                parts.push(json!({"type": "text", "text": if m.content.is_empty() { " " } else { &m.content }}));
            }
            match messages.last_mut() {
                Some(last) if last["role"] == m.role => {
                    last["content"].as_array_mut().unwrap().extend(parts);
                }
                _ => messages.push(json!({"role": m.role, "content": parts})),
            }
        }
        if messages.first().is_some_and(|m| m["role"] != "user") {
            messages.insert(0, json!({"role": "user", "content": [{"type": "text", "text": "(conversation continues)"}]}));
        }
        let mut body = json!({"model": model, "messages": messages, "max_tokens": req.max_tokens, "stream": true});
        if let Some(s) = &req.system {
            body["system"] = json!(s);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = json!(t);
        }
        let resp = self
            .send(p, self.req(p, reqwest::Method::POST, format!("{}/v1/messages", base(p))).json(&body))
            .await?;
        let s = sse(resp).filter_map(|ev| async move {
            let (_event, data) = match ev {
                Ok(x) => x,
                Err(e) => return Some(Err(e)),
            };
            let v: Value = serde_json::from_str(&data).ok()?;
            match v["type"].as_str()? {
                "content_block_delta" => Some(Ok(StreamChunk {
                    text: v.pointer("/delta/text").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    ..Default::default()
                })),
                "message_start" => Some(Ok(StreamChunk {
                    tokens_in: v.pointer("/message/usage/input_tokens").and_then(|t| t.as_i64()),
                    ..Default::default()
                })),
                "message_delta" => Some(Ok(StreamChunk {
                    tokens_out: v.pointer("/usage/output_tokens").and_then(|t| t.as_i64()),
                    ..Default::default()
                })),
                "error" => Some(Err(LlmError::Protocol(
                    v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("error").to_string(),
                ))),
                _ => None,
            }
        });
        Ok(Box::pin(s))
    }

    // ------------------------------------------------------------ embeddings

    pub async fn embed(&self, p: &ProviderConn, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let parse = |arr: Option<&Vec<Value>>| -> Vec<Vec<f32>> {
            arr.cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.as_array().cloned().unwrap_or_default().iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                .collect()
        };
        let out = match p.kind {
            ProviderKind::Ollama => {
                let v: Value = self
                    .send(
                        p,
                        self.req(p, reqwest::Method::POST, format!("{}/api/embed", base(p)))
                            .json(&json!({"model": model, "input": texts, "truncate": true})),
                    )
                    .await?
                    .json()
                    .await
                    .map_err(|e| LlmError::Protocol(e.to_string()))?;
                parse(v["embeddings"].as_array())
            }
            ProviderKind::Openai => {
                let v: Value = self
                    .send(
                        p,
                        self.req(p, reqwest::Method::POST, format!("{}/embeddings", base(p)))
                            .json(&json!({"model": model, "input": texts})),
                    )
                    .await?
                    .json()
                    .await
                    .map_err(|e| LlmError::Protocol(e.to_string()))?;
                let mut data = v["data"].as_array().cloned().unwrap_or_default();
                data.sort_by_key(|d| d["index"].as_i64().unwrap_or(0));
                data.into_iter()
                    .map(|d| d["embedding"].as_array().cloned().unwrap_or_default().iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                    .collect()
            }
            ProviderKind::Anthropic => {
                return Err(LlmError::Unsupported(
                    "Anthropic has no embeddings API — choose an Ollama or OpenAI-compatible embedding model".into(),
                ))
            }
            ProviderKind::Builtin => self.local()?.embed(model, texts.to_vec()).await?,
        };
        if out.len() != texts.len() || out.iter().any(|v| v.is_empty()) {
            return Err(LlmError::Protocol(format!("expected {} embeddings, got {}", texts.len(), out.len())));
        }
        Ok(out)
    }

    // ------------------------------------------------------------ audio

    /// OpenAI-compatible speech-to-text (`/audio/transcriptions`).
    pub async fn transcribe(&self, p: &ProviderConn, model: &str, audio: Vec<u8>, filename: &str, mime: &str) -> Result<Transcript> {
        if p.kind != ProviderKind::Openai {
            return Err(LlmError::Unsupported(
                "transcription needs an OpenAI-compatible provider (OpenAI, Groq, a local Whisper server…)".into(),
            ));
        }
        let part = reqwest::multipart::Part::bytes(audio)
            .file_name(filename.to_string())
            .mime_str(mime)
            .map_err(|e| LlmError::Protocol(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("model", model.to_string())
            .text("response_format", "verbose_json")
            .part("file", part);
        let resp = self
            .send(
                p,
                self.req(p, reqwest::Method::POST, format!("{}/audio/transcriptions", base(p)))
                    .timeout(Duration::from_secs(900))
                    .multipart(form),
            )
            .await?;
        let body = resp.text().await.map_err(|e| LlmError::Protocol(e.to_string()))?;
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            // Some servers ignore response_format and return plain text.
            return Ok(Transcript { text: body, segments: Vec::new() });
        };
        let segments = v["segments"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                (
                    s["start"].as_f64().unwrap_or(0.0),
                    s["end"].as_f64().unwrap_or(0.0),
                    s["text"].as_str().unwrap_or_default().trim().to_string(),
                )
            })
            .collect();
        Ok(Transcript { text: v["text"].as_str().unwrap_or_default().to_string(), segments })
    }

    // ------------------------------------------------------------ ollama model management

    /// Streams `ollama pull` progress lines as JSON values.
    pub async fn ollama_pull(&self, p: &ProviderConn, model: &str) -> Result<Pin<Box<dyn Stream<Item = Result<Value>> + Send>>> {
        if p.kind != ProviderKind::Ollama {
            return Err(LlmError::Unsupported("only Ollama servers can pull models".into()));
        }
        let resp = self
            .send(
                p,
                self.req(p, reqwest::Method::POST, format!("{}/api/pull", base(p))).json(&json!({"model": model, "stream": true})),
            )
            .await?;
        Ok(Box::pin(lines(resp).filter_map(|l| async move {
            match l {
                Ok(l) if l.trim().is_empty() => None,
                Ok(l) => Some(serde_json::from_str::<Value>(&l).map_err(|e| LlmError::Protocol(e.to_string()))),
                Err(e) => Some(Err(e)),
            }
        })))
    }

    pub async fn ollama_delete(&self, p: &ProviderConn, model: &str) -> Result<()> {
        self.send(p, self.req(p, reqwest::Method::DELETE, format!("{}/api/delete", base(p))).json(&json!({"model": model})))
            .await?;
        Ok(())
    }
}

fn base(p: &ProviderConn) -> String {
    let b = p.base_url.trim_end_matches('/');
    match p.kind {
        ProviderKind::Anthropic => b.trim_end_matches("/v1").to_string(),
        _ => b.to_string(),
    }
}

fn root_cause(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        s = inner.to_string();
        src = inner.source();
    }
    s
}

/// Splits a streaming HTTP body into lines.
fn lines(resp: reqwest::Response) -> impl Stream<Item = Result<String>> + Send {
    let bytes = resp.bytes_stream();
    futures::stream::unfold((bytes, Vec::<u8>::new(), false), |(mut bytes, mut buf, mut done)| async move {
        loop {
            if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let s = String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_string();
                return Some((Ok(s), (bytes, buf, done)));
            }
            if done {
                if buf.is_empty() {
                    return None;
                }
                let s = String::from_utf8_lossy(&buf).to_string();
                buf.clear();
                return Some((Ok(s), (bytes, buf, done)));
            }
            match bytes.next().await {
                Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                Some(Err(e)) => {
                    done = true;
                    return Some((Err(LlmError::Connect("stream".into(), root_cause(&e))), (bytes, buf, done)));
                }
                None => done = true,
            }
        }
    })
}

/// Parses a server-sent-events body into (event, data) pairs.
fn sse(resp: reqwest::Response) -> impl Stream<Item = Result<(String, String)>> + Send {
    let ls = lines(resp);
    futures::stream::unfold((Box::pin(ls), String::new(), String::new()), |(mut ls, mut event, mut data)| async move {
        loop {
            match ls.next().await {
                Some(Ok(line)) => {
                    if line.is_empty() {
                        if !data.is_empty() {
                            let out = (std::mem::take(&mut event), std::mem::take(&mut data));
                            return Some((Ok(out), (ls, event, data)));
                        }
                        continue;
                    }
                    if let Some(rest) = line.strip_prefix("data:") {
                        if !data.is_empty() {
                            data.push('\n');
                        }
                        data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                    } else if let Some(rest) = line.strip_prefix("event:") {
                        event = rest.trim().to_string();
                    }
                }
                Some(Err(e)) => return Some((Err(e), (ls, event, data))),
                None => {
                    if !data.is_empty() {
                        let out = (std::mem::take(&mut event), std::mem::take(&mut data));
                        return Some((Ok(out), (ls, event, data)));
                    }
                    return None;
                }
            }
        }
    })
}

/// Some embedding models are trained with task prefixes; add them automatically.
pub fn embed_prefix(model: &str, is_query: bool) -> &'static str {
    let m = model.to_lowercase();
    if m.contains("nomic-embed") {
        if is_query { "search_query: " } else { "search_document: " }
    } else if m.contains("e5") {
        if is_query { "query: " } else { "passage: " }
    } else if m.contains("mxbai-embed") && is_query {
        "Represent this sentence for searching relevant passages: "
    } else {
        ""
    }
}
