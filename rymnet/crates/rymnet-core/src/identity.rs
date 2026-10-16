use crate::node::NodeId;
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("identity file already exists: {0}")]
    AlreadyExists(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeIdentity {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub public_key: String,
    pub private_key: String,
    pub created_at: DateTime<Utc>,
}

impl NodeIdentity {
    pub fn generate() -> Self {
        let mut rng = OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        let verifying_key = signing_key.verifying_key();

        Self {
            record_type: "rymnet.node.identity".to_string(),
            version: 1,
            public_key: STANDARD.encode(verifying_key.to_bytes()),
            private_key: STANDARD.encode(signing_key.to_bytes()),
            created_at: Utc::now(),
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path.as_ref())
            .with_context(|| format!("read node identity at {}", path.as_ref().display()))?;
        serde_json::from_slice(&bytes).context("parse node identity")
    }

    pub fn save_new(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if path.exists() {
            return Err(IdentityError::AlreadyExists(path.display().to_string()).into());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("serialize node identity")?;
        write_private_file(path, &bytes)
    }

    pub fn signing_key(&self) -> Result<SigningKey> {
        let private_key = STANDARD
            .decode(&self.private_key)
            .context("decode private key")?;
        let private_key: [u8; 32] = private_key
            .try_into()
            .map_err(|_| anyhow::anyhow!("private key must be 32 bytes"))?;
        Ok(SigningKey::from_bytes(&private_key))
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey> {
        let public_key = STANDARD
            .decode(&self.public_key)
            .context("decode public key")?;
        let public_key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?;
        VerifyingKey::from_bytes(&public_key).context("parse public key")
    }

    pub fn node_id(&self) -> Result<NodeId> {
        Ok(NodeId::from_public_key(&self.verifying_key()?.to_bytes()))
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("create private file {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identity_has_node_id() {
        let identity = NodeIdentity::generate();
        let node_id = identity.node_id().expect("node id");
        assert!(node_id.as_str().starts_with("rym1"));
    }
}
