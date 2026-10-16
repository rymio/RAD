use crate::account_session::AccountSession;
use crate::vfs::VirtualComputer;
use anyhow::{Context, Result, bail};
use chrono::Local;
use rymnet_core::NodeIdentity;
use rymnet_mail::{AttachmentRef, MailBody, MailEnvelope, MailboxKeyRecord};
use rymnet_network::{PeerMessage, b64_to_bytes, bytes_to_b64, send_request};
use rymnet_storage::{ChunkStore, DataLayout, PeerConfig, PeerEntry};
use serde_json::json;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;

struct InboxMessage {
    envelope: MailEnvelope,
    body: MailBody,
    verified: bool,
}

/// Guided compose flow: `emailto: rob<enter>`, then Cc, Bcc, Subject, and a
/// blank-line-terminated body, matching a classic Unix mail prompt. Looks
/// the recipient's mailbox key up over the network (it is never assumed to
/// be cached locally), seals the message so only they can read it, signs
/// it with the sender's own account identity, and delivers it to every
/// configured peer as a best-effort broadcast -- there is no routing table
/// telling a sender which peer(s) a recipient actually checks from, so
/// broadcasting to all of the sender's own peers is what maximizes the
/// chance of overlap, the same tradeoff `sync` already makes for vfs data.
pub async fn compose_and_send(
    layout: &DataLayout,
    session: &AccountSession,
    to_arg: Option<&str>,
) -> Result<()> {
    let to = match to_arg {
        Some(to) => to.trim().to_string(),
        None => prompt_line("To: ")?,
    };
    if to.is_empty() {
        bail!("no recipient given");
    }
    let cc = prompt_line("Cc (comma-separated, optional): ")?;
    let bcc = prompt_line("Bcc (comma-separated, optional): ")?;
    let subject = prompt_line("Subject: ")?;
    println!("Body (blank line to finish):");
    let body = prompt_multiline()?;

    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `peer add` before sending mail");
    }

    let mail_body = MailBody {
        subject,
        cc: split_addresses(&cc),
        bcc: split_addresses(&bcc),
        body,
        in_reply_to: None,
        thread_id: None,
        attachment: None,
    };
    send_envelope(layout, session, &peers, &to, &mail_body, false).await?;
    Ok(())
}

/// Builds a minimal `MailBody` and sends it, for callers that just need
/// "deliver this text to this user" without the full compose UX -- e.g.
/// `irc`'s `/msg`. Runs quiet (`send_envelope`'s own status printlns
/// would corrupt a raw-mode alternate-screen display painted outside its
/// own `draw()` cycle), returning `(delivered, total)` so the caller can
/// show its own status line instead.
pub(crate) async fn send_quick(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    to: &str,
    text: &str,
) -> Result<(usize, usize)> {
    let body = MailBody {
        subject: "(chat)".to_string(),
        cc: Vec::new(),
        bcc: Vec::new(),
        body: text.to_string(),
        in_reply_to: None,
        thread_id: None,
        attachment: None,
    };
    send_envelope(layout, session, peers, to, &body, true).await
}

