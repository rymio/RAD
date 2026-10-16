mod account;
mod browse;
mod commands;
mod mail;
mod session;
mod vfs;

use crate::commands::{AppState, ExecResult};
use crate::session::{SessionStore, WebSession};
use crate::vfs::VirtualComputer;
use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use rymnet_core::NodeIdentity;
use rymnet_storage::{AppConfig, DataLayout};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

const SESSION_COOKIE: &str = "rymweb_session";
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
const INDEX_HTML: &str = include_str!("../web/index.html");

#[derive(Debug, Parser)]
#[command(name = "rymweb", version, about = "Rymnet web terminal -- rymsh, in a browser")]
struct Cli {
    /// Data directory holding this server's own node identity and peers.toml.
    /// Run `rym init` and `rym peer connect <ip>` here first.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<String>,

    /// Address to listen on for the web terminal, e.g. 0.0.0.0:8080.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
}

#[derive(Clone)]
struct SharedState {
    app: Arc<AppState>,
    sessions: SessionStore,
}

#[derive(Deserialize)]
struct ExecRequest {
    line: String,
}

#[derive(Serialize)]
struct ExecResponse {
    lines: Vec<String>,
    prompt: String,
}

fn main() -> Result<()> {
    // The default worker thread stack (2 MiB, or as little as 512 KiB for
    // secondary threads on macOS) isn't enough for the deep async call
    // chains here (exec -> command handler -> peer fetch -> HTML parse);
    // #[tokio::main] doesn't expose thread_stack_size, so the runtime is
    // built by hand.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("rymnet_web=info".parse()?))
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let layout = match cli.data_dir.as_deref() {
        Some(dir) => DataLayout::from_root(rymnet_storage::layout::expand_tilde(dir)?),
        None => DataLayout::default()?,
    };
    let _config = AppConfig::load(layout.config_path())
        .with_context(|| format!("load config {}; run `rym init` first", layout.config_path().display()))?;
    let identity = NodeIdentity::load(layout.identity_path())
        .with_context(|| "load node identity; run `rym init` first")?;

    info!("rymweb starting");
    info!("node id: {}", identity.node_id()?);
    info!("listening on http://{}", cli.listen);

    let state = SharedState {
        app: Arc::new(AppState {
            layout,
            server_identity: identity,
        }),
        sessions: Arc::new(RwLock::new(HashMap::new())),
    };

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(600));
        loop {
            interval.tick().await;
            let mut sessions = cleanup_state.sessions.write().await;
            sessions.retain(|_, session| session.last_seen.elapsed() < SESSION_IDLE_TIMEOUT);
        }
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/exec", post(exec))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cli.listen)
        .await
        .with_context(|| format!("bind {}", cli.listen))?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

fn session_id_from_cookies(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{SESSION_COOKIE}=")) {
            return Some(value.to_string());
        }
    }
    None
}

async fn exec(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<ExecRequest>,
) -> Response {
    let existing_id = session_id_from_cookies(&headers);
    let session_id = existing_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut sessions = state.sessions.write().await;
    if !sessions.contains_key(&session_id) {
        let vfs = match VirtualComputer::open(state.app.layout.root.join("web-sessions-guest").join(&session_id)) {
            Ok(vfs) => vfs,
            Err(err) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {err:#}")).into_response();
            }
        };
        sessions.insert(
            session_id.clone(),
            WebSession {
                account: None,
                vfs,
                pending: None,
                last_inbox: Vec::new(),
                last_seen: Instant::now(),
            },
        );
    }
    let session = sessions.get_mut(&session_id).expect("just inserted");
    session.last_seen = Instant::now();

    let ExecResult { lines, prompt } = commands::exec(&state.app, &session_id, session, &request.line).await;
    drop(sessions);

    let body = Json(ExecResponse { lines, prompt });
    let mut response = body.into_response();
    if existing_id.is_none() {
        let cookie = format!("{SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict");
        response
            .headers_mut()
            .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).expect("valid cookie header"));
    }
    response
}
