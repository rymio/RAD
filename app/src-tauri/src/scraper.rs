//! Scraper (Browsers → Scraper): saves a whole website for offline reading, and a clean text copy
//! of every page for the vault (RAG), so Gilgamesh and every chat can use it.
//!
//! A crawl starts at one page and follows the links that stay on the site (or one section of
//! it, or its subdomains), politely: robots.txt is honoured unless switched off, requests are
//! spaced out, and the sitemap seeds the queue. Each page is saved with its images, styles and
//! fonts (from any host) and the documents it links to (PDFs and the like). When the crawl ends,
//! the links in the saved pages are rewritten to point at the saved copies, and scripts are
//! taken out, so the copy is static and private: it is shown sandboxed, with nothing allowed to
//! load from the internet.
//!
//! On disk (the app data folder, not the vault): `scraped/index.json` lists the sites;
//! `scraped/<id>/site/<host>/<path>` is the offline copy; `scraped/<id>/rag/*.md` the text of
//! each page (title, source address, main content) that "Add to RAG" uploads; and
//! `scraped/<id>/pages.json` the pages. The archive is served at `rad://localhost/scraped/<id>/…`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use regex::Regex;
use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

use crate::client::{CmdError, CmdResult};

const UA: &str = "Mozilla/5.0 (compatible; RAD-Scraper/1.0; personal offline copy)";
const MAX_PAGE_BYTES: usize = 15 * 1024 * 1024;
const MAX_ASSET_BYTES: usize = 25 * 1024 * 1024;
const MAX_DOC_BYTES: usize = 60 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Default)]
pub struct Scraper {
    /// Guards index.json.
    lock: Mutex<()>,
    /// Crawls running now: their stop flags.
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Options {
    /// "site" (this host), "section" (pages under the start page's folder) or "subdomains".
    #[serde(default = "d_scope")]
    scope: String,
    #[serde(default = "d_pages")]
    max_pages: u32,
    #[serde(default = "d_depth")]
    max_depth: u32,
    /// Images, styles, fonts and media, so pages look as they did.
    #[serde(default = "yes")]
    assets: bool,
    /// Linked PDFs, Word, Excel, PowerPoint, EPUB… (also added to RAG).
    #[serde(default = "yes")]
    documents: bool,
    #[serde(default = "yes")]
    robots: bool,
    #[serde(default = "d_delay")]
    delay_ms: u64,
    #[serde(default = "d_conc")]
    concurrency: u32,
}
fn d_scope() -> String {
    "site".into()
}
fn d_pages() -> u32 {
    500
}
fn d_depth() -> u32 {
    12
}
fn d_delay() -> u64 {
    250
}
fn d_conc() -> u32 {
    4
}
fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Rag {
    project: i64,
    docs: Vec<i64>,
    added: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Site {
    id: String,
    title: String,
    url: String,
    host: String,
    created: i64,
    updated: i64,
    /// "running", "done", "stopped" or "failed".
    status: String,
    pages: u32,
    assets: u32,
    documents: u32,
    bytes: u64,
    errors: u32,
    /// Left out: robots.txt said no.
    blocked: u32,
    queued: u32,
    current: Option<String>,
    message: Option<String>,
    /// The first page's file in the offline copy.
    start: Option<String>,
    options: Options,
    #[serde(default)]
    rag: Option<Rag>,
    /// The vault copy is older than the last scrape.
    #[serde(default)]
    rag_stale: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Page {
    url: String,
    file: String,
    title: String,
    depth: u32,
    bytes: u64,
    words: u32,
    /// "page" or "document".
    kind: String,
    /// Its text for RAG (a file in rag/), if it has enough of it.
    rag: Option<String>,
}

fn root<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(d) = std::env::var("RAD_SCRAPER_DIR") {
        return d.into();
    }
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("scraped")
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

fn read_index<R: tauri::Runtime>(app: &AppHandle<R>) -> Vec<Site> {
    std::fs::read(root(app).join("index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_index<R: tauri::Runtime>(app: &AppHandle<R>, sites: &[Site]) -> CmdResult<()> {
    let dir = root(app);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let tmp = dir.join("index.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(sites).map_err(io)?).map_err(io)?;
    std::fs::rename(tmp, dir.join("index.json")).map_err(io)
}

/// Changes one site in the index (under the lock).
fn update_site<R: tauri::Runtime>(app: &AppHandle<R>, id: &str, f: impl FnOnce(&mut Site)) -> Option<Site> {
    let st = app.state::<Scraper>();
    let _g = st.lock.lock().unwrap();
    let mut sites = read_index(app);
    let s = sites.iter_mut().find(|s| s.id == id)?;
    f(s);
    s.updated = crate::now_ms();
    let out = s.clone();
    write_index(app, &sites).ok()?;
    Some(out)
}

// ------------------------------------------------------------------ addresses and files

/// The same page however it was linked: no fragment, no tracking parameters.
fn normalize(mut u: Url) -> Url {
    u.set_fragment(None);
    if u.query().is_some() {
        let kept: Vec<(String, String)> = u.query_pairs().filter(|(k, _)| !(k.starts_with("utm_") || matches!(k.as_ref(), "fbclid" | "gclid" | "mc_cid" | "mc_eid" | "ref_src"))).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        if kept.is_empty() {
            u.set_query(None);
        } else {
            u.query_pairs_mut().clear().extend_pairs(kept);
        }
    }
    u
}

fn bare_host(h: &str) -> &str {
    h.strip_prefix("www.").unwrap_or(h)
}

struct Scope {
    host: String,
    section: String,
    kind: String,
}

impl Scope {
    fn new(start: &Url, kind: &str) -> Self {
        let path = start.path();
        let section = match path.rfind('/') {
            Some(i) => path[..=i].to_string(),
            None => "/".into(),
        };
        Scope { host: bare_host(start.host_str().unwrap_or("")).to_string(), section, kind: kind.into() }
    }
    fn contains(&self, u: &Url) -> bool {
        if !matches!(u.scheme(), "http" | "https") {
            return false;
        }
        let h = bare_host(u.host_str().unwrap_or(""));
        match self.kind.as_str() {
            "subdomains" => h == self.host || h.ends_with(&format!(".{}", self.host)),
            "section" => h == self.host && u.path().starts_with(&self.section),
            _ => h == self.host,
        }
    }
}

const DOC_EXT: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "epub", "rtf", "csv", "txt", "md"];
const ASSET_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif", "css", "woff", "woff2", "ttf", "otf", "eot", "mp4", "webm", "mp3", "ogg", "wav"];
const SKIP_EXT: &[&str] = &["zip", "gz", "tgz", "rar", "7z", "exe", "dmg", "pkg", "msi", "iso", "apk", "bin", "js", "mjs", "json", "xml", "rss", "atom"];

fn ext_of(u: &Url) -> String {
    let last = u.path().rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.len() <= 5 => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn clean_segment(s: &str) -> String {
    let d = percent_decode(s);
    let mut out: String = d.chars().map(|c| if c.is_alphanumeric() || "._-~".contains(c) { c } else { '_' }).collect();
    if out.is_empty() || out == "." || out == ".." {
        out = "_".into();
    }
    out.chars().take(80).collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn short_hash(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex()[..8].to_string()
}

/// Where a page or file goes in the offline copy, like a browser would want to find it:
/// pages end in .html (a folder's page is its index.html), other files keep their name.
fn file_for(u: &Url, page: bool, content_type: Option<&str>) -> String {
    let host = clean_segment(u.host_str().unwrap_or("site"));
    let mut segs: Vec<String> = u.path().split('/').filter(|s| !s.is_empty()).map(clean_segment).collect();
    let dir_like = u.path().ends_with('/') || segs.is_empty();
    let mut name = if dir_like { if page { "index.html".to_string() } else { "index".to_string() } } else { segs.pop().unwrap() };
    if page && !dir_like {
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".html") || lower.ends_with(".htm")) {
            if lower.contains('.') && !lower.ends_with(".php") && !lower.ends_with(".asp") && !lower.ends_with(".aspx") && !lower.ends_with(".jsp") {
                name.push_str(".html");
            } else {
                segs.push(name);
                name = "index.html".into();
            }
        }
    }
    if !page && !name.contains('.') {
        if let Some(ext) = content_type.and_then(|t| mime_guess::get_mime_extensions_str(t.split(';').next().unwrap_or(t).trim())).and_then(|e| e.first()) {
            name = format!("{name}.{ext}");
        }
    }
    if let Some(q) = u.query() {
        let h = short_hash(q);
        name = match name.rsplit_once('.') {
            Some((a, b)) => format!("{a}__{h}.{b}"),
            None => format!("{name}__{h}"),
        };
    }
    let mut parts = vec![host];
    parts.extend(segs);
    parts.push(name);
    parts.join("/")
}

/// `to` as seen from the folder `from` is in (both relative to the copy's root).
fn relative(from: &str, to: &str) -> String {
    let a: Vec<&str> = from.split('/').collect();
    let b: Vec<&str> = to.split('/').collect();
    let dir = &a[..a.len() - 1];
    let common = dir.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let mut out: Vec<String> = vec!["..".into(); dir.len() - common];
    out.extend(b[common..].iter().map(|s| url_escape(s)));
    out.join("/")
}

fn url_escape(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "._-~".contains(c) { c.to_string() } else { c.to_string().bytes().map(|b| format!("%{b:02X}")).collect() }).collect()
}

// ------------------------------------------------------------------ robots.txt

#[derive(Default)]
struct Robots {
    rules: Vec<(bool, String)>,
    delay: Option<f64>,
    sitemaps: Vec<String>,
}

impl Robots {
    fn parse(text: &str) -> Self {
        // Groups: user-agent lines, then their rules. Ours ("rad") wins over "*".
        let mut groups: Vec<(Vec<String>, Vec<(bool, String)>, Option<f64>)> = Vec::new();
        let mut sitemaps = Vec::new();
        let mut open_agents = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once(':') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match k.as_str() {
                "user-agent" => {
                    if !open_agents {
                        groups.push((Vec::new(), Vec::new(), None));
                    }
                    open_agents = true;
                    groups.last_mut().unwrap().0.push(v.to_ascii_lowercase());
                }
                "allow" | "disallow" => {
                    open_agents = false;
                    if let Some(g) = groups.last_mut() {
                        if !v.is_empty() {
                            g.1.push((k == "allow", v));
                        }
                    }
                }
                "crawl-delay" => {
                    open_agents = false;
                    if let Some(g) = groups.last_mut() {
                        g.2 = v.parse().ok();
                    }
                }
                "sitemap" => sitemaps.push(v),
                _ => {}
            }
        }
        let pick = groups.iter().find(|g| g.0.iter().any(|a| a.contains("rad"))).or_else(|| groups.iter().find(|g| g.0.iter().any(|a| a == "*")));
        match pick {
            Some(g) => Robots { rules: g.1.clone(), delay: g.2, sitemaps },
            None => Robots { sitemaps, ..Default::default() },
        }
    }

    fn allowed(&self, path_and_query: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, pat) in &self.rules {
            if pattern_match(pat, path_and_query) {
                let len = pat.len();
                if best.is_none_or(|(l, a)| len > l || (len == l && *allow && !a)) {
                    best = Some((len, *allow));
                }
            }
        }
        best.is_none_or(|(_, a)| a)
    }
}

/// robots.txt patterns: a prefix, with `*` for anything and `$` for the end.
fn pattern_match(pat: &str, path: &str) -> bool {
    let (pat, anchored) = match pat.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pat, false),
    };
    let parts: Vec<&str> = pat.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if let Some(at) = path[pos..].find(part) {
            pos += at + part.len();
        } else {
            return false;
        }
    }
    !anchored || pos == path.len() || (parts.len() > 1 && parts.last() == Some(&""))
}

