use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use rad_api::*;
use rad_vault::blobs::{self, BlobWriter};
use rad_vault::repo::{self, NewDocument};
use serde::Deserialize;

use crate::auth::Authed;
use crate::error::{ApiErr, ApiResult};
use crate::ingest::enqueue;
use crate::rag;
use crate::state::{db_vault, Shared};

// ------------------------------------------------------------------ projects

pub async fn list_projects(State(state): State<Shared>, a: Authed) -> ApiResult<Json<Vec<Project>>> {
    db_vault(a.vault, &state, |_, c| Ok(Json(repo::projects(c)?))).await
}

pub async fn get_project(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Project>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::project(c, id)?))).await
}

pub async fn create_project(State(state): State<Shared>, a: Authed, Json(input): Json<ProjectInput>) -> ApiResult<Json<Project>> {
    db_vault(a.vault, &state, move |st, c| {
        let p = repo::create_project(c, &input)?;
        st.changed(c, "project", p.id, "create");
        Ok(Json(p))
    })
    .await
}

pub async fn update_project(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(input): Json<ProjectInput>) -> ApiResult<Json<Project>> {
    db_vault(a.vault, &state, move |st, c| {
        let p = repo::update_project(c, id, &input)?;
        st.changed(c, "project", id, "update");
        Ok(Json(p))
    })
    .await
}

pub async fn delete_project(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        repo::project(c, id)?;
        repo::delete_project(c, id)?;
        st.changed(c, "project", id, "delete");
        enqueue(st, c, "gc", None)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

// ------------------------------------------------------------------ documents

pub async fn list_documents(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Vec<Document>>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::documents(c, id)?))).await
}

#[derive(Deserialize)]
pub struct Limit {
    limit: Option<i64>,
}

pub async fn recent_documents(State(state): State<Shared>, a: Authed, Query(q): Query<Limit>) -> ApiResult<Json<Vec<Document>>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::recent_documents(c, q.limit.unwrap_or(30).clamp(1, 200))?))).await
}

pub async fn get_document(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Document>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::document(c, id)?))).await
}

pub async fn patch_document(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(p): Json<DocumentPatch>) -> ApiResult<Json<Document>> {
    db_vault(a.vault, &state, move |st, c| {
        let before = repo::document(c, id)?;
        let d = repo::patch_document(c, id, &p)?;
        if before.project_id != d.project_id {
            enqueue(st, c, "embed", Some(id))?;
            st.changed(c, "project", before.project_id, "update");
            st.changed(c, "project", d.project_id, "update");
        }
        st.changed(c, "document", id, "update");
        Ok(Json(d))
    })
    .await
}

