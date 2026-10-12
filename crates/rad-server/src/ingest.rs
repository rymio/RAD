//! Background job worker: extraction, image captioning, transcription, chunking, embedding.

use std::sync::Arc;
use std::time::Duration;

use rad_api::{Anchor, DocStatus, Event, Id, Settings};
use rad_ingest::{Kind, Media, Section};
use rad_llm::{ChatMsg, ChatRequest, ImagePart, LlmError};
use rad_vault::repo::{self, Job, NewChunk};
use rad_vault::{blobs, search, Vault};
use tokio::sync::Semaphore;

use crate::error::ApiErr;
use crate::media;
use crate::models;
use crate::state::{AppState, Shared};

const CONCURRENCY: usize = 2;
const MAX_ATTEMPTS: i64 = 8;
const EMBED_BATCH: usize = 32;
const MAX_EXTRACT_BYTES: i64 = 1024 * 1024 * 1024;

const VISION_PROMPT: &str = "You are indexing an image for a private, searchable knowledge base.\n\
1. Transcribe ALL readable text in the image verbatim, keeping line breaks (write \"none\" if there is no text).\n\
2. Then describe the image thoroughly: what it shows, people, objects, places, the app or website if it is a screenshot, \
charts with their values, tables, dates, amounts and anything someone might later search for.\n\
Answer in exactly this format:\nTEXT:\n<text>\n\nDESCRIPTION:\n<description>";

enum JobError {
    /// Worth retrying later (network, rate limits, provider down).
    Transient(String),
    Permanent(String),
}

impl From<ApiErr> for JobError {
    fn from(e: ApiErr) -> Self {
        if e.code == "upstream" {
            JobError::Transient(e.message)
        } else {
            JobError::Permanent(e.message)
        }
    }
}

impl From<rad_vault::VaultError> for JobError {
    fn from(e: rad_vault::VaultError) -> Self {
        JobError::Permanent(e.to_string())
    }
}

impl From<LlmError> for JobError {
    fn from(e: LlmError) -> Self {
        match &e {
            LlmError::Connect(..) => JobError::Transient(e.to_string()),
            LlmError::Http { status, .. } if *status == 429 || *status >= 500 || *status == 408 => JobError::Transient(e.to_string()),
            _ => JobError::Permanent(e.to_string()),
        }
    }
}

type JResult<T> = Result<T, JobError>;

async fn blocking<T: Send + 'static>(
    vault: &Arc<Vault>,
    f: impl FnOnce(&rad_vault::Connection) -> rad_vault::Result<T> + Send + 'static,
) -> JResult<T> {
    let v = vault.clone();
    tokio::task::spawn_blocking(move || f(&v.conn()))
        .await
        .map_err(|e| JobError::Permanent(e.to_string()))?
        .map_err(JobError::from)
}

