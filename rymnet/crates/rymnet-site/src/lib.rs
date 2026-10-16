use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, canonical_json, sign_json_value, verify_json_value};
use rymnet_storage::ChunkStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SiteError {
    #[error("site path must stay inside the site folder")]
    UnsafePath,
    #[error("absolute paths are not allowed")]
    AbsolutePath,
    #[error("site contains no index.html")]
    MissingIndex,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SiteRef {
    pub domain: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SiteValidation {
    pub accepted: Vec<String>,
    pub rejected: Vec<FileRejection>,
}

impl SiteValidation {
    pub fn is_ok(&self) -> bool {
        self.rejected.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRejection {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SiteManifest {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub domain: String,
    pub site_version: u64,
    pub created_at: DateTime<Utc>,
    pub root_path: String,
    pub files: Vec<ManifestFile>,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub mime: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct PublishResult {
    pub manifest: SiteManifest,
    pub manifest_hash: String,
    pub manifest_path: PathBuf,
    pub files_hashed: usize,
}

pub fn create_site(domain: &str, sites_root: impl AsRef<Path>) -> Result<PathBuf> {
    let source_dir = source_dir(sites_root, domain);
    fs::create_dir_all(&source_dir).with_context(|| format!("create {}", source_dir.display()))?;

    let index_path = source_dir.join("index.html");
    if !index_path.exists() {
        fs::write(&index_path, default_index(domain))
            .with_context(|| format!("write {}", index_path.display()))?;
    }

    let style_path = source_dir.join("style.css");
    if !style_path.exists() {
        fs::write(&style_path, DEFAULT_STYLE)
            .with_context(|| format!("write {}", style_path.display()))?;
    }

    Ok(source_dir)
}

pub fn upload_site(
    domain: &str,
    sites_root: impl AsRef<Path>,
    upload_dir: impl AsRef<Path>,
) -> Result<SiteValidation> {
    let upload_dir = upload_dir.as_ref();
    let validation = validate_folder(upload_dir)?;
    if !validation.is_ok() {
        return Ok(validation);
    }

    let source_dir = source_dir(sites_root, domain);
    if source_dir.exists() {
        fs::remove_dir_all(&source_dir)
            .with_context(|| format!("replace {}", source_dir.display()))?;
    }
    fs::create_dir_all(&source_dir).with_context(|| format!("create {}", source_dir.display()))?;

    for relative in &validation.accepted {
        let relative_path = Path::new(relative.trim_start_matches('/'));
        let from = upload_dir.join(relative_path);
        let to = source_dir.join(relative_path);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::copy(&from, &to)
            .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
    }

    Ok(validation)
}

pub fn validate_site_source(domain: &str, sites_root: impl AsRef<Path>) -> Result<SiteValidation> {
    validate_folder(source_dir(sites_root, domain))
}

pub fn validate_folder(root: impl AsRef<Path>) -> Result<SiteValidation> {
    let root = root.as_ref();
    let mut validation = SiteValidation::default();
    let mut has_index = false;

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if entry.file_type().is_dir() {
            continue;
        }

        let relative = safe_relative_path(root, entry.path())?;
        let display = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
        if relative == Path::new("index.html") {
            has_index = true;
        }

        match validate_file(entry.path(), &relative) {
            Ok(()) => validation.accepted.push(display),
            Err(err) => validation.rejected.push(FileRejection {
                path: display,
                reason: err.to_string(),
            }),
        }
    }

    validation.accepted.sort();
    validation.rejected.sort_by(|a, b| a.path.cmp(&b.path));

    if !has_index {
        validation.rejected.push(FileRejection {
            path: "/index.html".to_string(),
            reason: SiteError::MissingIndex.to_string(),
        });
    }

    Ok(validation)
}

pub fn publish_site(
    domain: &str,
    sites_root: impl AsRef<Path>,
    chunks_root: impl AsRef<Path>,
    identity: &NodeIdentity,
) -> Result<PublishResult> {
    let sites_root = sites_root.as_ref();
    let validation = validate_site_source(domain, sites_root)?;
    if !validation.is_ok() {
        anyhow::bail!("site validation failed; run `rym site validate {domain}` for details");
    }

    let source_dir = source_dir(sites_root, domain);
    let store = ChunkStore::new(chunks_root.as_ref());
    let mut files = Vec::new();
    for relative in validation.accepted {
        let relative_path = Path::new(relative.trim_start_matches('/'));
        let bytes = fs::read(source_dir.join(relative_path))
            .with_context(|| format!("read {}", relative_path.display()))?;
        let hash = store.put(&bytes)?;
        let mime = mime_guess::from_path(relative_path)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        files.push(ManifestFile {
            path: relative,
            mime,
            hash,
            size: bytes.len() as u64,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let site_version = next_site_version(sites_root, domain)?;
    let created_at = Utc::now();
    let unsigned = unsigned_manifest_value(domain, site_version, created_at, &files);
    let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
    let manifest = SiteManifest {
        record_type: "rymnet.site.manifest".to_string(),
        version: 1,
        domain: domain.to_string(),
        site_version,
        created_at,
        root_path: "/".to_string(),
        files,
        signature,
    };
    let manifest_hash = manifest.hash()?;
    let manifest_dir = manifests_dir(sites_root, domain);
    fs::create_dir_all(&manifest_dir)
        .with_context(|| format!("create {}", manifest_dir.display()))?;
    let manifest_path = manifest_dir.join(format!(
        "{}.json",
        manifest_hash.trim_start_matches("blake3:")
    ));
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let published_dir = published_dir(sites_root, domain);
    fs::create_dir_all(&published_dir)
        .with_context(|| format!("create {}", published_dir.display()))?;
    fs::write(
        published_dir.join("latest_manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .with_context(|| format!("write latest manifest for {domain}"))?;

    Ok(PublishResult {
        files_hashed: manifest.files.len(),
        manifest,
        manifest_hash,
        manifest_path,
    })
}

pub fn save_replicated_manifest(
    sites_root: impl AsRef<Path>,
    manifest: &SiteManifest,
) -> Result<PathBuf> {
    let manifest_hash = manifest.hash()?;
    let manifest_dir = manifests_dir(&sites_root, &manifest.domain);
    fs::create_dir_all(&manifest_dir)
        .with_context(|| format!("create {}", manifest_dir.display()))?;
    let manifest_path = manifest_dir.join(format!(
        "{}.json",
        manifest_hash.trim_start_matches("blake3:")
    ));
    fs::write(&manifest_path, serde_json::to_vec_pretty(manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let published_dir = published_dir(&sites_root, &manifest.domain);
    fs::create_dir_all(&published_dir)
        .with_context(|| format!("create {}", published_dir.display()))?;
    fs::write(
        published_dir.join("latest_manifest.json"),
        serde_json::to_vec_pretty(manifest)?,
    )
    .with_context(|| format!("write latest manifest for {}", manifest.domain))?;
    Ok(manifest_path)
}

pub fn set_pin(sites_root: impl AsRef<Path>, domain: &str, pinned: bool) -> Result<()> {
    let pin_path = sites_root.as_ref().join(domain).join("pinned.json");
    if pinned {
        if let Some(parent) = pin_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(
            &pin_path,
            serde_json::to_vec_pretty(&json!({
                "type": "rymnet.site.pin",
                "version": 1,
                "domain": domain,
                "pinned_at": Utc::now(),
            }))?,
        )
        .with_context(|| format!("write {}", pin_path.display()))?;
    } else if pin_path.exists() {
        fs::remove_file(&pin_path).with_context(|| format!("remove {}", pin_path.display()))?;
    }
    Ok(())
}

pub fn is_pinned(sites_root: impl AsRef<Path>, domain: &str) -> bool {
    sites_root
        .as_ref()
        .join(domain)
        .join("pinned.json")
        .exists()
}

pub fn load_latest_manifest(
    domain: &str,
    sites_root: impl AsRef<Path>,
) -> Result<Option<SiteManifest>> {
    let path = published_dir(sites_root, domain).join("latest_manifest.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse latest site manifest")
}

pub fn list_latest_manifests(sites_root: impl AsRef<Path>) -> Result<Vec<SiteManifest>> {
    let sites_root = sites_root.as_ref();
    if !sites_root.exists() {
        return Ok(Vec::new());
    }

    let mut manifests = Vec::new();
    for entry in
        fs::read_dir(sites_root).with_context(|| format!("read {}", sites_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let domain = entry.file_name().to_string_lossy().to_string();
        if let Some(manifest) = load_latest_manifest(&domain, sites_root)? {
            manifests.push(manifest);
        }
    }
    manifests.sort_by(|a, b| a.domain.cmp(&b.domain));
    Ok(manifests)
}

pub fn manifest_file_for_path<'a>(
    manifest: &'a SiteManifest,
    request_path: &str,
) -> Option<&'a ManifestFile> {
    let normalized = normalize_site_path(request_path).ok()?;
    manifest.files.iter().find(|file| file.path == normalized)
}

pub fn normalize_site_path(path: &str) -> Result<String> {
    let mut path = path.trim();
    if path.is_empty() || path == "/" {
        path = "/index.html";
    }
    if !path.starts_with('/') {
        anyhow::bail!("site path must begin with /");
    }
    if path.contains('\\') || path.contains("//") || path.contains('\0') {
        anyhow::bail!("unsafe site path");
    }
    let relative = Path::new(path.trim_start_matches('/'));
    validate_safe_components(relative)?;
    if path.ends_with('/') {
        Ok(format!("{path}index.html"))
    } else {
        Ok(path.to_string())
    }
}

impl SiteManifest {
    pub fn verify(&self, owner_public_key: &str) -> Result<bool> {
        verify_json_value(owner_public_key, &self.signature, &self.unsigned_value())
    }

    pub fn hash(&self) -> Result<String> {
        let bytes = canonical_json(self)?;
        Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
    }

    pub fn unsigned_value(&self) -> Value {
        unsigned_manifest_value(
            &self.domain,
            self.site_version,
            self.created_at,
            &self.files,
        )
    }
}

pub fn source_dir(sites_root: impl AsRef<Path>, domain: &str) -> PathBuf {
    sites_root.as_ref().join(domain).join("source")
}

pub fn published_dir(sites_root: impl AsRef<Path>, domain: &str) -> PathBuf {
    sites_root.as_ref().join(domain).join("published")
}

pub fn manifests_dir(sites_root: impl AsRef<Path>, domain: &str) -> PathBuf {
    sites_root.as_ref().join(domain).join("manifests")
}

fn next_site_version(sites_root: &Path, domain: &str) -> Result<u64> {
    let dir = manifests_dir(sites_root, domain);
    if !dir.exists() {
        return Ok(1);
    }
    let mut count = 0;
    for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            count += 1;
        }
    }
    Ok(count + 1)
}

fn validate_file(path: &Path, relative: &Path) -> Result<()> {
    validate_safe_components(relative)?;
    let ext = relative
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
        anyhow::bail!("file extension .{ext} is not allowed");
    }

    match ext.as_str() {
        "html" | "htm" => validate_html(
            &fs::read_to_string(path).with_context(|| format!("read HTML {}", path.display()))?,
        ),
        "css" => validate_css(
            &fs::read_to_string(path).with_context(|| format!("read CSS {}", path.display()))?,
        ),
        _ => Ok(()),
    }
}

fn validate_html(html: &str) -> Result<()> {
    let lower = html.to_ascii_lowercase();
    let blocked = [
        ("<script", "HTML script tags are not allowed"),
        ("<iframe", "HTML iframe tags are not allowed"),
        ("<object", "HTML object tags are not allowed"),
        ("<embed", "HTML embed tags are not allowed"),
        ("<form", "HTML forms are not allowed in v0.1"),
        ("javascript:", "javascript: URLs are not allowed"),
        ("<link", "external remote CSS references are not allowed"),
    ];
    for (needle, reason) in blocked {
        if lower.contains(needle) {
            if needle == "<link" && !contains_remote_stylesheet(&lower) {
                continue;
            }
            anyhow::bail!(reason);
        }
    }
    if contains_inline_event_handler(&lower) {
        anyhow::bail!("inline event handlers are not allowed");
    }
    Ok(())
}

fn validate_css(css: &str) -> Result<()> {
    let lower = css.to_ascii_lowercase();
    for (needle, reason) in [
        ("url(http://", "remote CSS URLs are not allowed"),
        ("url(https://", "remote CSS URLs are not allowed"),
        ("@import http://", "remote CSS imports are not allowed"),
        ("@import https://", "remote CSS imports are not allowed"),
        ("expression(", "CSS expression() is not allowed"),
        ("javascript:", "javascript: in CSS is not allowed"),
    ] {
        if lower.contains(needle) {
            anyhow::bail!(reason);
        }
    }
    Ok(())
}

fn contains_remote_stylesheet(html: &str) -> bool {
    html.split('<').any(|tag| {
        let tag = tag.trim_start();
        tag.starts_with("link")
            && tag.contains("stylesheet")
            && (tag.contains("href=\"http://")
                || tag.contains("href='http://")
                || tag.contains("href=\"https://")
                || tag.contains("href='https://"))
    })
}

fn contains_inline_event_handler(html: &str) -> bool {
    let explicit = [
        "onload=",
        "onclick=",
        "onerror=",
        "onmouseover=",
        "onfocus=",
    ];
    if explicit.iter().any(|needle| html.contains(needle)) {
        return true;
    }

    html.split('<').any(|tag| {
        tag.split_whitespace().skip(1).any(|attr| {
            let attr = attr.trim_start_matches('/');
            attr.starts_with("on") && attr.contains('=')
        })
    })
}

fn safe_relative_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| SiteError::UnsafePath)?
        .to_path_buf();
    validate_safe_components(&relative)?;
    Ok(relative)
}

fn validate_safe_components(path: &Path) -> Result<()> {
    if path.is_absolute() {
        return Err(SiteError::AbsolutePath.into());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(SiteError::UnsafePath.into()),
        }
    }
    Ok(())
}

fn unsigned_manifest_value(
    domain: &str,
    site_version: u64,
    created_at: DateTime<Utc>,
    files: &[ManifestFile],
) -> Value {
    let files = files
        .iter()
        .map(|file| {
            let mut map = BTreeMap::new();
            map.insert("path", json!(file.path));
            map.insert("mime", json!(file.mime));
            map.insert("hash", json!(file.hash));
            map.insert("size", json!(file.size));
            json!(map)
        })
        .collect::<Vec<_>>();

    json!({
        "type": "rymnet.site.manifest",
        "version": 1,
        "domain": domain,
        "site_version": site_version,
        "created_at": created_at,
        "root_path": "/",
        "files": files,
    })
}

fn default_index(domain: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>{domain}</title>
  <link rel="stylesheet" href="/style.css">
</head>
<body>
  <main>
    <h1>Welcome to {domain}</h1>
    <p>This site is hosted on Rymnet.</p>
  </main>
</body>
</html>
"#
    )
}

const DEFAULT_STYLE: &str = r#"body {
  font-family: system-ui, sans-serif;
  max-width: 760px;
  margin: 40px auto;
  padding: 0 20px;
  line-height: 1.5;
}
"#;

const ALLOWED_EXTENSIONS: &[&str] = &[
    "html", "htm", "css", "png", "jpg", "jpeg", "webp", "gif", "txt", "ico",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_html_javascript() {
        assert!(validate_html(r#"<script>alert("bad")</script>"#).is_err());
        assert!(validate_html(r#"<a href="javascript:alert(1)">bad</a>"#).is_err());
        assert!(validate_html(r#"<img src="/x.png" onerror="bad()">"#).is_err());
        assert!(validate_html(r#"<form></form>"#).is_err());
    }

    #[test]
    fn rejects_css_remote_imports() {
        assert!(validate_css(r#"@import https://example.com/a.css;"#).is_err());
        assert!(validate_css(r#"body { background: url(http://example.com/a.png); }"#).is_err());
        assert!(validate_css(r#"body { color: red; }"#).is_ok());
    }

    #[test]
    fn creates_manifest_and_verifies_signature() {
        let temp =
            std::env::temp_dir().join(format!("rymnet-site-test-{}", blake3::hash(b"manifest")));
        let _ = fs::remove_dir_all(&temp);
        let sites = temp.join("sites");
        let chunks = temp.join("chunks");
        let identity = NodeIdentity::generate();

        create_site("rob.rym", &sites).expect("create");
        let result = publish_site("rob.rym", &sites, &chunks, &identity).expect("publish");
        assert_eq!(result.files_hashed, 2);
        assert!(
            result
                .manifest
                .verify(&identity.public_key)
                .expect("verify")
        );
        assert!(result.manifest_hash.starts_with("blake3:"));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn normalizes_gateway_paths() {
        assert_eq!(normalize_site_path("/").unwrap(), "/index.html");
        assert_eq!(normalize_site_path("/about.html").unwrap(), "/about.html");
        assert!(normalize_site_path("/../secret.txt").is_err());
        assert!(normalize_site_path("/a//b.html").is_err());
    }
}
