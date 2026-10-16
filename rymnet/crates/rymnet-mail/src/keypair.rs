use anyhow::Result;
use argon2::Argon2;
use x25519_dalek::{PublicKey, StaticSecret};

/// The recipient's long-term mail keypair. Derived from the same
/// username+password as their rymnet account, but with a different
/// domain-separation label and a fully independent Argon2id call, so it is
/// cryptographically unrelated to the account's Ed25519 identity or vfs
/// cipher key -- compromising one does not help an attacker derive the
/// others. Anyone who knows the account's username+password can always
/// recompute this deterministically (no separate secret to lose), and the
/// public half is safe to publish for senders to encrypt to.
pub struct MailboxKeypair {
    pub secret: StaticSecret,
    pub public: PublicKey,
}

pub fn derive_mailbox_keypair(username: &str, password: &str) -> Result<MailboxKeypair> {
    let normalized = username.trim().to_ascii_lowercase();
    let salt = blake3::hash(format!("rymnet.mail.salt.v1|{normalized}").as_bytes());

    let mut seed = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt.as_bytes(), &mut seed)
        .map_err(|err| anyhow::anyhow!("derive mailbox key: {err}"))?;

    let secret = StaticSecret::from(seed);
    let public = PublicKey::from(&secret);
    Ok(MailboxKeypair { secret, public })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_password_dependent() {
        let a = derive_mailbox_keypair("rob", "hunter2").unwrap();
        let b = derive_mailbox_keypair("rob", "hunter2").unwrap();
        assert_eq!(a.public.to_bytes(), b.public.to_bytes());

        let c = derive_mailbox_keypair("rob", "different").unwrap();
        assert_ne!(a.public.to_bytes(), c.public.to_bytes());
    }

    #[test]
    fn independent_from_account_salt_space() {
        // sanity: same inputs, different label than rymnet-account's
        // derive_account, so the two keypairs never coincide by construction
        let mail = derive_mailbox_keypair("rob", "hunter2").unwrap();
        let salt = blake3::hash(b"rymnet.account.salt.v1|rob");
        let mut seed = [0u8; 32];
        Argon2::default()
            .hash_password_into(b"hunter2", salt.as_bytes(), &mut seed)
            .unwrap();
        assert_ne!(mail.secret.to_bytes(), seed);
    }
}
