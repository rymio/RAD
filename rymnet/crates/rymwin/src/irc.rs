use anyhow::{Result, bail};
use rymnet_chat::ChatMessage;
use rymnet_core::NodeIdentity;
use rymnet_network::{PeerMessage, send_request};
use rymnet_storage::{DataLayout, PeerConfig};
use serde_json::json;
use std::collections::HashMap;

pub struct ChannelMessage {
    pub message: ChatMessage,
    pub verified: bool,
}

/// Local disk first, then every configured peer, dedup by message id --
/// the same ordering `rymwin`'s/`rymsh`'s mail code already needs (a
/// two-node test where each side only peers with the other never
/// discovers content already sitting locally otherwise). `verified`
/// cross-checks the signing key against the sender's *current*
/// registered claim, the same "Verified: yes" meaning mail already uses
/// -- not just that the signature is internally consistent.
pub async fn fetch(layout: &DataLayout, identity: &NodeIdentity, channel: &str) -> Result<Vec<ChannelMessage>> {
    let peers = PeerConfig::load(layout.peers_path())?;
    let dir_name = rymnet_chat::channel_dir_name(channel)?;

    let mut by_id: HashMap<String, ChatMessage> = HashMap::new();

    let local_dir = layout.chat_channel_dir(&dir_name);
    if let Ok(entries) = std::fs::read_dir(&local_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path())
                && let Ok(message) = serde_json::from_slice::<ChatMessage>(&bytes)
            {
                by_id.entry(message.id.clone()).or_insert(message);
            }
        }
    }
    for peer in &peers.peers {
        if let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new("chat.channel.fetch", json!({ "channel": channel })),
        )
        .await
            && let Some(values) = response.payload.get("messages").and_then(|v| v.as_array())
        {
            for value in values {
                if let Ok(message) = serde_json::from_value::<ChatMessage>(value.clone()) {
                    by_id.entry(message.id.clone()).or_insert(message);
                }
            }
        }
    }

    let mut claim_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut messages = Vec::new();
    for message in by_id.into_values() {
        let signature_ok = message.verify_signature().unwrap_or(false);
        let sender_key = sender_owner_key(&peers, identity, &message.from, &mut claim_cache).await;
        let verified = signature_ok && sender_key.as_deref() == Some(message.from_public_key.as_str());
        messages.push(ChannelMessage { message, verified });
    }
    messages.sort_by_key(|m| m.message.sent_at);
    Ok(messages)
}

/// Signs and broadcasts a channel message to every configured peer.
/// Returns (peers that accepted it, peers tried) -- same shape as
/// `mail::send`'s return.
pub async fn send(layout: &DataLayout, username: &str, identity: &NodeIdentity, channel: &str, text: &str) -> Result<(usize, usize)> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `rym peer connect <ip>` before chatting");
    }
    let message = ChatMessage::create(channel, username, identity, text)?;

    let mut delivered = 0;
    for peer in &peers.peers {
        let accepted = send_request(
            peer,
            identity,
            PeerMessage::new("chat.channel.deliver", json!({ "message": &message })),
        )
        .await
        .is_ok_and(|response| response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"));
        if accepted {
            delivered += 1;
        }
    }
    Ok((delivered, peers.peers.len()))
}

async fn sender_owner_key(
    peers: &PeerConfig,
    identity: &NodeIdentity,
    username: &str,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    if let Some(cached) = cache.get(username) {
        return cached.clone();
    }
    let mut found = None;
    for peer in &peers.peers {
        if let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new("account.claim.request", json!({ "username": username })),
        )
        .await
            && let Some(claim_value) = response.payload.get("claim").filter(|v| !v.is_null())
            && let Ok(claim) = serde_json::from_value::<rymnet_account::AccountClaimRecord>(claim_value.clone())
            && claim.verify().unwrap_or(false)
        {
            found = Some(claim.owner_public_key);
            break;
        }
    }
    cache.insert(username.to_string(), found.clone());
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not run by default (needs a real reachable peer network) -- run
    /// explicitly with `cargo test -p rymwin -- --ignored
    /// chat_send_and_fetch_round_trip`. Mirrors `mail.rs`'s
    /// `mail_send_and_receive_round_trip`: two fresh throwaway accounts
    /// via the real `account::login`, one real send, one real fetch,
    /// against the real peer network.
    #[test]
    #[ignore]
    fn chat_send_and_fetch_round_trip() {
        let layout = rymnet_storage::DataLayout::default().expect("resolve ~/.rymnet");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let sender_name = format!("chattest_sender_{suffix}");
        let reader_name = format!("chattest_reader_{suffix}");
        let channel = format!("#chattest-{suffix}");
        let password = "correcthorsebatterystaple";

        runtime.block_on(async {
            let sender = crate::account::login(&layout, &sender_name, password)
                .await
                .expect("sender login");
            let reader = crate::account::login(&layout, &reader_name, password)
                .await
                .expect("reader login");

            let (delivered, total) = send(&layout, &sender_name, &sender.identity, &channel, "hello from the live test")
                .await
                .expect("send");
            assert!(delivered > 0, "expected delivery to at least one peer, got 0/{total}");

            let messages = fetch(&layout, &reader.identity, &channel).await.expect("fetch");
            let found = messages
                .iter()
                .find(|m| m.message.from == sender_name)
                .expect("message should appear");
            assert_eq!(found.message.body, "hello from the live test");
            assert!(found.verified, "message should verify against sender's registered claim");

            let _ = std::fs::remove_dir_all(&sender.cache_dir);
            let _ = std::fs::remove_dir_all(&reader.cache_dir);
        });
    }
}
