//! Products (sidebar → Products): the owner's products (a SaaS platform and its iOS and Android
//! apps), each with everything the App Store, Google Play and its website need: the detailed
//! description, the store listings, the icon, screenshots and feature graphic. It's all kept on
//! this computer in the app's data folder:
//!
//!   products/<id>/product.json     the product (its shape is lib/products.svelte.ts's `Product`)
//!   products/<id>/assets/…         icon, screenshots, feature graphic (PNG/JPEG as given)
//!   products/<id>/rag/…            the Markdown copy that "Add to RAG" uploads to the vault
//!   products/_guides/…             App Store / Google Play guides, fetched ahead (works offline)
//!   products/gnu.json              the gnu.au API address (the token is in the keychain)
//!
//! The bots that fill in App Store Connect and Play Console get a copy of the assets in their VM
//! (`products_to_container`). The gnu.au REST API (api/README.md in gnu.au) is called from here,
//! so the token never reaches the web view and there's no CORS.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Response;
use tauri::{AppHandle, Manager, Runtime, State};
use url::Url;

use crate::client::{CmdError, CmdResult};
use crate::{secrets, App};

const TOKEN_NAME: &str = "gnu-au-api-token";
const DEFAULT_BASE: &str = "https://gnu.au/api/v1";
/// Biggest picture taken in (Apple's previews aside, nothing in a listing comes near it).
const MAX_ASSET: u64 = 40 * 1024 * 1024;
const MAX_GUIDE: usize = 6 * 1024 * 1024;
const UA: &str = "Mozilla/5.0 (compatible; RAD-Products/1.0; store guide for offline reading)";

#[derive(Default)]
pub struct Products {
    /// Guards the product files (saves from the UI and bots' copies).
    lock: Mutex<()>,
}

fn io(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("io", e.to_string())
}

pub fn root<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    if let Ok(d) = std::env::var("RAD_PRODUCTS_DIR") {
        return PathBuf::from(d);
    }
    app.path().app_data_dir().unwrap_or_else(|_| ".".into()).join("products")
}

fn valid_id(id: &str) -> CmdResult<()> {
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || id.starts_with('_') {
        return Err(CmdError::new("bad_request", "invalid product id"));
    }
    Ok(())
}

/// A file name inside one of a product's folders: no paths, nothing hidden.
fn valid_name(name: &str) -> CmdResult<()> {
    if name.is_empty() || name.len() > 120 || name.starts_with('.') || name.contains(['/', '\\', '\0']) || name.contains("..") {
        return Err(CmdError::new("bad_request", "invalid file name"));
    }
    Ok(())
}

fn product_dir<R: Runtime>(app: &AppHandle<R>, id: &str) -> CmdResult<PathBuf> {
    valid_id(id)?;
    Ok(root(app).join(id))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> CmdResult<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(io)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

/// Keeps letters, digits, dots, dashes and underscores (spaces become dashes).
fn clean_name(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '-' }).collect();
    let s = s.trim_matches(['-', '.']).to_string();
    let mut out = String::new();
    for c in s.chars() {
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out.chars().take(80).collect()
}

// ------------------------------------------------------------------ products

#[tauri::command]
pub fn products_list(app: AppHandle) -> Vec<Value> {
    let mut out: Vec<Value> = std::fs::read_dir(root(&app))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('_'))
        .filter_map(|e| std::fs::read(e.path().join("product.json")).ok())
        .filter_map(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|v| v.get("id").and_then(Value::as_str).is_some())
        .collect();
    out.sort_by(|a, b| b["updated"].as_f64().unwrap_or(0.0).total_cmp(&a["updated"].as_f64().unwrap_or(0.0)));
    out
}

#[tauri::command]
pub fn products_save(app: AppHandle, state: State<'_, Products>, product: Value) -> CmdResult<Value> {
    let id = product.get("id").and_then(Value::as_str).ok_or_else(|| CmdError::new("bad_request", "the product has no id"))?.to_string();
    let dir = product_dir(&app, &id)?;
    let _g = state.lock.lock().unwrap();
    write_atomic(&dir.join("product.json"), &serde_json::to_vec_pretty(&product).map_err(io)?)?;
    Ok(product)
}

#[tauri::command]
pub fn products_delete(app: AppHandle, state: State<'_, Products>, id: String) -> CmdResult<()> {
    let dir = product_dir(&app, &id)?;
    let _g = state.lock.lock().unwrap();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(io)?;
    }
    Ok(())
}

