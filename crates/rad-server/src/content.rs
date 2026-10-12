//! Serves blob bytes out of the vault with HTTP Range support (PDF viewers, audio/video seeking).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use rad_vault::{blobs, Vault};

use crate::error::{ApiErr, ApiResult};

const MAX_RANGE: u64 = 8 * 1024 * 1024;
const INLINE_FULL: i64 = 32 * 1024 * 1024;

fn parse_range(h: &str, size: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    let first = spec.split(',').next()?.trim();
    let (a, b) = first.split_once('-')?;
    if size == 0 {
        return None;
    }
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        let start = size.saturating_sub(n);
        return Some((start, size));
    }
    let start: u64 = a.parse().ok()?;
    if start >= size {
        return None;
    }
    let end = if b.is_empty() { size } else { (b.parse::<u64>().ok()? + 1).min(size) };
    Some((start, end.min(start + MAX_RANGE)))
}

pub async fn blob_response(vault: Arc<Vault>, blob_id: i64, mime: &str, headers: &HeaderMap, download_name: Option<&str>) -> ApiResult<Response> {
    let v = vault.clone();
    let meta = tokio::task::spawn_blocking(move || blobs::meta(&v.conn(), blob_id)).await.map_err(ApiErr::internal)??;
    let size = meta.size as u64;
    let mut b = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header("x-content-type-options", "nosniff");
    if let Some(n) = download_name {
        let enc = percent_encoding::utf8_percent_encode(n, percent_encoding::NON_ALPHANUMERIC).to_string();
        b = b.header(header::CONTENT_DISPOSITION, format!("inline; filename*=UTF-8''{enc}"));
    }

    if let Some(r) = headers.get(header::RANGE).and_then(|h| h.to_str().ok()) {
        let Some((start, end)) = parse_range(r, size) else {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                .body(Body::empty())
                .unwrap());
        };
        let v = vault.clone();
        let data = tokio::task::spawn_blocking(move || blobs::read_range(&v.conn(), blob_id, start, end))
            .await
            .map_err(ApiErr::internal)??;
        return Ok(b
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {}-{}/{}", start, end - 1, size))
            .header(header::CONTENT_LENGTH, data.len())
            .body(Body::from(data))
            .unwrap());
    }

    b = b.header(header::CONTENT_LENGTH, HeaderValue::from(size));
    if meta.size <= INLINE_FULL {
        let v = vault.clone();
        let data = tokio::task::spawn_blocking(move || blobs::read_all(&v.conn(), blob_id)).await.map_err(ApiErr::internal)??;
        return Ok(b.body(Body::from(data)).unwrap());
    }
    // Large file: stream 1 MiB at a time.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    tokio::task::spawn_blocking(move || {
        let chunk = blobs::CHUNK as u64;
        let mut pos = 0u64;
        while pos < size {
            let end = (pos + chunk).min(size);
            let r = blobs::read_range(&vault.conn(), blob_id, pos, end).map_err(|e| std::io::Error::other(e.to_string()));
            let failed = r.is_err();
            if tx.blocking_send(r).is_err() || failed {
                break;
            }
            pos = end;
        }
    });
    Ok(b.body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))).unwrap())
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 100)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 1000)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 1000)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=0-", 100_000_000), Some((0, 8 * 1024 * 1024)));
    }
}