/// Sends a file: seals its bytes to the recipient's mailbox key (a
/// second, independent `seal()` call from the one the mail body itself
/// gets inside `send_envelope`), stores the sealed ciphertext
/// content-addressed in the local `ChunkStore`, pushes it to every
/// configured peer via `file.chunk.offer` (the same broadcast-to-my-own-
/// peers idea `mail.deliver` already uses -- see `compose_and_send`'s
/// doc comment), then sends a small mail message carrying only the
/// pointer (`AttachmentRef`: name/size/hash/unsealing keys), not the
/// bytes themselves.
pub(crate) async fn send_file(layout: &DataLayout, session: &AccountSession, path: &Path, to: &str) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mime = mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string();
    let size = bytes.len() as u64;

    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        bail!("no peers configured; add one with `peer add` before sending files");
    }

    println!("Looking up {to}'s mailbox key...");
    let key = find_mailbox_key(layout, &peers, &session.identity, to)
        .await
        .with_context(|| {
            format!(
                "{to} has no published mailbox key; they need to log into rymsh at least once \
                 while connected to a shared peer"
            )
        })?;

    let sealed = rymnet_mail::crypto::seal(&key.public_key()?, &bytes)?;
    let ciphertext_hash = ChunkStore::new(layout.chunks_root()).put(&sealed.ciphertext)?;

    println!("Pushing sealed file chunk to peers...");
    let mut chunk_delivered = 0;
    for peer in &peers.peers {
        let accepted = send_request(
            peer,
            &session.identity,
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
    println!("Chunk delivered to {chunk_delivered}/{} peer(s).", peers.peers.len());

    let body = MailBody {
        subject: format!("File: {name}"),
        cc: Vec::new(),
        bcc: Vec::new(),
        body: String::new(),
        in_reply_to: None,
        thread_id: None,
        attachment: Some(AttachmentRef {
            name,
            size,
            mime,
            ciphertext_hash,
            ephemeral_public_key: sealed.ephemeral_public,
            nonce: sealed.nonce,
        }),
    };
    send_envelope(layout, session, &peers, to, &body, false).await?;
    Ok(())
}

/// Fetches a message's attached file from every configured peer (local
/// disk isn't checked first here the way inbox fetch does -- the sealed
/// chunk was never "ours," it only ever lived wherever `send_file`
/// pushed it), verifies the chunk hash, unseals it with this account's
/// mailbox secret, and writes the plaintext into `dest` in this
/// account's virtual computer.
async fn save_attachment(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    vfs: &VirtualComputer,
    attachment: &AttachmentRef,
    dest: &str,
) -> Result<()> {
    let store = ChunkStore::new(layout.chunks_root());
    let mut sealed_bytes = None;
    if store.exists(&attachment.ciphertext_hash).unwrap_or(false) {
        sealed_bytes = store.get(&attachment.ciphertext_hash).ok();
    }
    if sealed_bytes.is_none() {
        for peer in &peers.peers {
            let Ok(response) = send_request(
                peer,
                &session.identity,
                PeerMessage::new("file.chunk.request", json!({ "hash": attachment.ciphertext_hash })),
            )
            .await
            else {
                continue;
            };
            let Some(hash) = response.payload.get("hash").and_then(|v| v.as_str()) else { continue };
            let Some(b64) = response.payload.get("bytes").and_then(|v| v.as_str()) else { continue };
            let Ok(bytes) = b64_to_bytes(b64) else { continue };
            // Re-verify both that this is really the hash we asked for
            // and that the bytes actually match it -- the same
            // double-check `rym site fetch` does before trusting a peer's
            // response, since nothing stops a peer from lying about
            // either.
            if hash == attachment.ciphertext_hash && store.verify(hash, &bytes) {
                let _ = store.put(&bytes);
                sealed_bytes = Some(bytes);
                break;
            }
        }
    }
    let sealed_bytes = sealed_bytes
        .with_context(|| format!("no peer could provide the file chunk {}", attachment.ciphertext_hash))?;

    let plaintext = rymnet_mail::crypto::open(
        &session.mailbox_secret,
        &attachment.ephemeral_public_key,
        &attachment.nonce,
        &sealed_bytes,
    )
    .context("failed to unseal attachment -- it wasn't sealed for this account")?;

    let real_path = vfs.real_path(dest);
    if let Some(parent) = real_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&real_path, &plaintext).with_context(|| format!("write {}", real_path.display()))?;
    Ok(())
}

async fn send_envelope(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    to: &str,
    body: &MailBody,
    quiet: bool,
) -> Result<(usize, usize)> {
    if !quiet {
        println!("Looking up {to}'s mailbox key...");
    }
    let key = find_mailbox_key(layout, peers, &session.identity, to)
        .await
        .with_context(|| {
            format!(
                "{to} has no published mailbox key; they need to log into rymsh at least once \
                 while connected to a shared peer"
            )
        })?;
    let envelope = MailEnvelope::create(
        to,
        &session.username,
        &session.identity,
        &key.public_key()?,
        body,
    )?;

    let claim = rymnet_account::find_claim(layout.account_claims_dir(), &session.username)?
        .context("missing local account claim; log in again")?;

    let mut delivered = 0;
    for peer in &peers.peers {
        let _ = send_request(
            peer,
            &session.identity,
            PeerMessage::new("account.claim.announce", json!({ "claim": claim })),
        )
        .await;
        if deliver_to_peer(peer, &session.identity, &envelope).await {
            delivered += 1;
        }
    }
    if !quiet {
        println!(
            "Sent to {to}: delivered to {delivered}/{} peer(s).",
            peers.peers.len()
        );
    }
    Ok((delivered, peers.peers.len()))
}