pub async fn delete_document(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        let d = repo::delete_document(c, id)?;
        st.changed(c, "document", id, "delete");
        st.changed(c, "project", d.project_id, "update");
        enqueue(st, c, "gc", None)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

pub async fn reindex_document(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Document>> {
    db_vault(a.vault, &state, move |st, c| {
        repo::set_document_status(c, id, DocStatus::Queued, None, None)?;
        enqueue(st, c, "ingest", Some(id))?;
        st.changed(c, "document", id, "update");
        Ok(Json(repo::document(c, id)?))
    })
    .await
}

fn header_str(h: &HeaderMap, name: &str) -> Option<String> {
    let raw = h.get(name)?.to_str().ok()?;
    Some(percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned())
}

fn clean_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let s: String = base.chars().filter(|c| !c.is_control()).take(200).collect();
    if s.is_empty() { "untitled".into() } else { s }
}

fn display_title(name: &str) -> String {
    name.to_string()
}

/// Streams the request body straight into the vault (1 MiB pieces), deduplicating by hash.
pub async fn upload(State(state): State<Shared>, a: Authed, Path(project_id): Path<Id>, headers: HeaderMap, body: Body) -> ApiResult<Json<UploadResponse>> {
    let filename = clean_filename(&header_str(&headers, HEADER_FILENAME).unwrap_or_else(|| "untitled".into()));
    let source = match header_str(&headers, HEADER_SOURCE).as_deref() {
        Some("paste") => "paste",
        Some("screenshot") => "screenshot",
        Some("chat") => "chat",
        Some("web") => "web",
        _ => "upload",
    };
    db_vault(a.vault.clone(), &state, move |_, c| Ok(repo::project(c, project_id)?)).await?;

    let v = a.vault.clone();
    let mut writer = Some(tokio::task::spawn_blocking(move || BlobWriter::begin(&v.conn())).await.map_err(ApiErr::internal)??);
    let mut head: Vec<u8> = Vec::new();
    let mut buf: Vec<u8> = Vec::with_capacity(blobs::CHUNK);
    let mut stream = body.into_data_stream();
    let max = state.cfg.max_upload_bytes;

    let result: ApiResult<()> = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiErr::bad_request(format!("upload interrupted: {e}")))?;
            if head.len() < 8192 {
                head.extend_from_slice(&chunk[..chunk.len().min(8192 - head.len())]);
            }
            buf.extend_from_slice(&chunk);
            if writer.as_ref().map(|w| w.size()).unwrap_or(0) + buf.len() as u64 > max {
                return Err(ApiErr::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "file exceeds the server's upload limit"));
            }
            if buf.len() >= blobs::CHUNK {
                let v = a.vault.clone();
                let data = std::mem::take(&mut buf);
                let mut w = writer.take().ok_or_else(|| ApiErr::internal("upload writer lost"))?;
                writer = Some(
                    tokio::task::spawn_blocking(move || -> rad_vault::Result<BlobWriter> {
                        w.write(&v.conn(), &data)?;
                        Ok(w)
                    })
                    .await
                    .map_err(ApiErr::internal)??,
                );
            }
        }
        Ok(())
    }
    .await;

    if let Err(e) = result {
        if let Some(w) = writer {
            let v = a.vault.clone();
            let _ = tokio::task::spawn_blocking(move || w.abort(&v.conn())).await;
        }
        return Err(e);
    }
    let mut writer = writer.ok_or_else(|| ApiErr::internal("upload writer lost"))?;

    let mime = rad_ingest::detect_mime(&head, &filename);
    db_vault(a.vault, &state, move |st, c| {
        let tx = rad_vault::rusqlite::Transaction::new_unchecked(c, rad_vault::rusqlite::TransactionBehavior::Immediate)?;
        writer.write(&tx, &buf)?;
        let (blob, _existed) = writer.finish(&tx, &mime)?;
        if let Some(existing) = repo::document_in_project_by_blob(&tx, project_id, blob.id)? {
            tx.commit()?;
            return Ok(Json(UploadResponse { document: repo::document(c, existing)?, duplicate: true }));
        }
        let d = repo::create_document(
            &tx,
            &NewDocument {
                project_id,
                blob_id: blob.id,
                title: &display_title(&filename),
                original_name: &filename,
                mime: &mime,
                size: blob.size,
                source_kind: source,
                tags: &[],
            },
        )?;
        repo::enqueue_job(&tx, "ingest", Some(d.id), serde_json::json!({}))?;
        tx.commit()?;
        st.jobs_wake.notify_one();
        st.changed(c, "document", d.id, "create");
        st.changed(c, "project", project_id, "update");
        Ok(Json(UploadResponse { document: d, duplicate: false }))
    })
    .await
}

pub async fn blob_exists(State(state): State<Shared>, a: Authed, Path(hash): Path<String>) -> ApiResult<Response> {
    let exists = db_vault(a.vault, &state, move |_, c| Ok(blobs::by_hash(c, &hash.to_lowercase())?.is_some())).await?;
    Ok(if exists { StatusCode::OK.into_response() } else { StatusCode::NOT_FOUND.into_response() })
}

pub async fn from_blob(State(state): State<Shared>, a: Authed, Path(project_id): Path<Id>, Json(req): Json<FromBlobRequest>) -> ApiResult<Json<UploadResponse>> {
    db_vault(a.vault, &state, move |st, c| {
        let blob = blobs::by_hash(c, &req.hash.to_lowercase())?.ok_or_else(ApiErr::not_found)?;
        if let Some(existing) = repo::document_in_project_by_blob(c, project_id, blob.id)? {
            return Ok(Json(UploadResponse { document: repo::document(c, existing)?, duplicate: true }));
        }
        let filename = clean_filename(&req.filename);
        let source = req.source_kind.as_deref().unwrap_or("upload");
        let mime = if blob.mime == "application/octet-stream" { rad_ingest::detect_mime(&[], &filename) } else { blob.mime.clone() };
        let d = repo::create_document(
            c,
            &NewDocument {
                project_id,
                blob_id: blob.id,
                title: &display_title(&filename),
                original_name: &filename,
                mime: &mime,
                size: blob.size,
                source_kind: source,
                tags: &[],
            },
        )?;
        enqueue(st, c, "ingest", Some(d.id))?;
        st.changed(c, "document", d.id, "create");
        st.changed(c, "project", project_id, "update");
        Ok(Json(UploadResponse { document: d, duplicate: false }))
    })
    .await
}

pub async fn content(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, headers: HeaderMap) -> ApiResult<Response> {
    let (blob_id, mime, name) = db_vault(a.vault.clone(), &state, move |_, c| {
        let d = repo::document(c, id)?;
        Ok((repo::document_blob_id(c, id)?, d.mime, d.original_name))
    })
    .await?;
    crate::content::blob_response(a.vault, blob_id, &mime, &headers, Some(&name)).await
}

