use anyhow::Result;
use chrono::{DateTime, Utc};
use rymnet_core::{NodeIdentity, sign_json_value, verify_json_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::board::validate_board;

/// A single board post: signed, but deliberately not encrypted -- same
/// "verified origin, not confidentiality" posture as `rymnet_chat::ChatMessage`,
/// for the same reason (boards have no membership/ownership concept, so
/// there's no single key to seal against). Any node will relay/store a
/// post for any board; the only thing actually verified is that
/// `from_public_key` really signed this content. Callers should
/// cross-check that key against `from`'s current `AccountClaimRecord`,
/// the same "Verified: yes" pattern mail and chat already use.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Post {
    #[serde(rename = "type")]
    pub record_type: String,
    pub version: u16,
    pub id: String,
    pub board: String,
    pub from: String,
    pub from_public_key: String,
    pub title: String,
    pub body: String,
    pub link_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub signature: String,
}

impl Post {
    pub fn create(board: &str, from: &str, from_identity: &NodeIdentity, title: &str, body: &str, link_url: Option<String>) -> Result<Self> {
        validate_board(board)?;
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let unsigned = unsigned_value(&id, board, from, &from_identity.public_key, title, body, link_url.as_deref(), created_at);
        let signature = sign_json_value(&from_identity.signing_key()?, &unsigned)?;

        Ok(Self {
            record_type: "rymnet.board.post".to_string(),
            version: 1,
            id,
            board: board.to_string(),
            from: from.to_string(),
            from_public_key: from_identity.public_key.clone(),
            title: title.to_string(),
            body: body.to_string(),
            link_url,
            created_at,
            signature,
        })
    }

    /// Checks only that the post wasn't tampered with in transit and
    /// really was signed by whatever key claims to be `from_public_key`
    /// -- it does *not* prove that key owns the `from` username. See the
    /// type-level doc comment.
    pub fn verify_signature(&self) -> Result<bool> {
        verify_json_value(&self.from_public_key, &self.signature, &self.unsigned_value())
    }

    pub fn approx_size(&self) -> u64 {
        (self.title.len() + self.body.len() + self.link_url.as_ref().map_or(0, |url| url.len())) as u64
    }

    /// The lightweight, unsigned index-line view of this post -- see
    /// `PostSummary`'s own doc comment for why it carries no signature.
    pub fn summary(&self) -> PostSummary {
        PostSummary {
            id: self.id.clone(),
            board: self.board.clone(),
            from: self.from.clone(),
            title: self.title.clone(),
            has_link: self.link_url.is_some(),
            created_at: self.created_at,
        }
    }

    fn unsigned_value(&self) -> Value {
        unsigned_value(&self.id, &self.board, &self.from, &self.from_public_key, &self.title, &self.body, self.link_url.as_deref(), self.created_at)
    }
}

#[allow(clippy::too_many_arguments)]
fn unsigned_value(
    id: &str,
    board: &str,
    from: &str,
    from_public_key: &str,
    title: &str,
    body: &str,
    link_url: Option<&str>,
    created_at: DateTime<Utc>,
) -> Value {
    json!({
        "type": "rymnet.board.post",
        "version": 1,
        "id": id,
        "board": board,
        "from": from,
        "from_public_key": from_public_key,
        "title": title,
        "body": body,
        "link_url": link_url,
        "created_at": created_at,
    })
}

/// A compact, unsigned index-line entry for a post -- what a board's
/// append-only `index.jsonl` actually stores, and what list/poll
/// requests read instead of the full post body. Deliberately excludes a
/// comment count: a board's index is append-only by design (an entry is
/// written once, at post-creation time, and never rewritten -- that's
/// what keeps listing cheap at thousands of posts), and a live comment
/// count would require mutating an existing line every time a comment
/// arrives. Comment counts are shown once a thread is actually opened
/// (which already fetches the real comment list), not in the list view.
///
/// Because this is unsigned, it's only ever trustworthy to the extent
/// its source is: every entry is derived locally from a `Post` that
/// already passed `verify_signature()` in that node's own delivery
/// handler before being written to disk, so by the time a peer answers
/// `board.post.fetch` with these, everything in its own index already
/// passed verification on *its* end at write-time. A client fetching
/// this list from a remote peer is trusting that peer not to lie about
/// title/author text, the same posture already accepted for every other
/// fetch response in this project -- closed for certain the moment a
/// specific post is actually opened (`board.post.get` + a real
/// `verify_signature()` check), the same as chat verifies each message.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostSummary {
    pub id: String,
    pub board: String,
    pub from: String,
    pub title: String,
    pub has_link: bool,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_verifies() {
        let identity = NodeIdentity::generate();
        let post = Post::create("rust", "alice", &identity, "Hello", "First post", None).unwrap();
        assert!(post.verify_signature().unwrap());
        assert_eq!(post.board, "rust");
        assert_eq!(post.from, "alice");
        assert_eq!(post.title, "Hello");
    }

    #[test]
    fn tampering_breaks_signature() {
        let identity = NodeIdentity::generate();
        let mut post = Post::create("rust", "alice", &identity, "Hello", "First post", None).unwrap();
        post.body.push('!');
        assert!(!post.verify_signature().unwrap());
    }

    #[test]
    fn rejects_invalid_board_name() {
        let identity = NodeIdentity::generate();
        assert!(Post::create("Not Valid", "alice", &identity, "Hello", "body", None).is_err());
    }

    #[test]
    fn summary_reflects_link_presence_without_carrying_the_body() {
        let identity = NodeIdentity::generate();
        let post = Post::create("rust", "alice", &identity, "A link", "", Some("https://example.com".to_string())).unwrap();
        let summary = post.summary();
        assert!(summary.has_link);
        assert_eq!(summary.title, "A link");
        assert_eq!(summary.id, post.id);
    }
}
