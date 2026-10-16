use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, canonical_json, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccountError {
    #[error(
        "username must be 3-32 lowercase letters, digits, '-' or '_', and cannot start/end with '-' or '_'"
    )]
    InvalidUsername,
    #[error("username is already claimed by a different key")]
    AlreadyClaimed,
    #[error("username is registered elsewhere on the network under a different key")]
    Conflict,
}

/// A signed proof that `owner_public_key` registered `username` first.
/// Mirrors `rymnet_domain::DomainClaimRecord`: earliest `created_at` wins on
/// conflict, exactly like `.rym` domain claims.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountClaimRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub username: String,
    pub owner_public_key: String,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

impl AccountClaimRecord {
    pub fn create(username: &str, identity: &NodeIdentity) -> Result<Self> {
        validate_username(username)?;
        let created_at = Utc::now();
        let unsigned = unsigned_claim_value(username, &identity.public_key, created_at);
        let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
        Ok(Self {
            record_type: "rymnet.account.claim".to_string(),
            version: 1,
            username: username.to_string(),
            owner_public_key: identity.public_key.clone(),
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

    pub fn hash(&self) -> Result<String> {
        let bytes = canonical_json(self)?;
        Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
    }

    pub fn unsigned_value(&self) -> Value {
        unsigned_claim_value(&self.username, &self.owner_public_key, self.created_at)
    }

    pub fn save(&self, claims_dir: impl AsRef<Path>) -> Result<PathBuf> {
        fs::create_dir_all(claims_dir.as_ref())
            .with_context(|| format!("create {}", claims_dir.as_ref().display()))?;
        let path = claim_path(claims_dir, &self.username);
        let bytes = serde_json::to_vec_pretty(self).context("serialize account claim")?;
        fs::write(&path, bytes).with_context(|| format!("write claim {}", path.display()))?;
        Ok(path)
    }
}

pub fn validate_username(username: &str) -> Result<()> {
    let len_ok = (3..=32).contains(&username.len());
    let chars_ok = username
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    let boundary_ok = !username.starts_with(['-', '_']) && !username.ends_with(['-', '_']);
    if !len_ok || !chars_ok || !boundary_ok {
        return Err(AccountError::InvalidUsername.into());
    }
    Ok(())
}

pub fn find_claim(
    claims_dir: impl AsRef<Path>,
    username: &str,
) -> Result<Option<AccountClaimRecord>> {
    let path = claim_path(claims_dir, username);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read claim {}", path.display()))?;
    Ok(Some(
        serde_json::from_slice(&bytes).context("parse account claim")?,
    ))
}

pub fn list_claims(claims_dir: impl AsRef<Path>) -> Result<Vec<AccountClaimRecord>> {
    let claims_dir = claims_dir.as_ref();
    if !claims_dir.exists() {
        return Ok(Vec::new());
    }
    let mut claims = Vec::new();
    for entry in
        fs::read_dir(claims_dir).with_context(|| format!("read {}", claims_dir.display()))?
    {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(entry.path())?;
        claims.push(serde_json::from_slice(&bytes)?);
    }
    claims.sort_by(|a: &AccountClaimRecord, b: &AccountClaimRecord| a.username.cmp(&b.username));
    Ok(claims)
}

pub fn winning_claim(claims: &[AccountClaimRecord]) -> Result<Option<&AccountClaimRecord>> {
    let mut valid = Vec::new();
    for claim in claims {
        if claim.verify()? {
            valid.push(claim);
        }
    }
    valid.sort_by(|a, b| match a.created_at.cmp(&b.created_at) {
        Ordering::Equal => {
            let a_hash = a.hash().unwrap_or_default();
            let b_hash = b.hash().unwrap_or_default();
            a_hash.cmp(&b_hash)
        }
        order => order,
    });
    Ok(valid.into_iter().next())
}

/// Registers `username` for `identity` locally. If a valid claim already
/// exists for a different key, registration fails rather than overwriting
/// it, matching how `.rym` domains are claimed.
pub fn claim_username(
    claims_dir: impl AsRef<Path>,
    username: &str,
    identity: &NodeIdentity,
) -> Result<AccountClaimRecord> {
    validate_username(username)?;
    if let Some(existing) = find_claim(&claims_dir, username)? {
        if existing.verify()? {
            if existing.owner_public_key != identity.public_key {
                return Err(AccountError::AlreadyClaimed.into());
            }
            return Ok(existing);
        }
    }
    let claim = AccountClaimRecord::create(username, identity)?;
    claim.save(claims_dir)?;
    Ok(claim)
}

/// Merges a claim received from a peer into local storage using the
/// earliest-created-at-wins rule. Returns `AccountError::Conflict` if the
/// incoming claim loses to an already-accepted claim from a different key,
/// so callers can tell the user their username/password pair collided with
/// someone else's on the network.
pub fn merge_remote_claim(
    claims_dir: impl AsRef<Path>,
    incoming: &AccountClaimRecord,
) -> Result<AccountClaimRecord> {
    validate_username(&incoming.username)?;
    if !incoming.verify()? {
        anyhow::bail!("account claim signature invalid");
    }
    let merged = match find_claim(&claims_dir, &incoming.username)? {
        Some(existing) if existing.verify()? => {
            let candidates = [existing, incoming.clone()];
            winning_claim(&candidates)?
                .cloned()
                .expect("at least one valid claim")
        }
        _ => incoming.clone(),
    };
    merged.save(&claims_dir)?;
    if merged.owner_public_key != incoming.owner_public_key {
        return Err(AccountError::Conflict.into());
    }
    Ok(merged)
}

fn claim_path(claims_dir: impl AsRef<Path>, username: &str) -> PathBuf {
    claims_dir.as_ref().join(format!("{username}.json"))
}

fn unsigned_claim_value(
    username: &str,
    owner_public_key: &str,
    created_at: DateTime<Utc>,
) -> Value {
    json!({
        "type": "rymnet.account.claim",
        "version": 1,
        "username": username,
        "owner_public_key": owner_public_key,
        "created_at": created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_username_rules() {
        assert!(validate_username("rob").is_ok());
        assert!(validate_username("rob-99").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("-rob").is_err());
        assert!(validate_username("rob_").is_err());
        assert!(validate_username("Rob").is_err());
        assert!(validate_username("rob rob").is_err());
    }

    #[test]
    fn claim_signs_and_verifies() {
        let identity = NodeIdentity::generate();
        let claim = AccountClaimRecord::create("rob", &identity).expect("claim");
        assert!(claim.verify().expect("verify"));
    }

    #[test]
    fn earliest_claim_wins_conflict() {
        let a = NodeIdentity::generate();
        let b = NodeIdentity::generate();
        let first = AccountClaimRecord::create("rob", &a).expect("first");
        let mut second = AccountClaimRecord::create("rob", &b).expect("second");
        second.created_at = first.created_at + chrono::Duration::seconds(1);
        second.signature =
            sign_json_value(&b.signing_key().unwrap(), &second.unsigned_value()).unwrap();

        let dir =
            std::env::temp_dir().join(format!("rymnet-account-test-{}", blake3::hash(b"conflict")));
        let _ = fs::remove_dir_all(&dir);
        first.save(&dir).expect("save first");

        let result = merge_remote_claim(&dir, &second);
        assert!(matches!(
            result.unwrap_err().downcast::<AccountError>().unwrap(),
            AccountError::Conflict
        ));
        let stored = find_claim(&dir, "rob").expect("find").expect("exists");
        assert_eq!(stored.owner_public_key, first.owner_public_key);
        let _ = fs::remove_dir_all(&dir);
    }
}
