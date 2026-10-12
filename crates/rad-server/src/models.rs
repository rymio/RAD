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
