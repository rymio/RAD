//! Retrieval: hybrid BM25 + vector search, reciprocal-rank fusion, MMR diversification,
//! neighbour merging, and prompt assembly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rad_api::{Citation, Id, Memory, Message, Project, Settings};
use rad_vault::recall::{self, ChatChunkInfo};
use rad_vault::repo::{self, ChunkInfo};
use rad_vault::{search, Vault};

use crate::error::ApiResult;
use crate::models;
use crate::state::{db_vault, Shared};

const CANDIDATES: usize = 40;
/// Chat history sent verbatim with each message: at most this many recent messages…
const HISTORY_MESSAGES: usize = 12;
/// …and this many characters (each message clipped to `HISTORY_MESSAGE_CHARS`).
const HISTORY_CHARS: usize = 14_000;
const HISTORY_MESSAGE_CHARS: usize = 4000;
const RRF_K: f32 = 60.0;
const MMR_LAMBDA: f32 = 0.6;

#[derive(Debug, Clone)]
pub struct Passage {
    pub citation: Citation,
    pub text: String,
}

/// Embeds the query with the configured embedding model, if the vault has an index for it.
async fn query_vector(state: &Shared, vault: &Arc<Vault>, settings: &Settings, query: &str) -> Option<(search::Space, Vec<f32>)> {
    let m = settings.embedding_model.clone()?;
    let key = models::model_key(&m);
    let m2 = m.clone();
    let (space, conn) = db_vault(vault.clone(), state, move |_, c| {
        let space = search::space(c, &key)?;
        Ok((space, models::resolve(c, &m2).ok()))
    })
    .await
    .ok()?;
    let (space, conn) = (space?, conn?);
    let text = format!("{}{}", rad_llm::embed_prefix(&m.model, true), query);
    match tokio::time::timeout(std::time::Duration::from_secs(30), state.llm.embed(&conn, &m.model, &[text])).await {
        Ok(Ok(mut v)) if !v.is_empty() => Some((space, v.remove(0))),
        Ok(Ok(_)) => None,
        Ok(Err(e)) => {
            tracing::warn!("query embedding failed, falling back to keyword search: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("query embedding timed out, falling back to keyword search");
            None
        }
    }
}

/// Earlier conversations to search alongside the vault.
#[derive(Debug, Clone)]
pub struct Recall {
    /// The conversation being answered (its own history is already in the prompt).
    pub exclude_chat: Id,
    /// The current chat's project scope; empty = all projects.
    pub scope: Vec<Id>,
}

/// A search hit: a document passage or a passage of an earlier conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Key {
    Doc(Id),
    Chat(Id),
}

