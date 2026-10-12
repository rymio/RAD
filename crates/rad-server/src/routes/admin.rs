use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::Json;
use futures::{Stream, StreamExt};
use rad_api::*;
use rad_vault::repo;
use serde::Deserialize;

use crate::auth::Authed;
use crate::error::{ApiErr, ApiResult};
use crate::ingest::enqueue;
use crate::models;
use crate::state::{db_vault, Shared};

// ------------------------------------------------------------------ providers

pub async fn list_providers(State(state): State<Shared>, a: Authed) -> ApiResult<Json<Vec<Provider>>> {
    db_vault(a.vault, &state, |_, c| Ok(Json(repo::providers(c)?.iter().map(|p| p.public()).collect()))).await
}

pub async fn create_provider(State(state): State<Shared>, a: Authed, Json(input): Json<ProviderInput>) -> ApiResult<Json<Provider>> {
    if input.kind == Some(ProviderKind::Builtin) {
        return Err(ApiErr::bad_request("the built-in engine is always present and can't be added again"));
    }
    db_vault(a.vault, &state, move |st, c| {
        let p = repo::create_provider(c, &input)?;
        st.changed(c, "provider", p.id, "create");
        Ok(Json(p.public()))
    })
    .await
}

pub async fn update_provider(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(input): Json<ProviderInput>) -> ApiResult<Json<Provider>> {
    db_vault(a.vault, &state, move |st, c| {
        let was_builtin = repo::provider(c, id)?.kind == ProviderKind::Builtin;
        if input.kind.is_some_and(|k| (k == ProviderKind::Builtin) != was_builtin) {
            return Err(ApiErr::bad_request("a provider can't be turned into or out of the built-in engine"));
        }
        let p = repo::update_provider(c, id, &input)?;
        st.changed(c, "provider", id, "update");
        Ok(Json(p.public()))
    })
    .await
}

