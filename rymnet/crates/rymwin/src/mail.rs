use anyhow::{Context, Result, bail};
use rymnet_core::NodeIdentity;
use rymnet_mail::{AttachmentRef, MailBody, MailEnvelope, MailboxKeyRecord};
use rymnet_network::{PeerMessage, b64_to_bytes, bytes_to_b64, send_request};
use rymnet_storage::{ChunkStore, DataLayout, PeerConfig, PeerEntry};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use x25519_dalek::StaticSecret;

pub struct InboxMessage {
    pub envelope: MailEnvelope,
    pub body: MailBody,
    pub verified: bool,
}

/// Same send flow as `rymsh`/`rymweb`: look up the recipient's published
/// mailbox key (checking local disk first, then every configured peer),
/// seal the message to it, sign the envelope with this account's
/// identity, and deliver to every configured peer. Returns
/// (peers that accepted it, peers tried).
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
    attachment: Option<AttachmentRef>,
) -> Result<(usize, usize)> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `rym peer connect <ip>` before sending mail");
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
        attachment,
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

/// Sends a file: seals its bytes to the recipient's mailbox key (a
/// second, independent seal from the one the mail body itself gets
/// inside `send`), stores the sealed ciphertext content-addressed in the
/// local `ChunkStore`, pushes it to every configured peer via
/// `file.chunk.offer`, then sends a small mail message carrying only the
/// pointer (`AttachmentRef`), not the bytes themselves.
pub async fn send_file(layout: &DataLayout, username: &str, identity: &NodeIdentity, to: &str, path: &Path) -> Result<(usize, usize)> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".to_string());
    let mime = mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string();
    let size = bytes.len() as u64;

    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `rym peer connect <ip>` before sending files");
    }
    let key = find_mailbox_key(layout, &peers, identity, to)
        .await
        .with_context(|| {
            format!(
                "{to} has no published mailbox key; they need to log in at least once while \
                 connected to a shared peer"
            )
        })?;

    let sealed = rymnet_mail::crypto::seal(&key.public_key()?, &bytes)?;
    let ciphertext_hash = ChunkStore::new(layout.chunks_root()).put(&sealed.ciphertext)?;

    let mut chunk_delivered = 0;
    for peer in &peers.peers {
        let accepted = send_request(
            peer,
            identity,
            PeerMessage::new(
                "file.chunk.offer",
                json!({ "to": to, "hash": ciphertext_hash, "bytes": bytes_to_b64(&sealed.ciphertext) }),
            ),
        )
        .await
        .is_ok_and(|response| response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"));
        if accepted {
            chunk_delivered += 1;
        }
    }
    if chunk_delivered == 0 {
        bail!("could not push the file chunk to any peer; the recipient won't be able to fetch it");
    }

    send(
        layout,
        username,
        identity,
        to,
        &format!("File: {name}"),
        Vec::new(),
        Vec::new(),
        "",
        None,
        None,
        Some(AttachmentRef {
            name,
            size,
            mime,
            ciphertext_hash,
            ephemeral_public_key: sealed.ephemeral_public,
            nonce: sealed.nonce,
        }),
    )
    .await
}

/// Fetches an attachment's sealed bytes from every configured peer
/// (local disk isn't checked first -- the sealed chunk was never
/// "ours," it only ever lived wherever `send_file` pushed it), verifies
/// the chunk hash, unseals it with this account's mailbox secret, and
/// writes the plaintext to `dest`.
pub async fn save_attachment(
    layout: &DataLayout,
    identity: &NodeIdentity,
    mailbox_secret: &StaticSecret,
    attachment: &AttachmentRef,
    dest: &Path,
) -> Result<()> {
    let peers = PeerConfig::load(layout.peers_path())?;
    let store = ChunkStore::new(layout.chunks_root());
    let mut sealed_bytes = if store.exists(&attachment.ciphertext_hash).unwrap_or(false) {
        store.get(&attachment.ciphertext_hash).ok()
    } else {
        None
    };
    if sealed_bytes.is_none() {
        for peer in &peers.peers {
            let Ok(response) = send_request(
                peer,
                identity,
                PeerMessage::new("file.chunk.request", json!({ "hash": attachment.ciphertext_hash })),
            )
            .await
            else {
                continue;
            };
            let Some(hash) = response.payload.get("hash").and_then(|v| v.as_str()) else { continue };
            let Some(b64) = response.payload.get("bytes").and_then(|v| v.as_str()) else { continue };
            let Ok(bytes) = b64_to_bytes(b64) else { continue };
            if hash == attachment.ciphertext_hash && store.verify(hash, &bytes) {
                let _ = store.put(&bytes);
                sealed_bytes = Some(bytes);
                break;
            }
        }
    }
    let sealed_bytes = sealed_bytes
        .with_context(|| format!("no peer could provide the file chunk {}", attachment.ciphertext_hash))?;

    let plaintext = rymnet_mail::crypto::open(mailbox_secret, &attachment.ephemeral_public_key, &attachment.nonce, &sealed_bytes)
        .context("failed to unseal attachment -- it wasn't sealed for this account")?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(dest, &plaintext).with_context(|| format!("write {}", dest.display()))
}

