use anyhow::Result;
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use ed25519_dalek::SigningKey;
use rymnet_core::NodeIdentity;

use crate::crypto::CIPHER_KEY_LEN;

/// Everything derived from a username+password pair. Both the node identity
/// (used to sign the account claim and the vfs pointer) and the symmetric
/// key used to encrypt file contents come from the same Argon2id output, so
/// logging in with the same username+password anywhere on the network
/// regenerates identical keys. Nothing here is ever stored or sent as-is.
pub struct DerivedAccount {
    pub identity: NodeIdentity,
    pub cipher_key: [u8; CIPHER_KEY_LEN],
}

pub fn derive_account(username: &str, password: &str) -> Result<DerivedAccount> {
    let normalized = username.trim().to_ascii_lowercase();
    let salt = blake3::hash(format!("rymnet.account.salt.v1|{normalized}").as_bytes());

    let mut output = [0u8; 64];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt.as_bytes(), &mut output)
        .map_err(|err| anyhow::anyhow!("derive account keys: {err}"))?;

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&output[0..32]);
    let mut cipher_key = [0u8; CIPHER_KEY_LEN];
    cipher_key.copy_from_slice(&output[32..64]);

    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    let identity = NodeIdentity {
        record_type: "rymnet.account.identity".to_string(),
        version: 1,
        public_key: STANDARD.encode(verifying_key.to_bytes()),
        private_key: STANDARD.encode(signing_key.to_bytes()),
        created_at: Utc::now(),
    };

    Ok(DerivedAccount {
        identity,
        cipher_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_username_password_yields_same_keys() {
        let a = derive_account("rob", "correct horse battery staple").expect("derive a");
        let b = derive_account("rob", "correct horse battery staple").expect("derive b");
        assert_eq!(a.identity.public_key, b.identity.public_key);
        assert_eq!(a.identity.private_key, b.identity.private_key);
        assert_eq!(a.cipher_key, b.cipher_key);
    }

    #[test]
    fn different_password_yields_different_keys() {
        let a = derive_account("rob", "password one").expect("derive a");
        let b = derive_account("rob", "password two").expect("derive b");
        assert_ne!(a.identity.public_key, b.identity.public_key);
        assert_ne!(a.cipher_key, b.cipher_key);
    }

    #[test]
    fn different_username_yields_different_keys_for_same_password() {
        let a = derive_account("rob", "same password").expect("derive a");
        let b = derive_account("alice", "same password").expect("derive b");
        assert_ne!(a.identity.public_key, b.identity.public_key);
        assert_ne!(a.cipher_key, b.cipher_key);
    }
}