pub async fn delete_provider(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        if repo::provider(c, id)?.kind == ProviderKind::Builtin {
            return Err(ApiErr::bad_request("the built-in engine can't be removed — switch it off instead"));
        }
        repo::delete_provider(c, id)?;
        // Drop settings that pointed at this provider.
        let mut s = repo::settings(c)?;
        for slot in [&mut s.chat_model, &mut s.rewrite_model, &mut s.vision_model, &mut s.embedding_model, &mut s.transcription_model] {
            if slot.as_ref().is_some_and(|m| m.provider_id == id) {
                *slot = None;
            }
        }
        repo::save_settings(c, &s)?;
        st.changed(c, "provider", id, "delete");
        st.changed(c, "settings", 0, "update");
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn provider_conn(state: &Shared, a: &Authed, id: Id) -> ApiResult<rad_llm::ProviderConn> {
    db_vault(a.vault.clone(), state, move |_, c| Ok(models::conn(&repo::provider(c, id)?))).await
}

pub async fn provider_models(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Vec<ModelInfo>>> {
    let conn = provider_conn(&state, &a, id).await?;
    Ok(Json(state.llm.list_models(&conn).await?))
}

#[derive(serde::Serialize)]
pub struct TestResult {
    ok: bool,
    latency_ms: Option<u64>,
    models: usize,
    error: Option<String>,
}

pub async fn test_provider(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<TestResult>> {
    let conn = provider_conn(&state, &a, id).await?;
    let start = std::time::Instant::now();
    Ok(Json(match state.llm.list_models(&conn).await {
        Ok(m) => TestResult { ok: true, latency_ms: Some(start.elapsed().as_millis() as u64), models: m.len(), error: None },
        Err(e) => TestResult { ok: false, latency_ms: None, models: 0, error: Some(e.to_string()) },
    }))
}

pub async fn pull_model(
    State(state): State<Shared>,
    a: Authed,
    Path(id): Path<Id>,
    Json(req): Json<PullRequest>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    let conn = provider_conn(&state, &a, id).await?;
    let s = state.llm.ollama_pull(&conn, req.model.trim()).await?;
    let stream = s.map(|r| {
        let v = match r {
            Ok(v) => v,
            Err(e) => serde_json::json!({"error": e.to_string()}),
        };
        Ok(SseEvent::default().json_data(v).unwrap_or_default())
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

pub async fn delete_model(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(req): Json<PullRequest>) -> ApiResult<StatusCode> {
    let conn = provider_conn(&state, &a, id).await?;
    state.llm.ollama_delete(&conn, &req.model).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ built-in models

pub async fn local_models(State(state): State<Shared>, a: Authed) -> ApiResult<Json<LocalModels>> {
    let provider_id = db_vault(a.vault, &state, |_, c| {
        Ok(repo::providers(c)?.into_iter().find(|p| p.kind == ProviderKind::Builtin).map(|p| p.id))
    })
    .await?;
    let models = state
        .library
        .status()
        .into_iter()
        .map(|m| LocalModel {
            id: m.spec.id.into(),
            name: m.spec.name.into(),
            description: m.spec.description.into(),
            kind: m.spec.kind.as_str().into(),
            file: m.spec.file.into(),
            bytes: m.spec.bytes,
            parameters: m.spec.parameters.into(),
            license: m.spec.license.into(),
            recommended: m.spec.recommended,
            installed: m.path.is_some(),
            bundled: m.bundled,
            downloaded: m.downloading.map(|(done, _)| done),
            error: m.error,
            memory_needed: m.memory_needed,
            fit: m.fit.as_str().into(),
        })
        .collect();
    Ok(Json(LocalModels {
        provider_id,
        dir: state.library.dir().display().to_string(),
        total_memory: rad_local::total_memory(),
        models,
    }))
}

fn local_spec(id: &str) -> ApiResult<&'static rad_local::ModelSpec> {
    rad_local::spec(id).ok_or_else(ApiErr::not_found)
}

pub async fn download_local_model(State(state): State<Shared>, _a: Authed, Path(id): Path<String>) -> ApiResult<StatusCode> {
    state.library.start_download(local_spec(&id)?).map_err(|e| ApiErr::bad_request(e.to_string()))?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn delete_local_model(State(state): State<Shared>, a: Authed, Path(id): Path<String>) -> ApiResult<StatusCode> {
    let spec = local_spec(&id)?;
    state.library.delete(spec).map_err(|e| ApiErr::bad_request(e.to_string()))?;
    // Settings that used it fall back to "not set" (chat) — the embedding default stays.
    db_vault(a.vault, &state, move |st, c| {
        let mut s = repo::settings(c)?;
        let builtin: Vec<Id> = repo::providers(c)?.into_iter().filter(|p| p.kind == ProviderKind::Builtin).map(|p| p.id).collect();
        let mut changed = false;
        for slot in [&mut s.chat_model, &mut s.rewrite_model] {
            if slot.as_ref().is_some_and(|m| builtin.contains(&m.provider_id) && m.model == spec.id) {
                *slot = None;
                changed = true;
            }
        }
        if changed {
            repo::save_settings(c, &s)?;
            st.changed(c, "settings", 0, "update");
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn provider_health(State(state): State<Shared>, _a: Authed) -> Json<Vec<ProviderHealth>> {
    Json(state.health.read().unwrap().clone())
}

// ------------------------------------------------------------------ settings

pub async fn get_settings(State(state): State<Shared>, a: Authed) -> ApiResult<Json<Settings>> {
    db_vault(a.vault, &state, |_, c| Ok(Json(repo::settings(c)?))).await
}

pub async fn put_settings(State(state): State<Shared>, a: Authed, Json(mut s): Json<Settings>) -> ApiResult<Json<Settings>> {
    s.top_k = s.top_k.clamp(1, 30);
    s.chunk_chars = s.chunk_chars.clamp(400, 8000);
    s.chunk_overlap = s.chunk_overlap.min(s.chunk_chars / 3);
    s.context_chars = s.context_chars.clamp(2000, 400_000);
    s.max_output_tokens = s.max_output_tokens.clamp(128, 64_000);
    s.temperature = s.temperature.clamp(0.0, 2.0);
    s.backup_keep = s.backup_keep.clamp(1, 100);
    db_vault(a.vault, &state, move |st, c| {
        let before = repo::settings(c)?;
        repo::save_settings(c, &s)?;
        if before.embedding_model != s.embedding_model && s.embedding_model.is_some() {
            enqueue(st, c, "reindex", None)?;
        }
        if before.vision_model.is_none() && s.vision_model.is_some() {
            // Make earlier images searchable now that captions are possible.
            let mut q = c.prepare("SELECT id FROM documents WHERE mime LIKE 'image/%' AND notice LIKE '%vision model%'")?;
            let ids: Vec<Id> = q.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for id in ids {
                enqueue(st, c, "ingest", Some(id))?;
            }
        }
        if before.transcription_model.is_none() && s.transcription_model.is_some() {
            let mut q = c.prepare("SELECT id FROM documents WHERE notice LIKE '%transcription model%'")?;
            let ids: Vec<Id> = q.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for id in ids {
                enqueue(st, c, "ingest", Some(id))?;
            }
        }
        st.changed(c, "settings", 0, "update");
        Ok(Json(s))
    })
    .await
}

// ------------------------------------------------------------------ memories

pub async fn list_memories(State(state): State<Shared>, a: Authed) -> ApiResult<Json<Vec<Memory>>> {
    db_vault(a.vault, &state, |_, c| Ok(Json(repo::memories(c)?))).await
}

pub async fn create_memory(State(state): State<Shared>, a: Authed, Json(input): Json<MemoryInput>) -> ApiResult<Json<Memory>> {
    let text = input.text.unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiErr::bad_request("memory text is empty"));
    }
    db_vault(a.vault, &state, move |st, c| {
        let m = repo::create_memory(c, &text)?;
        st.changed(c, "memory", m.id, "create");
        Ok(Json(m))
    })
    .await
}

pub async fn update_memory(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(input): Json<MemoryInput>) -> ApiResult<Json<Memory>> {
    db_vault(a.vault, &state, move |st, c| {
        let m = repo::update_memory(c, id, &input)?;
        st.changed(c, "memory", id, "update");
        Ok(Json(m))
    })
    .await
}

pub async fn delete_memory(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        repo::delete_memory(c, id)?;
        st.changed(c, "memory", id, "delete");
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

// ------------------------------------------------------------------ devices

pub async fn list_devices(State(state): State<Shared>, _a: Authed) -> Json<Vec<Device>> {
    Json(state.devices.list())
}

pub async fn revoke_device(State(state): State<Shared>, a: Authed, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if id == a.device.id {
        return Err(ApiErr::bad_request("you can't revoke the device you're using"));
    }
    if !state.devices.revoke(&id)? {
        return Err(ApiErr::not_found());
    }
    state.end_device_sessions(&id);
    tracing::warn!("device {id} revoked by “{}”", a.device.name);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub struct PairingCodeReq {
    ttl_minutes: Option<i64>,
}

pub async fn pairing_code(State(state): State<Shared>, _a: Authed, body: Option<Json<PairingCodeReq>>) -> Json<PairingCode> {
    let ttl = body.and_then(|b| b.0.ttl_minutes).unwrap_or(10).clamp(1, 60);
    let (code, expires_at) = state.new_pairing_code(ttl * 60_000);
    Json(PairingCode { code, expires_at })
}