pub async fn retrieve(
    state: &Shared,
    vault: &Arc<Vault>,
    settings: &Settings,
    query: &str,
    projects: Vec<Id>,
    top_k: usize,
    budget_chars: usize,
    recall: Option<Recall>,
) -> ApiResult<Vec<Passage>> {
    let qv = query_vector(state, vault, settings, query).await;
    let query = query.to_string();
    db_vault(vault.clone(), state, move |_, c| {
        let fts = search::fts_search(c, &query, &projects, CANDIDATES)?;
        let vec_hits = match &qv {
            Some((space, v)) => search::knn(c, *space, v, CANDIDATES, &projects)?,
            None => Vec::new(),
        };

        // Earlier conversations this chat may draw on.
        let (mut chat_fts, mut chat_vec, mut chat_infos) = (Vec::new(), Vec::new(), HashMap::new());
        if let Some(r) = &recall {
            chat_fts = recall::fts_search(c, &query, CANDIDATES)?;
            if let Some((space, v)) = &qv {
                chat_vec = recall::knn(c, *space, v, CANDIDATES)?;
            }
            let mut ids: Vec<Id> = chat_fts.iter().chain(&chat_vec).map(|h| h.0).collect();
            ids.sort_unstable();
            ids.dedup();
            chat_infos = recall::chunk_infos(c, &ids)?
                .into_iter()
                .filter(|i| {
                    i.chat_id != r.exclude_chat
                        && (r.scope.is_empty() || i.scope.projects.is_empty() || i.scope.projects.iter().any(|p| r.scope.contains(p)))
                })
                .map(|i| (i.chunk_id, i))
                .collect();
        }

        // Reciprocal rank fusion.
        let mut fused: HashMap<Key, f32> = HashMap::new();
        let mut add = |hits: &[(Id, f32)], key: fn(Id) -> Key, is_vec: bool| {
            let mut rank = 0;
            for (id, score) in hits {
                // Ignore vectors that are essentially unrelated.
                if is_vec && *score > 0.85 {
                    continue;
                }
                *fused.entry(key(*id)).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
                rank += 1;
            }
        };
        add(&fts, Key::Doc, false);
        add(&vec_hits, Key::Doc, true);
        chat_fts.retain(|h| chat_infos.contains_key(&h.0));
        chat_vec.retain(|h| chat_infos.contains_key(&h.0));
        add(&chat_fts, Key::Chat, false);
        add(&chat_vec, Key::Chat, true);

        let mut ranked: Vec<(Key, f32)> = fused.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        // Documents stay the main source: earlier conversations take at most a third.
        let cap = (top_k / 3).max(2);
        let mut chats = 0;
        ranked.retain(|(k, _)| match k {
            Key::Chat(_) => {
                chats += 1;
                chats <= cap
            }
            Key::Doc(_) => true,
        });
        ranked.truncate(CANDIDATES);
        if ranked.is_empty() {
            return Ok(Vec::new());
        }

        // MMR diversification using stored vectors, when available.
        let selected: Vec<(Key, f32)> = match &qv {
            Some((space, q)) => {
                let doc_ids: Vec<Id> = ranked.iter().filter_map(|r| if let Key::Doc(id) = r.0 { Some(id) } else { None }).collect();
                let chat_ids: Vec<Id> = ranked.iter().filter_map(|r| if let Key::Chat(id) = r.0 { Some(id) } else { None }).collect();
                let mut vecs: HashMap<Key, Vec<f32>> = search::vectors(c, *space, &doc_ids)?.into_iter().map(|(k, v)| (Key::Doc(k), v)).collect();
                vecs.extend(recall::vectors(c, *space, &chat_ids)?.into_iter().map(|(k, v)| (Key::Chat(k), v)));
                mmr(&ranked, &vecs, q, top_k)
            }
            None => ranked.iter().take(top_k).cloned().collect(),
        };

        let doc_ids: Vec<Id> = selected.iter().filter_map(|s| if let Key::Doc(id) = s.0 { Some(id) } else { None }).collect();
        let mut pieces: Vec<Piece> = repo::chunk_infos(c, &doc_ids)?.into_iter().map(Piece::from_doc).collect();
        for s in &selected {
            if let Key::Chat(id) = s.0 {
                if let Some(i) = chat_infos.remove(&id) {
                    pieces.push(Piece::from_chat(i));
                }
            }
        }
        let score_of: HashMap<Key, f32> = selected.iter().cloned().collect();
        Ok(build_passages(pieces, &score_of, budget_chars))
    })
    .await
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

fn mmr<K: Copy + Eq + std::hash::Hash>(ranked: &[(K, f32)], vecs: &HashMap<K, Vec<f32>>, q: &[f32], k: usize) -> Vec<(K, f32)> {
    let max_rel = ranked.first().map(|r| r.1).unwrap_or(1.0).max(1e-6);
    let mut pool: Vec<(K, f32)> = ranked.to_vec();
    let mut out: Vec<(K, f32)> = Vec::new();
    while !pool.is_empty() && out.len() < k {
        let mut best = 0;
        let mut best_score = f32::MIN;
        for (i, (id, rel)) in pool.iter().enumerate() {
            let relevance = rel / max_rel + vecs.get(id).map(|v| cosine(v, q) * 0.25).unwrap_or(0.0);
            let redundancy = out
                .iter()
                .filter_map(|(sid, _)| Some(cosine(vecs.get(id)?, vecs.get(sid)?)))
                .fold(0.0f32, f32::max);
            let s = MMR_LAMBDA * relevance - (1.0 - MMR_LAMBDA) * redundancy;
            if s > best_score {
                best_score = s;
                best = i;
            }
        }
        out.push(pool.remove(best));
    }
    out
}

/// One retrieved passage before merging: a document chunk or a piece of an earlier chat.
struct Piece {
    key: Key,
    /// The document or chat it belongs to; adjacent pieces of the same parent are merged.
    parent: Key,
    ord: i64,
    text: String,
    citation: Citation,
}

impl Piece {
    fn from_doc(i: ChunkInfo) -> Piece {
        Piece {
            key: Key::Doc(i.chunk_id),
            parent: Key::Doc(i.document_id),
            ord: i.ord,
            citation: Citation {
                n: 0,
                chunk_id: i.chunk_id,
                document_id: i.document_id,
                project_id: i.project_id,
                project_name: i.project_name,
                title: i.title,
                mime: i.mime,
                source_kind: i.source_kind,
                anchor: i.anchor,
                snippet: String::new(),
                score: 0.0,
                chat_id: None,
            },
            text: i.text,
        }
    }

    fn from_chat(i: ChatChunkInfo) -> Piece {
        let date = chrono::DateTime::from_timestamp_millis(i.created_at)
            .map(|d| d.with_timezone(&chrono::Local).format("%-d %b %Y").to_string());
        Piece {
            key: Key::Chat(i.chunk_id),
            parent: Key::Chat(i.chat_id),
            ord: i.ord,
            citation: Citation {
                n: 0,
                chunk_id: i.chunk_id,
                document_id: 0,
                project_id: 0,
                project_name: "Conversations".into(),
                title: i.chat_title,
                mime: "application/x-rad-chat".into(),
                source_kind: "chat".into(),
                anchor: rad_api::Anchor { heading: date, ..Default::default() },
                snippet: String::new(),
                score: 0.0,
                chat_id: Some(i.chat_id),
            },
            text: i.text,
        }
    }
}

/// Merges adjacent pieces of the same document or chat into one passage and numbers them.
fn build_passages(mut pieces: Vec<Piece>, score_of: &HashMap<Key, f32>, budget: usize) -> Vec<Passage> {
    // Keep relevance order for numbering, but merge neighbours.
    let order: Vec<Key> = {
        let mut v: Vec<(Key, f32)> = pieces.iter().map(|p| (p.key, *score_of.get(&p.key).unwrap_or(&0.0))).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().map(|x| x.0).collect()
    };
    pieces.sort_by_key(|p| (p.parent, p.ord));
    let mut groups: Vec<Vec<Piece>> = Vec::new();
    for piece in pieces {
        match groups.last_mut() {
            Some(g) if g.last().is_some_and(|l| l.parent == piece.parent && piece.ord - l.ord == 1) => g.push(piece),
            _ => groups.push(vec![piece]),
        }
    }
    let rank_of = |g: &Vec<Piece>| g.iter().filter_map(|p| order.iter().position(|o| *o == p.key)).min().unwrap_or(usize::MAX);
    groups.sort_by_key(rank_of);

    let mut out = Vec::new();
    let mut used = 0usize;
    let mut seen_text: HashSet<u64> = HashSet::new();
    for g in groups {
        let mut text = String::new();
        for (i, part) in g.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&part.text);
        }
        let h = blake3::hash(text.as_bytes());
        if !seen_text.insert(u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap())) {
            continue;
        }
        if used + text.len() > budget && !out.is_empty() {
            break;
        }
        let text: String = if text.len() > budget { text.chars().take(budget).collect() } else { text };
        used += text.len();
        let score = g.iter().map(|p| *score_of.get(&p.key).unwrap_or(&0.0)).fold(0.0, f32::max);
        let mut citation = g.into_iter().next().expect("groups are never empty").citation;
        citation.n = out.len() as u32 + 1;
        // A conversation excerpt is shown in full in the sources panel; documents have a preview.
        citation.snippet = text.chars().take(if citation.chat_id.is_some() { 1500 } else { 320 }).collect();
        citation.score = score;
        out.push(Passage { citation, text });
    }
    out
}