#[tauri::command]
pub fn products_folder(app: AppHandle, id: String) -> CmdResult<String> {
    let dir = product_dir(&app, &id)?;
    std::fs::create_dir_all(dir.join("assets")).map_err(io)?;
    Ok(dir.to_string_lossy().into_owned())
}

// ------------------------------------------------------------------ assets

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub kind: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    /// The picture has an alpha channel (Apple's icon and Play's feature graphic mustn't).
    pub alpha: bool,
    pub added: i64,
}

/// Whether a PNG has an alpha channel (colour type 4 or 6, or a tRNS chunk). JPEGs never do.
fn png_alpha(bytes: &[u8]) -> bool {
    if bytes.len() < 33 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return false;
    }
    if matches!(bytes[25], 4 | 6) {
        return true;
    }
    // Chunks after IHDR, up to the first IDAT: a tRNS one means transparency.
    let mut i = 8usize;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let kind = &bytes[i + 4..i + 8];
        if kind == b"tRNS" {
            return true;
        }
        if kind == b"IDAT" {
            break;
        }
        i += 12 + len;
    }
    false
}

fn read_asset(path: &Path, name: &str, kind: &str) -> CmdResult<Asset> {
    let meta = std::fs::metadata(path).map_err(io)?;
    let (width, height) = image::image_dimensions(path).map_err(|e| CmdError::new("bad_image", format!("{name}: not a PNG or JPEG picture ({e})")))?;
    let mut head = vec![0u8; 64 * 1024];
    let n = std::io::Read::read(&mut std::fs::File::open(path).map_err(io)?, &mut head).map_err(io)?;
    head.truncate(n);
    Ok(Asset { name: name.to_string(), kind: kind.to_string(), width, height, size: meta.len(), alpha: png_alpha(&head), added: chrono::Utc::now().timestamp_millis() })
}

/// Copies pictures from this computer into the product (PNG and JPEG only, which is what both
/// stores take). Returns one entry per path: the asset, or {error}.
#[tauri::command]
pub fn products_add_assets(app: AppHandle, id: String, paths: Vec<String>, kind: String) -> CmdResult<Vec<Value>> {
    let dir = product_dir(&app, &id)?.join("assets");
    std::fs::create_dir_all(&dir).map_err(io)?;
    let kind = clean_name(&kind);
    let mut out = Vec::new();
    for p in paths {
        let src = PathBuf::from(&p);
        let file = src.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
            out.push(json!({ "name": file, "error": "Only PNG and JPEG pictures (what the App Store and Google Play take)" }));
            continue;
        }
        if std::fs::metadata(&src).map(|m| m.len() > MAX_ASSET).unwrap_or(true) {
            out.push(json!({ "name": file, "error": "Missing, or bigger than 40 MB" }));
            continue;
        }
        let stem = clean_name(src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().as_str());
        let stem = if stem.is_empty() { "picture".to_string() } else { stem };
        let mut name = format!("{kind}-{stem}.{ext}");
        let mut n = 2;
        while dir.join(&name).exists() {
            name = format!("{kind}-{stem}-{n}.{ext}");
            n += 1;
        }
        let dest = dir.join(&name);
        if let Err(e) = std::fs::copy(&src, &dest) {
            out.push(json!({ "name": file, "error": e.to_string() }));
            continue;
        }
        match read_asset(&dest, &name, &kind) {
            Ok(a) => out.push(serde_json::to_value(a).map_err(io)?),
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                out.push(json!({ "name": file, "error": e.message }));
            }
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn products_asset(app: AppHandle, id: String, name: String) -> CmdResult<Response> {
    valid_name(&name)?;
    let path = product_dir(&app, &id)?.join("assets").join(&name);
    Ok(Response::new(tokio::fs::read(path).await.map_err(io)?))
}

#[tauri::command]
pub fn products_remove_asset(app: AppHandle, id: String, name: String) -> CmdResult<()> {
    valid_name(&name)?;
    let path = product_dir(&app, &id)?.join("assets").join(&name);
    if path.exists() {
        std::fs::remove_file(path).map_err(io)?;
    }
    Ok(())
}

/// Writes a text file of the product (the RAG copy, the brief a bot reads) and returns its path.
#[tauri::command]
pub fn products_write_text(app: AppHandle, id: String, name: String, content: String) -> CmdResult<String> {
    valid_name(&name)?;
    let path = product_dir(&app, &id)?.join("rag").join(&name);
    write_atomic(&path, content.as_bytes())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Gives a bot's computer the product: its assets and `files` (name → text, e.g. the listing a
/// bot types in) go in `/root/shared/products/<id>/`, which is what's returned.
#[tauri::command]
pub fn products_to_container(app: AppHandle, id: String, container: String, files: std::collections::BTreeMap<String, String>) -> CmdResult<String> {
    if container.is_empty() || !container.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(CmdError::new("bad_request", "invalid container"));
    }
    let src = product_dir(&app, &id)?.join("assets");
    let shared = app.path().app_data_dir().map_err(io)?.join("containers").join(&container).join("shared");
    if !shared.is_dir() {
        return Err(CmdError::new("not_found", "That bot's computer doesn't exist yet: start the bot once first"));
    }
    let dest = shared.join("products").join(&id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(io)?;
    }
    std::fs::create_dir_all(&dest).map_err(io)?;
    for e in std::fs::read_dir(&src).into_iter().flatten().flatten() {
        if e.path().is_file() {
            std::fs::copy(e.path(), dest.join(e.file_name())).map_err(io)?;
        }
    }
    for (name, text) in files {
        valid_name(&name)?;
        std::fs::write(dest.join(&name), text).map_err(io)?;
    }
    Ok(format!("/root/shared/products/{id}"))
}

// ------------------------------------------------------------------ store guides

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Guide {
    pub url: String,
    pub title: String,
    pub store: String,
    pub path: String,
    pub words: u32,
    pub fetched: i64,
}

fn guides_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    root(app).join("_guides")
}

