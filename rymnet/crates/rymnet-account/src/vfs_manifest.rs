use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// One encrypted file inside a user's virtual computer. `chunk_hash` points
/// at the ciphertext blob in the content-addressed chunk store; `nonce` is
/// required to decrypt it with the account's derived cipher key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VfsFileEntry {
    pub path: String,
    pub chunk_hash: String,
    pub nonce: String,
    pub plaintext_size: u64,
    pub ciphertext_size: u64,
}

/// The plaintext listing of a user's virtual computer. This struct itself is
/// encrypted as a single blob before it ever leaves the machine, so peers
/// hosting it cannot see filenames, only the resulting ciphertext size.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlainVfsManifest {
    pub vfs_version: u64,
    pub created_at: DateTime<Utc>,
    pub files: Vec<VfsFileEntry>,
}

impl PlainVfsManifest {
    pub fn total_ciphertext_size(&self) -> u64 {
        self.files.iter().map(|file| file.ciphertext_size).sum()
    }
}

/// A signed, public record announcing where to find a user's latest
/// encrypted virtual computer on the network. Reveals only the username,
/// version, timestamp, and total size — never file names or contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VfsPointerRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub username: String,
    pub owner_public_key: String,
    pub vfs_version: u64,
    pub manifest_chunk_hash: String,
    pub manifest_nonce: String,
    pub total_bytes: u64,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

impl VfsPointerRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        username: &str,
        identity: &NodeIdentity,
        vfs_version: u64,
        manifest_chunk_hash: &str,
        manifest_nonce: &str,
        total_bytes: u64,
    ) -> Result<Self> {
        let created_at = Utc::now();
        let unsigned = unsigned_pointer_value(
            username,
            &identity.public_key,
            vfs_version,
            manifest_chunk_hash,
            manifest_nonce,
            total_bytes,
            created_at,
        );
        let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
        Ok(Self {
            record_type: "rymnet.account.vfs_pointer".to_string(),
            version: 1,
            username: username.to_string(),
            owner_public_key: identity.public_key.clone(),
            vfs_version,
            manifest_chunk_hash: manifest_chunk_hash.to_string(),
            manifest_nonce: manifest_nonce.to_string(),
            total_bytes,
            created_at,
            signature,
        })
    }

    /// Verifies the signature and that it was actually signed by the
    /// account's claimed owner key (pass the matching `AccountClaimRecord`'s
    /// `owner_public_key`).
    pub fn verify_owned_by(&self, owner_public_key: &str) -> Result<bool> {
        if self.owner_public_key != owner_public_key {
            return Ok(false);
        }
        verify_json_value(
            &self.owner_public_key,
            &self.signature,
            &self.unsigned_value(),
        )
    }

    pub fn unsigned_value(&self) -> Value {
        unsigned_pointer_value(
            &self.username,
            &self.owner_public_key,
            self.vfs_version,
            &self.manifest_chunk_hash,
            &self.manifest_nonce,
            self.total_bytes,
            self.created_at,
        )
    }

    pub fn save(&self, pointers_dir: impl AsRef<Path>) -> Result<PathBuf> {
        fs::create_dir_all(pointers_dir.as_ref())
            .with_context(|| format!("create {}", pointers_dir.as_ref().display()))?;
        let path = pointer_path(pointers_dir, &self.username);
        fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write pointer {}", path.display()))?;
        Ok(path)
    }
}

pub fn find_pointer(
    pointers_dir: impl AsRef<Path>,
    username: &str,
) -> Result<Option<VfsPointerRecord>> {
    let path = pointer_path(pointers_dir, username);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read pointer {}", path.display()))?;
    Ok(Some(
        serde_json::from_slice(&bytes).context("parse vfs pointer")?,
    ))
}

fn pointer_path(pointers_dir: impl AsRef<Path>, username: &str) -> PathBuf {
    pointers_dir.as_ref().join(format!("{username}.json"))
}

#[allow(clippy::too_many_arguments)]
fn unsigned_pointer_value(
    username: &str,
    owner_public_key: &str,
    vfs_version: u64,
    manifest_chunk_hash: &str,
    manifest_nonce: &str,
    total_bytes: u64,
    created_at: DateTime<Utc>,
) -> Value {
    json!({
        "type": "rymnet.account.vfs_pointer",
        "version": 1,
        "username": username,
        "owner_public_key": owner_public_key,
        "vfs_version": vfs_version,
        "manifest_chunk_hash": manifest_chunk_hash,
        "manifest_nonce": manifest_nonce,
        "total_bytes": total_bytes,
        "created_at": created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_signs_and_verifies() {
        let identity = NodeIdentity::generate();
        let pointer = VfsPointerRecord::create("rob", &identity, 1, "blake3:aa", "nonce", 42)
            .expect("create");
        assert!(
            pointer
                .verify_owned_by(&identity.public_key)
                .expect("verify")
        );

        let other = NodeIdentity::generate();
        assert!(!pointer.verify_owned_by(&other.public_key).expect("verify"));
    }
}
