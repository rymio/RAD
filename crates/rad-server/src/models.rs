use rad_api::{ModelRef, ProviderInput, ProviderKind};
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
    let mut s = repo::settings(c)?;
    if s.embedding_model.is_none() {
        s.embedding_model = Some(ModelRef { provider_id: id, model: rad_local::DEFAULT_EMBEDDING.id.into() });
        repo::save_settings(c, &s)?;
        st.changed(c, "settings", 0, "update");
        crate::ingest::enqueue(st, c, "reindex", None)?;
    }
    Ok(())
}