fn read_guides<R: Runtime>(app: &AppHandle<R>) -> Vec<Guide> {
    std::fs::read(guides_dir(app).join("index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

#[tauri::command]
pub fn products_guides(app: AppHandle) -> Vec<Guide> {
    read_guides(&app).into_iter().filter(|g| Path::new(&g.path).exists()).collect()
}

#[derive(Deserialize)]
pub struct GuideSource {
    pub url: String,
    /// "apple" or "google".
    pub store: String,
}

/// Fetches the store guides (one page each, no crawling) as Markdown, so they can be read and
/// put in RAG while offline. Returns one entry per source: the guide, or {url, error}.
#[tauri::command]
pub async fn products_fetch_guides(app: AppHandle, sources: Vec<GuideSource>) -> CmdResult<Vec<Value>> {
    let client = reqwest::Client::builder().user_agent(UA).timeout(Duration::from_secs(40)).build().map_err(io)?;
    let dir = guides_dir(&app);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let mut index = read_guides(&app);
    let mut out = Vec::new();
    for s in sources {
        let Ok(url) = Url::parse(&s.url) else {
            out.push(json!({ "url": s.url, "error": "not a web address" }));
            continue;
        };
        let got = async {
            let resp = client.get(url.as_str()).header("Accept-Language", "en").send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status().as_u16()));
            }
            let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let final_url = resp.url().clone();
            let body = resp.bytes().await.map_err(|e| e.to_string())?;
            if body.len() > MAX_GUIDE {
                return Err("too big".into());
            }
            Ok::<_, String>(crate::scraper::page_markdown(&body, &ct, &final_url))
        }
        .await;
        match got {
            Ok((_, _, words)) if words < 60 => out.push(json!({ "url": s.url, "error": "The page had almost no text (it may need JavaScript)" })),
            Ok((title, md, words)) => {
                let store = if s.store == "google" { "google" } else { "apple" };
                let slug = clean_name(&format!("{store}-{}", url.path().trim_matches('/').replace('/', "-")));
                let path = dir.join(format!("{}.md", if slug.is_empty() { store.to_string() } else { slug }));
                let now = chrono::Local::now();
                let doc = format!(
                    "# {title}\n\n_{} guide, fetched by RAD Products on {} from {}_\n\n{md}\n",
                    if store == "google" { "Google Play" } else { "Apple App Store" },
                    now.format("%-d %B %Y"),
                    s.url
                );
                write_atomic(&path, doc.as_bytes())?;
                let g = Guide { url: s.url.clone(), title, store: store.into(), path: path.to_string_lossy().into_owned(), words, fetched: now.timestamp_millis() };
                index.retain(|x| x.url != g.url);
                index.push(g.clone());
                out.push(serde_json::to_value(g).map_err(io)?);
            }
            Err(e) => out.push(json!({ "url": s.url, "error": e })),
        }
    }
    write_atomic(&dir.join("index.json"), &serde_json::to_vec_pretty(&index).map_err(io)?)?;
    Ok(out)
}