/// A history message as sent to the model.
pub fn history_text(m: &Message) -> String {
    m.content.chars().take(HISTORY_MESSAGE_CHARS).collect()
}

/// Index of the first history message sent verbatim; older ones are only in the chat's summary.
pub fn window_start(history: &[Message]) -> usize {
    let mut used = 0usize;
    let mut start = history.len();
    for (i, m) in history.iter().enumerate().rev().take(HISTORY_MESSAGES) {
        used += history_text(m).len();
        if used > HISTORY_CHARS {
            break;
        }
        start = i;
    }
    start
}

pub fn strip_thinking(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out
}

pub fn system_prompt(settings: &Settings, memories: &[Memory], single_project: Option<&Project>, rag: bool) -> String {
    let mut s = String::new();
    s.push_str(settings.persona.trim());
    s.push_str(&format!("\n\nToday is {}.", chrono::Local::now().format("%A, %d %B %Y")));
    let mems: Vec<&Memory> = memories.iter().filter(|m| m.enabled).collect();
    if !mems.is_empty() {
        s.push_str("\n\nThings your owner asked you to remember:\n");
        for m in mems.iter().take(60) {
            s.push_str(&format!("- {}\n", m.text));
        }
    }
    if let Some(p) = single_project {
        s.push_str(&format!("\n\nThe conversation is scoped to the project “{}”.", p.name));
        if !p.description.trim().is_empty() {
            s.push_str(&format!(" Project description: {}", p.description.trim()));
        }
        if !p.system_prompt.trim().is_empty() {
            s.push_str(&format!("\nProject instructions: {}", p.system_prompt.trim()));
        }
    }
    if rag {
        s.push_str(
            "\n\nHow to answer:\n\
             - The user message contains numbered SOURCES retrieved from the owner's private vault (documents, notes, images, recordings) and, marked as such, excerpts of earlier conversations with you. Use them when relevant.\n\
             - Cite every claim taken from a source inline, like [1] or [2][3], directly after the claim. Only cite source numbers that exist.\n\
             - If the sources don't answer the question, say so briefly, then help from general knowledge and make clear that part isn't from their files.\n\
             - Be concise and well-structured. Use Markdown (lists, tables, code blocks) when it helps.",
        );
    } else {
        s.push_str("\n\nBe concise and well-structured. Use Markdown when it helps.");
    }
    s
}