// ------------------------------------------------------------------ reading a page

struct Parsed {
    title: String,
    links: Vec<Url>,
    assets: Vec<Url>,
    nofollow: bool,
    markdown: String,
    words: u32,
}

static CSS_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)url\(\s*(['"]?)([^'")]+)['"]?\s*\)"#).unwrap());
static CSS_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)@import\s+['"]([^'"]+)['"]"#).unwrap());

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}

fn srcset_urls(v: &str) -> impl Iterator<Item = &str> {
    v.split(',').filter_map(|c| c.split_whitespace().next())
}

fn parse_page(html: &str, page_url: &Url) -> Parsed {
    let doc = Html::parse_document(html);
    let base = doc.select(&sel("base[href]")).next().and_then(|b| b.value().attr("href")).and_then(|h| page_url.join(h).ok()).unwrap_or_else(|| page_url.clone());
    let join = |v: &str| -> Option<Url> {
        let v = v.trim();
        if v.is_empty() || v.starts_with('#') || v.starts_with("data:") || v.starts_with("javascript:") || v.starts_with("mailto:") || v.starts_with("tel:") {
            return None;
        }
        base.join(v).ok().filter(|u| matches!(u.scheme(), "http" | "https")).map(normalize)
    };
    let mut links = Vec::new();
    for a in doc.select(&sel("a[href], area[href], link[rel=next][href], link[rel=prev][href]")) {
        if a.value().attr("rel").is_some_and(|r| r.contains("nofollow")) {
            continue;
        }
        if let Some(u) = a.value().attr("href").and_then(join) {
            links.push(u);
        }
    }
    let mut assets = Vec::new();
    for (q, attrs) in [
        ("link[href]", &["href"][..]),
        ("img", &["src", "data-src", "srcset", "data-srcset"][..]),
        ("source", &["src", "srcset"][..]),
        ("video", &["src", "poster"][..]),
        ("audio[src]", &["src"][..]),
        ("input[type=image][src]", &["src"][..]),
        ("[background]", &["background"][..]),
    ] {
        for el in doc.select(&sel(q)) {
            if q == "link[href]" {
                let rel = el.value().attr("rel").unwrap_or("").to_ascii_lowercase();
                if !(rel.contains("stylesheet") || rel.contains("icon")) {
                    continue;
                }
            }
            for a in attrs {
                if let Some(v) = el.value().attr(a) {
                    if a.contains("srcset") {
                        assets.extend(srcset_urls(v).filter_map(join));
                    } else if let Some(u) = join(v) {
                        assets.push(u);
                    }
                }
            }
        }
    }
    for el in doc.select(&sel("[style], style")) {
        let text = if el.value().name() == "style" { el.text().collect::<String>() } else { el.value().attr("style").unwrap_or("").to_string() };
        assets.extend(CSS_URL.captures_iter(&text).filter_map(|c| join(&c[2])));
    }
    let robots = doc.select(&sel("meta[name=robots]")).next().and_then(|m| m.value().attr("content")).unwrap_or("").to_ascii_lowercase();
    let title = doc
        .select(&sel("title"))
        .next()
        .map(|t| t.text().collect::<String>())
        .filter(|t| !t.trim().is_empty())
        .or_else(|| doc.select(&sel("meta[property='og:title']")).next().and_then(|m| m.value().attr("content")).map(String::from))
        .or_else(|| doc.select(&sel("h1")).next().map(|h| h.text().collect::<String>()))
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| page_url.path().to_string());
    let (markdown, words) = main_text(&doc);
    Parsed { title, links, assets, nofollow: robots.contains("nofollow"), markdown, words }
}

/// A fetched page's title and its own content as Markdown (Products uses it for the store
/// guides). `bytes` is the body as it came, `content_type` the response's header.
pub(crate) fn page_markdown(bytes: &[u8], content_type: &str, page_url: &Url) -> (String, String, u32) {
    let p = parse_page(&decode(bytes, content_type), page_url);
    (p.title, p.markdown, p.words)
}

