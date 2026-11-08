//! Model management: a pinned manifest (`models.toml`), resumable downloads with
//! SHA-256 verification, and status reporting for the Models screen.
//!
//! A manifest entry with an empty `sha256` is trust-on-first-use: the hash of the
//! first complete download is recorded next to the file (`<file>.sha256`) and
//! enforced from then on. Pin real hashes in `models.toml` before shipping.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("unknown model `{0}`")]
    Unknown(String),
    #[error("download failed: {0}")]
    Http(String),
    #[error("checksum mismatch for {name}: expected {expected}, got {actual}")]
    Checksum { name: String, expected: String, actual: String },
    #[error("download cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ModelError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Asr,
    Phonemes,
    Vad,
    Coach,
    Tts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub url: String,
    pub file: String,
    #[serde(default)]
    pub sha256: String,
    /// Approximate size for the UI before the server reports one.
    #[serde(default)]
    pub size_mb: u64,
    /// Milestone that first needs this model.
    #[serde(default)]
    pub milestone: String,
    #[serde(default)]
    pub default: bool,
    /// How to install a model that is not a plain download (empty `url`).
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    #[serde(rename = "model")]
    pub models: Vec<ModelEntry>,
}

impl Manifest {
    pub fn parse(toml_text: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(toml_text).map_err(|e| ModelError::Manifest(e.to_string()))?;
        let mut ids = std::collections::HashSet::new();
        for e in &m.models {
            if !ids.insert(&e.id) {
                return Err(ModelError::Manifest(format!("duplicate id `{}`", e.id)));
            }
            if !e.sha256.is_empty() && (e.sha256.len() != 64 || !e.sha256.chars().all(|c| c.is_ascii_hexdigit())) {
                return Err(ModelError::Manifest(format!("`{}` has a malformed sha256", e.id)));
            }
            if e.file.contains('/') || e.file.contains("..") {
                return Err(ModelError::Manifest(format!("`{}` file must be a bare file name", e.id)));
            }
        }
        Ok(m)
    }

    pub fn get(&self, id: &str) -> Result<&ModelEntry> {
        self.models.iter().find(|m| m.id == id).ok_or_else(|| ModelError::Unknown(id.into()))
    }

    /// The default model for a role.
    pub fn default_for(&self, role: &Role) -> Option<&ModelEntry> {
        self.models.iter().find(|m| &m.role == role && m.default).or_else(|| self.models.iter().find(|m| &m.role == role))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Missing,
    Partial { bytes: u64 },
    Ready { bytes: u64, verified: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

pub struct ModelStore {
    dir: PathBuf,
    manifest: Manifest,
}

impl ModelStore {
    pub fn new(dir: impl Into<PathBuf>, manifest: Manifest) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, manifest })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn path(&self, entry: &ModelEntry) -> PathBuf {
        self.dir.join(&entry.file)
    }

    fn partial_path(&self, entry: &ModelEntry) -> PathBuf {
        self.dir.join(format!("{}.part", entry.file))
    }

    fn recorded_hash_path(&self, entry: &ModelEntry) -> PathBuf {
        self.dir.join(format!("{}.sha256", entry.file))
    }

    pub fn status(&self, entry: &ModelEntry) -> Status {
        if let Ok(meta) = fs::metadata(self.path(entry)) {
            let verified = !entry.sha256.is_empty() || self.recorded_hash_path(entry).exists();
            return Status::Ready { bytes: meta.len(), verified };
        }
        match fs::metadata(self.partial_path(entry)) {
            Ok(meta) => Status::Partial { bytes: meta.len() },
            Err(_) => Status::Missing,
        }
    }

    /// Path of a ready model, if downloaded.
    pub fn ready_path(&self, id: &str) -> Option<PathBuf> {
        let entry = self.manifest.get(id).ok()?;
        matches!(self.status(entry), Status::Ready { .. }).then(|| self.path(entry))
    }

    /// Bytes used by all model files (complete and partial).
    pub fn disk_usage(&self) -> u64 {
        fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
            .unwrap_or(0)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let entry = self.manifest.get(id)?;
        for p in [self.path(entry), self.partial_path(entry), self.recorded_hash_path(entry)] {
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
        Ok(())
    }

    /// Download (or resume) a model and verify it. `progress` returns `false` to cancel.
    pub fn download(&self, id: &str, mut progress: impl FnMut(Progress) -> bool) -> Result<PathBuf> {
        let entry = self.manifest.get(id)?.clone();
        let final_path = self.path(&entry);
        if final_path.exists() {
            return Ok(final_path);
        }
        if entry.url.is_empty() {
            return Err(ModelError::Http(if entry.note.is_empty() {
                format!("{} has no download URL", entry.name)
            } else {
                entry.note.clone()
            }));
        }
        let part = self.partial_path(&entry);
        let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

        let client = reqwest::blocking::Client::builder()
            .timeout(None)
            .user_agent(concat!("aussie-accent-coach/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ModelError::Http(e.to_string()))?;
        let mut req = client.get(&entry.url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut resp = req.send().map_err(|e| ModelError::Http(e.to_string()))?;
        let status = resp.status();
        let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
        if !(status.is_success()) {
            return Err(ModelError::Http(format!("{} from {}", status, entry.url)));
        }
        let mut downloaded = if resumed { have } else { 0 };
        let total = resp.content_length().map(|len| len + downloaded);
        let mut file = if resumed {
            OpenOptions::new().append(true).open(&part)?
        } else {
            File::create(&part)?
        };
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = resp.read(&mut buf).map_err(|e| ModelError::Http(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            downloaded += n as u64;
            if !progress(Progress { downloaded, total }) {
                file.flush()?;
                return Err(ModelError::Cancelled);
            }
        }
        file.flush()?;
        drop(file);

        let actual = sha256_file(&part)?;
        let expected = if entry.sha256.is_empty() {
            fs::read_to_string(self.recorded_hash_path(&entry)).ok().map(|s| s.trim().to_lowercase())
        } else {
            Some(entry.sha256.to_lowercase())
        };
        match expected {
            Some(exp) if exp != actual => {
                fs::remove_file(&part)?;
                return Err(ModelError::Checksum { name: entry.name, expected: exp, actual });
            }
            None => {
                tracing::warn!(model = %entry.id, sha256 = %actual, "no pinned hash; recording trust-on-first-use hash");
                fs::write(self.recorded_hash_path(&entry), &actual)?;
            }
            _ => {}
        }
        fs::rename(&part, &final_path)?;
        Ok(final_path)
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests;
