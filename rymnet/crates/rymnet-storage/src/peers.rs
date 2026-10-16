use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PeerConfig {
    #[serde(default)]
    pub peers: Vec<PeerEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerEntry {
    pub name: String,
    pub public_key: String,
    pub endpoint: String,
}

impl PeerConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            fs::read_to_string(path).with_context(|| format!("read peers {}", path.display()))?;
        if text.trim().is_empty() || text.trim_start().starts_with('#') {
            return Ok(Self::default());
        }
        toml::from_str(&text).context("parse peers")
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(path, toml::to_string_pretty(self)?)
            .with_context(|| format!("write peers {}", path.display()))
    }

    pub fn add(&mut self, peer: PeerEntry) {
        self.peers.retain(|existing| existing.name != peer.name);
        self.peers.push(peer);
        self.peers.sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|peer| peer.name != name);
        self.peers.len() != before
    }

    pub fn find(&self, name: &str) -> Option<&PeerEntry> {
        self.peers.iter().find(|peer| peer.name == name)
    }
}
