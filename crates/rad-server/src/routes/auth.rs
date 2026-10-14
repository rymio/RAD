use axum::extract::State;
use axum::http::request::Parts;
use axum::Json;
use rad_api::*;
use rad_vault::Vault;

use crate::auth::{bearer, session_header, Authed, DeviceAuth};
use crate::error::{ApiErr, ApiResult};
use crate::state::Shared;

pub struct MaybeParts(pub Parts);

impl axum::extract::FromRequestParts<Shared> for MaybeParts {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &Shared) -> Result<Self, Self::Rejection> {
        Ok(MaybeParts(parts.clone()))
    }
}

pub async fn status(State(state): State<Shared>, MaybeParts(parts): MaybeParts) -> Json<ServerStatus> {
    let device = bearer(&parts).and_then(|t| state.devices.authenticate(t));
    let session_valid = match (&device, session_header(&parts)) {
        (Some(d), Some(s)) => state.current_vault().is_some() && state.check_session(s, &d.id),
        _ => false,
    };
    Json(ServerStatus {
        name: state.cfg.name.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: API_VERSION,
        initialized: state.is_initialized(),
        locked: state.current_vault().is_none(),
        device,
        session_valid,
    })
}

pub async fn pair(State(state): State<Shared>, Json(req): Json<PairRequest>) -> ApiResult<Json<PairResponse>> {
    state.unlock_allowed()?;
    if !state.consume_pairing_code(&req.code) {
        state.unlock_result(false);
        return Err(ApiErr::unauthorized("invalid or expired pairing code"));
    }
    let name = if req.device_name.trim().is_empty() { "RAD device".to_string() } else { req.device_name.trim().to_string() };
    let (device, device_token) = state.devices.add(&name, &req.os)?;
    tracing::info!("paired new device “{}” ({})", device.name, device.os);
    Ok(Json(PairResponse { device, device_token }))
}

pub async fn init_vault(State(state): State<Shared>, DeviceAuth(device): DeviceAuth, Json(req): Json<InitVaultRequest>) -> ApiResult<Json<SessionResponse>> {
    if state.is_initialized() {
        return Err(ApiErr::new(axum::http::StatusCode::CONFLICT, "exists", "a vault already exists on this server"));
    }
    let path = state.cfg.vault_path.clone();
    let (vault, recovery) = tokio::task::spawn_blocking(move || Vault::create(&path, &req.passphrase))
        .await
        .map_err(ApiErr::internal)??;
    state.set_unlocked(vault);
    tracing::info!("vault created at {}", state.cfg.vault_path.display());
    Ok(Json(SessionResponse { session: state.new_session(&device.id), recovery_key: Some(recovery) }))
}

pub async fn unlock(State(state): State<Shared>, DeviceAuth(device): DeviceAuth, Json(req): Json<UnlockRequest>) -> ApiResult<Json<SessionResponse>> {
    state.unlock_allowed()?;
    if !state.is_initialized() {
        return Err(ApiErr::new(axum::http::StatusCode::CONFLICT, "not_initialized", "no vault yet — create one first"));
    }
    let ok = match state.current_vault() {
        // Already unlocked by another device: prove knowledge of the passphrase.
        Some(v) => tokio::task::spawn_blocking(move || v.verify_passphrase(&req.passphrase)).await.map_err(ApiErr::internal)?,
        None => {
            // Another computer may be using this vault file (a network share or sync folder).
            crate::sharing::check_can_open(&state)?;
            let path = state.cfg.vault_path.clone();
            match tokio::task::spawn_blocking(move || Vault::open(&path, &req.passphrase)).await.map_err(ApiErr::internal)? {
                Ok(v) => {
                    crate::sharing::check_opened(&state, &v)?;
                    // Another request may have unlocked meanwhile; either way the passphrase is right.
                    if state.current_vault().is_none() {
                        state.set_unlocked(v);
                    }
                    true
                }
                Err(rad_vault::VaultError::WrongKey) => false,
                Err(e) => return Err(e.into()),
            }
        }
    };
    state.unlock_result(ok);
    if !ok {
        tracing::warn!("failed unlock attempt from device “{}”", device.name);
        return Err(ApiErr::new(axum::http::StatusCode::UNAUTHORIZED, "wrong_passphrase", "wrong passphrase"));
    }
    Ok(Json(SessionResponse { session: state.new_session(&device.id), recovery_key: None }))
}

pub async fn unlock_recovery(
    State(state): State<Shared>,
    DeviceAuth(device): DeviceAuth,
    Json(req): Json<RecoveryUnlockRequest>,
) -> ApiResult<Json<SessionResponse>> {
    state.unlock_allowed()?;
    if req.new_passphrase.chars().count() < rad_vault::MIN_PASSPHRASE {
        return Err(rad_vault::VaultError::WeakPassphrase(rad_vault::MIN_PASSPHRASE).into());
    }
    // Recovery re-keys the vault: no other computer may have it open, and every session goes.
    crate::sharing::check_alone(&state, "recover it")?;
    state.lock();
    let path = state.cfg.vault_path.clone();
    let res = tokio::task::spawn_blocking(move || -> rad_vault::Result<(Vault, String)> {
        let v = Vault::open_with_recovery(&path, &req.recovery_key)?;
        let new_recovery = v.rekey(&req.new_passphrase)?;
        Ok((v, new_recovery))
    })
    .await
    .map_err(ApiErr::internal)?;
    match res {
        Ok((v, recovery)) => {
            state.unlock_result(true);
            state.set_unlocked(v);
            tracing::warn!("vault recovered and re-keyed from device “{}”", device.name);
            Ok(Json(SessionResponse { session: state.new_session(&device.id), recovery_key: Some(recovery) }))
        }
        Err(e) => {
            state.unlock_result(false);
            Err(match e {
                rad_vault::VaultError::WrongKey => rad_vault::VaultError::BadRecoveryKey.into(),
                other => other.into(),
            })
        }
    }
}

pub async fn end_session(State(state): State<Shared>, a: Authed) -> ApiResult<Json<serde_json::Value>> {
    state.end_session(&a.session);
    Ok(Json(serde_json::json!({"ok": true})))
}
