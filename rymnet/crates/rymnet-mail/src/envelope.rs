use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::crypto::{open, seal};

/// Everything that's kept private, sealed inside the envelope's
/// ciphertext. `to`/`from`/timestamps live in the envelope itself in the
/// clear (a relaying peer has to know who to file mail under), but the
/// actual content of a message is nobody's business but the two ends.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailBody {
    pub subject: String,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    pub body: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub attachment: Option<AttachmentRef>,
}

/// A pointer to a file's bytes, not the bytes themselves -- those are
/// sealed *separately* from this `MailBody` (a second, independent
/// `crate::crypto::seal` call, its own ephemeral keypair/nonce) and
/// stored content-addressed in the sender's `ChunkStore` under
/// `ciphertext_hash`, then pushed to peers via a `file.chunk.offer`
/// broadcast alongside this mail message. Being inside `MailBody` means
/// this reference is only ever visible after the *mail* envelope itself
/// has been opened -- but unsealing the file needs its own
/// `ephemeral_public_key`/`nonce` here, since it was encrypted as its
/// own, independently-sized ciphertext blob, not embedded in this JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub ciphertext_hash: String,
    pub ephemeral_public_key: String,
    pub nonce: String,
}

/// A single mail message, sealed for one recipient and signed by the
/// sender's account identity. Relaying peers can verify the signature
/// (proving internal consistency) without being able to read a word of
/// the subject or body; only the recipient, decrypting with their
/// password-derived mailbox secret key, ever sees the plaintext.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailEnvelope {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub id: String,
    pub to: String,
    pub from: String,
    pub from_public_key: String,
    pub ephemeral_public_key: String,
    pub nonce: String,
    pub ciphertext: String,
    pub sent_at: DateTime<Utc>,
    pub signature: String,
}

impl MailEnvelope {
    pub fn create(
        to: &str,
        from: &str,
        from_identity: &NodeIdentity,
        recipient_mailbox_public: &PublicKey,
        body: &MailBody,
    ) -> Result<Self> {
        let plaintext = serde_json::to_vec(body).context("serialize mail body")?;
        let sealed = seal(recipient_mailbox_public, &plaintext)?;
        let id = uuid::Uuid::new_v4().to_string();
        let sent_at = Utc::now();
        let ciphertext = STANDARD.encode(&sealed.ciphertext);

        let unsigned = unsigned_value(
            &id,
            to,
            from,
            &from_identity.public_key,
            &sealed.ephemeral_public,
            &sealed.nonce,
            &ciphertext,
            sent_at,
        );
        let signature = sign_json_value(&from_identity.signing_key()?, &unsigned)?;

        Ok(Self {
            record_type: "rymnet.mail.envelope".to_string(),
            version: 1,
            id,
            to: to.to_string(),
            from: from.to_string(),
            from_public_key: from_identity.public_key.clone(),
            ephemeral_public_key: sealed.ephemeral_public,
            nonce: sealed.nonce,
            ciphertext,
            sent_at,
            signature,
        })
    }

    /// Checks only that the envelope wasn't tampered with in transit and
    /// really was signed by whatever key claims to be `from_public_key`.
    /// It does *not* prove that key actually owns the `from` username --
    /// callers should cross-check `from_public_key` against that
    /// username's `AccountClaimRecord` for real sender authentication.
    pub fn verify_signature(&self) -> Result<bool> {
        verify_json_value(
            &self.from_public_key,
            &self.signature,
            &self.unsigned_value(),
        )
    }

    pub fn decrypt_body(&self, recipient_secret: &StaticSecret) -> Result<MailBody> {
        let ciphertext = STANDARD
            .decode(&self.ciphertext)
            .context("decode ciphertext")?;
        let plaintext = open(
            recipient_secret,
            &self.ephemeral_public_key,
            &self.nonce,
            &ciphertext,
        )?;
        serde_json::from_slice(&plaintext).context("parse mail body")
    }

    pub fn approx_size(&self) -> u64 {
        self.ciphertext.len() as u64
    }

