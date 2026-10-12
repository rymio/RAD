//! Chat recall: past conversations, split into question-and-answer passages, searchable by
//! keyword (FTS5) and by meaning (sqlite-vec, one table per embedding space next to the
//! document vectors). Also the running summaries of long chats.

use std::collections::HashMap;

use rad_api::{Id, Scope, Ts};
use rusqlite::{params, Connection, OptionalExtension};

use crate::search::{fts_query, spaces, Space};
use crate::{now_ms, Result};

pub(crate) fn vec_table(space: i64) -> String {
    format!("chatvec_{space}")
}

fn vec_exists(c: &Connection, space: i64) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)", params![vec_table(space)], |r| r.get(0))?)
}

pub fn ensure_vectors(c: &Connection, space: Space) -> Result<()> {
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {} USING vec0(
            chunk_id integer primary key,
            embedding float[{}] distance_metric=cosine
        );",
        vec_table(space.id),
        space.dims
    ))?;
    Ok(())
}

/// Replaces a chat's passages with `chunks` — (message id, text) in order — keeping the
/// unchanged beginning and its vectors. Edits only ever change the end of a chat.
/// Returns whether anything changed.
pub fn replace_chunks(c: &Connection, chat_id: Id, chunks: &[(Id, String)]) -> Result<bool> {
    crate::in_tx(c, |tx| {
        let existing: Vec<(Id, Id, String)> = {
            let mut st = tx.prepare("SELECT id, message_id, text FROM chat_chunks WHERE chat_id = ?1 ORDER BY ord")?;
            let rows = st.query_map(params![chat_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let keep = existing.iter().zip(chunks).take_while(|(e, n)| e.1 == n.0 && e.2 == n.1).count();
        if keep == existing.len() && keep == chunks.len() {
            return Ok(false);
        }
        let all_spaces = spaces(tx)?;
        for (id, _, _) in &existing[keep..] {
            for (s, _) in &all_spaces {
                if vec_exists(tx, s.id)? {
                    tx.execute(&format!("DELETE FROM {} WHERE chunk_id = ?1", vec_table(s.id)), params![id])?;
                }
            }
            tx.execute("DELETE FROM chat_chunks WHERE id = ?1", params![id])?;
        }
        let now = now_ms();
        let mut ins = tx.prepare("INSERT INTO chat_chunks(chat_id, ord, message_id, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for (ord, (message_id, text)) in chunks.iter().enumerate().skip(keep) {
            ins.execute(params![chat_id, ord as i64, message_id, text, now])?;
        }
        Ok(true)
    })
}

/// Passages of a chat that have no vector in `space` yet.
pub fn missing_vectors(c: &Connection, space: Space, chat_id: Id) -> Result<Vec<(Id, String)>> {
    let sql = if vec_exists(c, space.id)? {
        format!("SELECT id, text FROM chat_chunks WHERE chat_id = ?1 AND id NOT IN (SELECT chunk_id FROM {}) ORDER BY ord", vec_table(space.id))
    } else {
        "SELECT id, text FROM chat_chunks WHERE chat_id = ?1 ORDER BY ord".to_string()
    };
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![chat_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn insert_vectors(c: &Connection, space: Space, rows: &[(Id, Vec<f32>)]) -> Result<()> {
    ensure_vectors(c, space)?;
    crate::in_tx(c, |tx| {
        let t = vec_table(space.id);
        let mut del = tx.prepare(&format!("DELETE FROM {t} WHERE chunk_id = ?1"))?;
        let mut ins = tx.prepare(&format!("INSERT INTO {t}(chunk_id, embedding) VALUES (?1, ?2)"))?;
        for (id, v) in rows {
            if v.len() != space.dims {
                continue;
            }
            del.execute(params![id])?;
            ins.execute(params![id, crate::search::to_blob(v)])?;
        }
        Ok(())
    })
}

pub fn delete_chat_vectors(c: &Connection, chat_id: Id) -> Result<()> {
    for (s, _) in spaces(c)? {
        if vec_exists(c, s.id)? {
            c.execute(
                &format!("DELETE FROM {} WHERE chunk_id IN (SELECT id FROM chat_chunks WHERE chat_id = ?1)", vec_table(s.id)),
                params![chat_id],
            )?;
        }
    }
    Ok(())
}

/// (passage id, bm25 score) best first.
pub fn fts_search(c: &Connection, text: &str, limit: usize) -> Result<Vec<(Id, f32)>> {
    let Some(q) = fts_query(text) else { return Ok(Vec::new()) };
    let mut st = c.prepare(
        "SELECT rowid, bm25(chat_chunks_fts) AS score FROM chat_chunks_fts WHERE chat_chunks_fts MATCH ?1 ORDER BY score LIMIT ?2",
    )?;
    let rows = st.query_map(params![q, limit as i64], |r| Ok((r.get::<_, Id>(0)?, -(r.get::<_, f64>(1)? as f32))))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Nearest passages: (passage id, cosine distance) best first.
pub fn knn(c: &Connection, space: Space, query: &[f32], k: usize) -> Result<Vec<(Id, f32)>> {
    if query.len() != space.dims || !vec_exists(c, space.id)? {
        return Ok(Vec::new());
    }
    let mut st = c.prepare(&format!(
        "SELECT chunk_id, distance FROM {} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
        vec_table(space.id)
    ))?;
    let rows = st.query_map(params![crate::search::to_blob(query), k as i64], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn vectors(c: &Connection, space: Space, ids: &[Id]) -> Result<HashMap<Id, Vec<f32>>> {
    let mut out = HashMap::new();
    if !vec_exists(c, space.id)? {
        return Ok(out);
    }
    let mut st = c.prepare(&format!("SELECT embedding FROM {} WHERE chunk_id = ?1", vec_table(space.id)))?;
    for id in ids {
        if let Some(b) = st.query_row(params![id], |r| r.get::<_, Vec<u8>>(0)).optional()? {
            out.insert(*id, crate::search::from_blob(&b));
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ChatChunkInfo {
    pub chunk_id: Id,
    pub chat_id: Id,
    pub ord: i64,
    pub message_id: Id,
    pub text: String,
    pub created_at: Ts,
    pub chat_title: String,
    pub scope: Scope,
}

pub fn chunk_infos(c: &Connection, ids: &[Id]) -> Result<Vec<ChatChunkInfo>> {
    let mut st = c.prepare_cached(
        "SELECT cc.id, cc.chat_id, cc.ord, cc.message_id, cc.text, cc.created_at, ch.title, ch.scope
         FROM chat_chunks cc JOIN chats ch ON ch.id = cc.chat_id WHERE cc.id = ?1",
    )?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let row = st
            .query_row(params![id], |r| {
                Ok(ChatChunkInfo {
                    chunk_id: r.get(0)?,
                    chat_id: r.get(1)?,
                    ord: r.get(2)?,
                    message_id: r.get(3)?,
                    text: r.get(4)?,
                    created_at: r.get(5)?,
                    chat_title: r.get(6)?,
                    scope: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                })
            })
            .optional()?;
        out.extend(row);
    }
    Ok(out)
}

/// Chats with at least one exchange but no passages yet (e.g. from before chat recall existed).
pub fn unindexed_chats(c: &Connection) -> Result<Vec<Id>> {
    let mut st = c.prepare(
        "SELECT ch.id FROM chats ch WHERE (SELECT COUNT(*) FROM messages m WHERE m.chat_id = ch.id) >= 2
         AND NOT EXISTS (SELECT 1 FROM chat_chunks cc WHERE cc.chat_id = ch.id)",
    )?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn chats_with_passages(c: &Connection) -> Result<Vec<Id>> {
    let mut st = c.prepare("SELECT DISTINCT chat_id FROM chat_chunks")?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The running summary of a chat's older messages and the last message id it covers.
pub fn summary(c: &Connection, chat_id: Id) -> Result<(String, Id)> {
    Ok(c
        .query_row("SELECT summary, summary_upto FROM chats WHERE id = ?1", params![chat_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .unwrap_or_default())
}

pub fn set_summary(c: &Connection, chat_id: Id, summary: &str, upto: Id) -> Result<()> {
    c.execute("UPDATE chats SET summary = ?1, summary_upto = ?2 WHERE id = ?3", params![summary, upto, chat_id])?;
    Ok(())
}
