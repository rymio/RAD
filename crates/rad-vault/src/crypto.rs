use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use zeroize::Zeroizing;

use crate::{Result, VaultError};

// Argon2id parameters for format version 1. ~0.5–1 s on a modern CPU.
const M_COST_KIB: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 4;

pub fn random_salt() -> [u8; 16] {
    let mut s = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut s);
    s
}

pub fn derive_key(passphrase: &str, salt: &[u8; 16]) -> Zeroizing<[u8; 32]> {
    let params = Params::new(M_COST_KIB, T_COST, P_COST, Some(32)).expect("valid argon2 params");
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out[..])
        .expect("argon2 derivation");
    out
}

/// `RAD1-XXXXX-XXXXX-…`: base32 of the 32-byte raw vault key plus a 2-byte checksum.
pub fn encode_recovery_key(key: &[u8; 32]) -> String {
    let mut bytes = Zeroizing::new(Vec::with_capacity(34));
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&blake3::hash(key).as_bytes()[..2]);
    let b32 = Zeroizing::new(data_encoding::BASE32_NOPAD.encode(&bytes));
    let groups: Vec<&str> = b32
        .as_bytes()
        .chunks(5)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!("RAD1-{}", groups.join("-"))
}

pub fn decode_recovery_key(s: &str) -> Result<Zeroizing<[u8; 32]>> {
    let cleaned: Zeroizing<String> = Zeroizing::new(
        s.trim()
            .trim_start_matches("RAD1-")
            .trim_start_matches("rad1-")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect(),
    );
    let bytes = Zeroizing::new(
        data_encoding::BASE32_NOPAD
            .decode(cleaned.as_bytes())
            .map_err(|_| VaultError::BadRecoveryKey)?,
    );
    if bytes.len() != 34 {
        return Err(VaultError::BadRecoveryKey);
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&bytes[..32]);
    if blake3::hash(&key[..]).as_bytes()[..2] != bytes[32..] {
        return Err(VaultError::BadRecoveryKey);
    }
    Ok(key)
}
