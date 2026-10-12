//! Retrieval: hybrid BM25 + vector search, reciprocal-rank fusion, MMR diversification,
//! neighbour merging, and prompt assembly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rad_api::{Citation, Id, Memory, Project, Settings};
use rad_vault::repo::{self, ChunkInfo};
use rad_vault::{search, Vault};

use crate::error::ApiResult;
use crate::models;
use crate::state::{db_vault, Shared};

const CANDIDATES: usize = 40;
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

pub async fn retrieve(
    state: &Shared,
    vault: &Arc<Vault>,
    settings: &Settings,
    query: &str,
    projects: Vec<Id>,
    top_k: usize,
    budget_chars: usize,
) -> ApiResult<Vec<Passage>> {
    let qv = query_vector(state, vault, settings, query).await;
    let query = query.to_string();
    db_vault(vault.clone(), state, move |_, c| {
        let fts = search::fts_search(c, &query, &projects, CANDIDATES)?;
        let vec_hits = match &qv {
            Some((space, v)) => search::knn(c, *space, v, CANDIDATES, &projects)?,
            None => Vec::new(),
        };

        // Reciprocal rank fusion.
        let mut fused: HashMap<Id, f32> = HashMap::new();
        for (rank, (id, _)) in fts.iter().enumerate() {
            *fused.entry(*id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        for (rank, (id, dist)) in vec_hits.iter().enumerate() {
            // Ignore vectors that are essentially unrelated.
            if *dist > 0.85 {
                continue;
            }
            *fused.entry(*id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        let mut ranked: Vec<(Id, f32)> = fused.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(CANDIDATES);
        if ranked.is_empty() {
            return Ok(Vec::new());
        }

        // MMR diversification using stored vectors, when available.
        let selected: Vec<(Id, f32)> = match &qv {
            Some((space, q)) => {
                let ids: Vec<Id> = ranked.iter().map(|r| r.0).collect();
                let vecs = search::vectors(c, *space, &ids)?;
                mmr(&ranked, &vecs, q, top_k)
            }
            None => ranked.iter().take(top_k).cloned().collect(),
        };

        let ids: Vec<Id> = selected.iter().map(|s| s.0).collect();
        let infos = repo::chunk_infos(c, &ids)?;
        let score_of: HashMap<Id, f32> = selected.iter().cloned().collect();
        Ok(build_passages(infos, &score_of, budget_chars))
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

fn mmr(ranked: &[(Id, f32)], vecs: &HashMap<Id, Vec<f32>>, q: &[f32], k: usize) -> Vec<(Id, f32)> {
    let max_rel = ranked.first().map(|r| r.1).unwrap_or(1.0).max(1e-6);
    let mut pool: Vec<(Id, f32)> = ranked.to_vec();
    let mut out: Vec<(Id, f32)> = Vec::new();
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

/// Merges adjacent chunks of the same document into one passage and numbers them.
fn build_passages(mut infos: Vec<ChunkInfo>, score_of: &HashMap<Id, f32>, budget: usize) -> Vec<Passage> {
    // Keep relevance order for numbering, but merge neighbours.
    let order: Vec<Id> = {
        let mut v: Vec<(Id, f32)> = infos.iter().map(|i| (i.chunk_id, *score_of.get(&i.chunk_id).unwrap_or(&0.0))).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().map(|x| x.0).collect()
    };
    infos.sort_by_key(|i| (i.document_id, i.ord));
    let mut groups: Vec<Vec<ChunkInfo>> = Vec::new();
    for info in infos {
        match groups.last_mut() {
            Some(g) if g.last().is_some_and(|l| l.document_id == info.document_id && info.ord - l.ord == 1) => g.push(info),
            _ => groups.push(vec![info]),
        }
    }
    let rank_of = |g: &Vec<ChunkInfo>| g.iter().filter_map(|i| order.iter().position(|o| *o == i.chunk_id)).min().unwrap_or(usize::MAX);
    groups.sort_by_key(rank_of);

    let mut out = Vec::new();
    let mut used = 0usize;
    let mut seen_text: HashSet<u64> = HashSet::new();
    for g in groups {
        let first = &g[0];
        let mut text = String::new();
        for (i, part) in g.iter().enumerate() {
            if i > 0 {
                text.push_str("\n");
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
        let score = g.iter().map(|i| *score_of.get(&i.chunk_id).unwrap_or(&0.0)).fold(0.0, f32::max);
        out.push(Passage {
            citation: Citation {
                n: out.len() as u32 + 1,
                chunk_id: first.chunk_id,
                document_id: first.document_id,
                project_id: first.project_id,
                project_name: first.project_name.clone(),
                title: first.title.clone(),
                mime: first.mime.clone(),
                source_kind: first.source_kind.clone(),
                anchor: first.anchor.clone(),
                snippet: text.chars().take(320).collect(),
                score,
            },
            text,
        });
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
             - The user message contains numbered SOURCES retrieved from the owner's private vault (documents, notes, images, recordings). Use them when relevant.\n\
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
        let kind = if p.citation.source_kind == "note" { "note" } else { "file" };
        s.push_str(&format!(
            "[{}] {} “{}”{} — project {}\n{}\n\n",
            p.citation.n,
            kind,
            p.citation.title,
            if label.is_empty() { String::new() } else { format!(" ({label})") },
            p.citation.project_name,
            p.text.trim()
        ));
    }
    s
}

#[cfg(test)]
mod tests {
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
}
