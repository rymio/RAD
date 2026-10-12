use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::Json;
use futures::{Stream, StreamExt};
use rad_api::*;
use rad_llm::{ChatMsg, ChatRequest, ImagePart};
use rad_vault::repo::{self, NewMessage};
use rad_vault::{blobs, Vault};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::auth::Authed;
use crate::error::{ApiErr, ApiResult};
use crate::models;
use crate::rag;
use crate::state::{db_vault, Shared};

const HISTORY_MESSAGES: usize = 12;
const HISTORY_CHARS: usize = 14_000;
const ATTACHMENT_CHARS: usize = 16_000;

#[derive(Deserialize)]
pub struct ListQuery {
    limit: Option<i64>,
}

pub async fn list(State(state): State<Shared>, a: Authed, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<Chat>>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(repo::chats(c, q.limit.unwrap_or(200).clamp(1, 1000))?))).await
}

pub async fn create(State(state): State<Shared>, a: Authed, Json(input): Json<ChatInput>) -> ApiResult<Json<Chat>> {
    db_vault(a.vault, &state, move |st, c| {
        let ch = repo::create_chat(c, &input)?;
        st.changed(c, "chat", ch.id, "create");
        Ok(Json(ch))
    })
    .await
}

pub async fn get(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<Json<ChatWithMessages>> {
    db_vault(a.vault, &state, move |_, c| Ok(Json(ChatWithMessages { chat: repo::chat(c, id)?, messages: repo::messages(c, id)? }))).await
}

pub async fn update(State(state): State<Shared>, a: Authed, Path(id): Path<Id>, Json(input): Json<ChatInput>) -> ApiResult<Json<Chat>> {
    db_vault(a.vault, &state, move |st, c| {
        let ch = repo::update_chat(c, id, &input)?;
        st.changed(c, "chat", id, "update");
        Ok(Json(ch))
    })
    .await
}

pub async fn remove(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        repo::delete_chat(c, id)?;
        st.changed(c, "chat", id, "delete");
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

pub async fn delete_message(State(state): State<Shared>, a: Authed, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    db_vault(a.vault, &state, move |st, c| {
        let m = repo::message(c, id)?;
        repo::delete_message(c, id)?;
        st.changed(c, "chat", m.chat_id, "update");
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Everything the generation task needs, loaded in one blocking section.
struct Prepared {
    chat: Chat,
    settings: Settings,
    memories: Vec<Memory>,
    projects: Vec<Project>,
    history: Vec<Message>,
    user_msg: Message,
    model: ModelRef,
    conn: rad_llm::ProviderConn,
    rewrite: Option<(ModelRef, rad_llm::ProviderConn)>,
    images: Vec<ImagePart>,
    attached_text: String,
    remembered: Option<String>,
}

fn remember_request(content: &str) -> Option<String> {
    let t = content.trim();
    let lower = t.to_lowercase();
    for p in ["remember that ", "remember: ", "please remember that ", "remember, "] {
        if lower.starts_with(p) {
            let rest = t[p.len()..].trim().trim_end_matches('.');
            if rest.len() > 2 {
                return Some(rest.to_string());
            }
        }
    }
    None
}

pub async fn send(
    State(state): State<Shared>,
    a: Authed,
    Path(chat_id): Path<Id>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    if req.content.trim().is_empty() && req.attachments.is_empty() && !req.regenerate {
        return Err(ApiErr::bad_request("message is empty"));
    }
    let vault = a.vault.clone();
    let prepared = db_vault(a.vault.clone(), &state, move |st, c| {
        let chat = repo::chat(c, chat_id)?;
        let settings = repo::settings(c)?;
        if let Some(from) = req.truncate_from {
            repo::delete_messages_from(c, chat_id, from)?;
        }
        let mut msgs = repo::messages(c, chat_id)?;
        let user_msg = if req.regenerate {
            while msgs.last().is_some_and(|m| m.role != "user") {
                let m = msgs.pop().unwrap();
                repo::delete_message(c, m.id)?;
            }
            msgs.pop().ok_or_else(|| ApiErr::bad_request("nothing to regenerate"))?
        } else {
            let mut atts = Vec::new();
            for id in &req.attachments {
                if let Ok(d) = repo::document(c, *id) {
                    atts.push(Attachment { document_id: d.id, title: d.title, mime: d.mime });
                }
            }
            repo::add_message(
                c,
                &NewMessage {
                    chat_id,
                    role: "user",
                    content: req.content.trim(),
                    attachments: &atts,
                    citations: &[],
                    model: None,
                    error: None,
                    tokens_in: None,
                    tokens_out: None,
                },
            )?
        };
        let history: Vec<Message> = msgs.into_iter().filter(|m| m.role == "user" || m.role == "assistant").collect();

        let model = req
            .model
            .clone()
            .or_else(|| chat.model.clone())
            .or_else(|| settings.chat_model.clone())
            .ok_or_else(|| ApiErr::bad_request("choose a chat model in Settings → Models first"))?;
        let conn = models::resolve(c, &model)?;
        let rewrite = settings.rewrite_model.as_ref().and_then(|m| models::resolve(c, m).ok().map(|p| (m.clone(), p)));

        // Attachments: images go to the model as images; other documents as text.
        let mut images = Vec::new();
        let mut attached_text = String::new();
        for att in &user_msg.attachments {
            let blob_id = match repo::document_blob_id(c, att.document_id) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if att.mime.starts_with("image/") {
                let bytes = blobs::read_all(c, blob_id)?;
                if let Some((data, mime)) = rad_ingest::vision_image(&bytes) {
                    images.push(ImagePart { mime, data });
                }
            } else {
                let mut text = repo::document_text(c, att.document_id, ATTACHMENT_CHARS)?;
                if text.trim().is_empty() && (att.mime.starts_with("text/") || att.mime.contains("json")) {
                    let bytes = blobs::read_all(c, blob_id)?;
                    text = rad_ingest::decode_text(&bytes).chars().take(ATTACHMENT_CHARS).collect();
                }
                if !text.trim().is_empty() {
                    attached_text.push_str(&format!("ATTACHED FILE “{}”:\n{}\n\n", att.title, text.trim()));
                }
            }
        }

        let remembered = if req.regenerate { None } else { remember_request(&user_msg.content) };
        if let Some(r) = &remembered {
            let m = repo::create_memory(c, r)?;
            st.changed(c, "memory", m.id, "create");
        }

        if chat.title == "New chat" && !req.regenerate {
            let t: String = user_msg.content.lines().next().unwrap_or("").chars().take(60).collect();
            let t = if t.trim().is_empty() { "Image chat".to_string() } else { t.trim().to_string() };
            repo::update_chat(c, chat_id, &ChatInput { title: Some(t), ..Default::default() })?;
        }
        st.changed(c, "chat", chat_id, "update");

        Ok(Prepared {
            chat: repo::chat(c, chat_id)?,
            settings,
            memories: repo::memories(c)?,
            projects: repo::projects(c)?,
            history,
            user_msg,
            model,
            conn,
            rewrite,
            images,
            attached_text,
            remembered,
        })
    })
    .await?;

    let (tx, rx) = mpsc::channel::<ChatStreamEvent>(128);
    let st = state.clone();
    tokio::spawn(async move {
        generate(st, vault, prepared, tx).await;
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(|ev| Ok(SseEvent::default().json_data(ev).unwrap_or_default()));
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Picks projects mentioned as @Name (spaces may be written as - or _).
fn mentioned_projects(text: &str, projects: &[Project]) -> Vec<Id> {
    let lower = text.to_lowercase();
    projects
        .iter()
        .filter(|p| {
            let n = p.name.to_lowercase();
            lower.contains(&format!("@{n}")) || lower.contains(&format!("@{}", n.replace(' ', "-"))) || lower.contains(&format!("@{}", n.replace(' ', "_")))
        })
        .map(|p| p.id)
        .collect()
}

async fn rewrite_query(state: &Shared, p: &Prepared) -> String {
    let q = p.user_msg.content.clone();
    if !p.settings.query_rewrite || p.history.is_empty() || q.trim().is_empty() {
        return q;
    }
    let (model, conn) = match &p.rewrite {
        Some((m, c)) => (m.model.clone(), c.clone()),
        None => (p.model.model.clone(), p.conn.clone()),
    };
    let mut convo = String::new();
    for m in p.history.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
        convo.push_str(&format!("{}: {}\n", m.role, m.content.chars().take(600).collect::<String>()));
    }
    let req = ChatRequest {
        system: Some(
            "You rewrite the user's latest message into a standalone search query for a document search engine. \
             Resolve pronouns and references using the conversation. Output ONLY the query, no quotes, no explanation."
                .into(),
        ),
        messages: vec![ChatMsg::new("user", format!("Conversation:\n{convo}\nLatest message: {q}\n\nStandalone search query:"))],
        temperature: Some(0.0),
        max_tokens: 120,
    };
    match tokio::time::timeout(Duration::from_secs(25), state.llm.complete(&conn, &model, &req)).await {
        Ok(Ok(r)) => {
            let r = strip_thinking(&r);
            let r = r.trim().trim_matches('"').lines().last().unwrap_or("").trim().to_string();
            if r.is_empty() || r.len() > 400 { q } else { format!("{r} {q}") }
        }
        _ => q,
    }
}

/// Removes <think>…</think> blocks some reasoning models emit.
fn strip_thinking(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out
}

async fn generate(state: Shared, vault: Arc<Vault>, p: Prepared, tx: mpsc::Sender<ChatStreamEvent>) {
    let _ = tx.send(ChatStreamEvent::UserMessage { message: p.user_msg.clone() }).await;

    // ---- retrieval
    let mut scope = p.chat.scope.projects.clone();
    let mentioned = mentioned_projects(&p.user_msg.content, &p.projects);
    if !mentioned.is_empty() {
        scope = mentioned;
    }
    let rag_mode = p.chat.mode == ChatMode::Rag;
    let mut passages = Vec::new();
    if rag_mode && !p.user_msg.content.trim().is_empty() {
        let _ = tx.send(ChatStreamEvent::Status { text: "Searching your vault…".into() }).await;
        let query = rewrite_query(&state, &p).await;
        match rag::retrieve(&state, &vault, &p.settings, &query, scope.clone(), p.settings.top_k as usize, p.settings.context_chars as usize).await {
            Ok(ps) => passages = ps,
            Err(e) => tracing::warn!("retrieval failed: {}", e.message),
        }
    }
    let citations: Vec<Citation> = passages.iter().map(|x| x.citation.clone()).collect();
    if rag_mode {
        let _ = tx.send(ChatStreamEvent::Sources { citations: citations.clone() }).await;
    }

    // ---- prompt
    let single_project = if scope.len() == 1 { p.projects.iter().find(|x| x.id == scope[0]) } else { None };
    let mut system = rag::system_prompt(&p.settings, &p.memories, single_project, rag_mode);
    if let Some(r) = &p.remembered {
        system.push_str(&format!("\n\nThe owner just asked you to remember: “{r}”. It has been saved to long-term memory — confirm briefly."));
    }
    let mut messages: Vec<ChatMsg> = Vec::new();
    let mut used = 0usize;
    let mut hist: Vec<ChatMsg> = Vec::new();
    for m in p.history.iter().rev().take(HISTORY_MESSAGES) {
        let text: String = m.content.chars().take(4000).collect();
        used += text.len();
        if used > HISTORY_CHARS {
            break;
        }
        hist.push(ChatMsg::new(&m.role, text));
    }
    hist.reverse();
    messages.extend(hist);
    let mut final_user = String::new();
    if rag_mode {
        final_user.push_str(&rag::sources_block(&passages));
    }
    final_user.push_str(&p.attached_text);
    if rag_mode || !p.attached_text.is_empty() {
        final_user.push_str("QUESTION: ");
    }
    final_user.push_str(if p.user_msg.content.trim().is_empty() { "Describe this." } else { &p.user_msg.content });
    messages.push(ChatMsg { role: "user".into(), content: final_user, images: p.images.clone() });

    let req = ChatRequest { system: Some(system), messages, temperature: Some(p.settings.temperature), max_tokens: p.settings.max_output_tokens };
    let _ = tx.send(ChatStreamEvent::Status { text: "Thinking…".into() }).await;

    // ---- stream
    let mut answer = String::new();
    let mut error: Option<String> = None;
    let (mut tin, mut tout) = (None, None);
    let mut client_gone = false;
    match state.llm.chat_stream(&p.conn, &p.model.model, &req).await {
        Ok(mut s) => {
            while let Some(chunk) = s.next().await {
                match chunk {
                    Ok(c) => {
                        if c.tokens_in.is_some() {
                            tin = c.tokens_in;
                        }
                        if c.tokens_out.is_some() {
                            tout = c.tokens_out;
                        }
                        if !c.text.is_empty() {
                            answer.push_str(&c.text);
                            if tx.send(ChatStreamEvent::Delta { text: c.text }).await.is_err() {
                                client_gone = true;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error = Some(e.to_string());
                        break;
                    }
                }
            }
        }
        Err(e) => error = Some(e.to_string()),
    }
    if client_gone && error.is_none() && !answer.is_empty() {
        answer.push_str("\n\n*(stopped)*");
    }

    // Only keep citations the answer actually uses (plus all when none are referenced explicitly).
    let used_cites: Vec<Citation> = {
        let referenced: Vec<Citation> = citations.iter().filter(|c| answer.contains(&format!("[{}]", c.n))).cloned().collect();
        if referenced.is_empty() { citations.clone() } else { referenced }
    };

    let chat_id = p.chat.id;
    let model_name = p.model.model.clone();
    let st = state.clone();
    let saved = db_vault(vault, &state, move |_, c| {
        let m = repo::add_message(
            c,
            &NewMessage {
                chat_id,
                role: "assistant",
                content: &answer,
                attachments: &[],
                citations: &used_cites,
                model: Some(&model_name),
                error: error.as_deref(),
                tokens_in: tin,
                tokens_out: tout,
            },
        )?;
        st.changed(c, "chat", chat_id, "update");
        Ok(m)
    })
    .await;
    match saved {
        Ok(m) => {
            if let Some(e) = &m.error {
                let _ = tx.send(ChatStreamEvent::Error { message: e.clone() }).await;
            }
            let _ = tx.send(ChatStreamEvent::Done { message: m }).await;
        }
        Err(e) => {
            let _ = tx.send(ChatStreamEvent::Error { message: e.message }).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_detection() {
        assert_eq!(remember_request("Remember that my passport expires in 2031.").as_deref(), Some("my passport expires in 2031"));
        assert!(remember_request("Do you remember that thing?").is_none());
    }

    #[test]
    fn strip_think() {
        assert_eq!(strip_thinking("<think>hmm</think>query"), "query");
    }
}
