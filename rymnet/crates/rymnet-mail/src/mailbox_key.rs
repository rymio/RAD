use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use x25519_dalek::PublicKey;

/// A signed, publishable record announcing a username's mail encryption
/// public key. Signed with the same Ed25519 identity that owns the
/// username claim, so a sender can verify "this key really belongs to the
/// account named rob" by checking it against rob's already-verified
/// `AccountClaimRecord`, exactly the way a vfs pointer is checked.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailboxKeyRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub username: String,
    pub owner_public_key: String,
    pub mailbox_public_key: String,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

impl MailboxKeyRecord {
    pub fn create(
        username: &str,
        identity: &NodeIdentity,
        mailbox_public: &PublicKey,
    ) -> Result<Self> {
        let created_at = Utc::now();
        let mailbox_public_key = STANDARD.encode(mailbox_public.to_bytes());
        let unsigned = unsigned_value(
            username,
            &identity.public_key,
            &mailbox_public_key,
            created_at,
        );
        let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
        Ok(Self {
            record_type: "rymnet.mail.mailbox_key".to_string(),
            version: 1,
            username: username.to_string(),
            owner_public_key: identity.public_key.clone(),
            mailbox_public_key,
            created_at,
            signature,
        })
    }

    pub fn verify(&self) -> Result<bool> {
        verify_json_value(
            &self.owner_public_key,
            &self.signature,
            &self.unsigned_value(),
        )
    }

    pub fn public_key(&self) -> Result<PublicKey> {
        let bytes: [u8; 32] = STANDARD
            .decode(&self.mailbox_public_key)
            .context("decode mailbox public key")?
            .try_into()
            .map_err(|_| anyhow::anyhow!("mailbox public key must be 32 bytes"))?;
        Ok(PublicKey::from(bytes))
    }

    pub fn unsigned_value(&self) -> Value {
        unsigned_value(
            &self.username,
            &self.owner_public_key,
            &self.mailbox_public_key,
            self.created_at,
        )
    }

    pub fn save(&self, dir: impl AsRef<Path>) -> Result<PathBuf> {
        fs::create_dir_all(dir.as_ref())
            .with_context(|| format!("create {}", dir.as_ref().display()))?;
        let path = record_path(dir, &self.username);
        fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write mailbox key {}", path.display()))?;
        Ok(path)
    }
}

pub fn find_mailbox_key(dir: impl AsRef<Path>, username: &str) -> Result<Option<MailboxKeyRecord>> {
    let path = record_path(dir, username);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(Some(
        serde_json::from_slice(&bytes).context("parse mailbox key")?,
    ))
}

fn record_path(dir: impl AsRef<Path>, username: &str) -> PathBuf {
    dir.as_ref().join(format!("{username}.json"))
}

fn unsigned_value(
    username: &str,
    owner_public_key: &str,
    mailbox_public_key: &str,
    created_at: DateTime<Utc>,
) -> Value {
    json!({
        "type": "rymnet.mail.mailbox_key",
        "version": 1,
        "username": username,
        "owner_public_key": owner_public_key,
        "mailbox_public_key": mailbox_public_key,
        "created_at": created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_mailbox_keypair;

    #[test]
    fn signs_and_verifies() {
        let identity = NodeIdentity::generate();
        let keypair = derive_mailbox_keypair("rob", "hunter2").unwrap();
        let record = MailboxKeyRecord::create("rob", &identity, &keypair.public).unwrap();
        assert!(record.verify().unwrap());
        assert_eq!(
            record.public_key().unwrap().to_bytes(),
            keypair.public.to_bytes()
        );
    }
}
