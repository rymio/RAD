use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rymnet_core::{NodeIdentity, canonical_json, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("domain must end with .rym")]
    MissingRymSuffix,
    #[error("domain contains an invalid character")]
    InvalidCharacter,
    #[error("domain has an empty label")]
    EmptyLabel,
    #[error("domain labels cannot start or end with hyphen")]
    HyphenBoundary,
    #[error("domain is already claimed")]
    AlreadyClaimed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DomainClaimRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub domain: String,
    pub owner_public_key: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signature: String,
}

impl DomainClaimRecord {
    pub fn create(domain: &str, identity: &NodeIdentity) -> Result<Self> {
        validate_domain(domain)?;
        let created_at = Utc::now();
        let expires_at = created_at + Duration::days(365);
        let unsigned = unsigned_claim_value(domain, &identity.public_key, created_at, expires_at);
        let signature = sign_json_value(&identity.signing_key()?, &unsigned)?;
        Ok(Self {
            record_type: "rymnet.domain.claim".to_string(),
            version: 1,
            domain: domain.to_string(),
            owner_public_key: identity.public_key.clone(),
            created_at,
            expires_at,
            signature,
        })
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    pub fn verify(&self) -> Result<bool> {
        let unsigned = self.unsigned_value();
        verify_json_value(&self.owner_public_key, &self.signature, &unsigned)
    }

    pub fn hash(&self) -> Result<String> {
        let bytes = canonical_json(self)?;
        Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
    }

    pub fn save(&self, claims_dir: impl AsRef<Path>) -> Result<PathBuf> {
        fs::create_dir_all(claims_dir.as_ref())
            .with_context(|| format!("create {}", claims_dir.as_ref().display()))?;
        let path = claim_path(claims_dir, &self.domain);
        let bytes = serde_json::to_vec_pretty(self).context("serialize domain claim")?;
        fs::write(&path, bytes).with_context(|| format!("write claim {}", path.display()))?;
        Ok(path)
    }

    pub fn unsigned_value(&self) -> Value {
        unsigned_claim_value(
            &self.domain,
            &self.owner_public_key,
            self.created_at,
            self.expires_at,
        )
    }
}

pub fn validate_domain(domain: &str) -> Result<()> {
    if !domain.ends_with(".rym") {
        return Err(DomainError::MissingRymSuffix.into());
    }
    if domain
        .chars()
        .any(|ch| !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '.'))
    {
        return Err(DomainError::InvalidCharacter.into());
    }
    for label in domain.split('.') {
        if label.is_empty() {
            return Err(DomainError::EmptyLabel.into());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(DomainError::HyphenBoundary.into());
        }
    }
    Ok(())
}

pub fn claim_domain(
    claims_dir: impl AsRef<Path>,
    domain: &str,
    identity: &NodeIdentity,
) -> Result<DomainClaimRecord> {
    validate_domain(domain)?;
    if let Some(existing) = find_claim(&claims_dir, domain)? {
        if existing.verify()? && !existing.is_expired() {
            return Err(DomainError::AlreadyClaimed.into());
        }
    }
    let claim = DomainClaimRecord::create(domain, identity)?;
    claim.save(claims_dir)?;
    Ok(claim)
}

pub fn find_claim(claims_dir: impl AsRef<Path>, domain: &str) -> Result<Option<DomainClaimRecord>> {
    let path = claim_path(claims_dir, domain);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| format!("read claim {}", path.display()))?;
    let claim = serde_json::from_slice(&bytes).context("parse domain claim")?;
    Ok(Some(claim))
}

pub fn list_claims(claims_dir: impl AsRef<Path>) -> Result<Vec<DomainClaimRecord>> {
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
    claims.sort_by(|a: &DomainClaimRecord, b: &DomainClaimRecord| a.domain.cmp(&b.domain));
    Ok(claims)
}

pub fn winning_claim<'a>(claims: &'a [DomainClaimRecord]) -> Result<Option<&'a DomainClaimRecord>> {
    let mut valid = Vec::new();
    for claim in claims {
        if claim.verify()? && !claim.is_expired() {
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

fn claim_path(claims_dir: impl AsRef<Path>, domain: &str) -> PathBuf {
    claims_dir.as_ref().join(format!("{domain}.json"))
}

fn unsigned_claim_value(
    domain: &str,
    owner_public_key: &str,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Value {
    json!({
        "type": "rymnet.domain.claim",
        "version": 1,
        "domain": domain,
        "owner_public_key": owner_public_key,
        "created_at": created_at,
        "expires_at": expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn validates_domain_rules() {
        assert!(validate_domain("rob.rym").is_ok());
        assert!(validate_domain("relay.sigs.rym").is_ok());
        assert!(validate_domain("my-site.rym").is_ok());
        assert!(validate_domain("Rob.rym").is_err());
        assert!(validate_domain("rob com.rym").is_err());
        assert!(validate_domain("rob_.rym").is_err());
        assert!(validate_domain("rob").is_err());
        assert!(validate_domain("rob.com").is_err());
        assert!(validate_domain("-rob.rym").is_err());
        assert!(validate_domain("rob-.rym").is_err());
        assert!(validate_domain("rob..rym").is_err());
    }

    #[test]
    fn claim_signs_and_verifies() {
        let identity = NodeIdentity::generate();
        let claim = DomainClaimRecord::create("rob.rym", &identity).expect("claim");
        assert!(claim.verify().expect("verify"));
    }

    #[test]
    fn earliest_claim_wins_conflict() {
        let identity = NodeIdentity::generate();
        let first = DomainClaimRecord::create("rob.rym", &identity).expect("first");
        let mut second = DomainClaimRecord::create("rob.rym", &identity).expect("second");
        second.created_at = first.created_at + Duration::seconds(1);
        second.signature =
            sign_json_value(&identity.signing_key().unwrap(), &second.unsigned_value()).unwrap();
        let claims = vec![second, first.clone()];
        let winner = winning_claim(&claims).expect("winner").expect("some");
        assert_eq!(winner.created_at, first.created_at);
    }

    #[test]
    fn local_search_finds_saved_claim() {
        let dir = std::env::temp_dir().join(format!(
            "rymnet-domain-test-{}",
            blake3::hash(b"local-search")
        ));
        let _ = fs::remove_dir_all(&dir);
        let identity = NodeIdentity::generate();
        let claim = claim_domain(&dir, "rob.rym", &identity).expect("claim");
        let found = find_claim(&dir, "rob.rym").expect("find").expect("exists");
        assert_eq!(found.domain, claim.domain);
        let _ = fs::remove_dir_all(&dir);
    }
}
