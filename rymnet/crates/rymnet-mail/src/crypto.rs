use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use rand::rngs::OsRng as RandOsRng;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

/// An anonymously-encrypted ("sealed box") message: the sender uses a
/// fresh, single-use X25519 keypair to perform ECDH with the recipient's
/// long-term public key, derives a symmetric key from that shared secret,
/// and encrypts with it. The sender never needs the recipient's password,
/// and -- because the ephemeral key is discarded immediately after use --
/// not even the sender can decrypt the message again from the ephemeral
/// key alone once it leaves memory.
pub struct Sealed {
    pub ephemeral_public: String,
    pub nonce: String,
    pub ciphertext: Vec<u8>,
}

pub fn seal(recipient_public: &PublicKey, plaintext: &[u8]) -> Result<Sealed> {
    let ephemeral_secret = EphemeralSecret::random_from_rng(RandOsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(recipient_public);
    let key_bytes = blake3::hash(shared.as_bytes());

    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes.as_bytes()));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("seal failed"))?;

    Ok(Sealed {
        ephemeral_public: STANDARD.encode(ephemeral_public.to_bytes()),
        nonce: STANDARD.encode(nonce),
        ciphertext,
    })
}

pub fn open(
    recipient_secret: &StaticSecret,
    ephemeral_public: &str,
    nonce: &str,
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let ephemeral_public_bytes: [u8; 32] = STANDARD
        .decode(ephemeral_public)
        .context("decode ephemeral public key")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("ephemeral public key must be 32 bytes"))?;
    let ephemeral_public = PublicKey::from(ephemeral_public_bytes);

    let shared = recipient_secret.diffie_hellman(&ephemeral_public);
    let key_bytes = blake3::hash(shared.as_bytes());

    let nonce_bytes = STANDARD.decode(nonce).context("decode nonce")?;
    if nonce_bytes.len() != 24 {
        anyhow::bail!("nonce must be 24 bytes");
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes.as_bytes()));
    cipher
        .decrypt(XNonce::from_slice(&nonce_bytes), ciphertext)
        .map_err(|_| anyhow::anyhow!("open failed: wrong key or corrupted message"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let recipient_secret = StaticSecret::from([7u8; 32]);
        let recipient_public = PublicKey::from(&recipient_secret);

        let sealed = seal(&recipient_public, b"hello mailbox").expect("seal");
        let opened = open(
            &recipient_secret,
            &sealed.ephemeral_public,
            &sealed.nonce,
            &sealed.ciphertext,
        )
        .expect("open");
        assert_eq!(opened, b"hello mailbox");
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let recipient_secret = StaticSecret::from([7u8; 32]);
        let recipient_public = PublicKey::from(&recipient_secret);
        let attacker_secret = StaticSecret::from([9u8; 32]);

        let sealed = seal(&recipient_public, b"secret").expect("seal");
        assert!(
            open(
                &attacker_secret,
                &sealed.ephemeral_public,
                &sealed.nonce,
                &sealed.ciphertext,
            )
            .is_err()
        );
    }
}
