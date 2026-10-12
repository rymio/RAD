//! Keyword (FTS5/BM25) and vector (sqlite-vec) search inside the vault.

use std::collections::HashMap;

use rad_api::Id;
use rusqlite::{params, Connection, OptionalExtension};

use crate::{now_ms, Result};

/// Turns free text into a safe FTS5 query: every term quoted, OR-ed together, with
/// prefix matching on longer terms.
pub fn fts_query(text: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        let t = raw.trim();
        if t.chars().count() < 2 || STOP.contains(&t.to_lowercase().as_str()) {
            continue;
        }
        let q = if t.chars().count() >= 4 { format!("\"{t}\"*") } else { format!("\"{t}\"") };
        if !terms.contains(&q) {
            terms.push(q);
        }
        if terms.len() >= 24 {
            break;
        }
    }
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "are", "was", "were", "be", "it",
    "this", "that", "with", "as", "at", "by", "from", "what", "which", "who", "how", "when", "where", "why",
    "do", "does", "did", "my", "me", "i", "you", "your", "can", "could", "would", "should", "about", "tell",
    "please", "show", "give", "find", "any", "there", "have", "has", "had",
];

/// Returns (chunk_id, bm25 score) best first. Lower bm25 is better; we negate it.
pub fn fts_search(c: &Connection, text: &str, projects: &[Id], limit: usize) -> Result<Vec<(Id, f32)>> {
    let Some(q) = fts_query(text) else { return Ok(Vec::new()) };
    let filter = project_filter("ch.project_id", projects);
    let sql = format!(
        "SELECT ch.id, bm25(chunks_fts, 1.0, 0.4) AS score
         FROM chunks_fts JOIN chunks ch ON ch.id = chunks_fts.rowid
         WHERE chunks_fts MATCH ?1 {filter}
         ORDER BY score LIMIT ?2"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![q, limit as i64], |r| Ok((r.get::<_, Id>(0)?, -(r.get::<_, f64>(1)? as f32))))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn project_filter(col: &str, projects: &[Id]) -> String {
    if projects.is_empty() {
        String::new()
    } else {
        let ids: Vec<String> = projects.iter().map(|p| p.to_string()).collect();
        format!("AND {col} IN ({})", ids.join(","))
    }
}

// ------------------------------------------------------------------ embedding spaces

#[derive(Debug, Clone, Copy)]
pub struct Space {
    pub id: i64,
    pub dims: usize,
}

fn vec_table(space: i64) -> String {
    format!("vec_{space}")
}

pub fn space(c: &Connection, model_key: &str) -> Result<Option<Space>> {
    Ok(c.query_row(
        "SELECT id, dims FROM embedding_spaces WHERE model_key = ?1",
        params![model_key],
        |r| Ok(Space { id: r.get(0)?, dims: r.get::<_, i64>(1)? as usize }),
    )
    .optional()?)
}

pub fn ensure_space(c: &Connection, model_key: &str, dims: usize) -> Result<Space> {
    if let Some(s) = space(c, model_key)? {
        if s.dims == dims {
            return Ok(s);
        }
        // The model changed its output size: rebuild the space from scratch.
        drop_space(c, s.id)?;
    }
    c.execute(
        "INSERT INTO embedding_spaces(model_key, dims, created_at) VALUES (?1, ?2, ?3)",
        params![model_key, dims as i64, now_ms()],
    )?;
    let id = c.last_insert_rowid();
    c.execute_batch(&format!(
        "CREATE VIRTUAL TABLE {} USING vec0(
            chunk_id integer primary key,
            embedding float[{dims}] distance_metric=cosine,
            project_id integer partition key
        );",
        vec_table(id)
    ))?;
    Ok(Space { id, dims })
}

