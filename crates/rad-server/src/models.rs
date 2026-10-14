use rad_api::{Id, ModelRef, ProviderInput, ProviderKind};
use rad_llm::ProviderConn;
use rad_vault::repo::{self, ProviderSecret};
use rad_vault::Connection;

use crate::error::{ApiErr, ApiResult};
use crate::state::AppState;

const BUILTIN_NAME: &str = "Built-in (llama.cpp)";
const BUILTIN_URL: &str = "builtin://llama.cpp";

pub fn conn(p: &ProviderSecret) -> ProviderConn {
    ProviderConn {
        kind: p.kind,
        name: p.name.clone(),
        base_url: p.base_url.clone(),
        api_key: p.api_key.clone(),
        headers: p.headers.clone(),
    }
}

/// Looks up the provider behind a model reference.
pub fn resolve(c: &Connection, m: &ModelRef) -> ApiResult<ProviderConn> {
    let p = repo::provider(c, m.provider_id).map_err(|_| ApiErr::bad_request("the selected model's provider no longer exists"))?;
    if !p.enabled {
        return Err(ApiErr::bad_request(format!("provider “{}” is disabled", p.name)));
    }
    Ok(conn(&p))
}

/// The model that answers a chat: the first usable candidate (per-message choice, the chat's
/// own choice, the default), else a downloaded built-in model. Candidates whose provider was
/// disabled or removed are skipped instead of failing the chat.
pub fn pick_chat_model(st: &AppState, c: &Connection, candidates: &[Option<&ModelRef>]) -> ApiResult<(ModelRef, ProviderConn)> {
    for m in candidates.iter().flatten() {
        if let Ok(conn) = resolve(c, m) {
            return Ok(((*m).clone(), conn));
        }
    }
    let builtin = repo::providers(c)?.into_iter().find(|p| p.kind == ProviderKind::Builtin && p.enabled);
    let downloaded = st
        .library
        .status()
        .into_iter()
        .filter(|m| m.spec.kind == rad_local::ModelKind::Chat && m.path.is_some())
        .max_by_key(|m| m.spec.recommended);
    if let (Some(p), Some(m)) = (builtin, downloaded) {
        return Ok((ModelRef { provider_id: p.id, model: m.spec.id.into() }, conn(&p)));
    }
    Err(ApiErr::bad_request(if candidates.iter().any(Option::is_some) {
        "the chosen chat model isn't available (its provider is disabled or removed) — pick another in Settings → Models"
    } else {
        "choose a chat model in Settings → Models first"
    }))
}

pub fn model_key(m: &ModelRef) -> String {
    format!("{}:{}", m.provider_id, m.model)
}

/// Every vault gets the built-in provider, and uses its embedding model unless the user
/// already chose one — so semantic search works with no other program installed.
pub fn ensure_builtin(st: &AppState, c: &Connection) -> rad_vault::Result<()> {
    let id = match repo::providers(c)?.into_iter().find(|p| p.kind == ProviderKind::Builtin) {
        Some(p) => p.id,
        None => {
            let p = repo::create_provider(
                c,
                &ProviderInput {
                    kind: Some(ProviderKind::Builtin),
                    name: Some(BUILTIN_NAME.into()),
                    base_url: Some(BUILTIN_URL.into()),
                    ..Default::default()
                },
            )?;
            st.changed(c, "provider", p.id, "create");
            p.id
        }
    };
    let s = repo::settings(c)?;
    if s.embedding_model.is_none() && repo::provider(c, id)?.enabled {
        let mut s = s;
        s.embedding_model = Some(ModelRef { provider_id: id, model: rad_local::DEFAULT_EMBEDDING.id.into() });
        repo::save_settings(c, &s)?;
        st.changed(c, "settings", 0, "update");
        crate::ingest::enqueue(st, c, "reindex", None)?;
    }
    heal_settings(st, c)?;
    Ok(())
}

/// Repoints every model role whose provider was switched off or removed, so nothing keeps
/// failing on it: embeddings move to the built-in engine (and the vault is re-indexed), chat
/// moves to a downloaded built-in model, and roles the built-in engine can't fill (vision,
/// transcription) are cleared. Chats that chose such a model follow the default again.
/// Returns whether anything changed.
pub fn heal_settings(st: &AppState, c: &Connection) -> rad_vault::Result<bool> {
    let providers = repo::providers(c)?;
    let usable = |m: &ModelRef| providers.iter().any(|p| p.id == m.provider_id && p.enabled);
    let builtin = providers.iter().find(|p| p.kind == ProviderKind::Builtin && p.enabled).map(|p| p.id);
    let builtin_chat = builtin.and_then(|id| {
        st.library
            .status()
            .into_iter()
            .filter(|m| m.spec.kind == rad_local::ModelKind::Chat && m.path.is_some())
            .max_by_key(|m| m.spec.recommended)
            .map(|m| ModelRef { provider_id: id, model: m.spec.id.into() })
    });

    let mut s = repo::settings(c)?;
    let before = s.clone();
    if s.embedding_model.as_ref().is_some_and(|m| !usable(m)) {
        s.embedding_model = builtin.map(|id| ModelRef { provider_id: id, model: rad_local::DEFAULT_EMBEDDING.id.into() });
    }
    if s.chat_model.as_ref().is_some_and(|m| !usable(m)) {
        s.chat_model = builtin_chat.clone();
    }
    if s.rewrite_model.as_ref().is_some_and(|m| !usable(m)) {
        s.rewrite_model = None; // falls back to the chat model
    }
    for slot in [&mut s.vision_model, &mut s.transcription_model] {
        if slot.as_ref().is_some_and(|m| !usable(m)) {
            *slot = None;
        }
    }
    let changed = s.embedding_model != before.embedding_model
        || s.chat_model != before.chat_model
        || s.rewrite_model != before.rewrite_model
        || s.vision_model != before.vision_model
        || s.transcription_model != before.transcription_model;
    if changed {
        tracing::info!("model settings pointed at a disabled or removed provider — switched to what's available");
        repo::save_settings(c, &s)?;
        st.changed(c, "settings", 0, "update");
        if s.embedding_model != before.embedding_model && s.embedding_model.is_some() {
            crate::ingest::enqueue(st, c, "reindex", None)?;
        }
    }

    // Documents left without a semantic index by an earlier failure get another go.
    if !changed && s.embedding_model.is_some() {
        let stuck: i64 = c.query_row("SELECT COUNT(*) FROM documents WHERE notice LIKE 'Semantic index unavailable%'", [], |r| r.get(0))?;
        if stuck > 0 {
            crate::ingest::enqueue(st, c, "reindex", None)?;
        }
    }

    // Per-chat model choices on unusable providers: the chat follows the default again.
    let stale: Vec<Id> = repo::chats(c, 100_000)?.into_iter().filter(|ch| ch.model.as_ref().is_some_and(|m| !usable(m))).map(|ch| ch.id).collect();
    for id in &stale {
        repo::update_chat(c, *id, &rad_api::ChatInput { model: Some(None), ..Default::default() })?;
        st.changed(c, "chat", *id, "update");
    }
    Ok(changed || !stale.is_empty())
}
