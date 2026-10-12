//! Keeps a live server-sent-events connection so every device sees changes immediately.

use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::App;

pub fn start(app: &AppHandle) {
    stop(app);
    let handle = app.clone();
    let task = tauri::async_runtime::spawn(async move { run(handle).await });
    *app.state::<App>().events_task.lock().unwrap() = Some(task.inner().abort_handle());
}

pub fn stop(app: &AppHandle) {
    if let Some(t) = app.state::<App>().events_task.lock().unwrap().take() {
        t.abort();
    }
}

async fn run(app: AppHandle) {
    let mut backoff = 1u64;
    let mut first = true;
    loop {
        let st = app.state::<App>();
        if !st.client.has_session() {
            return;
        }
        let rb = match st.client.request(&st.client.stream_http, reqwest::Method::GET, "/v1/events") {
            Ok(r) => r,
            Err(_) => return,
        };
        match st.client.send(rb).await {
            Ok(resp) => {
                let _ = app.emit("rad-connection", true);
                if !first {
                    let _ = app.emit("rad-resync", ());
                }
                first = false;
                backoff = 1;
                let mut stream = resp.bytes_stream();
                let mut buf = Vec::<u8>::new();
                while let Some(chunk) = stream.next().await {
                    let Ok(chunk) = chunk else { break };
                    buf.extend_from_slice(&chunk);
                    while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line);
                        let Some(data) = line.trim_end().strip_prefix("data:") else { continue };
                        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
                        if v["type"] == "locked" {
                            crate::commands::handle_locked(&app);
                            return;
                        }
                        let _ = app.emit("rad-event", v);
                    }
                }
            }
            Err(e) if e.code == "session_required" || e.code == "vault_locked" || e.code == "unauthorized" => {
                crate::commands::handle_locked(&app);
                return;
            }
            Err(_) => {}
        }
        // Disconnected: was it a lock, or the network?
        match st.client.json(reqwest::Method::GET, "/v1/status", None).await {
            Ok(s) if s["session_valid"] == false => {
                crate::commands::handle_locked(&app);
                return;
            }
            Ok(_) => {}
            Err(_) => {
                let _ = app.emit("rad-connection", false);
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}
