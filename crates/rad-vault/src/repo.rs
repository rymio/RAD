//! Typed access to vault tables. All functions take a `&Connection` so callers can group
//! them inside a transaction.

use std::collections::BTreeMap;

use rad_api::*;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{now_ms, Result, VaultError};

fn json<T: serde::de::DeserializeOwned + Default>(s: Option<String>) -> T {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

// ------------------------------------------------------------------ meta & change log

pub fn meta_get(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT value FROM vault_meta WHERE key = ?1", params![key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute("INSERT OR REPLACE INTO vault_meta(key, value) VALUES (?1, ?2)", params![key, value])?;
    Ok(())
}

pub fn log_change(c: &Connection, entity: &str, entity_id: Id, op: &str) -> Result<i64> {
    c.execute(
        "INSERT INTO change_log(entity, entity_id, op, at) VALUES (?1, ?2, ?3, ?4)",
        params![entity, entity_id, op, now_ms()],
    )?;
    Ok(c.last_insert_rowid())
}

pub fn changes_since(c: &Connection, since: i64, limit: i64) -> Result<Vec<ChangeEntry>> {
    let mut st = c.prepare(
        "SELECT seq, entity, entity_id, op, at FROM change_log WHERE seq > ?1 ORDER BY seq LIMIT ?2",
    )?;
    let rows = st.query_map(params![since, limit], |r| {
        Ok(ChangeEntry { seq: r.get(0)?, entity: r.get(1)?, entity_id: r.get(2)?, op: r.get(3)?, at: r.get(4)? })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn last_seq(c: &Connection) -> Result<i64> {
    Ok(c.query_row("SELECT COALESCE(MAX(seq), 0) FROM change_log", [], |r| r.get(0))?)
}

/// Keeps the change log bounded; clients that fall further behind do a full refresh.
pub fn trim_change_log(c: &Connection, keep: i64) -> Result<()> {
    c.execute("DELETE FROM change_log WHERE seq <= (SELECT MAX(seq) FROM change_log) - ?1", params![keep])?;
    Ok(())
}

// ------------------------------------------------------------------ projects

const PROJECT_COLS: &str = "p.id, p.name, p.description, p.color, p.icon, p.system_prompt, p.archived,
    (SELECT COUNT(*) FROM documents d WHERE d.project_id = p.id AND d.source_kind != 'note'),
    (SELECT COUNT(*) FROM documents d WHERE d.project_id = p.id AND d.source_kind = 'note'),
    p.created_at, p.updated_at";

fn project_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        color: r.get(3)?,
        icon: r.get(4)?,
        system_prompt: r.get(5)?,
        archived: r.get(6)?,
        document_count: r.get(7)?,
        note_count: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

pub fn projects(c: &Connection) -> Result<Vec<Project>> {
    let mut st = c.prepare(&format!("SELECT {PROJECT_COLS} FROM projects p ORDER BY p.archived, p.name COLLATE NOCASE"))?;
    let rows = st.query_map([], project_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn project(c: &Connection, id: Id) -> Result<Project> {
    c.query_row(&format!("SELECT {PROJECT_COLS} FROM projects p WHERE p.id = ?1"), params![id], project_row)
        .optional()?
        .ok_or(VaultError::Missing)
}

pub fn create_project(c: &Connection, input: &ProjectInput) -> Result<Project> {
    let name = input.name.clone().unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return Err(VaultError::Invalid("project name is required".into()));
    }
    let now = now_ms();
    c.execute(
        "INSERT INTO projects(name, description, color, icon, system_prompt, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            name,
            input.description.clone().unwrap_or_default(),
            input.color.clone().unwrap_or_else(|| {
                // Rotate through a pleasant palette so projects are distinguishable by default.
                const PALETTE: [&str; 10] = ["#7c6cf2", "#3b82f6", "#06b6d4", "#2f9e67", "#f5a524", "#f97316", "#e5484d", "#ec4899", "#a855f7", "#84cc16"];
                let n: i64 = c.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap_or(0);
                PALETTE[(n as usize) % PALETTE.len()].to_string()
            }),
            input.icon.clone().unwrap_or_else(|| "folder".into()),
            input.system_prompt.clone().unwrap_or_default(),
            now
        ],
    )?;
    project(c, c.last_insert_rowid())
}

pub fn update_project(c: &Connection, id: Id, input: &ProjectInput) -> Result<Project> {
    let mut p = project(c, id)?;
    if let Some(v) = &input.name {
        if v.trim().is_empty() {
            return Err(VaultError::Invalid("project name is required".into()));
        }
        p.name = v.trim().to_string();
    }
    if let Some(v) = &input.description { p.description = v.clone(); }
    if let Some(v) = &input.color { p.color = v.clone(); }
    if let Some(v) = &input.icon { p.icon = v.clone(); }
    if let Some(v) = &input.system_prompt { p.system_prompt = v.clone(); }
    if let Some(v) = input.archived { p.archived = v; }
    c.execute(
        "UPDATE projects SET name=?1, description=?2, color=?3, icon=?4, system_prompt=?5, archived=?6, updated_at=?7 WHERE id=?8",
        params![p.name, p.description, p.color, p.icon, p.system_prompt, p.archived, now_ms(), id],
    )?;
    project(c, id)
}

pub fn delete_project(c: &Connection, id: Id) -> Result<()> {
    crate::in_tx(c, |tx| {
        let doc_ids: Vec<Id> = {
            let mut st = tx.prepare("SELECT id FROM documents WHERE project_id = ?1")?;
            let rows = st.query_map(params![id], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for d in doc_ids {
            crate::search::delete_vectors_for_document(tx, d)?;
        }
        tx.execute("DELETE FROM jobs WHERE document_id IN (SELECT id FROM documents WHERE project_id = ?1)", params![id])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    })
}

// ------------------------------------------------------------------ documents

const DOC_COLS: &str = "d.id, d.project_id, d.title, d.original_name, d.mime, d.size, b.hash, d.source_kind,
    d.status, d.error, d.notice, d.page_count, d.chunk_count,
    EXISTS(SELECT 1 FROM thumbs t WHERE t.blob_id = d.blob_id), d.tags, d.pinned, d.created_at, d.updated_at, d.blob_id";

fn doc_row(r: &Row) -> rusqlite::Result<Document> {
    Ok(Document {
        id: r.get(0)?,
        project_id: r.get(1)?,
        title: r.get(2)?,
        original_name: r.get(3)?,
        mime: r.get(4)?,
        size: r.get(5)?,
        hash: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        source_kind: r.get(7)?,
        status: DocStatus::parse(&r.get::<_, String>(8)?),
        error: r.get(9)?,
        notice: r.get(10)?,
        page_count: r.get(11)?,
        chunk_count: r.get(12)?,
        has_thumb: r.get(13)?,
        tags: json(r.get(14)?),
        pinned: r.get(15)?,
        created_at: r.get(16)?,
        updated_at: r.get(17)?,
    })
}

pub fn documents(c: &Connection, project_id: Id) -> Result<Vec<Document>> {
    let mut st = c.prepare(&format!(
        "SELECT {DOC_COLS} FROM documents d JOIN blobs b ON b.id = d.blob_id
         WHERE d.project_id = ?1 ORDER BY d.pinned DESC, d.updated_at DESC"
    ))?;
    let rows = st.query_map(params![project_id], doc_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn recent_documents(c: &Connection, limit: i64) -> Result<Vec<Document>> {
    let mut st = c.prepare(&format!(
        "SELECT {DOC_COLS} FROM documents d JOIN blobs b ON b.id = d.blob_id ORDER BY d.updated_at DESC LIMIT ?1"
    ))?;
    let rows = st.query_map(params![limit], doc_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn document(c: &Connection, id: Id) -> Result<Document> {
    c.query_row(
        &format!("SELECT {DOC_COLS} FROM documents d JOIN blobs b ON b.id = d.blob_id WHERE d.id = ?1"),
        params![id],
        doc_row,
    )
    .optional()?
    .ok_or(VaultError::Missing)
}

pub fn document_blob_id(c: &Connection, id: Id) -> Result<i64> {
    c.query_row("SELECT blob_id FROM documents WHERE id = ?1", params![id], |r| r.get(0))
        .optional()?
        .ok_or(VaultError::Missing)
}

pub fn document_in_project_by_blob(c: &Connection, project_id: Id, blob_id: i64) -> Result<Option<Id>> {
    Ok(c.query_row(
        "SELECT id FROM documents WHERE project_id = ?1 AND blob_id = ?2 AND source_kind != 'note' LIMIT 1",
        params![project_id, blob_id],
        |r| r.get(0),
    )
    .optional()?)
}

pub struct NewDocument<'a> {
    pub project_id: Id,
    pub blob_id: i64,
    pub title: &'a str,
    pub original_name: &'a str,
    pub mime: &'a str,
    pub size: i64,
    pub source_kind: &'a str,
    pub tags: &'a [String],
}

pub fn create_document(c: &Connection, d: &NewDocument) -> Result<Document> {
    project(c, d.project_id)?;
    let now = now_ms();
    c.execute(
        "INSERT INTO documents(project_id, blob_id, title, original_name, mime, size, source_kind, status, tags, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9, ?9)",
        params![
            d.project_id, d.blob_id, d.title, d.original_name, d.mime, d.size, d.source_kind,
            serde_json::to_string(d.tags)?, now
        ],
    )?;
    let id = c.last_insert_rowid();
    touch_project(c, d.project_id)?;
    document(c, id)
}

pub fn touch_project(c: &Connection, id: Id) -> Result<()> {
    c.execute("UPDATE projects SET updated_at = ?1 WHERE id = ?2", params![now_ms(), id])?;
    Ok(())
}

pub fn patch_document(c: &Connection, id: Id, p: &DocumentPatch) -> Result<Document> {
    let mut d = document(c, id)?;
    if let Some(t) = &p.title { d.title = t.clone(); }
    if let Some(t) = &p.tags { d.tags = t.clone(); }
    if let Some(v) = p.pinned { d.pinned = v; }
    let moved = matches!(p.project_id, Some(pid) if pid != d.project_id);
    if let Some(pid) = p.project_id {
        project(c, pid)?;
        d.project_id = pid;
    }
    c.execute(
        "UPDATE documents SET title=?1, tags=?2, pinned=?3, project_id=?4, updated_at=?5 WHERE id=?6",
        params![d.title, serde_json::to_string(&d.tags)?, d.pinned, d.project_id, now_ms(), id],
    )?;
    if moved {
        c.execute("UPDATE chunks SET project_id = ?1 WHERE document_id = ?2", params![d.project_id, id])?;
        // Vector partitions are keyed by project; the caller re-embeds.
        crate::search::delete_vectors_for_document(c, id)?;
    }
    document(c, id)
}

pub fn set_document_status(
    c: &Connection,
    id: Id,
    status: DocStatus,
    error: Option<&str>,
    notice: Option<&str>,
) -> Result<()> {
    c.execute(
        "UPDATE documents SET status=?1, error=?2, notice=?3, updated_at=?4 WHERE id=?5",
        params![status.as_str(), error, notice, now_ms(), id],
    )?;
    Ok(())
}

pub fn set_document_notice(c: &Connection, id: Id, notice: Option<&str>) -> Result<()> {
    c.execute("UPDATE documents SET notice=?1 WHERE id=?2", params![notice, id])?;
    Ok(())
}

pub fn set_document_extracted(c: &Connection, id: Id, page_count: Option<i64>, title: Option<&str>) -> Result<()> {
    c.execute(
        "UPDATE documents SET page_count = COALESCE(?1, page_count),
            title = CASE WHEN ?2 IS NOT NULL AND source_kind NOT IN ('note') AND title = original_name THEN ?2 ELSE title END
         WHERE id = ?3",
        params![page_count, title, id],
    )?;
    Ok(())
}

pub fn delete_document(c: &Connection, id: Id) -> Result<Document> {
    let d = document(c, id)?;
    crate::in_tx(c, |tx| {
        crate::search::delete_vectors_for_document(tx, id)?;
        tx.execute("DELETE FROM jobs WHERE document_id = ?1", params![id])?;
        tx.execute("DELETE FROM documents WHERE id = ?1", params![id])?;
        Ok(())
    })?;
    Ok(d)
}

/// Replaces the blob behind a document (used when a note is edited).
pub fn replace_document_blob(c: &Connection, id: Id, blob_id: i64, size: i64, title: &str, tags: &[String]) -> Result<()> {
    c.execute(
        "UPDATE documents SET blob_id=?1, size=?2, title=?3, original_name=?3, tags=?4, status='queued', error=NULL, updated_at=?5 WHERE id=?6",
        params![blob_id, size, title, serde_json::to_string(tags)?, now_ms(), id],
    )?;
    Ok(())
}

// ------------------------------------------------------------------ chunks

pub struct NewChunk {
    pub text: String,
    pub context: String,
    pub anchor: Anchor,
    pub kind: String,
}

/// Replaces all chunks of a document atomically. Returns the new chunk ids in order.
pub fn replace_chunks(c: &Connection, document_id: Id, chunks: &[NewChunk]) -> Result<Vec<Id>> {
    crate::in_tx(c, |tx| replace_chunks_tx(tx, document_id, chunks))
}

fn replace_chunks_tx(tx: &Connection, document_id: Id, chunks: &[NewChunk]) -> Result<Vec<Id>> {
    let project_id: Id = tx.query_row("SELECT project_id FROM documents WHERE id = ?1", params![document_id], |r| r.get(0))?;
    crate::search::delete_vectors_for_document(tx, document_id)?;
    tx.execute("DELETE FROM chunks WHERE document_id = ?1", params![document_id])?;
    let mut ids = Vec::with_capacity(chunks.len());
    {
        let mut st = tx.prepare(
            "INSERT INTO chunks(document_id, project_id, ord, text, context, anchor, kind) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (i, ch) in chunks.iter().enumerate() {
            st.execute(params![
                document_id, project_id, i as i64, ch.text, ch.context,
                serde_json::to_string(&ch.anchor)?, ch.kind
            ])?;
            ids.push(tx.last_insert_rowid());
        }
    }
    tx.execute("UPDATE documents SET chunk_count = ?1 WHERE id = ?2", params![chunks.len() as i64, document_id])?;
    Ok(ids)
}

fn chunk_row(r: &Row) -> rusqlite::Result<Chunk> {
    Ok(Chunk {
        id: r.get(0)?,
        document_id: r.get(1)?,
        ord: r.get(2)?,
        text: r.get(3)?,
        anchor: json(r.get(4)?),
        kind: r.get(5)?,
    })
}

pub fn chunks_for_document(c: &Connection, document_id: Id) -> Result<Vec<Chunk>> {
    let mut st = c.prepare("SELECT id, document_id, ord, text, anchor, kind FROM chunks WHERE document_id = ?1 ORDER BY ord")?;
    let rows = st.query_map(params![document_id], chunk_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// (chunk_id, project_id, embed text) for every chunk of a document.
pub fn chunks_for_embedding(c: &Connection, document_id: Id) -> Result<Vec<(Id, Id, String)>> {
    let mut st = c.prepare(
        "SELECT id, project_id, CASE WHEN context = '' THEN text ELSE context || char(10) || text END
         FROM chunks WHERE document_id = ?1 ORDER BY ord",
    )?;
    let rows = st.query_map(params![document_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Rich chunk info used to build citations.
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub chunk_id: Id,
    pub document_id: Id,
    pub project_id: Id,
    pub ord: i64,
    pub text: String,
    pub anchor: Anchor,
    pub title: String,
    pub project_name: String,
    pub mime: String,
    pub source_kind: String,
}

pub fn chunk_infos(c: &Connection, ids: &[Id]) -> Result<Vec<ChunkInfo>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut st = c.prepare_cached(
        "SELECT ch.id, ch.document_id, ch.project_id, ch.ord, ch.text, ch.anchor, d.title, p.name, d.mime, d.source_kind
         FROM chunks ch JOIN documents d ON d.id = ch.document_id JOIN projects p ON p.id = ch.project_id
         WHERE ch.id = ?1",
    )?;
    for id in ids {
        if let Some(info) = st
            .query_row(params![id], |r| {
                Ok(ChunkInfo {
                    chunk_id: r.get(0)?,
                    document_id: r.get(1)?,
                    project_id: r.get(2)?,
                    ord: r.get(3)?,
                    text: r.get(4)?,
                    anchor: json(r.get(5)?),
                    title: r.get(6)?,
                    project_name: r.get(7)?,
                    mime: r.get(8)?,
                    source_kind: r.get(9)?,
                })
            })
            .optional()?
        {
            out.push(info);
        }
    }
    Ok(out)
}

pub fn chunk_by_ord(c: &Connection, document_id: Id, ord: i64) -> Result<Option<(Id, String)>> {
    Ok(c.query_row(
        "SELECT id, text FROM chunks WHERE document_id = ?1 AND ord = ?2",
        params![document_id, ord],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

pub fn document_text(c: &Connection, document_id: Id, max_chars: usize) -> Result<String> {
    let mut out = String::new();
    for ch in chunks_for_document(c, document_id)? {
        if out.len() + ch.text.len() > max_chars {
            break;
        }
        out.push_str(&ch.text);
        out.push_str("\n\n");
    }
    Ok(out)
}

// ------------------------------------------------------------------ chats & messages

fn chat_row(r: &Row) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: r.get(0)?,
        title: r.get(1)?,
        scope: json(r.get(2)?),
        model: r.get::<_, Option<String>>(3)?.and_then(|s| serde_json::from_str(&s).ok()),
        mode: r.get::<_, Option<String>>(4)?.and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok()).unwrap_or_default(),
        pinned: r.get(5)?,
        message_count: r.get(6)?,
        last_message: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

const CHAT_COLS: &str = "c.id, c.title, c.scope, c.model, c.mode, c.pinned,
    (SELECT COUNT(*) FROM messages m WHERE m.chat_id = c.id),
    (SELECT substr(m.content, 1, 160) FROM messages m WHERE m.chat_id = c.id ORDER BY m.id DESC LIMIT 1),
    c.created_at, c.updated_at";

pub fn chats(c: &Connection, limit: i64) -> Result<Vec<Chat>> {
    let mut st = c.prepare(&format!(
        "SELECT {CHAT_COLS} FROM chats c ORDER BY c.pinned DESC, c.updated_at DESC LIMIT ?1"
    ))?;
    let rows = st.query_map(params![limit], chat_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn chat(c: &Connection, id: Id) -> Result<Chat> {
    c.query_row(&format!("SELECT {CHAT_COLS} FROM chats c WHERE c.id = ?1"), params![id], chat_row)
        .optional()?
        .ok_or(VaultError::Missing)
}

fn mode_str(m: ChatMode) -> &'static str {
    match m {
        ChatMode::Rag => "rag",
        ChatMode::ChatOnly => "chat_only",
    }
}

pub fn create_chat(c: &Connection, input: &ChatInput) -> Result<Chat> {
    let now = now_ms();
    let model = match &input.model {
        Some(Some(m)) => Some(serde_json::to_string(m)?),
        _ => None,
    };
    c.execute(
        "INSERT INTO chats(title, scope, model, mode, pinned, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            input.title.clone().unwrap_or_else(|| "New chat".into()),
            serde_json::to_string(&input.scope.clone().unwrap_or_default())?,
            model,
            mode_str(input.mode.unwrap_or_default()),
            input.pinned.unwrap_or(false),
            now
        ],
    )?;
    chat(c, c.last_insert_rowid())
}

pub fn update_chat(c: &Connection, id: Id, input: &ChatInput) -> Result<Chat> {
    let mut ch = chat(c, id)?;
    if let Some(t) = &input.title { ch.title = t.clone(); }
    if let Some(s) = &input.scope { ch.scope = s.clone(); }
    if let Some(m) = &input.model { ch.model = m.clone(); }
    if let Some(m) = input.mode { ch.mode = m; }
    if let Some(p) = input.pinned { ch.pinned = p; }
    c.execute(
        "UPDATE chats SET title=?1, scope=?2, model=?3, mode=?4, pinned=?5 WHERE id=?6",
        params![
            ch.title,
            serde_json::to_string(&ch.scope)?,
            ch.model.as_ref().map(serde_json::to_string).transpose()?,
            mode_str(ch.mode),
            ch.pinned,
            id
        ],
    )?;
    chat(c, id)
}

pub fn touch_chat(c: &Connection, id: Id) -> Result<()> {
    c.execute("UPDATE chats SET updated_at = ?1 WHERE id = ?2", params![now_ms(), id])?;
    Ok(())
}

pub fn delete_chat(c: &Connection, id: Id) -> Result<()> {
    crate::recall::delete_chat_vectors(c, id)?;
    c.execute("DELETE FROM chats WHERE id = ?1", params![id])?;
    Ok(())
}

fn message_row(r: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        attachments: json(r.get(4)?),
        citations: json(r.get(5)?),
        model: r.get(6)?,
        error: r.get(7)?,
        tokens_in: r.get(8)?,
        tokens_out: r.get(9)?,
        created_at: r.get(10)?,
    })
}

const MSG_COLS: &str = "id, chat_id, role, content, attachments, citations, model, error, tokens_in, tokens_out, created_at";

pub fn messages(c: &Connection, chat_id: Id) -> Result<Vec<Message>> {
    let mut st = c.prepare(&format!("SELECT {MSG_COLS} FROM messages WHERE chat_id = ?1 ORDER BY id"))?;
    let rows = st.query_map(params![chat_id], message_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn message(c: &Connection, id: Id) -> Result<Message> {
    c.query_row(&format!("SELECT {MSG_COLS} FROM messages WHERE id = ?1"), params![id], message_row)
        .optional()?
        .ok_or(VaultError::Missing)
}

pub struct NewMessage<'a> {
    pub chat_id: Id,
    pub role: &'a str,
    pub content: &'a str,
    pub attachments: &'a [Attachment],
    pub citations: &'a [Citation],
    pub model: Option<&'a str>,
    pub error: Option<&'a str>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

pub fn add_message(c: &Connection, m: &NewMessage) -> Result<Message> {
    c.execute(
        "INSERT INTO messages(chat_id, role, content, attachments, citations, model, error, tokens_in, tokens_out, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            m.chat_id, m.role, m.content,
            serde_json::to_string(m.attachments)?, serde_json::to_string(m.citations)?,
            m.model, m.error, m.tokens_in, m.tokens_out, now_ms()
        ],
    )?;
    let id = c.last_insert_rowid();
    touch_chat(c, m.chat_id)?;
    message(c, id)
}

pub fn delete_messages_from(c: &Connection, chat_id: Id, from_id: Id) -> Result<usize> {
    // A summary that covered the removed messages no longer describes the chat.
    c.execute("UPDATE chats SET summary = '', summary_upto = 0 WHERE id = ?1 AND summary_upto >= ?2", params![chat_id, from_id])?;
    Ok(c.execute("DELETE FROM messages WHERE chat_id = ?1 AND id >= ?2", params![chat_id, from_id])?)
}

pub fn delete_message(c: &Connection, id: Id) -> Result<()> {
    c.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
    Ok(())
}

// ------------------------------------------------------------------ memories

fn memory_row(r: &Row) -> rusqlite::Result<Memory> {
    Ok(Memory { id: r.get(0)?, text: r.get(1)?, enabled: r.get(2)?, created_at: r.get(3)? })
}

pub fn memories(c: &Connection) -> Result<Vec<Memory>> {
    let mut st = c.prepare("SELECT id, text, enabled, created_at FROM memories ORDER BY id DESC")?;
    let rows = st.query_map([], memory_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn create_memory(c: &Connection, text: &str) -> Result<Memory> {
    c.execute("INSERT INTO memories(text, created_at) VALUES (?1, ?2)", params![text.trim(), now_ms()])?;
    let id = c.last_insert_rowid();
    Ok(c.query_row("SELECT id, text, enabled, created_at FROM memories WHERE id = ?1", params![id], memory_row)?)
}

pub fn update_memory(c: &Connection, id: Id, input: &MemoryInput) -> Result<Memory> {
    if let Some(t) = &input.text {
        c.execute("UPDATE memories SET text = ?1 WHERE id = ?2", params![t.trim(), id])?;
    }
    if let Some(e) = input.enabled {
        c.execute("UPDATE memories SET enabled = ?1 WHERE id = ?2", params![e, id])?;
    }
    c.query_row("SELECT id, text, enabled, created_at FROM memories WHERE id = ?1", params![id], memory_row)
        .optional()?
        .ok_or(VaultError::Missing)
}

pub fn delete_memory(c: &Connection, id: Id) -> Result<()> {
    c.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
    Ok(())
}

// ------------------------------------------------------------------ providers

/// Provider including secrets; never leaves the server.
#[derive(Debug, Clone)]
pub struct ProviderSecret {
    pub id: Id,
    pub kind: ProviderKind,
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    pub created_at: Ts,
}

impl ProviderSecret {
    pub fn public(&self) -> Provider {
        Provider {
            id: self.id,
            kind: self.kind,
            name: self.name.clone(),
            base_url: self.base_url.clone(),
            has_api_key: self.api_key.as_deref().is_some_and(|k| !k.is_empty()),
            header_names: self.headers.keys().cloned().collect(),
            enabled: self.enabled,
            created_at: self.created_at,
        }
    }
}

fn provider_row(r: &Row) -> rusqlite::Result<ProviderSecret> {
    Ok(ProviderSecret {
        id: r.get(0)?,
        kind: ProviderKind::parse(&r.get::<_, String>(1)?).unwrap_or(ProviderKind::Openai),
        name: r.get(2)?,
        base_url: r.get(3)?,
        api_key: r.get(4)?,
        headers: json(r.get(5)?),
        enabled: r.get(6)?,
        created_at: r.get(7)?,
    })
}

const PROVIDER_COLS: &str = "id, kind, name, base_url, api_key, headers, enabled, created_at";

pub fn providers(c: &Connection) -> Result<Vec<ProviderSecret>> {
    let mut st = c.prepare(&format!("SELECT {PROVIDER_COLS} FROM providers ORDER BY id"))?;
    let rows = st.query_map([], provider_row)?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn provider(c: &Connection, id: Id) -> Result<ProviderSecret> {
    c.query_row(&format!("SELECT {PROVIDER_COLS} FROM providers WHERE id = ?1"), params![id], provider_row)
        .optional()?
        .ok_or(VaultError::Missing)
}

pub fn create_provider(c: &Connection, p: &ProviderInput) -> Result<ProviderSecret> {
    let kind = p.kind.ok_or_else(|| VaultError::Invalid("provider kind is required".into()))?;
    let base_url = p.base_url.clone().unwrap_or_default().trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        return Err(VaultError::Invalid("base URL is required".into()));
    }
    c.execute(
        &format!("INSERT INTO providers(kind, name, base_url, api_key, headers, enabled, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
        params![
            kind.as_str(),
            p.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| kind.as_str().to_string()),
            base_url,
            p.api_key.clone().filter(|k| !k.is_empty()),
            serde_json::to_string(&p.headers.clone().unwrap_or_default())?,
            p.enabled.unwrap_or(true),
            now_ms()
        ],
    )?;
    provider(c, c.last_insert_rowid())
}

pub fn update_provider(c: &Connection, id: Id, p: &ProviderInput) -> Result<ProviderSecret> {
    let mut cur = provider(c, id)?;
    if let Some(k) = p.kind { cur.kind = k; }
    if let Some(n) = &p.name { cur.name = n.clone(); }
    if let Some(u) = &p.base_url { cur.base_url = u.trim().trim_end_matches('/').to_string(); }
    if let Some(k) = &p.api_key { cur.api_key = if k.is_empty() { None } else { Some(k.clone()) }; }
    if let Some(h) = &p.headers { cur.headers = h.clone(); }
    if let Some(e) = p.enabled { cur.enabled = e; }
    c.execute(
        "UPDATE providers SET kind=?1, name=?2, base_url=?3, api_key=?4, headers=?5, enabled=?6 WHERE id=?7",
        params![cur.kind.as_str(), cur.name, cur.base_url, cur.api_key, serde_json::to_string(&cur.headers)?, cur.enabled, id],
    )?;
    provider(c, id)
}

pub fn delete_provider(c: &Connection, id: Id) -> Result<()> {
    c.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
    Ok(())
}

// ------------------------------------------------------------------ settings

pub fn settings(c: &Connection) -> Result<Settings> {
    let raw: Option<String> = c.query_row("SELECT value FROM settings WHERE key = 'settings'", [], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

pub fn save_settings(c: &Connection, s: &Settings) -> Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO settings(key, value) VALUES ('settings', ?1)",
        params![serde_json::to_string(s)?],
    )?;
    Ok(())
}

/// Small per-vault app data (browser favourites, saved SSH hosts…), JSON under `app:<key>`.
pub fn app_data(c: &Connection, key: &str) -> Result<serde_json::Value> {
    let raw: Option<String> = c.query_row("SELECT value FROM settings WHERE key = ?1", params![format!("app:{key}")], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(serde_json::Value::Null))
}

pub fn set_app_data(c: &Connection, key: &str, v: &serde_json::Value) -> Result<()> {
    c.execute("INSERT OR REPLACE INTO settings(key, value) VALUES (?1, ?2)", params![format!("app:{key}"), serde_json::to_string(v)?])?;
    Ok(())
}

// ------------------------------------------------------------------ jobs

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Id,
    pub kind: String,
    pub document_id: Option<Id>,
    pub payload: serde_json::Value,
    pub attempts: i64,
}

pub fn enqueue_job(c: &Connection, kind: &str, document_id: Option<Id>, payload: serde_json::Value) -> Result<Id> {
    // Collapse duplicates: one pending job per (kind, document).
    if let Some(doc) = document_id {
        if let Some(id) = c
            .query_row(
                "SELECT id FROM jobs WHERE kind = ?1 AND document_id = ?2 AND state = 'queued'",
                params![kind, doc],
                |r| r.get::<_, Id>(0),
            )
            .optional()?
        {
            c.execute("UPDATE jobs SET run_after = 0, payload = ?1 WHERE id = ?2", params![payload.to_string(), id])?;
            return Ok(id);
        }
    }
    let now = now_ms();
    c.execute(
        "INSERT INTO jobs(kind, document_id, payload, state, created_at, updated_at) VALUES (?1, ?2, ?3, 'queued', ?4, ?4)",
        params![kind, document_id, payload.to_string(), now],
    )?;
    Ok(c.last_insert_rowid())
}

/// Atomically claims the next runnable job.
/// Takes the next due job for this computer (`owner`). The write transaction makes this safe
/// when several computers share the vault: each job is claimed once.
pub fn claim_job(c: &Connection, owner: &str) -> Result<Option<Job>> {
    crate::in_tx(c, |tx| claim_job_tx(tx, owner))
}

fn claim_job_tx(tx: &Connection, owner: &str) -> Result<Option<Job>> {
    let job = tx
        .query_row(
            "SELECT id, kind, document_id, payload, attempts FROM jobs
             WHERE state = 'queued' AND run_after <= ?1 ORDER BY id LIMIT 1",
            params![now_ms()],
            |r| {
                Ok(Job {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    document_id: r.get(2)?,
                    payload: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                    attempts: r.get(4)?,
                })
            },
        )
        .optional()?;
    if let Some(j) = &job {
        tx.execute(
            "UPDATE jobs SET state = 'running', attempts = attempts + 1, updated_at = ?1, claimed_by = ?3 WHERE id = ?2",
            params![now_ms(), j.id, owner],
        )?;
    }
    Ok(job)
}

pub fn complete_job(c: &Connection, id: Id) -> Result<()> {
    c.execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn retry_job(c: &Connection, id: Id, delay_ms: i64, error: &str) -> Result<()> {
    c.execute(
        "UPDATE jobs SET state = 'queued', run_after = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
        params![now_ms() + delay_ms, error, now_ms(), id],
    )?;
    Ok(())
}

pub fn fail_job(c: &Connection, id: Id, error: &str) -> Result<()> {
    c.execute(
        "UPDATE jobs SET state = 'failed', error = ?1, updated_at = ?2 WHERE id = ?3",
        params![error, now_ms(), id],
    )?;
    Ok(())
}

/// After a crash or restart, jobs that were mid-flight go back to the queue — except those
/// another computer sharing the vault (`alive`) is still working on.
pub fn requeue_running_jobs(c: &Connection, owner: &str, alive: &[String]) -> Result<usize> {
    let mut n = 0;
    let mut st = c.prepare("SELECT id, claimed_by FROM jobs WHERE state = 'running'")?;
    let rows: Vec<(Id, Option<String>)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
    for (id, by) in rows {
        let theirs = by.as_deref().is_some_and(|b| b != owner && alive.iter().any(|a| a == b));
        if !theirs {
            n += c.execute("UPDATE jobs SET state = 'queued', claimed_by = NULL WHERE id = ?1 AND state = 'running'", params![id])?;
        }
    }
    Ok(n)
}

pub fn next_job_due(c: &Connection) -> Result<Option<i64>> {
    Ok(c.query_row("SELECT MIN(run_after) FROM jobs WHERE state = 'queued'", [], |r| r.get(0))?)
}

pub fn job_summary(c: &Connection) -> Result<JobSummary> {
    Ok(c.query_row(
        "SELECT
            COALESCE(SUM(state = 'queued'), 0),
            COALESCE(SUM(state = 'running'), 0),
            COALESCE(SUM(state = 'failed'), 0)
         FROM jobs",
        [],
        |r| Ok(JobSummary { queued: r.get(0)?, running: r.get(1)?, failed: r.get(2)? }),
    )?)
}

// ------------------------------------------------------------------ stats

pub struct Stats {
    pub projects: i64,
    pub documents: i64,
    pub notes: i64,
    pub chunks: i64,
    pub chats: i64,
}

pub fn stats(c: &Connection) -> Result<Stats> {
    Ok(c.query_row(
        "SELECT (SELECT COUNT(*) FROM projects),
                (SELECT COUNT(*) FROM documents WHERE source_kind != 'note'),
                (SELECT COUNT(*) FROM documents WHERE source_kind = 'note'),
                (SELECT COUNT(*) FROM chunks),
                (SELECT COUNT(*) FROM chats)",
        [],
        |r| Ok(Stats { projects: r.get(0)?, documents: r.get(1)?, notes: r.get(2)?, chunks: r.get(3)?, chats: r.get(4)? }),
    )?)
}