/// The page's own content as Markdown: the main/article part when there is one, without the
/// menus, sidebars, forms and scripts around it.
fn main_text(doc: &Html) -> (String, u32) {
    let body = doc.select(&sel("body")).next();
    let mut root = None;
    for q in ["main", "article", "[role=main]", "#content", "#main", ".content", ".post", ".entry-content"] {
        if let Some(el) = doc.select(&sel(q)).next() {
            if el.text().map(str::len).sum::<usize>() > 200 {
                root = Some(el);
                break;
            }
        }
    }
    let is_body = root.is_none();
    let Some(root) = root.or(body) else { return (String::new(), 0) };
    let mut out = String::new();
    render(root, &mut out, is_body, 0);
    // Tidy: trimmed lines, no runs of blank lines.
    let mut tidy = String::new();
    let mut blank = 0;
    let mut code = false;
    for l in out.lines() {
        let l = l.trim_end();
        if l.trim_start().starts_with("```") {
            code = !code;
        } else if code {
            // Code keeps its indentation and blank lines.
            tidy.push_str(l);
            tidy.push('\n');
            continue;
        }
        if l.trim().is_empty() {
            blank += 1;
            if blank == 1 && !tidy.is_empty() {
                tidy.push('\n');
            }
        } else {
            blank = 0;
            tidy.push_str(l.trim_start_matches(' '));
            tidy.push('\n');
        }
    }
    let words = tidy.split_whitespace().count() as u32;
    (tidy.trim().to_string(), words)
}

fn inline_text(el: ElementRef<'_>) -> String {
    el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render(el: ElementRef<'_>, out: &mut String, chrome: bool, depth: usize) {
    if depth > 200 {
        return;
    }
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let s = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if !s.is_empty() {
                    if t.starts_with(char::is_whitespace) && !out.ends_with([' ', '\n']) {
                        out.push(' ');
                    }
                    out.push_str(&s);
                    if t.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                }
            }
            Node::Element(e) => {
                let Some(c) = ElementRef::wrap(child) else { continue };
                let name = e.name();
                if matches!(name, "script" | "style" | "noscript" | "nav" | "aside" | "form" | "svg" | "iframe" | "button" | "select" | "template" | "canvas" | "dialog" | "object") {
                    continue;
                }
                if chrome && matches!(name, "header" | "footer") {
                    continue;
                }
                if e.attr("aria-hidden") == Some("true") || e.attr("hidden").is_some() {
                    continue;
                }
                match name {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let t = inline_text(c);
                        if !t.is_empty() {
                            let n = name[1..].parse::<usize>().unwrap_or(2);
                            out.push_str(&format!("\n\n{} {t}\n\n", "#".repeat(n)));
                        }
                    }
                    "br" => out.push('\n'),
                    "hr" => out.push_str("\n\n---\n\n"),
                    "li" => {
                        out.push_str("\n- ");
                        render(c, out, chrome, depth + 1);
                    }
                    "pre" => {
                        out.push_str("\n\n```\n");
                        out.push_str(c.text().collect::<String>().trim_end());
                        out.push_str("\n```\n\n");
                    }
                    "table" => {
                        out.push_str("\n\n");
                        let mut first = true;
                        for tr in c.select(&sel("tr")) {
                            let cells: Vec<String> = tr.select(&sel("th, td")).map(|x| inline_text(x).replace('|', "/")).collect();
                            if cells.is_empty() {
                                continue;
                            }
                            out.push_str(&format!("| {} |\n", cells.join(" | ")));
                            if first {
                                out.push_str(&format!("|{}\n", " --- |".repeat(cells.len())));
                                first = false;
                            }
                        }
                        out.push('\n');
                    }
                    "img" => {
                        if let Some(alt) = e.attr("alt").map(str::trim).filter(|a| a.len() > 3) {
                            out.push_str(&format!(" [image: {alt}] "));
                        }
                    }
                    "blockquote" => {
                        let mut inner = String::new();
                        render(c, &mut inner, chrome, depth + 1);
                        for l in inner.trim().lines() {
                            out.push_str(&format!("\n> {}", l.trim()));
                        }
                        out.push_str("\n\n");
                    }
                    "p" | "div" | "section" | "article" | "main" | "figure" | "figcaption" | "ul" | "ol" | "dl" | "dt" | "dd" | "details" | "summary" | "header" | "footer" | "address" => {
                        out.push_str("\n\n");
                        render(c, out, chrome, depth + 1);
                        out.push_str("\n\n");
                    }
                    _ => render(c, out, chrome, depth + 1),
                }
            }
            _ => {}
        }
    }
}

/// The charset from the Content-Type header or a <meta>, else UTF-8.
fn decode(bytes: &[u8], content_type: &str) -> String {
    let from_header = content_type.to_ascii_lowercase().split("charset=").nth(1).map(|c| c.trim_matches(|x: char| x == '"' || x == '\'' || x.is_whitespace()).to_string());
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let from_meta = head.split("charset=").nth(1).map(|c| c.trim_start_matches(['"', '\'']).split(|x: char| !(x.is_ascii_alphanumeric() || x == '-' || x == '_')).next().unwrap_or("").to_string());
    let label = from_header.or(from_meta).unwrap_or_else(|| "utf-8".into());
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    enc.decode(bytes).0.into_owned()
}

// ------------------------------------------------------------------ crawling

enum Kind {
    Page,
    Asset,
}

struct Fetched {
    url: Url,
    content_type: String,
    body: Vec<u8>,
}

async fn fetch(client: &reqwest::Client, u: &Url, limit: usize) -> Result<Fetched, String> {
    let mut resp = client.get(u.as_str()).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    if resp.content_length().is_some_and(|l| l as usize > limit) {
        return Err("too big".into());
    }
    let url = resp.url().clone();
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err("too big".into());
        }
    }
    Ok(Fetched { url, content_type, body })
}

struct Crawl<R: tauri::Runtime> {
    app: AppHandle<R>,
    id: String,
    dir: PathBuf,
    opts: Options,
    scope: Scope,
    robots: HashMap<String, Robots>,
    pages: Vec<Page>,
    /// Address → file, for the pages and the files saved.
    page_files: HashMap<String, String>,
    asset_files: HashMap<String, String>,
    used_files: HashSet<String>,
    /// Page texts already seen (the same page under two addresses is kept once for RAG).
    texts: HashSet<String>,
    site: Site,
    last_emit: Instant,
}

impl<R: tauri::Runtime> Crawl<R> {
    fn emit(&mut self, force: bool) {
        if !force && self.last_emit.elapsed() < Duration::from_millis(300) {
            return;
        }
        self.last_emit = Instant::now();
        let s = self.site.clone();
        if let Some(saved) = update_site(&self.app, &self.id, |x| *x = s) {
            let _ = self.app.emit("scraper-progress", saved);
        }
    }

    fn unique(&mut self, file: String) -> String {
        let mut f = file.clone();
        let mut n = 1;
        while self.used_files.contains(&f.to_ascii_lowercase()) {
            n += 1;
            f = match file.rsplit_once('.') {
                Some((a, b)) if !b.contains('/') => format!("{a}~{n}.{b}"),
                _ => format!("{file}~{n}"),
            };
        }
        self.used_files.insert(f.to_ascii_lowercase());
        f
    }

    fn save(&self, file: &str, bytes: &[u8]) -> Result<(), String> {
        let p = self.dir.join("site").join(file);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(p, bytes).map_err(|e| e.to_string())
    }

