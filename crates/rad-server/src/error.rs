use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use rad_api::ApiError;
use rad_vault::VaultError;

#[derive(Debug)]
pub struct ApiErr {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiErr {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiErr { status, code, message: message.into() }
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", m)
    }
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }
    pub fn locked() -> Self {
        Self::new(StatusCode::LOCKED, "vault_locked", "the vault is locked")
    }
    pub fn session() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "session_required", "unlock RAD with your passphrase")
    }
    pub fn internal(m: impl std::fmt::Display) -> Self {
        tracing::error!("internal error: {m}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", m.to_string())
    }
    pub fn upstream(m: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "upstream", m.to_string())
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (self.status, Json(ApiError { error: self.message, code: self.code.to_string() })).into_response()
    }
}

impl From<VaultError> for ApiErr {
    fn from(e: VaultError) -> Self {
        match e {
            VaultError::WrongKey => ApiErr::new(StatusCode::UNAUTHORIZED, "wrong_passphrase", e.to_string()),
            VaultError::BadRecoveryKey => ApiErr::new(StatusCode::BAD_REQUEST, "bad_recovery_key", e.to_string()),
            VaultError::WeakPassphrase(_) => ApiErr::new(StatusCode::BAD_REQUEST, "weak_passphrase", e.to_string()),
            VaultError::Missing => ApiErr::not_found(),
            VaultError::Invalid(m) => ApiErr::bad_request(m),
            VaultError::AlreadyExists(_) => ApiErr::new(StatusCode::CONFLICT, "exists", e.to_string()),
            VaultError::NotFound(_) => ApiErr::new(StatusCode::CONFLICT, "not_initialized", e.to_string()),
            other => ApiErr::internal(other),
        }
    }
}

impl From<rad_vault::rusqlite::Error> for ApiErr {
    fn from(e: rad_vault::rusqlite::Error) -> Self {
        VaultError::from(e).into()
    }
}

impl From<rad_llm::LlmError> for ApiErr {
    fn from(e: rad_llm::LlmError) -> Self {
        ApiErr::upstream(e)
    }
}

impl From<anyhow::Error> for ApiErr {
    fn from(e: anyhow::Error) -> Self {
        ApiErr::internal(e)
    }
}

pub type ApiResult<T> = Result<T, ApiErr>;
