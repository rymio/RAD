mod search_page;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use rymnet_domain::find_claim;
use rymnet_site::{load_latest_manifest, manifest_file_for_path};
use rymnet_storage::{ChunkStore, DataLayout};
use serde::Deserialize;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;

pub const DEFAULT_GATEWAY_ADDR: &str = "127.0.0.1:7447";
pub use search_page::SEARCH_DOMAIN;

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GatewayState {
    layout: DataLayout,
}

pub fn gateway_url(domain: &str) -> String {
    format!("http://{DEFAULT_GATEWAY_ADDR}/{domain}/")
}

pub fn router(layout: DataLayout) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/{*path}", get(serve_path))
        .with_state(GatewayState { layout })
}

pub async fn serve(layout: DataLayout, listen_addr: &str) -> Result<()> {
    let addr: SocketAddr = listen_addr
        .parse()
        .with_context(|| format!("parse gateway listen address {listen_addr}"))?;
    if !addr.ip().is_loopback() {
        anyhow::bail!("gateway must bind to localhost for v0.1");
    }
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind gateway {listen_addr}"))?;
    info!("gateway listening on http://{listen_addr}");
    axum::serve(listener, router(layout))
        .await
        .context("run gateway")
}

async fn root() -> Response {
    (
        StatusCode::NOT_FOUND,
        "Rymnet gateway expects /<domain.rym>/",
    )
        .into_response()
}

async fn serve_path(
    State(state): State<GatewayState>,
    Path(path): Path<String>,
    Query(params): Query<SearchParams>,
    headers: HeaderMap,
) -> Response {
    let domain = path.trim_start_matches('/').split('/').next().unwrap_or("");
    if domain == SEARCH_DOMAIN {
        return search_page::render(&state.layout, params.q.as_deref());
    }
    match resolve_path(&state.layout, &path, &headers) {
        Ok(bytes) => bytes.into_response(),
        Err(GatewayError::NotFound(message)) => (StatusCode::NOT_FOUND, message).into_response(),
        Err(GatewayError::Verification(message)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
        }
    }
}

