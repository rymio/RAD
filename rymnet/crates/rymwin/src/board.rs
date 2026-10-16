use anyhow::{Result, bail};
use rymnet_board::{Comment, Post, PostSummary};
use rymnet_core::NodeIdentity;
use rymnet_network::{PeerMessage, send_request};
use rymnet_storage::{DataLayout, PeerConfig};
use serde_json::json;
use std::collections::HashMap;

pub struct CommentView {
    pub comment: Comment,
    pub verified: bool,
}

/// Local disk first (the board's append-only index, not a directory
/// scan of full post bodies -- see `rymnet_board::post::PostSummary`'s
/// doc comment for why that distinction is what keeps this cheap at
/// thousands of posts), then every configured peer, dedup by post id --
/// the same ordering `irc.rs`'s own `fetch` already needs.
pub async fn list_posts(layout: &DataLayout, identity: &NodeIdentity, board: &str) -> Result<Vec<PostSummary>> {
    let peers = PeerConfig::load(layout.peers_path())?;
    let dir_name = rymnet_board::board_dir_name(board)?;

    let mut by_id: HashMap<String, PostSummary> = HashMap::new();

    let index_path = layout.board_index_path(&dir_name);
    if let Ok(text) = std::fs::read_to_string(&index_path) {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(summary) = serde_json::from_str::<PostSummary>(line) {
                by_id.entry(summary.id.clone()).or_insert(summary);
            }
        }
    }
    for peer in &peers.peers {
        if let Ok(response) = send_request(peer, identity, PeerMessage::new("board.post.fetch", json!({ "board": board }))).await
            && let Some(values) = response.payload.get("posts").and_then(|v| v.as_array())
        {
            for value in values {
                if let Ok(summary) = serde_json::from_value::<PostSummary>(value.clone()) {
                    by_id.entry(summary.id.clone()).or_insert(summary);
                }
            }
        }
    }

    let mut posts: Vec<PostSummary> = by_id.into_values().collect();
    posts.sort_by_key(|post| post.created_at);
    Ok(posts)
}

/// Fetches one post in full plus its comments, verifying each
/// independently. Unlike `list_posts`, there's no lightweight-index
/// shortcut here -- comment volume per post is human-scale, not the
/// "thousands" concern the index exists to solve.
pub async fn open_thread(layout: &DataLayout, identity: &NodeIdentity, board: &str, post_id: &str) -> Result<(Post, Vec<CommentView>)> {
    let peers = PeerConfig::load(layout.peers_path())?;

    let mut post = None;
    if let Ok(dir_name) = rymnet_board::board_dir_name(board)
        && let Ok(bytes) = std::fs::read(layout.board_post_path(&dir_name, post_id))
        && let Ok(local_post) = serde_json::from_slice::<Post>(&bytes)
    {
        post = Some(local_post);
    }
    for peer in &peers.peers {
        if post.is_some() {
            break;
        }
        let payload = json!({ "board": board, "post_id": post_id });
        if let Ok(response) = send_request(peer, identity, PeerMessage::new("board.post.get", payload)).await
            && let Some(post_value) = response.payload.get("post").filter(|v| !v.is_null())
            && let Ok(fetched) = serde_json::from_value::<Post>(post_value.clone())
        {
            post = Some(fetched);
        }
    }
    let Some(post) = post else {
        bail!("post not found on any peer");
    };
    if !post.verify_signature()? {
        bail!("post signature invalid");
    }

    let mut by_id: HashMap<String, Comment> = HashMap::new();
    for peer in &peers.peers {
        let payload = json!({ "board": board, "post_id": post_id });
        if let Ok(response) = send_request(peer, identity, PeerMessage::new("board.comment.fetch", payload)).await
            && let Some(values) = response.payload.get("comments").and_then(|v| v.as_array())
        {
            for value in values {
                if let Ok(comment) = serde_json::from_value::<Comment>(value.clone()) {
                    by_id.entry(comment.id.clone()).or_insert(comment);
                }
            }
        }
    }
    let mut comments: Vec<CommentView> = by_id
        .into_values()
        .map(|comment| {
            let verified = comment.verify_signature().unwrap_or(false);
            CommentView { comment, verified }
        })
        .collect();
    comments.sort_by_key(|view| view.comment.created_at);

    Ok((post, comments))
}

/// Signs and broadcasts a post to every configured peer. Returns
/// (peers that accepted it, peers tried), same shape as `irc::send`.
pub async fn submit_post(
    layout: &DataLayout,
    username: &str,
    identity: &NodeIdentity,
    board: &str,
    title: &str,
    body: &str,
    link_url: Option<String>,
) -> Result<(usize, usize)> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `rym peer connect <ip>` before posting");
    }
    let post = Post::create(board, username, identity, title, body, link_url)?;

    let mut delivered = 0;
    for peer in &peers.peers {
        let accepted = send_request(peer, identity, PeerMessage::new("board.post.deliver", json!({ "post": &post })))
            .await
            .is_ok_and(|response| response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"));
        if accepted {
            delivered += 1;
        }
    }
    Ok((delivered, peers.peers.len()))
}

/// Signs and broadcasts a comment to every configured peer, same shape
/// as `submit_post`.
pub async fn submit_comment(layout: &DataLayout, username: &str, identity: &NodeIdentity, board: &str, post_id: &str, body: &str) -> Result<(usize, usize)> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `rym peer connect <ip>` before commenting");
    }
    let comment = Comment::create(board, post_id, username, identity, body)?;

    let mut delivered = 0;
    for peer in &peers.peers {
        let accepted = send_request(peer, identity, PeerMessage::new("board.comment.deliver", json!({ "comment": &comment })))
            .await
            .is_ok_and(|response| response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"));
        if accepted {
            delivered += 1;
        }
    }
    Ok((delivered, peers.peers.len()))
}
