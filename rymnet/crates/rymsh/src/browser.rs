use anyhow::{Context, Result, bail};
use rymnet_core::NodeIdentity;
use rymnet_domain::DomainClaimRecord;
use rymnet_gateway::SEARCH_DOMAIN;
use rymnet_network::{PeerMessage, b64_to_bytes, send_request};
use rymnet_search::SearchRecord;
use rymnet_site::{SiteManifest, manifest_file_for_path};
use rymnet_storage::{DataLayout, PeerConfig, PeerEntry};
use scraper::{Html, Selector};
use serde_json::json;
use std::collections::HashSet;
use std::io::{self, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const WRAP_WIDTH: usize = 78;

pub struct Page {
    pub title: String,
    pub lines: Vec<String>,
    pub links: Vec<String>,
}

/// Fetches `target` (e.g. "rob.rym" or "rob.rym/about.html") and renders it
/// as text, like lynx. Tries the local gateway first (fast, single
/// round-trip, if `rymnetd` happens to be running) and -- since that
/// connection either succeeds or fails instantly, there's no cost to
/// trying -- falls back to resolving the page directly over the peer
/// protocol otherwise, using exactly the same `site.manifest.request`/
/// `site.chunk.request` messages `rym site fetch` already uses. Only
/// HTML/CSS pages exist on rymnet, so there is nothing to execute either
/// way -- just text, headings, and links to walk.
pub async fn fetch(gateway_addr: &str, layout: &DataLayout, target: &str) -> Result<Page> {
    let path = normalize_target(target);
    if let Ok(page) = fetch_via_gateway(gateway_addr, &path).await {
        return Ok(page);
    }
    fetch_via_peers(layout, &path).await
}

async fn fetch_via_gateway(gateway_addr: &str, path: &str) -> Result<Page> {
    let request = format!(
        "GET /{path} HTTP/1.1\r\nHost: {gateway_addr}\r\nUser-Agent: rymsh-lynx/0.1\r\nConnection: close\r\n\r\n"
    );

    let mut stream = TcpStream::connect(gateway_addr)
        .await
        .with_context(|| format!("connect to rymnet gateway at {gateway_addr}"))?;
    stream.write_all(request.as_bytes()).await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf);

    let (status_line, rest) = response
        .split_once("\r\n")
        .context("malformed gateway response")?;
    let (_headers, body) = rest.split_once("\r\n\r\n").unwrap_or((rest, ""));
    if !status_line.contains(" 200 ") {
        bail!("gateway: {status_line}  {}", body.trim());
    }

    Ok(render(path, body))
}

/// No local `rymnetd` (or it just doesn't have this domain): resolve
/// straight over the peer protocol instead, with the same signature/hash
/// verification the gateway itself performs -- this never trusts a peer's
/// content without checking it.
async fn fetch_via_peers(layout: &DataLayout, path: &str) -> Result<Page> {
    let domain = path.split('/').next().unwrap_or_default();
    if domain == SEARCH_DOMAIN {
        return fetch_search_via_peers(layout, path).await;
    }

    let identity = NodeIdentity::load(layout.identity_path())
        .context("load node identity; run `rym init` first")?;
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!(
            "no rymnet daemon running locally, and no peers configured to fetch {domain} from \
             (see `rym peer add`/`rym peer connect`)"
        );
    }

    let site_path = format!("/{}", path.split_once('/').map(|(_, rest)| rest).unwrap_or(""));
    let mut last_err = None;
    for peer in &peers.peers {
        match fetch_site_from_peer(peer, &identity, domain, &site_path).await {
            Ok(bytes) => return Ok(render(path, &String::from_utf8_lossy(&bytes))),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no peer could provide {domain}")))
}

async fn fetch_site_from_peer(
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

async fn fetch_search_via_peers(layout: &DataLayout, path: &str) -> Result<Page> {
    let Some(query) = query_param(path) else {
        return Ok(Page {
            title: "search.rym".to_string(),
            lines: vec![
                "A node-based search engine.".to_string(),
                "Type a query and press enter to search the network.".to_string(),
            ],
            links: Vec::new(),
        });
    };

    let identity = NodeIdentity::load(layout.identity_path())
        .context("load node identity; run `rym init` first")?;
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured to search (see `rym peer add`/`rym peer connect`)");
    }

    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for peer in &peers.peers {
        let Ok(response) = send_request(
            peer,
            &identity,
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
                results.push(record);
            }
        }
    }

    let mut lines = vec![format!("Search: {query}")];
    let mut links = Vec::new();
    if results.is_empty() {
        lines.push(String::new());
        lines.push("No results.".to_string());
    } else {
        for (index, record) in results.iter().enumerate() {
            let suffix = if record.path == "/index.html" {
                "/".to_string()
            } else {
                record.path.clone()
            };
            links.push(format!("{}{suffix}", record.domain));
            lines.push(String::new());
            let title = if record.title.is_empty() {
                record.domain.clone()
            } else {
                record.title.clone()
            };
            lines.push(format!("{}. {title}", index + 1));
            if !record.description.is_empty() {
                lines.push(record.description.clone());
            }
        }
    }

    Ok(Page {
        title: format!("search.rym: {query}"),
        lines,
        links,
    })
}