    fn allowed(&self, u: &Url) -> bool {
        if !self.opts.robots {
            return true;
        }
        let key = u.origin().ascii_serialization();
        let pq = match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        };
        self.robots.get(&key).is_none_or(|r| r.allowed(&pq))
    }

    /// The text copy of a page for RAG, unless it says little or repeats another page.
    fn rag_file(&mut self, title: &str, url: &Url, md: &str, words: u32) -> Option<String> {
        if words < 40 {
            return None;
        }
        let key = short_hash(md);
        if !self.texts.insert(key) {
            return None;
        }
        let host = url.host_str().unwrap_or("site");
        let safe: String = title.chars().map(|c| if c.is_alphanumeric() || " -_,.()&'".contains(c) { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
        let safe: String = safe.chars().take(90).collect();
        let n = self.pages.len() + 1;
        let file = format!("{} – {host} ({n}).md", if safe.is_empty() { "Page" } else { &safe });
        let date = chrono::Local::now().format("%-d %B %Y");
        let text = format!("# {title}\n\nSource: {url}\nSaved from {host} on {date} with RAD's Scraper.\n\n{md}\n");
        std::fs::create_dir_all(self.dir.join("rag")).ok()?;
        std::fs::write(self.dir.join("rag").join(&file), text).ok()?;
        Some(file)
    }
}

async fn run_crawl<R: tauri::Runtime>(app: AppHandle<R>, id: String, stop: Arc<AtomicBool>) {
    let result = crawl(app.clone(), id.clone(), stop.clone()).await;
    let stopped = stop.load(Ordering::Relaxed);
    let saved = update_site(&app, &id, |s| {
        s.current = None;
        s.queued = 0;
        match &result {
            Ok(()) => s.status = if stopped { "stopped".into() } else { "done".into() },
            Err(e) => {
                s.status = "failed".into();
                s.message = Some(e.clone());
            }
        }
        if s.rag.is_some() {
            s.rag_stale = true;
        }
    });
    if let Some(s) = saved {
        let _ = app.emit("scraper-progress", s);
    }
    app.state::<Scraper>().jobs.lock().unwrap().remove(&id);
}

async fn crawl<R: tauri::Runtime>(app: AppHandle<R>, id: String, stop: Arc<AtomicBool>) -> Result<(), String> {
    let site = read_index(&app).into_iter().find(|s| s.id == id).ok_or("no such site")?;
    let dir = root(&app).join(&id);
    let _ = std::fs::remove_dir_all(dir.join("site"));
    let _ = std::fs::remove_dir_all(dir.join("rag"));
    std::fs::create_dir_all(dir.join("site")).map_err(|e| e.to_string())?;
    let start = Url::parse(&site.url).map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()
        .map_err(|e| e.to_string())?;
    let opts = site.options.clone();
    let mut c = Crawl {
        app: app.clone(),
        id: id.clone(),
        dir: dir.clone(),
        scope: Scope::new(&start, &opts.scope),
        opts,
        robots: HashMap::new(),
        pages: Vec::new(),
        page_files: HashMap::new(),
        asset_files: HashMap::new(),
        used_files: HashSet::new(),
        texts: HashSet::new(),
        site,
        last_emit: Instant::now(),
    };
    c.site.pages = 0;
    c.site.assets = 0;
    c.site.documents = 0;
    c.site.bytes = 0;
    c.site.errors = 0;
    c.site.blocked = 0;
    c.site.message = None;
    c.site.status = "running".into();

    // robots.txt and the sitemap for the start host.
    let origin = start.origin().ascii_serialization();
    let mut seeds: Vec<Url> = vec![normalize(start.clone())];
    let mut delay = Duration::from_millis(c.opts.delay_ms);
    let robots = match fetch(&client, &Url::parse(&format!("{origin}/robots.txt")).unwrap(), 512 * 1024).await {
        Ok(f) => Robots::parse(&String::from_utf8_lossy(&f.body)),
        Err(_) => Robots::default(),
    };
    if c.opts.robots {
        if let Some(d) = robots.delay {
            delay = delay.max(Duration::from_secs_f64(d.min(30.0)));
        }
    }
    let mut sitemaps: Vec<String> = robots.sitemaps.clone();
    if sitemaps.is_empty() {
        sitemaps.push(format!("{origin}/sitemap.xml"));
    }
    c.robots.insert(origin.clone(), robots);
    static LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<loc>\s*(?:<!\[CDATA\[)?\s*([^<\]]+?)\s*(?:\]\]>)?\s*</loc>").unwrap());
    let mut maps_seen = 0;
    while let Some(sm) = sitemaps.pop() {
        if maps_seen >= 8 || stop.load(Ordering::Relaxed) || seeds.len() as u32 >= c.opts.max_pages {
            break;
        }
        maps_seen += 1;
        let Ok(u) = Url::parse(&sm) else { continue };
        let Ok(f) = fetch(&client, &u, 20 * 1024 * 1024).await else { continue };
        let text = String::from_utf8_lossy(&f.body);
        let index = text.contains("<sitemapindex");
        for cap in LOC.captures_iter(&text) {
            let Ok(loc) = Url::parse(cap[1].trim()) else { continue };
            if index {
                sitemaps.push(loc.to_string());
            } else if c.scope.contains(&loc) && (seeds.len() as u32) < c.opts.max_pages {
                seeds.push(normalize(loc));
            }
        }
    }

    let mut queue: VecDeque<(Url, u32, Kind)> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (i, s) in seeds.into_iter().enumerate() {
        if seen.insert(s.to_string()) {
            queue.push_back((s, if i == 0 { 0 } else { 1 }, Kind::Page));
        }
    }
    let mut page_count: u32 = 0;
    let max_assets = c.opts.max_pages.saturating_mul(40).max(200);
    let mut asset_count: u32 = 0;
    let conc = c.opts.concurrency.clamp(1, 8) as usize;
    let mut inflight = FuturesUnordered::new();
    let mut next_at = Instant::now();
    let mut url_of_file: HashMap<String, Url> = HashMap::new();
    let mut css_files: Vec<(String, Url)> = Vec::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Start what fits, spaced out by the delay. Pages first, so the page limit decides.
        while inflight.len() < conc && !queue.is_empty() && Instant::now() >= next_at {
            let pos = queue.iter().position(|q| matches!(q.2, Kind::Page)).unwrap_or(0);
            let (u, depth, kind) = queue.remove(pos).unwrap();
            if matches!(kind, Kind::Page) {
                if page_count >= c.opts.max_pages {
                    continue;
                }
                if !c.allowed(&u) {
                    c.site.blocked += 1;
                    continue;
                }
                page_count += 1;
            }
            next_at = Instant::now() + if matches!(kind, Kind::Page) { delay } else { delay / 3 };
            let client = client.clone();
            c.site.current = Some(u.to_string());
            inflight.push(async move {
                let limit = if matches!(kind, Kind::Page) { MAX_PAGE_BYTES.max(MAX_DOC_BYTES) } else { MAX_ASSET_BYTES };
                let r = fetch(&client, &u, limit).await;
                (u, depth, kind, r)
            });
        }
        c.site.queued = queue.len() as u32;
        c.emit(false);
        if inflight.is_empty() {
            if queue.is_empty() {
                break;
            }
            tokio::time::sleep_until(next_at.into()).await;
            continue;
        }
        // Wait for a fetch, or for the next start time.
        let done = tokio::select! {
            r = inflight.next() => r,
            _ = tokio::time::sleep_until(next_at.into()), if inflight.len() < conc && !queue.is_empty() => None,
        };
        let Some((asked, depth, kind, result)) = done else { continue };
        let f = match result {
            Ok(f) => f,
            Err(e) => {
                c.site.errors += 1;
                if c.pages.is_empty() && depth == 0 {
                    c.site.message = Some(format!("{asked}: {e}"));
                }
                continue;
            }
        };
        if c.site.bytes + f.body.len() as u64 > MAX_TOTAL_BYTES {
            c.site.message = Some("Stopped at 4 GB".into());
            break;
        }
        let ct = f.content_type.to_ascii_lowercase();
        let final_url = normalize(f.url.clone());
        match kind {
            Kind::Page if ct.contains("html") || ct.contains("xhtml") || (ct.is_empty() && f.body.starts_with(b"<")) => {
                if depth > 0 && !c.scope.contains(&final_url) {
                    continue; // redirected off the site
                }
                let html = decode(&f.body, &f.content_type);
                let p = parse_page(&html, &final_url);
                let key = final_url.to_string();
                if c.page_files.contains_key(&key) {
                    c.page_files.insert(normalize(asked).to_string(), c.page_files[&key].clone());
                    continue;
                }
                let file = c.unique(file_for(&final_url, true, None));
                if let Err(e) = c.save(&file, html.as_bytes()) {
                    c.site.errors += 1;
                    c.site.message = Some(e);
                    continue;
                }
                c.page_files.insert(key, file.clone());
                c.page_files.insert(normalize(asked).to_string(), file.clone());
                url_of_file.insert(file.clone(), final_url.clone());
                let rag = c.rag_file(&p.title, &final_url, &p.markdown, p.words);
                if c.pages.is_empty() {
                    c.site.start = Some(file.clone());
                    if c.site.title.is_empty() || c.site.title == c.site.host {
                        c.site.title = p.title.clone();
                    }
                }
                c.pages.push(Page { url: final_url.to_string(), file, title: p.title, depth, bytes: f.body.len() as u64, words: p.words, kind: "page".into(), rag });
                c.site.pages += 1;
                c.site.bytes += f.body.len() as u64;
                // Links on (unless nofollow and robots is respected).
                if !(p.nofollow && c.opts.robots) && depth < c.opts.max_depth {
                    for l in p.links {
                        let e = ext_of(&l);
                        if SKIP_EXT.contains(&e.as_str()) {
                            continue;
                        }
                        let is_doc = DOC_EXT.contains(&e.as_str());
                        let is_asset = ASSET_EXT.contains(&e.as_str());
                        if (is_doc && !c.opts.documents) || (is_asset && !c.opts.assets) {
                            continue;
                        }
                        if (!is_doc && !is_asset && !c.scope.contains(&l)) || (is_doc && !c.scope.contains(&l)) {
                            continue;
                        }
                        if seen.insert(l.to_string()) {
                            queue.push_back((l, depth + 1, if is_asset { Kind::Asset } else { Kind::Page }));
                        }
                    }
                }
                if c.opts.assets {
                    for a in p.assets {
                        if asset_count + (queue.len() as u32) < max_assets && seen.insert(a.to_string()) {
                            queue.push_back((a, depth, Kind::Asset));
                        }
                    }
                }
            }
            Kind::Page => {
                // A document (PDF…) linked from a page; anything else is left out.
                let e = ext_of(&final_url);
                let doc = DOC_EXT.contains(&e.as_str()) || ct.contains("pdf") || ct.contains("officedocument") || ct.contains("msword") || ct.contains("epub");
                if !doc || !c.opts.documents {
                    continue;
                }
                let file = c.unique(file_for(&final_url, false, Some(&f.content_type)));
                if c.save(&file, &f.body).is_err() {
                    c.site.errors += 1;
                    continue;
                }
                c.asset_files.insert(normalize(asked).to_string(), file.clone());
                c.asset_files.insert(final_url.to_string(), file.clone());
                let title = percent_decode(final_url.path().rsplit('/').next().unwrap_or("document"));
                c.pages.push(Page { url: final_url.to_string(), file, title, depth, bytes: f.body.len() as u64, words: 0, kind: "document".into(), rag: None });
                c.site.documents += 1;
                c.site.bytes += f.body.len() as u64;
            }
            Kind::Asset => {
                let file = c.unique(file_for(&final_url, false, Some(&f.content_type)));
                if c.save(&file, &f.body).is_err() {
                    c.site.errors += 1;
                    continue;
                }
                c.asset_files.insert(normalize(asked).to_string(), file.clone());
                c.asset_files.insert(final_url.to_string(), file.clone());
                asset_count += 1;
                c.site.assets += 1;
                c.site.bytes += f.body.len() as u64;
                // Stylesheets bring their own images and fonts.
                if ct.contains("css") || file.ends_with(".css") {
                    let text = decode(&f.body, &f.content_type);
                    for cap in CSS_URL.captures_iter(&text).map(|c| c[2].to_string()).chain(CSS_IMPORT.captures_iter(&text).map(|c| c[1].to_string())) {
                        if cap.starts_with("data:") {
                            continue;
                        }
                        if let Ok(u) = final_url.join(cap.trim()) {
                            let u = normalize(u);
                            if asset_count + (queue.len() as u32) < max_assets && seen.insert(u.to_string()) {
                                queue.push_back((u, depth, Kind::Asset));
                            }
                        }
                    }
                    css_files.push((file, final_url));
                }
            }
        }
    }
    drop(inflight);

    // The offline copy: links to what was saved, scripts out.
    c.site.current = Some("Making the offline copy…".into());
    c.emit(true);
    let site_dir = dir.join("site");
    let pages_map = c.page_files.clone();
    let assets_map = c.asset_files.clone();
    let page_list: Vec<(String, Url)> = c.pages.iter().filter(|p| p.kind == "page").filter_map(|p| Url::parse(&p.url).ok().map(|u| (p.file.clone(), u))).collect();
    tokio::task::spawn_blocking(move || {
        for (file, u) in page_list {
            let path = site_dir.join(&file);
            if let Ok(html) = std::fs::read_to_string(&path) {
                let _ = std::fs::write(&path, rewrite_html(&html, &u, &file, &pages_map, &assets_map));
            }
        }
        for (file, u) in css_files {
            let path = site_dir.join(&file);
            if let Ok(bytes) = std::fs::read(&path) {
                let css = String::from_utf8_lossy(&bytes);
                let _ = std::fs::write(&path, rewrite_css(&css, &u, &file, &assets_map));
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    std::fs::write(dir.join("pages.json"), serde_json::to_vec(&c.pages).unwrap_or_default()).map_err(|e| e.to_string())?;
    if c.pages.is_empty() && c.site.message.is_none() {
        c.site.message = Some(if c.site.blocked > 0 { "robots.txt doesn’t allow these pages".into() } else { "No pages could be saved".into() });
    }
    c.site.current = None;
    let s = c.site.clone();
    update_site(&app, &id, |x| {
        let keep_rag = x.rag.clone();
        *x = s;
        x.rag = keep_rag;
    });
    Ok(())
}

// ------------------------------------------------------------------ the offline copy

static SCRIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<script\b[^>]*>.*?</script\s*>|<script\b[^>]*/>").unwrap());
static NOSCRIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<noscript\b[^>]*>(.*?)</noscript\s*>").unwrap());
static DROP_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<base\b[^>]*>|<meta\b[^>]*http-equiv\s*=\s*["']?(?:refresh|content-security-policy)[^>]*>|<link\b[^>]*rel\s*=\s*["']?(?:preload|prefetch|modulepreload|dns-prefetch|preconnect|manifest|serviceworker)[^>]*>"#).unwrap());
static ATTR_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(\s(?:href|src|poster|data-src|background)\s*=\s*)(?:"([^"]*)"|'([^']*)')"#).unwrap());
static ATTR_SRCSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(\s(?:srcset|data-srcset)\s*=\s*)(?:"([^"]*)"|'([^']*)')"#).unwrap());
static INTEGRITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\s(?:integrity|crossorigin)\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+)"#).unwrap());

/// Where a link in the saved copy should go: a saved page or file (relative), a page that
/// wasn't saved (RAD offers to open it in its browser), or as it was.
fn map_url(v: &str, base: &Url, from_file: &str, pages: &HashMap<String, String>, assets: &HashMap<String, String>, is_link: bool) -> String {
    let raw = v.trim();
    if raw.is_empty() || raw.starts_with('#') || raw.starts_with("data:") || raw.starts_with("mailto:") || raw.starts_with("tel:") {
        return v.to_string();
    }
    if raw.to_ascii_lowercase().starts_with("javascript:") {
        return "#".into();
    }
    let Ok(abs) = base.join(raw) else { return v.to_string() };
    let frag = abs.fragment().map(|f| format!("#{f}")).unwrap_or_default();
    let key = normalize(abs.clone()).to_string();
    if let Some(f) = pages.get(&key).or_else(|| assets.get(&key)) {
        return format!("{}{frag}", relative(from_file, f));
    }
    if is_link && matches!(abs.scheme(), "http" | "https") {
        // Not saved: a link out, which RAD opens in its own browser when clicked.
        let depth = from_file.matches('/').count();
        return format!("{}__rad_out?u={}", "../".repeat(depth), url_escape(abs.as_str()));
    }
    abs.to_string()
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x2F;", "/")
}

fn rewrite_html(html: &str, page: &Url, file: &str, pages: &HashMap<String, String>, assets: &HashMap<String, String>) -> String {
    // The base for relative links, as the page had it.
    let base = Regex::new(r#"(?i)<base\b[^>]*href\s*=\s*["']([^"']+)["']"#).ok().and_then(|re| re.captures(html).and_then(|c| page.join(&c[1]).ok())).unwrap_or_else(|| page.clone());
    let s = SCRIPT.replace_all(html, "");
    let s = NOSCRIPT.replace_all(&s, "$1");
    let s = DROP_TAGS.replace_all(&s, "");
    let s = INTEGRITY.replace_all(&s, "");
    let s = ATTR_URL.replace_all(&s, |c: &regex::Captures<'_>| {
        let v = html_unescape(c.get(2).or(c.get(3)).map(|m| m.as_str()).unwrap_or(""));
        let is_link = c[1].to_ascii_lowercase().contains("href");
        format!("{}\"{}\"", &c[1], map_url(&v, &base, file, pages, assets, is_link).replace('"', "%22"))
    });
    let s = ATTR_SRCSET.replace_all(&s, |c: &regex::Captures<'_>| {
        let v = html_unescape(c.get(2).or(c.get(3)).map(|m| m.as_str()).unwrap_or(""));
        let parts: Vec<String> = v
            .split(',')
            .map(|cand| {
                let mut it = cand.split_whitespace();
                let u = it.next().unwrap_or("");
                let rest: Vec<&str> = it.collect();
                format!("{} {}", map_url(u, &base, file, pages, assets, false), rest.join(" ")).trim().to_string()
            })
            .collect();
        format!("{}\"{}\"", &c[1], parts.join(", ").replace('"', "%22"))
    });
    CSS_URL.replace_all(&s, |c: &regex::Captures<'_>| format!("url(\"{}\")", map_url(&html_unescape(&c[2]), &base, file, pages, assets, false))).into_owned()
}

fn rewrite_css(css: &str, u: &Url, file: &str, assets: &HashMap<String, String>) -> String {
    let empty = HashMap::new();
    let s = CSS_URL.replace_all(css, |c: &regex::Captures<'_>| format!("url(\"{}\")", map_url(&c[2], u, file, &empty, assets, false)));
    CSS_IMPORT.replace_all(&s, |c: &regex::Captures<'_>| format!("@import \"{}\"", map_url(&c[1], u, file, &empty, assets, false))).into_owned()
}

/// `rad://localhost/scraped/<id>/<file>`: a file of an offline copy, locked down so nothing in
/// it runs or reaches the internet. `…/__rad_out?u=` is a link that wasn't saved: RAD is told,
/// and opens it in its browser.
pub fn serve<R: tauri::Runtime>(app: &AppHandle<R>, path: &str, query: Option<&str>) -> (u16, String, Vec<u8>, bool) {
    let rest = path.trim_start_matches("/scraped/");
    let (id, file) = rest.split_once('/').unwrap_or((rest, ""));
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return (403, "text/plain".into(), Vec::new(), false);
    }
    let file = percent_decode(file);
    if file.rsplit('/').next() == Some("__rad_out") {
        let target = query.and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("u="))).map(percent_decode).unwrap_or_default();
        if target.starts_with("http://") || target.starts_with("https://") {
            let _ = app.emit("scraper-open-external", json!({ "url": target }));
        }
        let esc = target.replace('&', "&amp;").replace('<', "&lt;");
        let body = format!("<!doctype html><meta charset=utf-8><style>body{{font:15px system-ui;margin:40px;color:#555}}code{{word-break:break-all}}</style><h2>Not in the offline copy</h2><p>This link goes to <code>{esc}</code>, which wasn’t saved. RAD has opened it in its web browser.</p>");
        return (200, "text/html; charset=utf-8".into(), body.into_bytes(), true);
    }
    if file.split('/').any(|s| s == ".." || s.is_empty()) {
        return (403, "text/plain".into(), Vec::new(), false);
    }
    let p = root(app).join(id).join("site").join(&file);
    match std::fs::read(&p) {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&p).first_or_octet_stream();
            let html = mime.essence_str() == "text/html";
            if html {
                // The viewer follows clicks inside the (sandboxed) page.
                let _ = app.emit("scraper-viewing", json!({ "id": id, "file": file }));
            }
            let ct = if mime.type_() == "text" { format!("{}; charset=utf-8", mime.essence_str()) } else { mime.essence_str().to_string() };
            (200, ct, bytes, html)
        }
        Err(_) => (404, "text/html; charset=utf-8".into(), b"<!doctype html><meta charset=utf-8><p style='font:15px system-ui;margin:40px;color:#555'>This file isn&rsquo;t in the offline copy.</p>".to_vec(), true),
    }
}