pub async fn thumb(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Response> {
    let t = db_vault(a.vault, &state, move |_, c| {
        let blob_id = repo::document_blob_id(c, id)?;
        Ok(blobs::thumb(c, blob_id)?)
    })
    .await?;
    match t {
        Some((mime, data)) => Ok(([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, max-age=86400".into())], data).into_response()),
        None => Err(ApiErr::not_found()),
    }
}

pub async fn chunks(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Vec<Chunk>>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::chunks_for_document(c, id)?))).await
}

// ------------------------------------------------------------------ notes

fn validate_note(n: &NoteInput) -> ApiResult<String> {
    let t = n.title.trim();
    let title = if t.is_empty() {
        n.content.lines().map(|l| l.trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("Untitled note").chars().take(120).collect()
    } else {
        t.chars().take(200).collect()
    };
    if n.content.len() > 20 * 1024 * 1024 {
        return Err(ApiErr::bad_request("note is too large"));
    }
    Ok(title)
}

pub async fn create_note(State(state): State<Shared>, a: Authed, Path(project_id): Path<Id>, Json(n): Json<NoteInput>) -> ApiResult<Json<Note>> {
    let title = validate_note(&n)?;
    db_vault(a.vault, &state, move |st, c| {
        let tx = rad_vault::rusqlite::Transaction::new_unchecked(c, rad_vault::rusqlite::TransactionBehavior::Immediate)?;
        let (blob, _) = {
            let mut w = BlobWriter::begin(&tx)?;
            w.write(&tx, n.content.as_bytes())?;
            w.finish(&tx, "text/markdown")?
        };
        let tags = n.tags.clone().unwrap_or_default();
        let d = repo::create_document(
            &tx,
            &NewDocument {
                project_id,
                blob_id: blob.id,
                title: &title,
                original_name: &format!("{title}.md"),
                mime: "text/markdown",
                size: blob.size,
                source_kind: "note",
                tags: &tags,
            },
        )?;
        repo::enqueue_job(&tx, "ingest", Some(d.id), serde_json::json!({}))?;
        tx.commit()?;
        st.jobs_wake.notify_one();
        st.changed(c, "document", d.id, "create");
        st.changed(c, "project", project_id, "update");
        Ok(Json(Note { document: d, content: n.content }))
    })
    .await
}

pub async fn get_note(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<Note>> {
    db_vault(a.vault, &state, move |_, c| {
        let d = repo::document(c, id)?;
        let blob_id = repo::document_blob_id(c, id)?;
        let content = rad_ingest::decode_text(&blobs::read_all(c, blob_id)?);
        Ok(Json(Note { document: d, content }))
    })
    .await
}

pub async fn update_note(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(n): Json<NoteInput>) -> ApiResult<Json<Note>> {
    let title = validate_note(&n)?;
    db_vault(a.vault, &state, move |st, c| {
        let d = repo::document(c, id)?;
        if d.source_kind != "note" {
            return Err(ApiErr::bad_request("only notes can be edited"));
        }
        let tx = rad_vault::rusqlite::Transaction::new_unchecked(c, rad_vault::rusqlite::TransactionBehavior::Immediate)?;
        let (blob, _) = blobs::put(&tx, n.content.as_bytes(), "text/markdown")?;
        let tags = n.tags.clone().unwrap_or(d.tags.clone());
        repo::replace_document_blob(&tx, id, blob.id, blob.size, &title, &tags)?;
        repo::enqueue_job(&tx, "ingest", Some(id), serde_json::json!({}))?;
        tx.commit()?;
        st.jobs_wake.notify_one();
        enqueue(st, c, "gc", None)?;
        st.changed(c, "document", id, "update");
        Ok(Json(Note { document: repo::document(c, id)?, content: n.content }))
    })
    .await
}

// ------------------------------------------------------------------ search

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    projects: Option<String>,
    limit: Option<usize>,
}

pub async fn search(State(state): State<Shared>, a: Authed, Query(q): Query<SearchQuery>) -> ApiResult<Json<Vec<Citation>>> {
    let projects: Vec<Id> = q.projects.unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let settings = db_vault(a.vault.clone(), &state, |_, c| Ok(repo::settings(c)?)).await?;
    let limit = q.limit.unwrap_or(12).clamp(1, 50);
    let passages = rag::retrieve(&state, &a.vault, &settings, &q.q, projects, limit, 200_000, None).await?;
    Ok(Json(passages.into_iter().map(|p| p.citation).collect()))
}
