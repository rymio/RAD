use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};

pub const CIPHER_KEY_LEN: usize = 32;

/// Encrypts `plaintext` with a fresh random nonce. Returns the ciphertext
/// (including the AEAD authentication tag) and the base64-encoded nonce that
/// must be stored alongside it to decrypt later.
pub fn encrypt(cipher_key: &[u8; CIPHER_KEY_LEN], plaintext: &[u8]) -> Result<(Vec<u8>, String)> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(cipher_key));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
    Ok((ciphertext, STANDARD.encode(nonce)))
}

pub fn decrypt(
    cipher_key: &[u8; CIPHER_KEY_LEN],
    ciphertext: &[u8],
    nonce_b64: &str,
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(cipher_key));
    let nonce_bytes = STANDARD.decode(nonce_b64).context("decode nonce")?;
    if nonce_bytes.len() != 24 {
        bail!("nonce must be 24 bytes");
    }
    let nonce = XNonce::from_slice(&nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed: wrong password or corrupted data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let key = [7u8; CIPHER_KEY_LEN];
        let (ciphertext, nonce) = encrypt(&key, b"hello rymnet vfs").expect("encrypt");
        let plaintext = decrypt(&key, &ciphertext, &nonce).expect("decrypt");
        assert_eq!(plaintext, b"hello rymnet vfs");
    }

    #[test]
    fn wrong_key_fails() {
        let key = [1u8; CIPHER_KEY_LEN];
        let other = [2u8; CIPHER_KEY_LEN];
        let (ciphertext, nonce) = encrypt(&key, b"secret").expect("encrypt");
        assert!(decrypt(&other, &ciphertext, &nonce).is_err());
    }
}
