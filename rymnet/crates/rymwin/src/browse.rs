use anyhow::{Context, Result, bail};
use rymnet_core::NodeIdentity;
use rymnet_domain::DomainClaimRecord;
use rymnet_network::{PeerMessage, b64_to_bytes, send_request};
use rymnet_search::SearchRecord;
use rymnet_site::{SiteManifest, manifest_file_for_path};
use rymnet_storage::{PeerConfig, PeerEntry};
use scraper::{Html, Selector};
use serde_json::json;
use std::collections::HashSet;

/// The virtual "search engine" domain every gateway/browser front end in
/// this project treats specially -- not a real site, a signal to run
/// `search.query` against peers instead of `site.manifest.request`.
pub const SEARCH_DOMAIN: &str = "search.rym";

pub struct Page {
    pub title: String,
    pub text: String,
    pub links: Vec<String>,
}

pub struct SearchHit {
    pub domain: String,
    pub path: String,
    pub title: String,
    pub description: String,
}

/// What navigating to a given address bar / `lynx` path resolves to --
/// either an ordinary site fetch, or (if the path names the special
/// `search.rym` domain) a set of search hits. Shared by every front end
/// in `rymwin` that lets a user type a `.rym` address (`browserapp.rs`,
/// `terminal.rs`), so the "typing plain text with no .rym in it searches
/// instead" convention only has one implementation to stay consistent.
pub enum FetchOutcome {
    Page(Page),
    Search { query: String, hits: Vec<SearchHit> },
}

pub async fn fetch_for_path(identity: &NodeIdentity, peers: &PeerConfig, path: &str) -> Result<FetchOutcome, String> {
    let trimmed = path.trim_start_matches('/');
    let domain = trimmed.split('/').next().unwrap_or_default();
    if domain == SEARCH_DOMAIN {
        let query = query_param(trimmed, "q").unwrap_or_default();
        let hits = search(identity, peers, &query).await.map_err(|err| format!("{err:#}"))?;
        Ok(FetchOutcome::Search { query, hits })
    } else {
        let page = fetch_page(identity, peers, trimmed).await.map_err(|err| format!("{err:#}"))?;
        Ok(FetchOutcome::Page(page))
    }
}

/// Fetches and renders `path` (e.g. "example.rym" or
/// "example.rym/about.html") purely over the peer protocol -- `rymwin`
/// has no local gateway of its own, so this is the only resolution path,
/// with the same claim/manifest/chunk-hash verification pipeline every
/// other front end in this project uses.
pub async fn fetch_page(identity: &NodeIdentity, peers: &PeerConfig, path: &str) -> Result<Page> {
    let (domain, site_path) = split_path(path);
    let bytes = fetch_bytes(identity, peers, domain, &site_path).await?;
    Ok(render(path, &String::from_utf8_lossy(&bytes)))
}

fn split_path(path: &str) -> (&str, String) {
    let trimmed = path.trim_start_matches('/');
    match trimmed.split_once('/') {
        Some((domain, rest)) => (domain, format!("/{rest}")),
        None => (trimmed, "/".to_string()),
    }
}

async fn fetch_bytes(
    identity: &NodeIdentity,
    peers: &PeerConfig,
    domain: &str,
    site_path: &str,
) -> Result<Vec<u8>> {
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `rym peer connect <ip>` first");
    }
    let mut last_err = None;
    for peer in &peers.peers {
        match fetch_from_peer(peer, identity, domain, site_path).await {
            Ok(bytes) => return Ok(bytes),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no peer could provide {domain}")))
}

async fn fetch_from_peer(
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
        response.payload.get("claim").context("missing claim")?.clone(),
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
            .and_then(|v| v.as_str())
            .context("missing chunk bytes")?,
    )?;
    if format!("blake3:{}", blake3::hash(&bytes).to_hex()) != file.hash {
        bail!("chunk hash verification failed for {site_path}");
    }
    Ok(bytes)
}