#[tauri::command]
pub fn products_guide_text(app: AppHandle, url: String) -> CmdResult<String> {
    let g = read_guides(&app).into_iter().find(|g| g.url == url).ok_or_else(|| CmdError::new("not_found", "That guide hasn't been fetched"))?;
    std::fs::read_to_string(&g.path).map_err(io)
}

// ------------------------------------------------------------------ gnu.au

#[derive(Serialize, Deserialize, Default)]
struct GnuConfig {
    base: String,
}

fn gnu_config<R: Runtime>(app: &AppHandle<R>) -> GnuConfig {
    let mut c: GnuConfig = std::fs::read(root(app).join("gnu.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if c.base.trim().is_empty() {
        c.base = DEFAULT_BASE.into();
    }
    c.base = c.base.trim().trim_end_matches('/').to_string();
    c
}

#[tauri::command]
pub fn gnu_status(app: AppHandle, st: State<'_, App>) -> Value {
    json!({ "base": gnu_config(&app).base, "has_token": secrets::get_named(&st.cfg_dir, TOKEN_NAME).is_some() })
}

/// Saves the API address and (unless `token` is None) the token; an empty token forgets it.
#[tauri::command]
pub fn gnu_configure(app: AppHandle, st: State<'_, App>, base: String, token: Option<String>) -> CmdResult<Value> {
    let base = base.trim().trim_end_matches('/');
    let base = if base.is_empty() { DEFAULT_BASE } else { base };
    let u = Url::parse(base).map_err(|_| CmdError::new("bad_request", "The API address isn't a web address"))?;
    if !(u.scheme() == "https" || (u.scheme() == "http" && matches!(u.host_str(), Some("localhost") | Some("127.0.0.1")))) {
        return Err(CmdError::new("bad_request", "Use https (http only for localhost)"));
    }
    write_atomic(&root(&app).join("gnu.json"), &serde_json::to_vec(&GnuConfig { base: base.into() }).map_err(io)?)?;
    match token.map(|t| t.trim().to_string()) {
        Some(t) if t.is_empty() => secrets::delete_named(&st.cfg_dir, TOKEN_NAME),
        Some(t) => {
            if !t.starts_with("gnu_") {
                return Err(CmdError::new("bad_request", "A gnu.au token starts with gnu_ (Account → API tokens)"));
            }
            secrets::set_named(&st.cfg_dir, TOKEN_NAME, &t)?;
        }
        None => {}
    }
    Ok(gnu_status(app, st))
}

fn gnu_target<R: Runtime>(app: &AppHandle<R>, st: &App, path: &str) -> CmdResult<(String, String)> {
    let token = secrets::get_named(&st.cfg_dir, TOKEN_NAME).ok_or_else(|| CmdError::new("no_token", "Add your gnu.au API token first (Products → Settings)"))?;
    // Only paths under the API: no scheme, no host, no climbing out.
    if !path.starts_with('/') || path.starts_with("//") || path.contains("..") || path.contains("://") {
        return Err(CmdError::new("bad_request", "invalid API path"));
    }
    Ok((format!("{}{}", gnu_config(app).base, path), token))
}

async fn answer(resp: reqwest::Response) -> CmdResult<Value> {
    let status = resp.status().as_u16();
    let retry = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(String::from);
    let text = resp.text().await.map_err(io)?;
    let body = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text.chars().take(2000).collect())) };
    Ok(json!({ "status": status, "body": body, "retry_after": retry }))
}

/// One call to the gnu.au REST API: {status, body, retry_after}. HTTP errors come back as a
/// status (their body says what was wrong); only a failure to reach the server is an error.
#[tauri::command]
pub async fn gnu_request(app: AppHandle, st: State<'_, App>, method: String, path: String, body: Option<Value>) -> CmdResult<Value> {
    let (url, token) = gnu_target(&app, &st, &path)?;
    let method = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes()).map_err(|_| CmdError::new("bad_request", "invalid method"))?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(90)).build().map_err(io)?;
    let mut req = client.request(method, &url).bearer_auth(token).header("Accept", "application/json");
    if let Some(b) = body {
        req = req.json(&b);
    }
    answer(req.send().await.map_err(|e| CmdError::new("offline", format!("Can't reach gnu.au: {e}")))?).await
}

