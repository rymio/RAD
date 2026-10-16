use anyhow::Result;
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::channel::validate_channel;

/// A single public channel message: signed, but deliberately not
/// encrypted. Channels have no membership/ownership concept (unlike a
/// mail recipient) so there's no single key to seal against without
/// inventing a group-key-distribution scheme this codebase has no
/// precedent for -- this mirrors real IRC's own security model
/// (confidentiality is not the point; verified origin is). Any node
/// will relay/store a channel message for any channel; the only thing
/// actually verified is that `from_public_key` really signed this
/// content. Callers should cross-check that key against `from`'s
/// current `AccountClaimRecord` for real sender authentication, the
/// same "Verified: yes" pattern mail already uses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub id: String,
    pub channel: String,
    pub from: String,
    pub from_public_key: String,
    pub body: String,
    pub sent_at: DateTime<Utc>,
    pub signature: String,
}

impl ChatMessage {
    pub fn create(channel: &str, from: &str, from_identity: &NodeIdentity, body: &str) -> Result<Self> {
        validate_channel(channel)?;
        let id = uuid::Uuid::new_v4().to_string();
        let sent_at = Utc::now();
        let unsigned = unsigned_value(&id, channel, from, &from_identity.public_key, body, sent_at);
        let signature = sign_json_value(&from_identity.signing_key()?, &unsigned)?;

        Ok(Self {
            record_type: "rymnet.chat.message".to_string(),
            version: 1,
            id,
            channel: channel.to_string(),
            from: from.to_string(),
            from_public_key: from_identity.public_key.clone(),
            body: body.to_string(),
            sent_at,
            signature,
        })
    }

    /// Checks only that the message wasn't tampered with in transit and
    /// really was signed by whatever key claims to be `from_public_key`
    /// -- it does *not* prove that key owns the `from` username. See the
    /// type-level doc comment.
    pub fn verify_signature(&self) -> Result<bool> {
        verify_json_value(&self.from_public_key, &self.signature, &self.unsigned_value())
    }

    pub fn approx_size(&self) -> u64 {
        self.body.len() as u64
    }

    fn unsigned_value(&self) -> Value {
        unsigned_value(&self.id, &self.channel, &self.from, &self.from_public_key, &self.body, self.sent_at)
    }
}

fn unsigned_value(id: &str, channel: &str, from: &str, from_public_key: &str, body: &str, sent_at: DateTime<Utc>) -> Value {
    json!({
        "type": "rymnet.chat.message",
        "version": 1,
        "id": id,
        "channel": channel,
        "from": from,
        "from_public_key": from_public_key,
        "body": body,
        "sent_at": sent_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_verifies() {
        let identity = NodeIdentity::generate();
        let message = ChatMessage::create("#general", "alice", &identity, "hello everyone").unwrap();
        assert!(message.verify_signature().unwrap());
        assert_eq!(message.channel, "#general");
        assert_eq!(message.from, "alice");
        assert_eq!(message.body, "hello everyone");
    }

    #[test]
    fn tampering_breaks_signature() {
        let identity = NodeIdentity::generate();
        let mut message = ChatMessage::create("#general", "alice", &identity, "hello").unwrap();
        message.body.push('!');
        assert!(!message.verify_signature().unwrap());
    }

    #[test]
    fn rejects_invalid_channel_name() {
        let identity = NodeIdentity::generate();
        assert!(ChatMessage::create("general", "alice", &identity, "hi").is_err());
    }
}
