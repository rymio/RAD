//! HTTP client for the RAD server. The device token and session never reach the webview.

use std::sync::RwLock;
use std::time::Duration;

use rad_api::{ApiError, HEADER_SESSION};
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize)]
pub struct CmdError {
    pub code: String,
    pub message: String,
}

impl CmdError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        CmdError { code: code.into(), message: message.into() }
    }
    pub fn offline(e: impl std::fmt::Display) -> Self {
        CmdError::new("offline", format!("Can't reach the RAD server: {e}"))
    }
}

impl<E: std::fmt::Display> From<E> for CmdError {
    fn from(e: E) -> Self {
        CmdError::new("internal", e.to_string())
    }
}

pub type CmdResult<T> = Result<T, CmdError>;

#[derive(Default)]
struct Conn {
    base: Option<String>,
    device_token: Option<Zeroizing<String>>,
    session: Option<Zeroizing<String>>,
}

pub struct Client {
    pub http: reqwest::Client,
    pub stream_http: reqwest::Client,
    conn: RwLock<Conn>,
}

pub fn normalize_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    if u.starts_with("http://") || u.starts_with("https://") {
        u.to_string()
    } else {
        format!("http://{u}")
    }
}

impl Client {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .proxy(rad_tunnel::route::reqwest_proxy())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("RAD-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        let stream_http = reqwest::Client::builder()
            .proxy(rad_tunnel::route::reqwest_proxy())
            .connect_timeout(Duration::from_secs(8))
            .user_agent(concat!("RAD-desktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Client { http, stream_http, conn: RwLock::new(Conn::default()) }
    }

    pub fn set_server(&self, base: Option<String>, token: Option<String>) {
        let mut c = self.conn.write().unwrap();
        c.base = base;
        c.device_token = token.map(Zeroizing::new);
        c.session = None;
    }

    pub fn set_token(&self, token: Option<String>) {
        self.conn.write().unwrap().device_token = token.map(Zeroizing::new);
    }

    pub fn set_session(&self, s: Option<String>) {
        self.conn.write().unwrap().session = s.map(Zeroizing::new);
    }

    pub fn has_session(&self) -> bool {
        self.conn.read().unwrap().session.is_some()
    }

    pub fn has_token(&self) -> bool {
        self.conn.read().unwrap().device_token.is_some()
    }

    pub fn base(&self) -> CmdResult<String> {
        self.conn.read().unwrap().base.clone().ok_or_else(|| CmdError::new("no_server", "No RAD server configured"))
    }

    pub fn request(&self, client: &reqwest::Client, method: Method, path: &str) -> CmdResult<reqwest::RequestBuilder> {
        let base = self.base()?;
        let c = self.conn.read().unwrap();
        let mut r = client.request(method, format!("{base}{path}"));
        if let Some(t) = &c.device_token {
            r = r.bearer_auth(t.as_str());
        }
        if let Some(s) = &c.session {
            r = r.header(HEADER_SESSION, s.as_str());
        }
        Ok(r)
    }

    /// Sends and converts non-2xx answers into typed errors.
    pub async fn send(&self, rb: reqwest::RequestBuilder) -> CmdResult<reqwest::Response> {
        let resp = rb.send().await.map_err(CmdError::offline)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(match serde_json::from_str::<ApiError>(&body) {
            Ok(e) => CmdError::new(&e.code, e.error),
            Err(_) => CmdError::new("http", format!("HTTP {status}: {}", body.chars().take(300).collect::<String>())),
        })
    }

    pub async fn json(&self, method: Method, path: &str, body: Option<Value>) -> CmdResult<Value> {
        let mut rb = self.request(&self.http, method, path)?;
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let resp = self.send(rb).await?;
        if resp.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        let bytes = resp.bytes().await.map_err(CmdError::offline)?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| CmdError::new("protocol", e.to_string()))
    }
}