pub fn drop_space(c: &Connection, id: i64) -> Result<()> {
    c.execute_batch(&format!("DROP TABLE IF EXISTS {};", vec_table(id)))?;
    c.execute_batch(&format!("DROP TABLE IF EXISTS {};", crate::recall::vec_table(id)))?;
    c.execute("DELETE FROM embedded WHERE space_id = ?1", params![id])?;
    c.execute("DELETE FROM embedding_spaces WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn spaces(c: &Connection) -> Result<Vec<(Space, String)>> {
    let mut st = c.prepare("SELECT id, dims, model_key FROM embedding_spaces ORDER BY id")?;
    let rows = st.query_map([], |r| Ok((Space { id: r.get(0)?, dims: r.get::<_, i64>(1)? as usize }, r.get(2)?)))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub(crate) fn to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

pub(crate) fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

pub fn insert_vectors(c: &Connection, space: Space, rows: &[(Id, Id, Vec<f32>)]) -> Result<()> {
    crate::in_tx(c, |tx| {
        let t = vec_table(space.id);
        let mut del = tx.prepare(&format!("DELETE FROM {t} WHERE chunk_id = ?1"))?;
        let mut ins = tx.prepare(&format!("INSERT INTO {t}(chunk_id, embedding, project_id) VALUES (?1, ?2, ?3)"))?;
        for (chunk_id, project_id, v) in rows {
            if v.len() != space.dims {
                continue;
            }
            del.execute(params![chunk_id])?;
            ins.execute(params![chunk_id, to_blob(v), project_id])?;
        }
        Ok(())
    })
}

pub fn mark_embedded(c: &Connection, space: Space, document_id: Id) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO embedded(space_id, document_id) VALUES (?1, ?2)",
        params![space.id, document_id],
    )?;
    Ok(())
}

pub fn is_embedded(c: &Connection, space: Space, document_id: Id) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM embedded WHERE space_id = ?1 AND document_id = ?2)",
        params![space.id, document_id],
        |r| r.get(0),
    )?)
}

pub fn embedded_chunk_count(c: &Connection, space: Space) -> Result<i64> {
    Ok(c.query_row(&format!("SELECT COUNT(*) FROM {}", vec_table(space.id)), [], |r| r.get(0))?)
}

pub fn documents_missing_embeddings(c: &Connection, space: Space) -> Result<Vec<Id>> {
    let mut st = c.prepare(
        "SELECT d.id FROM documents d WHERE d.chunk_count > 0 AND d.status = 'ready'
         AND NOT EXISTS (SELECT 1 FROM embedded e WHERE e.space_id = ?1 AND e.document_id = d.id)",
    )?;
    let rows = st.query_map(params![space.id], |r| r.get(0))?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn delete_vectors_for_document(c: &Connection, document_id: Id) -> Result<()> {
    for (s, _) in spaces(c)? {
        c.execute(
            &format!(
                "DELETE FROM {} WHERE chunk_id IN (SELECT id FROM chunks WHERE document_id = ?1)",
                vec_table(s.id)
            ),
            params![document_id],
        )?;
        c.execute("DELETE FROM embedded WHERE space_id = ?1 AND document_id = ?2", params![s.id, document_id])?;
    }
    Ok(())
}

/// Nearest neighbours: (chunk_id, cosine distance) best first.
pub fn knn(c: &Connection, space: Space, query: &[f32], k: usize, projects: &[Id]) -> Result<Vec<(Id, f32)>> {
    if query.len() != space.dims {
        return Ok(Vec::new());
    }
    let t = vec_table(space.id);
    let blob = to_blob(query);
    let mut out: Vec<(Id, f32)> = Vec::new();
    if projects.is_empty() {
        let mut st = c.prepare(&format!(
            "SELECT chunk_id, distance FROM {t} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance"
        ))?;
        let rows = st.query_map(params![blob, k as i64], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32)))?;
        out = rows.collect::<std::result::Result<_, _>>()?;
    } else {
        let mut st = c.prepare(&format!(
            "SELECT chunk_id, distance FROM {t} WHERE embedding MATCH ?1 AND k = ?2 AND project_id = ?3 ORDER BY distance"
        ))?;
        for p in projects {
            let rows = st.query_map(params![blob, k as i64, p], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32)))?;
            for r in rows {
                out.push(r?);
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(k);
    }
    Ok(out)
}

pub fn vectors(c: &Connection, space: Space, chunk_ids: &[Id]) -> Result<HashMap<Id, Vec<f32>>> {
    let mut out = HashMap::new();
    let mut st = c.prepare(&format!("SELECT embedding FROM {} WHERE chunk_id = ?1", vec_table(space.id)))?;
    for id in chunk_ids {
        if let Some(b) = st.query_row(params![id], |r| r.get::<_, Vec<u8>>(0)).optional()? {
            out.insert(*id, from_blob(&b));
        }
    }
    Ok(out)
}
