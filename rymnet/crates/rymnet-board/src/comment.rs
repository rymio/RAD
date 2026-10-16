use anyhow::Result;
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::board::validate_board;

/// A single flat reply to a post -- signed, not encrypted, same posture
/// as `Post`/`ChatMessage`. Comments reply to the post only (one level,
/// no nested comment-on-comment threading), matching this feature's
/// "simple" scope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub id: String,
    pub board: String,
    pub post_id: String,
    pub from: String,
    pub from_public_key: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

impl Comment {
    pub fn create(board: &str, post_id: &str, from: &str, from_identity: &NodeIdentity, body: &str) -> Result<Self> {
        validate_board(board)?;
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let unsigned = unsigned_value(&id, board, post_id, from, &from_identity.public_key, body, created_at);
        let signature = sign_json_value(&from_identity.signing_key()?, &unsigned)?;

        Ok(Self {
            record_type: "rymnet.board.comment".to_string(),
            version: 1,
            id,
            board: board.to_string(),
            post_id: post_id.to_string(),
            from: from.to_string(),
            from_public_key: from_identity.public_key.clone(),
            body: body.to_string(),
            created_at,
            signature,
        })
    }

    /// Same caveat as `Post::verify_signature`/`ChatMessage::verify_signature`:
    /// proves the content wasn't tampered with and really was signed by
    /// `from_public_key`, not that the key owns the `from` username.
    pub fn verify_signature(&self) -> Result<bool> {
        verify_json_value(&self.from_public_key, &self.signature, &self.unsigned_value())
    }

    pub fn approx_size(&self) -> u64 {
        self.body.len() as u64
    }

    fn unsigned_value(&self) -> Value {
        unsigned_value(&self.id, &self.board, &self.post_id, &self.from, &self.from_public_key, &self.body, self.created_at)
    }
}

fn unsigned_value(id: &str, board: &str, post_id: &str, from: &str, from_public_key: &str, body: &str, created_at: DateTime<Utc>) -> Value {
    json!({
        "type": "rymnet.board.comment",
        "version": 1,
        "id": id,
        "board": board,
        "post_id": post_id,
        "from": from,
        "from_public_key": from_public_key,
        "body": body,
        "created_at": created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_verifies() {
        let identity = NodeIdentity::generate();
        let comment = Comment::create("rust", "post-1", "bob", &identity, "nice post").unwrap();
        assert!(comment.verify_signature().unwrap());
        assert_eq!(comment.post_id, "post-1");
        assert_eq!(comment.from, "bob");
    }

    #[test]
    fn tampering_breaks_signature() {
        let identity = NodeIdentity::generate();
        let mut comment = Comment::create("rust", "post-1", "bob", &identity, "nice post").unwrap();
        comment.body.push('!');
        assert!(!comment.verify_signature().unwrap());
    }

    #[test]
    fn rejects_invalid_board_name() {
        let identity = NodeIdentity::generate();
        assert!(Comment::create("Not Valid", "post-1", "bob", &identity, "hi").is_err());
    }
}