fn resolve_path(
    layout: &DataLayout,
    raw_path: &str,
    request_headers: &HeaderMap,
) -> Result<Response, GatewayError> {
    let (domain, site_path) = split_gateway_path(raw_path)?;
    let claim = find_claim(layout.domain_claims_dir(), &domain)
        .map_err(|err| GatewayError::Verification(err.to_string()))?
        .ok_or_else(|| GatewayError::NotFound(format!("domain not found: {domain}")))?;
    let claim_valid = claim
        .verify()
        .map_err(|err| GatewayError::Verification(err.to_string()))?;
    if !claim_valid || claim.is_expired() {
        return Err(GatewayError::Verification(format!(
            "invalid domain claim: {domain}"
        )));
    }

    let manifest = load_latest_manifest(&domain, layout.site_root())
        .map_err(|err| GatewayError::Verification(err.to_string()))?
        .ok_or_else(|| GatewayError::NotFound(format!("site not published: {domain}")))?;
    if manifest.domain != domain {
        return Err(GatewayError::Verification(
            "manifest domain mismatch".to_string(),
        ));
    }
    let manifest_valid = manifest
        .verify(&claim.owner_public_key)
        .map_err(|err| GatewayError::Verification(err.to_string()))?;
    if !manifest_valid {
        return Err(GatewayError::Verification(
            "manifest signature verification failed".to_string(),
        ));
    }

    let file = manifest_file_for_path(&manifest, &site_path)
        .ok_or_else(|| GatewayError::NotFound(format!("file not found: /{raw_path}")))?;

    // Content is already content-addressed by `file.hash`, so it's a
    // perfect ETag for free -- a republish that changes this file always
    // produces a different hash, and one that doesn't touch this file
    // (e.g. editing a different page) keeps the same hash, so browsers
    // that already have it cached skip re-downloading it. This replaces
    // an earlier flat `Cache-Control: max-age=300` with no validator at
    // all, which meant a browser could keep showing a page for up to five
    // minutes after `rym site publish` with no way to know it was stale --
    // exactly the "republished but still see the old page" bug this fixes.
    // `no-cache` despite the name does not disable caching; it means
    // "cache it, but always ask the server first" -- cheap here since
    // that revalidation is just a hash comparison, answered with a
    // bodyless 304 below.
    let etag_value = format!("\"{}\"", file.hash);
    let etag_header =
        HeaderValue::from_str(&etag_value).unwrap_or_else(|_| HeaderValue::from_static("\"invalid\""));
    if let Some(if_none_match) = request_headers.get(header::IF_NONE_MATCH)
        && if_none_match == etag_header
    {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        let headers = response.headers_mut();
        headers.insert(header::ETAG, etag_header);
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return Ok(response);
    }

    let store = ChunkStore::new(layout.chunks_root());
    let bytes = store
        .get(&file.hash)
        .map_err(|_| GatewayError::NotFound(format!("chunk missing: {}", file.hash)))?;
    if !store.verify(&file.hash, &bytes) {
        return Err(GatewayError::Verification(
            "chunk hash verification failed".to_string(),
        ));
    }

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&file.mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'none'; object-src 'none'; frame-src 'none'; base-uri 'none';",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::ETAG, etag_header);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

fn split_gateway_path(raw_path: &str) -> Result<(String, String), GatewayError> {
    let trimmed = raw_path.trim_start_matches('/');
    let (domain, rest) = trimmed
        .split_once('/')
        .map_or((trimmed, ""), |(domain, rest)| (domain, rest));
    if domain.is_empty() || !domain.ends_with(".rym") {
        return Err(GatewayError::NotFound(
            "invalid .rym gateway path".to_string(),
        ));
    }
    Ok((domain.to_string(), format!("/{rest}")))
}

#[derive(Debug)]
enum GatewayError {
    NotFound(String),
    Verification(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_gateway_paths() {
        let (domain, path) = split_gateway_path("rob.rym/").expect("split");
        assert_eq!(domain, "rob.rym");
        assert_eq!(path, "/");

        let (domain, path) = split_gateway_path("rob.rym/about.html").expect("split");
        assert_eq!(domain, "rob.rym");
        assert_eq!(path, "/about.html");

        assert!(split_gateway_path("rob.com/").is_err());
    }

    /// Regression test for a real bug: the gateway used to send a flat
    /// `Cache-Control: max-age=300` with no validator at all, so a
    /// browser could keep showing a page for up to five minutes after
    /// `rym site publish` with no way to even know the content had
    /// changed. Proves the fix end to end: a first fetch gets an ETag
    /// derived from the content hash; re-fetching with that ETag gets a
    /// bodyless 304; and -- the actual bug -- republishing changed
    /// content immediately invalidates the old ETag instead of quietly
    /// keeping it valid for the rest of the cache window.
    #[test]
    fn republishing_invalidates_the_old_etag() {
        let temp = std::env::temp_dir().join(format!("rymnet-gateway-test-{}", blake3::hash(b"etag")));
        let _ = std::fs::remove_dir_all(&temp);
        let layout = DataLayout::from_root(&temp);
        let identity = rymnet_core::NodeIdentity::generate();

        rymnet_domain::claim_domain(layout.domain_claims_dir(), "rob.rym", &identity).expect("claim");
        rymnet_site::create_site("rob.rym", layout.site_root()).expect("create site");
        rymnet_site::publish_site("rob.rym", layout.site_root(), layout.chunks_root(), &identity)
            .expect("publish");

        let no_headers = HeaderMap::new();
        let first = resolve_path(&layout, "rob.rym/", &no_headers).expect("resolve");
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get(header::ETAG)
            .expect("ETag should be set")
            .clone();

        // Re-fetching with that ETag should short-circuit to a bodyless
        // 304, without needing to touch the chunk store at all.
        let mut conditional = HeaderMap::new();
        conditional.insert(header::IF_NONE_MATCH, etag.clone());
        let cached = resolve_path(&layout, "rob.rym/", &conditional).expect("resolve conditional");
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);

        // Republish with genuinely different content -- the bug this
        // guards against is a stale cache surviving exactly this step.
        let source_dir = rymnet_site::source_dir(layout.site_root(), "rob.rym");
        std::fs::write(source_dir.join("index.html"), "<html><body>updated content</body></html>")
            .expect("overwrite index.html");
        rymnet_site::publish_site("rob.rym", layout.site_root(), layout.chunks_root(), &identity)
            .expect("republish");

        // The old ETag must no longer be treated as fresh.
        let after_republish = resolve_path(&layout, "rob.rym/", &conditional).expect("resolve after republish");
        assert_eq!(
            after_republish.status(),
            StatusCode::OK,
            "republishing changed content must invalidate the old ETag, not keep serving 304"
        );
        let new_etag = after_republish
            .headers()
            .get(header::ETAG)
            .expect("ETag should be set")
            .clone();
        assert_ne!(etag, new_etag, "changed content must get a different ETag");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