async fn deliver_to_peer(
    peer: &PeerEntry,
    identity: &NodeIdentity,
    envelope: &MailEnvelope,
) -> bool {
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
        let claim_response = send_request(
            peer,
            identity,
            PeerMessage::new("account.claim.request", json!({ "username": username })),
        )
        .await;
        let Ok(claim_response) = claim_response else {
            continue;
        };
        let Some(claim_value) = claim_response.payload.get("claim").filter(|v| !v.is_null()) else {
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

        let key_response = send_request(
            peer,
            identity,
            PeerMessage::new(
                "account.mailbox_key.request",
                json!({ "username": username }),
            ),
        )
        .await;
        let Ok(key_response) = key_response else {
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

fn split_addresses(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Fetches the account's inbox from every configured peer (deduplicating by
/// message id, since a broadcast delivery means several peers may hold the
/// same message), decrypts and verifies each one, and drops into a small
/// numbered list-and-read loop -- the same interaction style as `lynx`.
pub async fn open_inbox(layout: &DataLayout, session: &AccountSession, vfs: &VirtualComputer) -> Result<()> {
    let peers = PeerConfig::load(layout.peers_path())?;
    println!("Fetching inbox from {} peer(s)...", peers.peers.len());

    let mut by_id: HashMap<String, MailEnvelope> = HashMap::new();

    // A message can land on the very daemon this rymsh is talking to
    // (e.g. it was delivered here because *this* node was the sender's
    // peer), not just on nodes reachable through our own peers.toml --
    // check local disk directly first, the same fix already applied to
    // vfs pointers and mailbox keys.
    let local_dir = layout.mail_inbox_dir(&session.username);
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
            &session.identity,
            PeerMessage::new(
                "mail.inbox.request",
                json!({ "username": session.username }),
            ),
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
        let Ok(body) = envelope.decrypt_body(&session.mailbox_secret) else {
            continue; // not sealed for us, or corrupted -- silently skip
        };
        let signature_ok = envelope.verify_signature().unwrap_or(false);
        let sender_key =
            sender_owner_key(&peers, &session.identity, &envelope.from, &mut claim_cache).await;
        let verified =
            signature_ok && sender_key.as_deref() == Some(envelope.from_public_key.as_str());
        messages.push(InboxMessage {
            envelope,
            body,
            verified,
        });
    }
    messages.sort_by_key(|m| std::cmp::Reverse(m.envelope.sent_at));

    if messages.is_empty() {
        println!("Inbox empty.");
        return Ok(());
    }

    loop {
        println!();
        println!("Inbox: {} message(s)", messages.len());
        println!();
        println!("  #   From             Subject                              Date");
        for (i, m) in messages.iter().enumerate() {
            let mark = if m.verified { "" } else { "  [unverified]" };
            println!(
                "  {:<3} {:<16} {:<37}{}{}",
                i + 1,
                truncate(&m.envelope.from, 16),
                truncate(&m.body.subject, 36),
                m.envelope
                    .sent_at
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M"),
                mark
            );
        }
        println!();
        print!("email> ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input == "q" || input == "quit" {
            break;
        }
        let Ok(index) = input.parse::<usize>() else {
            println!("type a message number, or q to quit");
            continue;
        };
        let Some(message) = messages.get(index - 1) else {
            println!("no such message: {index}");
            continue;
        };
        read_message(layout, session, &peers, vfs, message, index).await?;
    }
    Ok(())
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

async fn read_message(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    vfs: &VirtualComputer,
    message: &InboxMessage,
    index: usize,
) -> Result<()> {
    println!();
    println!("From:     {}", message.envelope.from);
    println!("To:       {}", message.envelope.to);
    if !message.body.cc.is_empty() {
        println!("Cc:       {}", message.body.cc.join(", "));
    }
    println!("Subject:  {}", message.body.subject);
    println!(
        "Date:     {}",
        message
            .envelope
            .sent_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
    );
    println!(
        "Verified: {}",
        if message.verified {
            format!("yes (signed by {}'s registered key)", message.envelope.from)
        } else {
            "no -- sender identity could not be confirmed".to_string()
        }
    );
    if let Some(attachment) = &message.body.attachment {
        println!("Attachment: {} ({} bytes, {})", attachment.name, attachment.size, attachment.mime);
    }
    println!();
    println!("{}", message.body.body);
    println!();

    loop {
        print!("email message [{index}]> ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        match input.trim() {
            "q" | "quit" | "" => break,
            "r" | "reply" => {
                reply(layout, session, peers, message).await?;
                break;
            }
            "f" | "forward" => {
                forward(layout, session, peers, message).await?;
                break;
            }
            "s" | "save" => {
                let Some(attachment) = &message.body.attachment else {
                    println!("this message has no attachment");
                    continue;
                };
                let dest = prompt_line(&format!("Save as (path in your vfs) [{}]: ", attachment.name))?;
                let dest = if dest.is_empty() { attachment.name.clone() } else { dest };
                match save_attachment(layout, session, peers, vfs, attachment, &dest).await {
                    Ok(()) => println!("saved to {dest}"),
                    Err(err) => println!("error: {err:#}"),
                }
            }
            other => {
                println!("unknown command: {other} (r reply, f forward, s save attachment, q back to inbox)")
            }
        }
    }
    Ok(())
}

async fn reply(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    message: &InboxMessage,
) -> Result<()> {
    let to = message.envelope.from.clone();
    let subject = ensure_prefix(&message.body.subject, "Re: ");
    println!("Replying to {to}. Subject: {subject}");
    println!("Body (blank line to send):");
    let body_text = prompt_multiline()?;

    let body = MailBody {
        subject,
        cc: Vec::new(),
        bcc: Vec::new(),
        body: body_text,
        in_reply_to: Some(message.envelope.id.clone()),
        thread_id: Some(
            message
                .body
                .thread_id
                .clone()
                .unwrap_or_else(|| message.envelope.id.clone()),
        ),
        // Deliberately not carried over from the original message: an
        // `AttachmentRef`'s ciphertext is sealed to one specific
        // recipient's mailbox key, so copying it into a reply to someone
        // else would point at bytes they have no way to decrypt. Sending
        // a file to a new person means a fresh `sendfile`.
        attachment: None,
    };
    send_envelope(layout, session, peers, &to, &body, false).await?;
    Ok(())
}

async fn forward(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    message: &InboxMessage,
) -> Result<()> {
    let to = prompt_line("Forward to: ")?;
    if to.is_empty() {
        bail!("no recipient given");
    }
    let subject = ensure_prefix(&message.body.subject, "Fwd: ");
    println!("Body (optional note, blank line to send):");
    let note = prompt_multiline()?;

    let mut body_text = note;
    body_text.push_str(&format!(
        "\n\n---- Forwarded message ----\nFrom: {}\nSubject: {}\n\n{}",
        message.envelope.from, message.body.subject, message.body.body
    ));

    let body = MailBody {
        subject,
        cc: Vec::new(),
        bcc: Vec::new(),
        body: body_text,
        in_reply_to: None,
        thread_id: None,
        // Same reasoning as `reply`: the original attachment's ciphertext
        // is sealed to the original recipient's key, not this new one.
        attachment: None,
    };
    send_envelope(layout, session, peers, &to, &body, false).await?;
    Ok(())
}

fn ensure_prefix(subject: &str, prefix: &str) -> String {
    if subject
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        subject.to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        text.chars()
            .take(width.saturating_sub(1))
            .collect::<String>()
            + "\u{2026}"
    }
}

fn prompt_line(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_multiline() -> Result<String> {
    let mut lines = Vec::new();
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        lines.push(line.trim_end_matches('\n').to_string());
    }
    Ok(lines.join("\n"))
}
