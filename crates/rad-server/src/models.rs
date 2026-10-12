use rad_api::ModelRef;
use rad_llm::ProviderConn;
use rad_vault::repo::{self, ProviderSecret};
use rad_vault::Connection;

use crate::error::{ApiErr, ApiResult};

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