fn query_param(path: &str) -> Option<String> {
    let (_, query_string) = path.split_once('?')?;
    for pair in query_string.split('&') {
        if let Some(value) = pair.strip_prefix("q=") {
            let decoded = percent_decode(value);
            if !decoded.trim().is_empty() {
                return Some(decoded);
            }
        }
    }
    None
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

    let lines = wrap_text(&normalize_whitespace(&body_text), WRAP_WIDTH);
    Page {
        title,
        lines,
        links,
    }
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Resolves an anchor's `href` against the page it was found on. Returns
/// `None` for links rymsh's lynx clone cannot follow (external URLs,
/// fragments, javascript: links -- none of which are supposed to exist in
/// rymnet content anyway).
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

/// Minimal query-string percent-encoding (RFC 3986 unreserved chars pass
/// through, space becomes `+`, everything else becomes `%XX`) -- just
/// enough to safely embed free-text search queries in a GET request path.
pub(crate) fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
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
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn normalize_target(target: &str) -> String {
    let trimmed = target.trim().trim_start_matches('/');
    if trimmed.contains('/') || trimmed.is_empty() {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    }
}

fn print_page(current: &str, page: &Page) {
    println!();
    println!("=== {} ===", page.title);
    println!("({current})");
    println!();
    for line in &page.lines {
        println!("{line}");
    }
    if !page.links.is_empty() {
        println!();
        println!("References");
        for (index, link) in page.links.iter().enumerate() {
            println!("  [{}] {}", index + 1, link);
        }
    }
    println!();
}

/// The interactive read-eval-print loop: fetch, render, then let the user
/// type a link number to follow, `b` to go back, or `q` to return to rymsh.
pub async fn browse(gateway_addr: &str, layout: &DataLayout, start: &str) -> Result<()> {
    let mut history: Vec<String> = Vec::new();
    let mut current = normalize_target(start);
    let mut last_links: Vec<String> = Vec::new();

    loop {
        match fetch(gateway_addr, layout, &current).await {
            Ok(page) => {
                print_page(&current, &page);
                last_links = page.links;
                history.push(current.clone());
            }
            Err(err) => {
                println!("Error: {err:#}");
                last_links.clear();
            }
        }

        print!("lynx [{current}]> ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();

        match input {
            "q" | "quit" | "exit" => break,
            "b" | "back" => {
                history.pop();
                match history.pop() {
                    Some(prev) => current = prev,
                    None => println!("No previous page."),
                }
            }
            "" => {
                history.pop();
            }
            other => {
                if let Ok(index) = other.parse::<usize>() {
                    match last_links.get(index - 1) {
                        Some(link) => current = link.clone(),
                        None => println!("No such reference: {index}"),
                    }
                } else if other.contains(".rym") {
                    // looks like a domain (or domain/path) -- go there directly
                    current = normalize_target(other);
                } else {
                    // free text with no ".rym" in it: search the network,
                    // like typing into a real browser's address bar
                    current = format!("{SEARCH_DOMAIN}/?q={}", percent_encode(other));
                }
            }
        }
    }
    Ok(())
}