/// Queries every configured peer for signed search records matching
/// `query`, verifying each one (claim valid, record signed by that
/// claim's owner) before trusting it, and de-duplicates by domain+path.
pub async fn search(identity: &NodeIdentity, peers: &PeerConfig, query: &str) -> Result<Vec<SearchHit>> {
    if peers.peers.is_empty() {
        bail!("no peers configured to search");
    }
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for peer in &peers.peers {
        let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new("search.query", json!({ "query": query, "domain": null, "limit": 10 })),
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
            let Ok(claim) = serde_json::from_value::<DomainClaimRecord>(claim_value.clone()) else {
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

fn render(current_path: &str, html: &str) -> Page {
    let document = Html::parse_document(html);

    let title = Selector::parse("title")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| current_path.to_string());

    let mut links = Vec::new();
    if let Ok(selector) = Selector::parse("a[href]") {
        for el in document.select(&selector) {
            if let Some(href) = el.value().attr("href")
                && let Some(resolved) = resolve_link(current_path, href)
            {
                links.push(resolved);
            }
        }
    }

    let body_text = Selector::parse("body")
        .ok()
        .and_then(|selector| document.select(&selector).next())
        .map(|el| el.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| document.root_element().text().collect::<Vec<_>>().join(" "));
    let text = body_text.split_whitespace().collect::<Vec<_>>().join(" ");

    Page { title, text, links }
}

/// Resolves an anchor's `href` against the page it was found on. Returns
/// `None` for links this browser cannot follow (external URLs,
/// fragments, javascript:/mailto: links -- none of which are supposed to
/// exist in rymnet content anyway, since only HTML/CSS is allowed).
fn resolve_link(current_path: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    if href.contains("://") || href.starts_with("javascript:") || href.starts_with("mailto:") {
        return None;
    }
    if href.starts_with('/') {
        return Some(href.trim_start_matches('/').to_string());
    }
    if href.contains(".rym") {
        return Some(href.trim_start_matches('/').to_string());
    }
    let domain = current_path.split('/').next().unwrap_or_default();
    let dir = current_path.rsplit_once('/').map(|(dir, _)| dir);
    match dir {
        Some(dir) if !dir.is_empty() => Some(format!("{dir}/{href}")),
        _ => Some(format!("{domain}/{href}")),
    }
}

/// Minimal RFC 3986 percent-encoding, just enough to embed a free-text
/// search query in a synthetic `search.rym/?q=...` path.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let Ok(value) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                    out.push(value);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Extracts a query-string parameter from a `domain/path?a=1&b=2`-shaped
/// path, used to pull `q` out of a synthetic search path.
pub fn query_param(path: &str, key: &str) -> Option<String> {
    let (_, query) = path.split_once('?')?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not run by default (needs this machine's real `~/.rymnet` identity
    /// and a reachable peer network) -- run explicitly with
    /// `cargo test -p rymwin -- --ignored search_reaches_a_real_peer`
    /// to confirm the wiring (identity load, PeerConfig, send_request,
    /// signature verification) actually works end to end, which is the
    /// part clicking through the GUI by hand can't easily prove.
    #[test]
    #[ignore]
    fn search_reaches_a_real_peer() {
        let layout = rymnet_storage::DataLayout::default().expect("resolve ~/.rymnet");
        let identity = NodeIdentity::load(layout.identity_path()).expect("load node identity; run `rym init`");
        let peers = PeerConfig::load(layout.peers_path()).expect("load peers.toml");
        assert!(!peers.peers.is_empty(), "configure at least one peer with `rym peer connect` first");

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let hits = runtime
            .block_on(search(&identity, &peers, "test"))
            .expect("search should complete without error even with zero results");
        println!("search returned {} hit(s) from {} peer(s)", hits.len(), peers.peers.len());
    }

    #[test]
    fn renders_title_body_text_and_links() {
        let html = r##"
            <html><head><title>  My Page  </title></head>
            <body>
                <h1>Welcome</h1>
                <p>Hello   there,
                   world.</p>
                <a href="/other.rym/page.html">Other site</a>
                <a href="about.html">About (relative)</a>
                <a href="#skip">Fragment, ignored</a>
                <a href="https://example.com">External, ignored</a>
            </body></html>
        "##;
        let page = render("me.rym/blog/index.html", html);
        assert_eq!(page.title, "My Page");
        assert!(page.text.contains("Welcome"));
        assert!(page.text.contains("Hello there, world."));
        assert_eq!(
            page.links,
            vec!["other.rym/page.html".to_string(), "me.rym/blog/about.html".to_string()]
        );
    }

    #[test]
    fn falls_back_to_current_path_when_no_title() {
        let page = render("me.rym/index.html", "<body><p>hi</p></body>");
        assert_eq!(page.title, "me.rym/index.html");
    }

    #[test]
    fn resolve_link_handles_relative_absolute_and_external() {
        assert_eq!(resolve_link("me.rym/blog/post.html", "about.html"), Some("me.rym/blog/about.html".to_string()));
        assert_eq!(resolve_link("me.rym/index.html", "/other.rym/x.html"), Some("other.rym/x.html".to_string()));
        assert_eq!(resolve_link("me.rym/index.html", "#frag"), None);
        assert_eq!(resolve_link("me.rym/index.html", "https://evil.example/"), None);
        assert_eq!(resolve_link("me.rym/index.html", "mailto:a@b.com"), None);
    }

    #[test]
    fn percent_encode_decode_round_trip() {
        let query = "hello world & friends / stuff";
        let encoded = percent_encode(query);
        assert!(!encoded.contains(' '));
        assert_eq!(percent_decode(&encoded), query);
    }

    #[test]
    fn query_param_extracts_and_decodes() {
        let path = format!("search.rym/?q={}", percent_encode("rust programming"));
        assert_eq!(query_param(&path, "q").as_deref(), Some("rust programming"));
        assert_eq!(query_param("search.rym/", "q"), None);
    }
}