pub fn sources_block(passages: &[Passage]) -> String {
    if passages.is_empty() {
        return "SOURCES: (no matching documents or notes were found in the vault)\n\n".into();
    }
    let mut s = String::from("SOURCES:\n");
    for p in passages {
        let label = p.citation.anchor.label();
        let place = if label.is_empty() { String::new() } else { format!(" ({label})") };
        if p.citation.chat_id.is_some() {
            s.push_str(&format!("[{}] earlier conversation “{}”{}\n{}\n\n", p.citation.n, p.citation.title, place, p.text.trim()));
            continue;
        }
        let kind = if p.citation.source_kind == "note" { "note" } else { "file" };
        s.push_str(&format!(
            "[{}] {} “{}”{} — project {}\n{}\n\n",
            p.citation.n,
            kind,
            p.citation.title,
            place,
            p.citation.project_name,
            p.text.trim()
        ));
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn mmr_prefers_diverse() {
        let ranked = vec![(1, 1.0), (2, 0.99), (3, 0.5)];
        let mut vecs = HashMap::new();
        vecs.insert(1, vec![1.0, 0.0]);
        vecs.insert(2, vec![1.0, 0.001]);
        vecs.insert(3, vec![0.0, 1.0]);
        let out = mmr(&ranked, &vecs, &[0.8, 0.6], 2);
        assert_eq!(out[0].0, 1);
        assert_eq!(out[1].0, 3, "near-duplicate 2 should lose to diverse 3");
    }

    pub(crate) fn msg(id: Id, role: &str, content: &str) -> Message {
        Message {
            id,
            chat_id: 1,
            role: role.into(),
            content: content.into(),
            attachments: vec![],
            citations: vec![],
            model: None,
            error: None,
            tokens_in: None,
            tokens_out: None,
            created_at: 0,
        }
    }

    #[test]
    fn history_window() {
        let short: Vec<Message> = (1..=6).map(|i| msg(i, if i % 2 == 1 { "user" } else { "assistant" }, "hi")).collect();
        assert_eq!(window_start(&short), 0, "a short chat is sent whole");
        let long: Vec<Message> = (1..=20).map(|i| msg(i, if i % 2 == 1 { "user" } else { "assistant" }, "hi")).collect();
        assert_eq!(window_start(&long), 8, "only the last 12 messages");
        let big: Vec<Message> = (1..=6).map(|i| msg(i, "user", &"x".repeat(5000))).collect();
        assert_eq!(window_start(&big), 3, "and at most ~14k characters");
    }
}
