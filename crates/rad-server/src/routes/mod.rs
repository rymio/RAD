mod admin;
mod auth;
mod chat;
mod library;
mod vault;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use axum::Router;

use crate::state::Shared;

pub fn router(state: Shared) -> Router {
    let upload_limit = state.cfg.max_upload_bytes as usize;
    Router::new()
        // status, pairing, unlock
        .route("/v1/status", get(auth::status))
        .route("/v1/pair", post(auth::pair))
        .route("/v1/vault/init", post(auth::init_vault))
        .route("/v1/unlock", post(auth::unlock))
        .route("/v1/unlock/recovery", post(auth::unlock_recovery))
        .route("/v1/session/end", post(auth::end_session))
        // vault management & sync
        .route("/v1/vault/lock", post(vault::lock))
        .route("/v1/vault/info", get(vault::info))
        .route("/v1/vault/passphrase", post(vault::change_passphrase))
        .route("/v1/vault/backup", post(vault::backup))
        .route("/v1/vault/verify", post(vault::verify))
        .route("/v1/vault/compact", post(vault::compact))
        .route("/v1/events", get(vault::events))
        .route("/v1/changes", get(vault::changes))
        .route("/v1/jobs", get(vault::jobs))
        // projects, documents, notes
        .route("/v1/projects", get(library::list_projects).post(library::create_project))
        .route("/v1/projects/{id}", get(library::get_project).patch(library::update_project).delete(library::delete_project))
        .route(
            "/v1/projects/{id}/documents",
            get(library::list_documents).post(library::upload).layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route("/v1/projects/{id}/documents/from-blob", post(library::from_blob))
        .route("/v1/projects/{id}/notes", post(library::create_note))
        .route("/v1/blobs/{hash}", get(library::blob_exists).head(library::blob_exists))
        .route("/v1/documents/recent", get(library::recent_documents))
        .route(
            "/v1/documents/{id}",
            get(library::get_document).patch(library::patch_document).delete(library::delete_document),
        )
        .route("/v1/documents/{id}/reindex", post(library::reindex_document))
        .route("/v1/documents/{id}/content", get(library::content))
        .route("/v1/documents/{id}/thumb", get(library::thumb))
        .route("/v1/documents/{id}/chunks", get(library::chunks))
        .route("/v1/notes/{id}", get(library::get_note).put(library::update_note))
        .route("/v1/search", get(library::search))
        // chats
        .route("/v1/chats", get(chat::list).post(chat::create))
        .route("/v1/chats/{id}", get(chat::get).patch(chat::update).delete(chat::remove))
        .route("/v1/chats/{id}/messages", post(chat::send))
        .route("/v1/messages/{id}", delete(chat::delete_message))
        // providers, settings, memories, devices
        .route("/v1/providers", get(admin::list_providers).post(admin::create_provider))
        .route("/v1/providers/{id}", axum::routing::patch(admin::update_provider).delete(admin::delete_provider))
        .route("/v1/providers/{id}/models", get(admin::provider_models))
        .route("/v1/providers/{id}/test", post(admin::test_provider))
        .route("/v1/providers/{id}/pull", post(admin::pull_model))
        .route("/v1/providers/{id}/delete-model", post(admin::delete_model))
        .route("/v1/health/providers", get(admin::provider_health))
        .route("/v1/settings", get(admin::get_settings).put(admin::put_settings))
        .route("/v1/memories", get(admin::list_memories).post(admin::create_memory))
        .route("/v1/memories/{id}", axum::routing::patch(admin::update_memory).delete(admin::delete_memory))
        .route("/v1/devices", get(admin::list_devices))
        .route("/v1/devices/{id}", delete(admin::revoke_device))
        .route("/v1/devices/pairing-code", post(admin::pairing_code))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
