use anyhow::{Context, Result, bail};
use rymnet_core::NodeIdentity;
use rymnet_domain::DomainClaimRecord;
use rymnet_network::{PeerMessage, b64_to_bytes, send_request};
use rymnet_search::SearchRecord;
use rymnet_site::{SiteManifest, manifest_file_for_path};
use rymnet_storage::{PeerConfig, PeerEntry};
use serde_json::json;
use std::collections::HashSet;

pub struct SearchHit {
    pub domain: String,
    pub path: String,
    pub title: String,
    pub description: String,
}

/// Fetches one `.rym` page's raw bytes over the peer protocol, with the
/// same claim-verify/manifest-verify/chunk-hash-verify pipeline the static
/// gateway performs -- this server has no gateway of its own, it always
/// resolves content this way.
pub async fn fetch_page(
    identity: &NodeIdentity,
    peers: &PeerConfig,
    domain: &str,
    site_path: &str,
) -> Result<Vec<u8>> {
    if peers.peers.is_empty() {
        bail!("no peers configured on this server; nothing to fetch {domain} from");
    }
    let mut last_err = None;
    for peer in &peers.peers {
        match fetch_page_from_peer(peer, identity, domain, site_path).await {
            Ok(bytes) => return Ok(bytes),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no peer could provide {domain}")))
}

async fn fetch_page_from_peer(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    domain: &str,
    site_path: &str,
) -> Result<Vec<u8>> {
    let response = send_request(
        peer,
        identity,
        PeerMessage::new("site.manifest.request", json!({ "domain": domain })),
    )
    .await?;
    let claim: DomainClaimRecord = serde_json::from_value(
        response
            .payload
            .get("claim")
            .context("missing claim")?
            .clone(),
    )?;
    if claim.domain != domain || !claim.verify()? || claim.is_expired() {
        bail!("invalid domain claim for {domain}");
    }
    let manifest: SiteManifest = serde_json::from_value(
        response
            .payload
            .get("manifest")
            .context("missing manifest")?
            .clone(),
    )?;
    if manifest.domain != domain || !manifest.verify(&claim.owner_public_key)? {
        bail!("invalid site manifest for {domain}");
    }

    let file = manifest_file_for_path(&manifest, site_path)
        .with_context(|| format!("file not found: {site_path}"))?;

    let chunk_response = send_request(
        peer,
        identity,
        PeerMessage::new("site.chunk.request", json!({ "hash": file.hash })),
    )
    .await?;
    let bytes = b64_to_bytes(
        chunk_response
            .payload
            .get("bytes")
            .and_then(|value| value.as_str())
            .context("missing chunk bytes")?,
    )?;
    if format!("blake3:{}", blake3::hash(&bytes).to_hex()) != file.hash {
        bail!("chunk hash verification failed for {site_path}");
    }
    Ok(bytes)
}

/// Queries every configured peer for signed search records matching
/// `query`, verifying each one (claim valid, record signed by that claim's
/// owner) before it's trusted, and de-duplicates by domain+path.
pub async fn search(
    identity: &NodeIdentity,
    peers: &PeerConfig,
    query: &str,
) -> Result<Vec<SearchHit>> {
    if peers.peers.is_empty() {
        bail!("no peers configured on this server to search");
    }

    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for peer in &peers.peers {
        let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new(
                "search.query",
                json!({ "query": query, "domain": null, "limit": 10 }),
            ),
        )
        .await
        else {
            continue;
        };
        let Some(records) = response.payload.get("records").and_then(|v| v.as_array()) else {
            continue;
        };
        for item in records {
            let Some(claim_value) = item.get("claim") else {
                continue;
            };
            let Some(record_value) = item.get("record") else {
                continue;
            };
            let Ok(claim) = serde_json::from_value::<DomainClaimRecord>(claim_value.clone())
            else {
                continue;
            };
            let Ok(record) = serde_json::from_value::<SearchRecord>(record_value.clone()) else {
                continue;
            };
            if record.domain != claim.domain
                || !claim.verify().unwrap_or(false)
                || claim.is_expired()
                || !record.verify(&claim.owner_public_key).unwrap_or(false)
            {
                continue;
            }
            let key = format!("{}{}", record.domain, record.path);
            if seen.insert(key) {
                hits.push(SearchHit {
                    domain: record.domain,
                    path: record.path,
                    title: record.title,
                    description: record.description,
                });
            }
        }
    }
    Ok(hits)
}

/// Turns HTML into the same kind of plain-text-with-numbered-links
/// rendering `rymsh`'s lynx clone produces, so the web terminal shows an
/// identical "page" to what a terminal user would see.
pub fn render_text(html: &str) -> (String, Vec<String>) {
    use scraper::{Html, Selector};

    let document = Html::parse_document(html);
    let body_text = Selector::parse("body")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .map(|el| el.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| document.root_element().text().collect::<Vec<_>>().join(" "));
    let text = body_text.split_whitespace().collect::<Vec<_>>().join(" ");

    let mut links = Vec::new();
    if let Ok(selector) = Selector::parse("a[href]") {
        for el in document.select(&selector) {
            if let Some(href) = el.value().attr("href") {
                let href = href.trim();
                if !href.is_empty()
                    && !href.starts_with('#')
                    && !href.contains("://")
                    && !href.starts_with("javascript:")
                {
                    links.push(href.trim_start_matches('/').to_string());
                }
            }
        }
    }
    (text, links)
}