/// What an offline page may do: show its own saved files, nothing else.
pub const ARCHIVE_CSP: &str = "default-src 'none'; img-src rad: http://rad.localhost data:; media-src rad: http://rad.localhost; style-src rad: http://rad.localhost 'unsafe-inline'; font-src rad: http://rad.localhost data:; form-action 'none'; base-uri 'none'";

// ------------------------------------------------------------------ commands

#[tauri::command]
pub fn scraper_list(app: AppHandle) -> Vec<Site> {
    let mut sites = read_index(&app);
    // A crawl that was running when RAD quit has stopped.
    let running: HashSet<String> = app.state::<Scraper>().jobs.lock().unwrap().keys().cloned().collect();
    for s in &mut sites {
        if s.status == "running" && !running.contains(&s.id) {
            s.status = "stopped".into();
            s.current = None;
        }
    }
    sites.sort_by(|a, b| b.updated.cmp(&a.updated));
    sites
}

/// Starts saving a site (a new one, or `id` again from the start).
#[tauri::command]
pub fn scraper_start(app: AppHandle, state: State<'_, Scraper>, url: String, options: Options, id: Option<String>) -> CmdResult<Site> {
    let mut url = url.trim().to_string();
    if !url.contains("://") {
        url = format!("https://{url}");
    }
    let u = Url::parse(&url).map_err(|_| CmdError::new("bad_request", "That doesn’t look like a web address"))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(CmdError::new("bad_request", "Only http and https addresses can be saved"));
    }
    let host = u.host_str().unwrap_or("").to_string();
    let site = {
        let _g = state.lock.lock().unwrap();
        let mut sites = read_index(&app);
        let now = crate::now_ms();
        let existing = id.as_ref().and_then(|i| sites.iter().position(|s| &s.id == i));
        let site = match existing {
            Some(i) => {
                let s = &mut sites[i];
                if state.jobs.lock().unwrap().contains_key(&s.id) {
                    return Err(CmdError::new("busy", "That site is being saved already"));
                }
                s.url = u.to_string();
                s.options = options;
                s.status = "running".into();
                s.updated = now;
                s.clone()
            }
            None => {
                let s = Site {
                    id: format!("s{now}"),
                    title: host.clone(),
                    url: u.to_string(),
                    host: host.clone(),
                    created: now,
                    updated: now,
                    status: "running".into(),
                    pages: 0,
                    assets: 0,
                    documents: 0,
                    bytes: 0,
                    errors: 0,
                    blocked: 0,
                    queued: 0,
                    current: Some(u.to_string()),
                    message: None,
                    start: None,
                    options,
                    rag: None,
                    rag_stale: false,
                };
                sites.insert(0, s.clone());
                s
            }
        };
        write_index(&app, &sites)?;
        site
    };
    let stop = Arc::new(AtomicBool::new(false));
    state.jobs.lock().unwrap().insert(site.id.clone(), stop.clone());
    tauri::async_runtime::spawn(run_crawl(app.clone(), site.id.clone(), stop));
    Ok(site)
}