    fn unsigned_value(&self) -> Value {
        unsigned_value(
            &self.id,
            &self.to,
            &self.from,
            &self.from_public_key,
            &self.ephemeral_public_key,
            &self.nonce,
            &self.ciphertext,
            self.sent_at,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn unsigned_value(
    id: &str,
    to: &str,
    from: &str,
    from_public_key: &str,
    ephemeral_public_key: &str,
    nonce: &str,
    ciphertext: &str,
    sent_at: DateTime<Utc>,
) -> Value {
    json!({
        "type": "rymnet.mail.envelope",
        "version": 1,
        "id": id,
        "to": to,
        "from": from,
        "from_public_key": from_public_key,
        "ephemeral_public_key": ephemeral_public_key,
        "nonce": nonce,
        "ciphertext": ciphertext,
        "sent_at": sent_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_mailbox_keypair;

    #[test]
    fn round_trips_and_verifies() {
        let sender_identity = NodeIdentity::generate();
        let recipient_keys = derive_mailbox_keypair("bob", "bobpassword").unwrap();

        let body = MailBody {
            subject: "hi".to_string(),
            cc: vec![],
            bcc: vec![],
            body: "hello bob".to_string(),
            in_reply_to: None,
            thread_id: None,
            attachment: None,
        };
        let envelope = MailEnvelope::create(
            "bob",
            "alice",
            &sender_identity,
            &recipient_keys.public,
            &body,
        )
        .unwrap();

        assert!(envelope.verify_signature().unwrap());
        let decrypted = envelope.decrypt_body(&recipient_keys.secret).unwrap();
        assert_eq!(decrypted.subject, "hi");
        assert_eq!(decrypted.body, "hello bob");
    }

    #[test]
    fn tampering_breaks_signature() {
        let sender_identity = NodeIdentity::generate();
        let recipient_keys = derive_mailbox_keypair("bob", "bobpassword").unwrap();
        let body = MailBody {
            subject: "hi".to_string(),
            cc: vec![],
            bcc: vec![],
            body: "hello".to_string(),
            in_reply_to: None,
            thread_id: None,
            attachment: None,
        };
        let mut envelope = MailEnvelope::create(
            "bob",
            "alice",
            &sender_identity,
            &recipient_keys.public,
            &body,
        )
        .unwrap();
        envelope.ciphertext.push('x');
        assert!(!envelope.verify_signature().unwrap());
    }

    /// `attachment` is the newest field on `MailBody`. This proves two
    /// things at once: an old envelope with no `attachment` key at all in
    /// its serialized JSON still deserializes fine (`#[serde(default)]`,
    /// backward compatibility with mail sent before this field existed),
    /// and a real `AttachmentRef` round-trips through the full
    /// seal/sign/verify/decrypt pipeline unchanged.
    #[test]
    fn attachment_ref_round_trips_and_old_bodies_without_it_still_parse() {
        let sender_identity = NodeIdentity::generate();
        let recipient_keys = derive_mailbox_keypair("bob", "bobpassword").unwrap();

        let body = MailBody {
            subject: "File: notes.txt".to_string(),
            cc: vec![],
            bcc: vec![],
            body: String::new(),
            in_reply_to: None,
            thread_id: None,
            attachment: Some(AttachmentRef {
                name: "notes.txt".to_string(),
                size: 42,
                mime: "text/plain".to_string(),
                ciphertext_hash: "blake3:deadbeef".to_string(),
                ephemeral_public_key: "epk".to_string(),
                nonce: "nonce".to_string(),
            }),
        };
        let envelope =
            MailEnvelope::create("bob", "alice", &sender_identity, &recipient_keys.public, &body).unwrap();
        let decrypted = envelope.decrypt_body(&recipient_keys.secret).unwrap();
        let attachment = decrypted.attachment.expect("attachment should round-trip");
        assert_eq!(attachment.name, "notes.txt");
        assert_eq!(attachment.size, 42);
        assert_eq!(attachment.ciphertext_hash, "blake3:deadbeef");

        // An old-shaped body with no "attachment" key at all in its JSON
        // (as if sent before this field existed) must still parse, with
        // `attachment` defaulting to `None`.
        let old_shaped_json = serde_json::json!({
            "subject": "hi", "cc": [], "bcc": [], "body": "hello",
        });
        let parsed: MailBody = serde_json::from_value(old_shaped_json).unwrap();
        assert!(parsed.attachment.is_none());
    }
}