async fn deliver_to_peer(peer: &PeerEntry, identity: &NodeIdentity, envelope: &MailEnvelope) -> bool {
    match send_request(
        peer,
        identity,
        PeerMessage::new("mail.deliver", json!({ "envelope": envelope })),
    )
    .await
    {
        Ok(response) => response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted"),
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
        let Some(claim_value) = claim_response.payload.get("claim").filter(|v| !v.is_null()) else {
            continue;
        };
        let Ok(claim) = serde_json::from_value::<rymnet_account::AccountClaimRecord>(claim_value.clone()) else {
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

/// Fetches the inbox from local disk plus every configured peer,
/// decrypts and verifies each message, and de-duplicates by message id.
/// `Verified: yes` (the `verified` field) means the envelope's signature
/// checks out *and* the signing key matches that sender username's
/// current registered claim -- not just that the signature is internally
/// consistent.
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
        messages.push(InboxMessage { envelope, body, verified });
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

    /// Not run by default (needs a real reachable peer network and takes
    /// real network round trips) -- run explicitly with `cargo test -p
    /// rymwin -- --ignored mail_send_and_receive_round_trip`. Creates two
    /// fresh throwaway accounts via the exact same `account::login` path
    /// the desktop's own login screen uses, sends a real sealed message
    /// between them over the real peer network, and confirms it arrives
    /// decrypted, correctly addressed, and signature-verified.
    #[test]
    #[ignore]
    fn mail_send_and_receive_round_trip() {
        let layout = rymnet_storage::DataLayout::default().expect("resolve ~/.rymnet");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let sender_name = format!("mailtest_sender_{suffix}");
        let recipient_name = format!("mailtest_recipient_{suffix}");
        let password = "correcthorsebatterystaple";

        runtime.block_on(async {
            let sender = crate::account::login(&layout, &sender_name, password)
                .await
                .expect("sender login");
            let recipient = crate::account::login(&layout, &recipient_name, password)
                .await
                .expect("recipient login");

            let (delivered, total) = send(
                &layout,
                &sender_name,
                &sender.identity,
                &recipient_name,
                "Test subject",
                Vec::new(),
                Vec::new(),
                "Hello from the mail.rs live test.",
                None,
                None,
                None,
            )
            .await
            .expect("send");
            assert!(delivered > 0, "expected delivery to at least one peer, got 0/{total}");

            let messages = inbox(&layout, &recipient_name, &recipient.identity, &recipient.mailbox_secret)
                .await
                .expect("fetch inbox");
            let found = messages
                .iter()
                .find(|m| m.envelope.from == sender_name && m.body.subject == "Test subject")
                .expect("sent message should appear in recipient's inbox");
            assert_eq!(found.body.body, "Hello from the mail.rs live test.");
            assert!(found.verified, "message should verify against sender's registered claim");

            let _ = std::fs::remove_dir_all(&sender.cache_dir);
            let _ = std::fs::remove_dir_all(&recipient.cache_dir);
        });
    }

    /// Same shape and same live-network precondition as
    /// `mail_send_and_receive_round_trip` -- run explicitly with `cargo
    /// test -p rymwin -- --ignored file_send_and_save_round_trip`. Proves
    /// the whole pipeline end to end: seal a real file's bytes, push the
    /// sealed chunk to peers, deliver the pointer as mail, fetch the
    /// chunk back from a peer (not local disk -- the recipient process
    /// never had it), unseal it, and check the recovered bytes are
    /// byte-for-byte identical to the original.
    #[test]
    #[ignore]
    fn file_send_and_save_round_trip() {
        let layout = rymnet_storage::DataLayout::default().expect("resolve ~/.rymnet");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let sender_name = format!("filetest_sender_{suffix}");
        let recipient_name = format!("filetest_recipient_{suffix}");
        let password = "correcthorsebatterystaple";

        let source_path = std::env::temp_dir().join(format!("rymwin-filetest-source-{suffix}.txt"));
        let original_bytes = b"hello from the file transfer live test\n".repeat(100);
        std::fs::write(&source_path, &original_bytes).expect("write source file");
        let dest_path = std::env::temp_dir().join(format!("rymwin-filetest-dest-{suffix}.txt"));

        runtime.block_on(async {
            let sender = crate::account::login(&layout, &sender_name, password)
                .await
                .expect("sender login");
            let recipient = crate::account::login(&layout, &recipient_name, password)
                .await
                .expect("recipient login");

            let (delivered, total) = send_file(&layout, &sender_name, &sender.identity, &recipient_name, &source_path)
                .await
                .expect("send_file");
            assert!(delivered > 0, "expected chunk delivery to at least one peer, got 0/{total}");

            let messages = inbox(&layout, &recipient_name, &recipient.identity, &recipient.mailbox_secret)
                .await
                .expect("fetch inbox");
            let found = messages
                .iter()
                .find(|m| m.envelope.from == sender_name && m.body.attachment.is_some())
                .expect("file message should appear in recipient's inbox");
            let attachment = found.body.attachment.as_ref().unwrap();
            assert_eq!(attachment.size, original_bytes.len() as u64);

            save_attachment(&layout, &recipient.identity, &recipient.mailbox_secret, attachment, &dest_path)
                .await
                .expect("save_attachment");
            let recovered = std::fs::read(&dest_path).expect("read saved file");
            assert_eq!(recovered, original_bytes, "recovered file must match the original exactly");

            let _ = std::fs::remove_dir_all(&sender.cache_dir);
            let _ = std::fs::remove_dir_all(&recipient.cache_dir);
        });

        let _ = std::fs::remove_file(&source_path);
        let _ = std::fs::remove_file(&dest_path);
    }
}
