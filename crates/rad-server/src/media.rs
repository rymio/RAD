//! Audio/video: ffmpeg reads the blob from a loopback-only HTTP endpoint (with Range support),
//! so decrypted media never touches the disk. Transcripts come from an OpenAI-compatible
//! speech-to-text endpoint.

use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use rad_api::{Anchor, ModelRef};
use rad_ingest::Section;
use rad_vault::Vault;
use tokio::io::AsyncReadExt;

use crate::devices::random_hex;
use crate::error::{ApiErr, ApiResult};
use crate::state::Shared;

/// Each transcription request covers at most this many seconds (32 kbps mono ≈ 21 MB, under the
/// 25 MB limit most speech-to-text APIs enforce).
const SEGMENT_SECS: u64 = 80 * 60;

pub async fn bind_loopback() -> anyhow::Result<(tokio::net::TcpListener, SocketAddr)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    Ok((listener, addr))
}

pub fn serve_loopback(listener: tokio::net::TcpListener, state: Shared) {
    let state_for_stop = state.clone();
    let app = Router::new().route("/blob/{token}", get(serve)).with_state(state);
    let st = state_for_stop.clone();
    tokio::spawn(async move {
        let stop = async move {
            while !st.is_stopped() {
                st.stop_notify.notified().await;
            }
        };
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(stop).await {
            tracing::error!("media loopback server stopped: {e}");
        }
    });
}

async fn serve(State(state): State<Shared>, Path(token): Path<String>, headers: HeaderMap) -> ApiResult<Response> {
    let entry = state.media_tokens.lock().unwrap().get(&token).cloned();
    let (vault, blob_id, mime) = entry.ok_or_else(ApiErr::not_found)?;
    crate::content::blob_response(vault, blob_id, &mime, &headers, None).await
}

struct Token<'a> {
    state: &'a Shared,
    token: String,
}

impl Drop for Token<'_> {
    fn drop(&mut self) {
        self.state.media_tokens.lock().unwrap().remove(&self.token);
    }
}

fn register<'a>(state: &'a Shared, vault: Arc<Vault>, blob_id: i64, mime: &str) -> ApiResult<(Token<'a>, String)> {
    let addr = state.cfg.media_addr.ok_or_else(|| ApiErr::internal("media endpoint not running"))?;
    let token = random_hex(24);
    state.media_tokens.lock().unwrap().insert(token.clone(), (vault, blob_id, mime.to_string()));
    let url = format!("http://{addr}/blob/{token}");
    Ok((Token { state, token }, url))
}

async fn ffmpeg(state: &Shared, args: &[String], timeout: Duration) -> ApiResult<Vec<u8>> {
    let mut child = tokio::process::Command::new(&state.cfg.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ApiErr::internal(format!("ffmpeg not available ({}): {e}", state.cfg.ffmpeg)))?;
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let run = async {
        let mut buf = Vec::new();
        let mut ebuf = Vec::new();
        let (r1, r2) = tokio::join!(out.read_to_end(&mut buf), err.read_to_end(&mut ebuf));
        r1.map_err(ApiErr::internal)?;
        let _ = r2;
        let status = child.wait().await.map_err(ApiErr::internal)?;
        if !status.success() && buf.is_empty() {
            return Err(ApiErr::internal(format!("ffmpeg failed: {}", String::from_utf8_lossy(&ebuf).trim())));
        }
        Ok(buf)
    };
    tokio::time::timeout(timeout, run).await.map_err(|_| ApiErr::internal("ffmpeg timed out"))?
}

pub async fn check_ffmpeg(state: &Shared) -> bool {
    tokio::process::Command::new(&state.cfg.ffmpeg)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// JPEG frame from ~1 s into the video.
pub async fn video_thumbnail(state: &Shared, vault: Arc<Vault>, blob_id: i64, mime: &str) -> Option<Vec<u8>> {
    let (_t, url) = register(state, vault, blob_id, mime).ok()?;
    let args: Vec<String> = ["-ss", "1", "-i", &url, "-frames:v", "1", "-vf", "scale=480:-2", "-f", "image2", "-c:v", "mjpeg", "pipe:1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    ffmpeg(state, &args, Duration::from_secs(60)).await.ok().filter(|b| !b.is_empty())
}

/// Transcribes audio/video into timestamped transcript sections.
pub async fn transcribe(
    state: &Shared,
    vault: Arc<Vault>,
    blob_id: i64,
    mime: &str,
    model: &ModelRef,
    conn: &rad_llm::ProviderConn,
    section_chars: usize,
) -> ApiResult<(Vec<Section>, Option<f64>)> {
    let (_t, url) = register(state, vault, blob_id, mime)?;
    let mut sections = Vec::new();
    let mut duration: Option<f64> = None;
    for seg in 0..64u64 {
        let start = seg * SEGMENT_SECS;
        let args: Vec<String> = [
            "-ss", &start.to_string(), "-t", &SEGMENT_SECS.to_string(), "-i", &url,
            "-vn", "-ac", "1", "-ar", "16000", "-c:a", "libmp3lame", "-b:a", "32k", "-f", "mp3", "pipe:1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let audio = ffmpeg(state, &args, Duration::from_secs(1800)).await?;
        if audio.len() < 2048 {
            break; // past the end of the media
        }
        let t = state
            .llm
            .transcribe(conn, &model.model, audio, &format!("segment-{seg}.mp3"), "audio/mpeg")
            .await
            .map_err(ApiErr::from)?;
        if t.segments.is_empty() {
            if !t.text.trim().is_empty() {
                sections.push(Section {
                    text: t.text.trim().to_string(),
                    anchor: Anchor { time: Some(start as f64), ..Default::default() },
                    kind: "transcript",
                    markdown: false,
                });
            }
        } else {
            let mut cur = String::new();
            let mut cur_start = None;
            for (s, e, text) in t.segments {
                duration = Some(start as f64 + e);
                if cur_start.is_none() {
                    cur_start = Some(start as f64 + s);
                }
                cur.push_str(&text);
                cur.push(' ');
                if cur.len() >= section_chars {
                    sections.push(Section {
                        text: std::mem::take(&mut cur).trim().to_string(),
                        anchor: Anchor { time: cur_start.take(), ..Default::default() },
                        kind: "transcript",
                        markdown: false,
                    });
                }
            }
            if !cur.trim().is_empty() {
                sections.push(Section {
                    text: cur.trim().to_string(),
                    anchor: Anchor { time: cur_start, ..Default::default() },
                    kind: "transcript",
                    markdown: false,
                });
            }
        }
    }
    Ok((sections, duration))
}
