use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::Json;
use futures::Stream;
use rad_api::*;
use rad_vault::{blobs, repo, search};
use serde::Deserialize;
use tokio_stream::StreamExt;

use crate::auth::Authed;
use crate::backup;
use crate::error::{ApiErr, ApiResult};
use crate::state::{db_vault, Shared};

pub async fn lock(State(state): State<Shared>, _a: Authed) -> Json<serde_json::Value> {
    state.lock();
    Json(serde_json::json!({"ok": true}))
}

pub async fn info(State(state): State<Shared>, a: Authed) -> ApiResult<Json<VaultInfo>> {
    let backups = backup::list(&state.cfg.backup_dir);
    let path = state.cfg.vault_path.display().to_string();
    let backup_dir = state.cfg.backup_dir.display().to_string();
    let v = a.vault.clone();
    db_vault(a.vault, &state, move |_, c| {
        let s = repo::stats(c)?;
        let settings = repo::settings(c)?;
        let embedding_model = settings.embedding_model.as_ref().map(|m| m.model.clone());
        let embedded_chunks = match settings.embedding_model.as_ref() {
            Some(m) => match search::space(c, &crate::models::model_key(m))? {
                Some(sp) => search::embedded_chunk_count(c, sp)?,
                None => 0,
            },
            None => 0,
        };
        Ok(Json(VaultInfo {
            path,
            file_bytes: v.file_size(),
            blob_bytes: blobs::total_bytes(c)? as u64,
            projects: s.projects,
            documents: s.documents,
            notes: s.notes,
            chunks: s.chunks,
            embedded_chunks,
            chats: s.chats,
            created_at: repo::meta_get(c, "created_at")?.and_then(|v| v.parse().ok()).unwrap_or(0),
            format_version: repo::meta_get(c, "format_version")?.and_then(|v| v.parse().ok()).unwrap_or(1),
            embedding_model,
            backups,
            backup_dir,
        }))
    })
    .await
}

pub async fn change_passphrase(State(state): State<Shared>, a: Authed, Json(req): Json<ChangePassphraseRequest>) -> ApiResult<Json<RecoveryKit>> {
    state.unlock_allowed()?;
    crate::sharing::check_alone(&state, "change the passphrase")?;
    let v = a.vault.clone();
    let cur = req.current.clone();
    let ok = tokio::task::spawn_blocking(move || v.verify_passphrase(&cur)).await.map_err(ApiErr::internal)?;
    state.unlock_result(ok);
    if !ok {
        return Err(ApiErr::new(axum::http::StatusCode::UNAUTHORIZED, "wrong_passphrase", "current passphrase is wrong"));
    }
    let v = a.vault.clone();
    let new = req.new.clone();
    let recovery_key = tokio::task::spawn_blocking(move || v.rekey(&new)).await.map_err(ApiErr::internal)??;
    tracing::warn!("vault passphrase changed by device “{}”", a.device.name);
    Ok(Json(RecoveryKit { recovery_key }))
}

pub async fn sharing(State(state): State<Shared>, a: Authed) -> ApiResult<Json<VaultSharing>> {
    let v = a.vault.clone();
    let shared = tokio::task::spawn_blocking(move || v.is_shared()).await.map_err(ApiErr::internal)?;
    Ok(Json(crate::sharing::info(&state, shared)))
}

pub async fn set_sharing(State(state): State<Shared>, a: Authed, Json(req): Json<SharingRequest>) -> ApiResult<Json<VaultSharing>> {
    crate::sharing::check_toggle(&state, req.shared)?;
    let v = a.vault.clone();
    tokio::task::spawn_blocking(move || v.set_shared(req.shared)).await.map_err(ApiErr::internal)??;
    state.mark_sharing_changed();
    tracing::info!("vault sharing turned {} by device “{}”", if req.shared { "on" } else { "off" }, a.device.name);
    Ok(Json(crate::sharing::info(&state, req.shared)))
}

pub async fn backup(State(state): State<Shared>, a: Authed) -> ApiResult<Json<BackupInfo>> {
    let keep = db_vault(a.vault.clone(), &state, |_, c| Ok(repo::settings(c)?.backup_keep)).await?;
    Ok(Json(backup::run(&state, a.vault, keep).await?))
}

pub async fn verify(State(state): State<Shared>, a: Authed) -> ApiResult<Json<VerifyReport>> {
    let _ = state;
    let v = a.vault.clone();
    let messages = tokio::task::spawn_blocking(move || v.integrity_check()).await.map_err(ApiErr::internal)??;
    let ok = messages.iter().any(|m| m == "ok") && !messages.iter().any(|m| m.starts_with("cipher:"));
    Ok(Json(VerifyReport { ok, messages }))
}

pub async fn compact(State(state): State<Shared>, a: Authed) -> ApiResult<Json<serde_json::Value>> {
    let v = a.vault.clone();
    let removed = db_vault(a.vault.clone(), &state, |_, c| Ok(blobs::collect_garbage(c)?)).await?;
    tokio::task::spawn_blocking(move || v.compact()).await.map_err(ApiErr::internal)??;
    Ok(Json(serde_json::json!({"ok": true, "removed_blobs": removed})))
}

/// Live change feed for all devices (server-sent events).
pub async fn events(State(state): State<Shared>, a: Authed) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let last_seq = db_vault(a.vault.clone(), &state, |_, c| Ok(repo::last_seq(c)?)).await?;
    let rx = state.events.subscribe();
    let hello = futures::stream::once(async move { Event::Hello { last_seq } });
    let live = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|r| r.ok());
    let st = state.clone();
    let session = a.session.clone();
    let device_id = a.device.id.clone();
    // Forward a final `locked` event, and end the stream once the session is gone (lock/revoke).
    let stream = futures::StreamExt::scan(hello.chain(live), false, move |ended, ev| {
        let out = if *ended {
            None
        } else if matches!(ev, Event::Locked) {
            *ended = true;
            Some(ev)
        } else if st.check_session(&session, &device_id) {
            Some(ev)
        } else {
            None
        };
        futures::future::ready(out)
    })
    .map(|ev| Ok(SseEvent::default().json_data(ev).unwrap_or_default()));
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}

#[derive(Deserialize)]
pub struct Since {
    since: Option<i64>,
}

pub async fn changes(State(state): State<Shared>, a: Authed, Query(q): Query<Since>) -> ApiResult<Json<Vec<ChangeEntry>>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::changes_since(c, q.since.unwrap_or(0), 5000)?))).await
}

pub async fn jobs(State(state): State<Shared>, a: Authed) -> ApiResult<Json<JobSummary>> {
    db_vault(a.vault, &state, |_, c| Ok(Json(repo::job_summary(c)?))).await
}