#[tauri::command]
pub fn scraper_stop(state: State<'_, Scraper>, id: String) {
    if let Some(f) = state.jobs.lock().unwrap().get(&id) {
        f.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn scraper_delete(app: AppHandle, state: State<'_, Scraper>, id: String) -> CmdResult<()> {
    if let Some(f) = state.jobs.lock().unwrap().remove(&id) {
        f.store(true, Ordering::Relaxed);
    }
    let _g = state.lock.lock().unwrap();
    let sites: Vec<Site> = read_index(&app).into_iter().filter(|s| s.id != id).collect();
    write_index(&app, &sites)?;
    if id.chars().all(|c| c.is_ascii_alphanumeric()) && !id.is_empty() {
        let _ = std::fs::remove_dir_all(root(&app).join(&id));
    }
    Ok(())
}

#[tauri::command]
pub fn scraper_pages(app: AppHandle, id: String) -> Vec<Page> {
    std::fs::read(root(&app).join(&id).join("pages.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// A page's text (its RAG copy), for Gilgamesh.
#[tauri::command]
pub fn scraper_page_text(app: AppHandle, id: String, rag: String) -> CmdResult<String> {
    if rag.contains('/') || rag.contains("..") {
        return Err(CmdError::new("bad_request", "bad file"));
    }
    std::fs::read_to_string(root(&app).join(&id).join("rag").join(rag)).map_err(io)
}

/// The files "Add to RAG" uploads: each page's text, and the documents.
#[tauri::command]
pub fn scraper_rag_files(app: AppHandle, id: String) -> Vec<String> {
    let dir = root(&app).join(&id);
    scraper_pages(app.clone(), id)
        .into_iter()
        .filter_map(|p| match (p.kind.as_str(), p.rag) {
            ("page", Some(r)) => Some(dir.join("rag").join(r)),
            ("document", _) => Some(dir.join("site").join(p.file)),
            _ => None,
        })
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Remembers (or forgets, with no project) the vault copy of a site.
#[tauri::command]
pub fn scraper_set_rag(app: AppHandle, id: String, project: Option<i64>, docs: Vec<i64>) -> CmdResult<Site> {
    update_site(&app, &id, |s| {
        s.rag = project.map(|p| Rag { project: p, docs, added: crate::now_ms() });
        s.rag_stale = false;
    })
    .ok_or_else(|| CmdError::new("not_found", "No such site"))
}

#[tauri::command]
pub fn scraper_folder(app: AppHandle, id: String) -> String {
    root(&app).join(id).join("site").to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_for_addresses() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert_eq!(file_for(&u("https://ex.com/"), true, None), "ex.com/index.html");
        assert_eq!(file_for(&u("https://ex.com/about"), true, None), "ex.com/about/index.html");
        assert_eq!(file_for(&u("https://ex.com/a/b.html"), true, None), "ex.com/a/b.html");
        assert_eq!(file_for(&u("https://ex.com/p.php?id=3"), true, None), format!("ex.com/p.php/index__{}.html", short_hash("id=3")));
        assert_eq!(file_for(&u("https://cdn.ex.com/css/site"), false, Some("text/css; charset=utf-8")), "cdn.ex.com/css/site.css");
        assert_eq!(relative("ex.com/a/b/index.html", "ex.com/c/index.html"), "../../c/index.html");
        assert_eq!(relative("ex.com/index.html", "ex.com/img/x y.png"), "img/x%20y.png");
    }

    #[test]
    fn robots_rules() {
        let r = Robots::parse("User-agent: *\nDisallow: /private\nAllow: /private/ok\nDisallow: /*.pdf$\nCrawl-delay: 2\nSitemap: https://ex.com/s.xml\n\nUser-agent: badbot\nDisallow: /");
        assert!(r.allowed("/"));
        assert!(!r.allowed("/private/x"));
        assert!(r.allowed("/private/ok/1"));
        assert!(!r.allowed("/files/a.pdf"));
        assert!(r.allowed("/files/a.pdf?x"));
        assert_eq!(r.delay, Some(2.0));
        assert_eq!(r.sitemaps, vec!["https://ex.com/s.xml"]);
    }

    #[test]
    fn main_text_and_links() {
        let html = r#"<html><head><title>Hello  World</title><link rel=stylesheet href="/s.css"></head><body>
            <nav><a href="/menu">Menu</a></nav>
            <main><h1>Big title</h1><p>Some <b>bold</b> text and a <a href="/next?utm_source=x#top">link</a>.</p>
            <ul><li>one</li><li>two</li></ul><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>
            <p>More words to make this long enough for the main element to be chosen as the content of the page, which needs two hundred characters at least.</p>
            <img src="/i.png" srcset="/i2.png 2x, /i3.png 3x" alt="A picture"></main>
            <script>alert(1)</script></body></html>"#;
        let p = parse_page(html, &Url::parse("https://ex.com/page").unwrap());
        assert_eq!(p.title, "Hello World");
        assert!(p.links.iter().any(|l| l.as_str() == "https://ex.com/next"));
        assert!(p.assets.iter().any(|l| l.as_str() == "https://ex.com/s.css"));
        assert!(p.assets.iter().any(|l| l.as_str() == "https://ex.com/i3.png"));
        assert!(p.markdown.starts_with("# Big title"));
        assert!(p.markdown.contains("- one"));
        assert!(p.markdown.contains("| A | B |"));
        let code = parse_page("<main><p>Some code follows here, long enough to be the main part of this page for sure, which needs more than two hundred characters of text in it, so here are a few more words.</p><pre>fn main() {\n    let x = 1;\n\n    x\n}</pre></main>", &Url::parse("https://ex.com/").unwrap());
        assert!(code.markdown.contains("fn main() {\n    let x = 1;\n\n    x\n}"), "{}", code.markdown);
        assert!(!p.markdown.contains("Menu"));
        assert!(!p.markdown.contains("alert"));
    }

    #[test]
    fn rewrites_the_copy() {
        let pages: HashMap<String, String> = [("https://ex.com/next".to_string(), "ex.com/next/index.html".to_string())].into();
        let assets: HashMap<String, String> = [("https://ex.com/i.png".to_string(), "ex.com/i.png".to_string())].into();
        let html = r#"<p><a href="/next#top">n</a> <a href="https://other.org/x">o</a><img src="/i.png" srcset="/i.png 1x"><script>x()</script><div style="background:url('/i.png')"></div>"#;
        let out = rewrite_html(html, &Url::parse("https://ex.com/a/page").unwrap(), "ex.com/a/page/index.html", &pages, &assets);
        assert!(out.contains(r#"href="../../next/index.html#top""#), "{out}");
        assert!(out.contains("__rad_out?u=https%3A%2F%2Fother.org%2Fx"), "{out}");
        assert!(out.contains(r#"src="../../i.png""#), "{out}");
        assert!(out.contains(r#"srcset="../../i.png 1x""#), "{out}");
        assert!(!out.contains("<script"));
        assert!(out.contains(r#"url("../../i.png")"#), "{out}");
    }

    /// A whole crawl against a little site on 127.0.0.1: robots.txt, sitemap, styles with
    /// fonts and images, a PDF, a page with a query, a link out.
    #[tokio::test(flavor = "multi_thread")]
    async fn crawls_a_site() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let words = "This page has plenty of words so that it is long enough to be kept as text for the vault, which wants at least forty words of real content before a page counts as worth adding to RAG for Gilgamesh to read later on.";
        let mut files: HashMap<String, (&str, Vec<u8>)> = HashMap::new();
        files.insert("/robots.txt".into(), ("text/plain", format!("User-agent: *\nDisallow: /secret\nSitemap: {base}/sitemap.xml\n").into_bytes()));
        files.insert("/sitemap.xml".into(), ("application/xml", format!("<urlset><url><loc>{base}/</loc></url><url><loc>{base}/orphan</loc></url></urlset>").into_bytes()));
        files.insert("/".into(), ("text/html", format!(r#"<html><head><title>Home</title><link rel="stylesheet" href="/s.css"><script src="/app.js"></script></head><body><nav><a href="/about">About</a></nav><main><h1>Welcome</h1><p>{words}</p><a href="/about">About us</a> <a href="/secret/x">Secret</a> <a href="/docs/guide.pdf">Guide</a> <a href="/list?page=2&utm_source=x">More</a> <a href="https://external.example/">Out</a><img src="/logo.png"></main><script>alert(1)</script></body></html>"#).into_bytes()));
        files.insert("/about".into(), ("text/html; charset=utf-8", format!(r#"<html><head><title>About</title></head><body><article><h1>About</h1><p>{words} About.</p><img srcset="/logo.png 1x, /logo2.png 2x"><a href="/">Home</a></article></body></html>"#).into_bytes()));
        files.insert("/orphan".into(), ("text/html", format!("<html><head><title>Orphan</title></head><body><p>{words} Orphan.</p></body></html>").into_bytes()));
        files.insert("/list?page=2".into(), ("text/html", b"<html><head><title>List 2</title></head><body><p>short</p></body></html>".to_vec()));
        files.insert("/secret/x".into(), ("text/html", b"<html><title>Secret</title></html>".to_vec()));
        files.insert("/s.css".into(), ("text/css", b"body{background:url(bg.png)} @font-face{src:url('/f.woff2')}".to_vec()));
        for f in ["/bg.png", "/logo.png", "/logo2.png", "/f.woff2"] {
            files.insert(f.into(), ("application/octet-stream", vec![1, 2, 3]));
        }
        files.insert("/docs/guide.pdf".into(), ("application/pdf", b"%PDF-1.4 a pdf".to_vec()));
        let files = Arc::new(files);
        let hits = Arc::new(Mutex::new(Vec::<String>::new()));
        let (f2, h2) = (files.clone(), hits.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let (files, hits) = (f2.clone(), h2.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    hits.lock().unwrap().push(path.clone());
                    let resp = match files.get(&path) {
                        Some((ct, body)) => [format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes(), body.clone()].concat(),
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = sock.write_all(&resp).await;
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the only test that sets it; read by root().
        unsafe { std::env::set_var("RAD_SCRAPER_DIR", dir.path()) };
        let app = tauri::test::mock_app();
        app.manage(Scraper::default());
        let h = app.handle().clone();
        let opts: Options = serde_json::from_value(json!({ "delay_ms": 0 })).unwrap();
        let site = Site { id: "s1".into(), title: "127.0.0.1".into(), url: format!("{base}/"), host: "127.0.0.1".into(), created: 0, updated: 0, status: "running".into(), pages: 0, assets: 0, documents: 0, bytes: 0, errors: 0, blocked: 0, queued: 0, current: None, message: None, start: None, options: opts, rag: None, rag_stale: false };
        write_index(&h, &[site]).unwrap();
        run_crawl(h.clone(), "s1".into(), Arc::new(AtomicBool::new(false))).await;

        let s = read_index(&h).remove(0);
        let hits = hits.lock().unwrap().clone();
        assert_eq!(s.status, "done", "{:?}", s.message);
        assert!(!hits.iter().any(|p| p.starts_with("/secret")), "robots.txt was ignored: {hits:?}");
        assert!(!hits.iter().any(|p| p == "/app.js"), "scripts aren't fetched");
        assert_eq!(s.blocked, 1);
        let pages: Vec<Page> = serde_json::from_slice(&std::fs::read(dir.path().join("s1/pages.json")).unwrap()).unwrap();
        let urls: HashSet<String> = pages.iter().map(|p| p.url.clone()).collect();
        for u in ["/", "/about", "/orphan", "/list?page=2", "/docs/guide.pdf"] {
            assert!(urls.contains(&format!("{base}{u}")), "missing {u}: {urls:?}");
        }
        assert_eq!(s.pages, 4);
        assert_eq!(s.documents, 1);
        assert!(s.assets >= 5, "assets {}", s.assets); // s.css, logo, logo2, bg, font
        let home = std::fs::read_to_string(dir.path().join("s1/site/127.0.0.1/index.html")).unwrap();
        assert!(home.contains(r#"href="about/index.html""#), "{home}");
        assert!(home.contains(r#"href="docs/guide.pdf""#), "{home}");
        assert!(home.contains("__rad_out?u=https%3A%2F%2Fexternal.example%2F"), "{home}");
        assert!(home.contains(&format!(r#"href="list/index__{}.html""#, short_hash("page=2"))) || home.contains("index__"), "{home}");
        assert!(!home.contains("<script"), "{home}");
        let css = std::fs::read_to_string(dir.path().join("s1/site/127.0.0.1/s.css")).unwrap();
        assert!(css.contains(r#"url("bg.png")"#) && css.contains(r#"url("f.woff2")"#), "{css}");
        let about = std::fs::read_to_string(dir.path().join("s1/site/127.0.0.1/about/index.html")).unwrap();
        assert!(about.contains(r#"href="../index.html""#) && about.contains("../logo2.png 2x"), "{about}");
        // RAG: the three long pages, each with its source.
        let rag: Vec<&Page> = pages.iter().filter(|p| p.rag.is_some()).collect();
        assert_eq!(rag.len(), 3, "{:?}", rag.iter().map(|p| &p.url).collect::<Vec<_>>());
        let text = std::fs::read_to_string(dir.path().join("s1/rag").join(rag[0].rag.as_ref().unwrap())).unwrap();
        assert!(text.starts_with("# ") && text.contains(&format!("Source: {base}")), "{text}");
        assert!(!text.contains("alert"));
        // Served locked down, links out reported.
        let (st, ct, _, html) = serve(&h, "/scraped/s1/127.0.0.1/index.html", None);
        assert!(st == 200 && ct.starts_with("text/html") && html);
        assert_eq!(serve(&h, "/scraped/s1/../index.json", None).0, 403);
        assert_eq!(serve(&h, "/scraped/s1/__rad_out", Some("u=https%3A%2F%2Fexternal.example%2F")).0, 200);
    }

    /// A real site, a few pages: `RAD_SCRAPE_URL=https://… cargo test -p rad-app real_site -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn real_site() {
        let url = std::env::var("RAD_SCRAPE_URL").expect("set RAD_SCRAPE_URL");
        let dir = std::env::var("RAD_SCRAPER_OUT").map(PathBuf::from).unwrap_or_else(|_| tempfile::tempdir().unwrap().keep());
        unsafe { std::env::set_var("RAD_SCRAPER_DIR", &dir) };
        let app = tauri::test::mock_app();
        app.manage(Scraper::default());
        let h = app.handle().clone();
        let opts: Options = serde_json::from_value(json!({ "max_pages": 20, "scope": "section" })).unwrap();
        let site = Site { id: "real".into(), title: String::new(), url: url.clone(), host: String::new(), created: 0, updated: 0, status: "running".into(), pages: 0, assets: 0, documents: 0, bytes: 0, errors: 0, blocked: 0, queued: 0, current: None, message: None, start: None, options: opts, rag: None, rag_stale: false };
        write_index(&h, &[site]).unwrap();
        let t = Instant::now();
        run_crawl(h.clone(), "real".into(), Arc::new(AtomicBool::new(false))).await;
        let s = read_index(&h).remove(0);
        println!("{} in {:?}: {} pages, {} assets, {} documents, {} KB, {} errors, {} blocked, start {:?}, message {:?}", s.title, t.elapsed(), s.pages, s.assets, s.documents, s.bytes / 1024, s.errors, s.blocked, s.start, s.message);
        let pages: Vec<Page> = serde_json::from_slice(&std::fs::read(dir.join("real/pages.json")).unwrap()).unwrap();
        for p in pages.iter().take(8) {
            println!("  {} words · {} → {}", p.words, p.url, p.file);
        }
        println!("folder: {}", dir.display());
        assert_eq!(s.status, "done");
        assert!(s.pages > 1);
    }
}
