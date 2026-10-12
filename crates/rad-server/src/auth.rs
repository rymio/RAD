use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use rad_api::{Device, HEADER_SESSION};
use rad_vault::Vault;

use crate::error::ApiErr;
use crate::state::Shared;

pub fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim)
}

pub fn session_header(parts: &Parts) -> Option<&str> {
    parts.headers.get(HEADER_SESSION).and_then(|v| v.to_str().ok()).map(str::trim)
}

/// A paired device (bearer token). Does not require the vault to be unlocked.
pub struct DeviceAuth(pub Device);

impl FromRequestParts<Shared> for DeviceAuth {
    type Rejection = ApiErr;
    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, ApiErr> {
        let token = bearer(parts).ok_or_else(|| ApiErr::unauthorized("pair this device first"))?;
        state
            .devices
            .authenticate(token)
            .map(DeviceAuth)
            .ok_or_else(|| ApiErr::unauthorized("unknown or revoked device"))
    }
}

/// A paired device with a valid passphrase session on an unlocked vault.
pub struct Authed {
    pub device: Device,
    pub session: String,
    pub vault: Arc<Vault>,
}

impl FromRequestParts<Shared> for Authed {
    type Rejection = ApiErr;
    async fn from_request_parts(parts: &mut Parts, state: &Shared) -> Result<Self, ApiErr> {
        let DeviceAuth(device) = DeviceAuth::from_request_parts(parts, state).await?;
        let vault = state.vault()?;
        let session = session_header(parts).ok_or_else(ApiErr::session)?.to_string();
        if !state.check_session(&session, &device.id) {
            return Err(ApiErr::session());
        }
        state.touch();
        Ok(Authed { device, session, vault })
    }
}
