use anyhow::{Context, Result, bail};
use rymnet_core::NodeIdentity;
use rymnet_mail::{MailBody, MailEnvelope, MailboxKeyRecord};
use rymnet_network::{PeerMessage, send_request};
use rymnet_storage::{DataLayout, PeerConfig, PeerEntry};
use serde_json::json;
use std::collections::HashMap;
use x25519_dalek::StaticSecret;

pub struct InboxMessage {
    pub envelope: MailEnvelope,
    pub body: MailBody,
    pub verified: bool,
}

#[allow(clippy::too_many_arguments)]
pub async fn send(
    layout: &DataLayout,
    username: &str,
    identity: &NodeIdentity,
    to: &str,
    subject: &str,
    cc: Vec<String>,
    bcc: Vec<String>,
    body_text: &str,
    in_reply_to: Option<String>,
    thread_id: Option<String>,
) -> Result<(usize, usize)> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `peer add`/`peer connect` before sending mail");
    }
    let key = find_mailbox_key(layout, &peers, identity, to)
        .await
        .with_context(|| {
            format!(
                "{to} has no published mailbox key; they need to log in at least once while \
                 connected to a shared peer"
            )
        })?;
    let body = MailBody {
        subject: subject.to_string(),
        cc,
        bcc,
        body: body_text.to_string(),
        in_reply_to,
        thread_id,
        // rymweb has no file-transfer UI (out of scope for now -- see
        // rymsh's `sendfile`/rymwin's Terminal for the real thing); it
        // only ever composes plain mail.
        attachment: None,
    };
    let envelope = MailEnvelope::create(to, username, identity, &key.public_key()?, &body)?;

    let claim = rymnet_account::find_claim(layout.account_claims_dir(), username)?
        .context("missing local account claim; log in again")?;

    let mut delivered = 0;
    for peer in &peers.peers {
        let _ = send_request(
            peer,
            identity,
            PeerMessage::new("account.claim.announce", json!({ "claim": claim })),
        )
        .await;
        if deliver_to_peer(peer, identity, &envelope).await {
            delivered += 1;
        }
    }
    Ok((delivered, peers.peers.len()))
}

async fn deliver_to_peer(peer: &PeerEntry, identity: &NodeIdentity, envelope: &MailEnvelope) -> bool {
    match send_request(
        peer,
        identity,
        PeerMessage::new("mail.deliver", json!({ "envelope": envelope })),
    )
    .await
    {
        Ok(response) => {
            response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted")
        }
        Err(_) => false,
    }
}

async fn find_mailbox_key(
    layout: &DataLayout,
    peers: &PeerConfig,
    identity: &NodeIdentity,
    username: &str,
) -> Result<MailboxKeyRecord> {
    if let Ok(Some(local)) = rymnet_mail::find_mailbox_key(layout.mailbox_keys_dir(), username)
        && local.verify().unwrap_or(false)
    {
        return Ok(local);
    }
    for peer in &peers.peers {
        let Ok(claim_response) = send_request(
            peer,
            identity,
            PeerMessage::new("account.claim.request", json!({ "username": username })),
        )
        .await
        else {
            continue;
        };
        let Some(claim_value) = claim_response.payload.get("claim").filter(|v| !v.is_null())
        else {
            continue;
        };
        let Ok(claim) =
            serde_json::from_value::<rymnet_account::AccountClaimRecord>(claim_value.clone())
        else {
            continue;
        };
        if !claim.verify().unwrap_or(false) {
            continue;
        }
        let Ok(key_response) = send_request(
            peer,
            identity,
            PeerMessage::new("account.mailbox_key.request", json!({ "username": username })),
        )
        .await
        else {
            continue;
        };
        let Some(key_value) = key_response.payload.get("key").filter(|v| !v.is_null()) else {
            continue;
        };
        let Ok(key) = serde_json::from_value::<MailboxKeyRecord>(key_value.clone()) else {
            continue;
        };
        if key.verify().unwrap_or(false) && key.owner_public_key == claim.owner_public_key {
            return Ok(key);
        }
    }
    bail!("no peer could provide a verified mailbox key for {username}")
}

/// Fetches the inbox from this server's own local disk plus every
/// configured peer, decrypts and verifies each message, and de-duplicates
/// by message id.
pub async fn inbox(
    layout: &DataLayout,
    username: &str,
    identity: &NodeIdentity,
    mailbox_secret: &StaticSecret,
) -> Result<Vec<InboxMessage>> {
    let peers = PeerConfig::load(layout.peers_path())?;
    let mut by_id: HashMap<String, MailEnvelope> = HashMap::new();

    let local_dir = layout.mail_inbox_dir(username);
    if let Ok(entries) = std::fs::read_dir(&local_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path())
                && let Ok(envelope) = serde_json::from_slice::<MailEnvelope>(&bytes)
            {
                by_id.entry(envelope.id.clone()).or_insert(envelope);
            }
        }
    }
    for peer in &peers.peers {
        if let Ok(response) = send_request(
            peer,
            identity,
            PeerMessage::new("mail.inbox.request", json!({ "username": username })),
        )
        .await
            && let Some(envelopes) = response.payload.get("envelopes").and_then(|v| v.as_array())
        {
            for value in envelopes {
                if let Ok(envelope) = serde_json::from_value::<MailEnvelope>(value.clone()) {
                    by_id.entry(envelope.id.clone()).or_insert(envelope);
                }
            }
        }
    }

    let mut claim_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut messages = Vec::new();
    for envelope in by_id.into_values() {
        let Ok(body) = envelope.decrypt_body(mailbox_secret) else {
            continue;
        };
        let signature_ok = envelope.verify_signature().unwrap_or(false);
        let sender_key = sender_owner_key(&peers, identity, &envelope.from, &mut claim_cache).await;
        let verified = signature_ok && sender_key.as_deref() == Some(envelope.from_public_key.as_str());
        messages.push(InboxMessage {
            envelope,
            body,
            verified,
        });
    }
    messages.sort_by_key(|m| std::cmp::Reverse(m.envelope.sent_at));
    Ok(messages)
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
            && let Ok(claim) =
                serde_json::from_value::<rymnet_account::AccountClaimRecord>(claim_value.clone())
            && claim.verify().unwrap_or(false)
        {
            found = Some(claim.owner_public_key);
            break;
        }
    }
    cache.insert(username.to_string(), found.clone());
    found
}