pub async fn worker_loop(state: Shared) {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    loop {
        if state.is_stopped() {
            return;
        }
        let Some(vault) = state.current_vault() else {
            let _ = tokio::time::timeout(Duration::from_secs(5), state.jobs_wake.notified()).await;
            continue;
        };
        let permit = sem.clone().acquire_owned().await.expect("semaphore");
        let v = vault.clone();
        let job = tokio::task::spawn_blocking(move || repo::claim_job(&v.conn())).await;
        match job {
            Ok(Ok(Some(job))) => {
                let st = state.clone();
                tokio::spawn(async move {
                    run_job(&st, vault, job).await;
                    drop(permit);
                    st.jobs_wake.notify_one();
                });
            }
            Ok(Ok(None)) => {
                drop(permit);
                let v = vault.clone();
                let due = tokio::task::spawn_blocking(move || repo::next_job_due(&v.conn())).await.ok().and_then(|r| r.ok()).flatten();
                let wait = match due {
                    Some(t) => Duration::from_millis((t - rad_vault::now_ms()).clamp(200, 30_000) as u64),
                    None => Duration::from_secs(30),
                };
                drop(vault);
                let _ = tokio::time::timeout(wait, state.jobs_wake.notified()).await;
            }
            Ok(Err(e)) => {
                drop(permit);
                tracing::error!("claiming job failed: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(e) => {
                drop(permit);
                tracing::error!("job claim task failed: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn run_job(state: &Shared, vault: Arc<Vault>, job: Job) {
    let res = match job.kind.as_str() {
        "ingest" => match job.document_id {
            Some(d) => ingest_document(state, &vault, d).await,
            None => Ok(()),
        },
        "embed" => match job.document_id {
            Some(d) => embed_document(state, &vault, d).await.map(|_| ()),
            None => Ok(()),
        },
        "reindex" => reindex(state, &vault).await,
        "gc" => blocking(&vault, |c| blobs::collect_garbage(c).map(|_| ())).await,
        other => Err(JobError::Permanent(format!("unknown job kind {other}"))),
    };
    let id = job.id;
    let outcome = match res {
        Ok(()) => blocking(&vault, move |c| repo::complete_job(c, id)).await,
        Err(JobError::Transient(msg)) if job.attempts < MAX_ATTEMPTS => {
            let delay = (10_000i64 << job.attempts.min(8)).min(30 * 60_000);
            tracing::warn!("job {id} ({}) will retry in {}s: {msg}", job.kind, delay / 1000);
            if job.kind == "ingest" || job.kind == "embed" {
                if let Some(d) = job.document_id {
                    let notice = format!("Waiting for model provider — will retry: {msg}");
                    let _ = blocking(&vault, move |c| repo::set_document_notice(c, d, Some(&notice))).await;
                    emit_doc(state, &vault, d).await;
                }
            }
            blocking(&vault, move |c| repo::retry_job(c, id, delay, &msg)).await
        }
        Err(JobError::Transient(msg)) | Err(JobError::Permanent(msg)) => {
            tracing::warn!("job {id} ({}) failed: {msg}", job.kind);
            if let Some(d) = job.document_id {
                let m = msg.clone();
                if job.kind == "ingest" {
                    let _ = blocking(&vault, move |c| repo::set_document_status(c, d, DocStatus::Error, Some(&m), None)).await;
                } else {
                    let notice = format!("Semantic index unavailable: {m}");
                    let _ = blocking(&vault, move |c| repo::set_document_notice(c, d, Some(&notice))).await;
                }
                emit_doc(state, &vault, d).await;
            }
            blocking(&vault, move |c| repo::fail_job(c, id, &msg)).await
        }
    };
    if let Err(JobError::Permanent(e) | JobError::Transient(e)) = outcome {
        tracing::error!("updating job {id}: {e}");
    }
}

async fn emit_doc(state: &Shared, vault: &Arc<Vault>, doc_id: Id) {
    let st = state.clone();
    let _ = blocking(vault, move |c| {
        let d = repo::document(c, doc_id)?;
        let _ = st.events.send(Event::DocumentStatus {
            document_id: d.id,
            project_id: d.project_id,
            status: d.status,
            error: d.error.clone(),
        });
        st.changed(c, "document", d.id, "update");
        Ok(())
    })
    .await;
}

fn settings_of(c: &rad_vault::Connection) -> rad_vault::Result<Settings> {
    repo::settings(c)
}

async fn ingest_document(state: &Shared, vault: &Arc<Vault>, doc_id: Id) -> JResult<()> {
    let doc = match blocking(vault, move |c| {
        repo::set_document_status(c, doc_id, DocStatus::Processing, None, None)?;
        repo::document(c, doc_id)
    })
    .await
    {
        Ok(d) => d,
        Err(JobError::Permanent(_)) => return Ok(()), // document deleted meanwhile
        Err(e) => return Err(e),
    };
    emit_doc(state, vault, doc_id).await;
    let settings = blocking(vault, settings_of).await?;
    let blob_id = blocking(vault, move |c| repo::document_blob_id(c, doc_id)).await?;

    let kind = rad_ingest::classify(&doc.mime, &doc.original_name);
    let is_media = matches!(kind, Kind::Audio | Kind::Video);
    let bytes = if is_media || doc.size > MAX_EXTRACT_BYTES {
        Vec::new()
    } else {
        blocking(vault, move |c| blobs::read_all(c, blob_id)).await?
    };

    let name = doc.original_name.clone();
    let mime = doc.mime.clone();
    let mut ex = if is_media || doc.size > MAX_EXTRACT_BYTES {
        rad_ingest::Extracted {
            kind,
            title: None,
            sections: Vec::new(),
            page_count: None,
            media: match kind {
                Kind::Audio => Media::Audio,
                Kind::Video => Media::Video,
                _ => Media::None,
            },
            notice: (!is_media).then(|| "File is too large to extract text from; stored in the vault.".to_string()),
        }
    } else {
        let bytes2 = bytes.clone();
        let fut = tokio::task::spawn_blocking(move || rad_ingest::extract(&bytes2, &name, &mime));
        match tokio::time::timeout(Duration::from_secs(600), fut).await {
            Ok(Ok(Ok(ex))) => ex,
            Ok(Ok(Err(e))) => return Err(JobError::Permanent(e.to_string())),
            Ok(Err(e)) => return Err(JobError::Permanent(format!("extractor crashed: {e}"))),
            Err(_) => return Err(JobError::Permanent("extraction timed out".into())),
        }
    };

    let mut notices: Vec<String> = ex.notice.take().into_iter().collect();

    // ---- images: thumbnail + vision caption
    if let Media::Image { width, .. } = ex.media {
        if width > 0 {
            let b = bytes.clone();
            if let Ok(Some(thumb)) = tokio::task::spawn_blocking(move || rad_ingest::thumbnail(&b)).await {
                let _ = blocking(vault, move |c| blobs::set_thumb(c, blob_id, "image/jpeg", &thumb)).await;
            }
            match &settings.vision_model {
                Some(vm) => {
                    let vm2 = vm.clone();
                    let conn = blocking(vault, move |c| Ok(models::resolve(c, &vm2))).await?.map_err(JobError::from)?;
                    let b = bytes.clone();
                    let img = tokio::task::spawn_blocking(move || rad_ingest::vision_image(&b)).await.ok().flatten();
                    if let Some((data, mime)) = img {
                        let req = ChatRequest {
                            system: None,
                            messages: vec![ChatMsg { role: "user".into(), content: VISION_PROMPT.into(), images: vec![ImagePart { mime, data }] }],
                            temperature: Some(0.1),
                            max_tokens: 1500,
                        };
                        let caption = state.llm.complete(&conn, &vm.model, &req).await?;
                        if !caption.trim().is_empty() {
                            ex.sections.push(Section { text: caption, anchor: Anchor::default(), kind: "caption", markdown: false });
                        }
                    }
                }
                None => notices.push("Add a vision model in Settings → Models to make images searchable by their content.".into()),
            }
        }
    }

    // ---- audio / video: transcript (+ video thumbnail)
    if is_media {
        let ffmpeg_ok = state.ffmpeg_ok.load(std::sync::atomic::Ordering::Relaxed);
        if !ffmpeg_ok {
            notices.push("Install ffmpeg on the RAD server to transcribe audio and video.".into());
        } else {
            if kind == Kind::Video {
                if let Some(thumb) = media::video_thumbnail(state, vault.clone(), blob_id, &doc.mime).await {
                    let _ = blocking(vault, move |c| blobs::set_thumb(c, blob_id, "image/jpeg", &thumb)).await;
                }
            }
            match &settings.transcription_model {
                Some(tm) => {
                    let tm2 = tm.clone();
                    let conn = blocking(vault, move |c| Ok(models::resolve(c, &tm2))).await?.map_err(JobError::from)?;
                    let (sections, _dur) =
                        media::transcribe(state, vault.clone(), blob_id, &doc.mime, tm, &conn, settings.chunk_chars as usize).await?;
                    if sections.is_empty() {
                        notices.push("No speech was detected.".into());
                    }
                    ex.sections.extend(sections);
                }
                None => notices.push("Add a transcription model in Settings → Models to make audio and video searchable.".into()),
            }
        }
    }

    // Every file is at least findable by its name.
    if ex.sections.iter().all(|s| s.text.trim().is_empty()) {
        ex.sections.push(Section {
            text: format!("File “{}” ({}, {} bytes).", doc.original_name, rad_ingest::kind_name(kind), doc.size),
            anchor: Anchor::default(),
            kind: "meta",
            markdown: false,
        });
    }

    let title = if doc.source_kind == "note" { doc.title.clone() } else { ex.title.clone().unwrap_or_else(|| doc.title.clone()) };
    let chunks: Vec<NewChunk> = rad_ingest::chunk_document(&ex, &title, settings.chunk_chars as usize, settings.chunk_overlap as usize)
        .into_iter()
        .map(|c| NewChunk { text: c.text, context: c.context, anchor: c.anchor, kind: c.kind })
        .collect();
    let page_count = ex.page_count;
    let ex_title = ex.title.clone();
    blocking(vault, move |c| {
        repo::replace_chunks(c, doc_id, &chunks)?;
        repo::set_document_extracted(c, doc_id, page_count, ex_title.as_deref())?;
        Ok(())
    })
    .await?;

    // ---- embeddings (keyword search already works at this point)
    if settings.embedding_model.is_some() {
        match embed_document(state, vault, doc_id).await {
            Ok(_) => {}
            Err(JobError::Transient(msg)) => {
                notices.push(format!("Semantic index pending (will retry): {msg}"));
                let _ = blocking(vault, move |c| repo::enqueue_job(c, "embed", Some(doc_id), serde_json::json!({})).map(|_| ())).await;
                let _ = blocking(vault, move |c| {
                    c.execute("UPDATE jobs SET run_after = ?1 WHERE kind = 'embed' AND document_id = ?2", rad_vault::rusqlite::params![rad_vault::now_ms() + 60_000, doc_id])?;
                    Ok(())
                })
                .await;
            }
            Err(JobError::Permanent(msg)) => notices.push(format!("Semantic index unavailable: {msg}")),
        }
    }

    let notice = if notices.is_empty() { None } else { Some(notices.join(" ")) };
    blocking(vault, move |c| repo::set_document_status(c, doc_id, DocStatus::Ready, None, notice.as_deref())).await?;
    emit_doc(state, vault, doc_id).await;
    Ok(())
}

/// Embeds every chunk of a document with the configured embedding model.
async fn embed_document(state: &Shared, vault: &Arc<Vault>, doc_id: Id) -> JResult<bool> {
    let settings = blocking(vault, settings_of).await?;
    let Some(m) = settings.embedding_model.clone() else { return Ok(false) };
    let m2 = m.clone();
    let conn = blocking(vault, move |c| Ok(models::resolve(c, &m2))).await?.map_err(JobError::from)?;
    let rows = blocking(vault, move |c| repo::chunks_for_embedding(c, doc_id)).await?;
    if rows.is_empty() {
        return Ok(false);
    }
    let key = models::model_key(&m);
    let prefix = rad_llm::embed_prefix(&m.model, false);
    let mut space: Option<search::Space> = None;
    for batch in rows.chunks(EMBED_BATCH) {
        let texts: Vec<String> = batch.iter().map(|(_, _, t)| format!("{prefix}{}", truncate(t, 6000))).collect();
        let vecs = state.llm.embed(&conn, &m.model, &texts).await?;
        let dims = vecs[0].len();
        let sp = match space {
            Some(s) => s,
            None => {
                let k = key.clone();
                let s = blocking(vault, move |c| search::ensure_space(c, &k, dims)).await?;
                space = Some(s);
                s
            }
        };
        let rows: Vec<(Id, Id, Vec<f32>)> = batch.iter().zip(vecs).map(|((id, pid, _), v)| (*id, *pid, v)).collect();
        blocking(vault, move |c| search::insert_vectors(c, sp, &rows)).await?;
    }
    if let Some(sp) = space {
        blocking(vault, move |c| {
            search::mark_embedded(c, sp, doc_id)?;
            // Clear "pending" notices once the semantic index is in place.
            c.execute(
                "UPDATE documents SET notice = NULL WHERE id = ?1 AND (notice LIKE 'Semantic index%' OR notice LIKE 'Waiting for model provider%')",
                rad_vault::rusqlite::params![doc_id],
            )?;
            Ok(())
        })
        .await?;
        emit_doc(state, vault, doc_id).await;
    }
    Ok(true)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Queues embedding for every document that lacks vectors in the current embedding space.
async fn reindex(state: &Shared, vault: &Arc<Vault>) -> JResult<()> {
    let settings = blocking(vault, settings_of).await?;
    let Some(m) = settings.embedding_model else { return Ok(()) };
    let key = models::model_key(&m);
    let n = blocking(vault, move |c| {
        let ids: Vec<Id> = match search::space(c, &key)? {
            Some(sp) => search::documents_missing_embeddings(c, sp)?,
            None => {
                let mut st = c.prepare("SELECT id FROM documents WHERE chunk_count > 0")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            }
        };
        for id in &ids {
            repo::enqueue_job(c, "embed", Some(*id), serde_json::json!({}))?;
        }
        Ok(ids.len())
    })
    .await?;
    tracing::info!("reindex: queued {n} documents for embedding");
    state.jobs_wake.notify_waiters();
    Ok(())
}

pub fn enqueue(state: &AppState, c: &rad_vault::Connection, kind: &str, doc: Option<Id>) -> rad_vault::Result<()> {
    repo::enqueue_job(c, kind, doc, serde_json::json!({}))?;
    state.jobs_wake.notify_one();
    Ok(())
}