/// Uploads some of a product's assets to a gnu.au site (POST /sites/{id}/assets/, multipart).
#[tauri::command]
pub async fn gnu_upload(app: AppHandle, st: State<'_, App>, site: i64, id: String, names: Vec<String>) -> CmdResult<Value> {
    let (url, token) = gnu_target(&app, &st, &format!("/sites/{site}/assets/"))?;
    let dir = product_dir(&app, &id)?.join("assets");
    let boundary = format!("----rad{}", blake3::hash(format!("{url}{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)).as_bytes()).to_hex()[..24].to_string());
    let mut body = Vec::new();
    for n in &names {
        valid_name(n)?;
        let bytes = tokio::fs::read(dir.join(n)).await.map_err(io)?;
        let mime = mime_guess::from_path(n).first_or_octet_stream();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{n}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let client = reqwest::Client::builder().timeout(Duration::from_secs(300)).build().map_err(io)?;
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .map_err(|e| CmdError::new("offline", format!("Can't reach gnu.au: {e}")))?;
    answer(resp).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_ids_stay_inside() {
        for bad in ["", "../x", "a/b", ".hidden", "a\\b", "x..y"] {
            assert!(valid_name(bad).is_err(), "{bad}");
        }
        assert!(valid_name("icon-app.png").is_ok());
        for bad in ["", "_guides", "a/b", "a.b", "../x"] {
            assert!(valid_id(bad).is_err(), "{bad}");
        }
        assert!(valid_id("a1b2-c3").is_ok());
    }

    #[test]
    fn cleans_file_names() {
        assert_eq!(clean_name("My Screen shot (1)"), "My-Screen-shot-1");
        assert_eq!(clean_name("..x"), "x");
    }

    #[test]
    fn png_alpha_is_read_from_the_header() {
        let mut rgba = Vec::new();
        image::DynamicImage::new_rgba8(2, 2).write_to(&mut std::io::Cursor::new(&mut rgba), image::ImageFormat::Png).unwrap();
        let mut rgb = Vec::new();
        image::DynamicImage::new_rgb8(2, 2).write_to(&mut std::io::Cursor::new(&mut rgb), image::ImageFormat::Png).unwrap();
        assert!(png_alpha(&rgba));
        assert!(!png_alpha(&rgb));
        assert!(!png_alpha(b"\xff\xd8\xff not a png"));
    }

    #[test]
    fn guide_pages_become_markdown() {
        let html = format!("<html><head><title>Guide</title></head><body><nav>Menu</nav><main><h1>App name</h1><p>{}</p></main></body></html>", "Your app name can be up to 30 characters. ".repeat(20));
        let (title, md, words) = crate::scraper::page_markdown(html.as_bytes(), "text/html", &Url::parse("https://example.com/g").unwrap());
        assert_eq!(title, "Guide");
        assert!(md.contains("# App name"));
        assert!(words > 60);
    }

    /// Fetches the real store guides (network): `cargo test -p rad-app --lib products -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn real_guides_have_text() {
        let urls = [
            "https://developer.apple.com/app-store/review/guidelines/",
            "https://developer.apple.com/help/app-store-connect/reference/app-information/platform-version-information/",
            "https://developer.apple.com/help/app-store-connect/reference/app-information/screenshot-specifications/",
            "https://developer.apple.com/app-store/product-page/",
            "https://developer.apple.com/app-store/app-privacy-details/",
            "https://support.google.com/googleplay/android-developer/answer/9859152?hl=en",
            "https://support.google.com/googleplay/android-developer/answer/9866151?hl=en",
            "https://support.google.com/googleplay/android-developer/answer/10787469?hl=en",
        ];
        let client = reqwest::Client::builder().user_agent(UA).build().unwrap();
        for u in urls {
            let r = client.get(u).send().await.unwrap();
            let ct = r.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let url = r.url().clone();
            let status = r.status();
            let b = r.bytes().await.unwrap();
            let (title, md, words) = crate::scraper::page_markdown(&b, &ct, &url);
            println!("{status} {words:>6} words  {title}  ({u})\n    {}", md.chars().filter(|c| *c != '\n').skip(0).take(160).collect::<String>());
        }
    }
}
