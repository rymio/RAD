use crate::account_session::AccountSession;
use crate::mail;
use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal,
};
use rymnet_chat::ChatMessage;
use rymnet_core::NodeIdentity;
use rymnet_network::{PeerMessage, send_request};
use rymnet_storage::{DataLayout, PeerConfig};
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::{Write, stdout};
use std::time::{Duration, Instant};

/// How this actually solves "we don't know which node the user is
/// connected to": it doesn't try to find out. Sending broadcasts a
/// signed `ChatMessage` to every one of *our own* configured peers;
/// reading polls every one of our own configured peers and dedups by
/// message id. No presence lookup, no routing table -- the exact same
/// relay-and-poll tradeoff `rymnet-mail` already makes. That means this
/// is near-real-time, not true push: messages from others typically
/// show up within one `POLL_INTERVAL`, not instantly.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

struct IrcState {
    channel: Option<String>,
    messages: Vec<ChatMessage>,
    seen_ids: HashSet<String>,
    verified: HashMap<String, bool>,
    claim_cache: HashMap<String, Option<String>>,
    input: String,
    status: Option<String>,
    last_poll: Instant,
}

pub async fn run(layout: &DataLayout, session: &AccountSession) -> Result<()> {
    let peers = PeerConfig::load(layout.peers_path())?;
    if peers.peers.is_empty() {
        anyhow::bail!("no peers configured; add one with `peer add` before joining irc");
    }

    let mut state = IrcState {
        channel: None,
        messages: Vec::new(),
        seen_ids: HashSet::new(),
        verified: HashMap::new(),
        claim_cache: HashMap::new(),
        input: String::new(),
        status: Some("Type /join #channel to start, or /help for commands.".to_string()),
        last_poll: Instant::now() - POLL_INTERVAL,
    };

    terminal::enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = run_loop(&mut out, layout, session, &peers, &mut state).await;

    execute!(out, cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

async fn run_loop(
    out: &mut impl Write,
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    state: &mut IrcState,
) -> Result<()> {
    loop {
        draw(out, state)?;

        if state.channel.is_some() && state.last_poll.elapsed() >= POLL_INTERVAL {
            poll_channel(layout, session, peers, state).await;
            state.last_poll = Instant::now();
            continue;
        }

        if event::poll(Duration::from_millis(250))? {
            let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? else {
                continue;
            };
            match (code, modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                (KeyCode::Enter, _) => {
                    let line = std::mem::take(&mut state.input);
                    if handle_line(layout, session, peers, state, &line).await? {
                        break;
                    }
                }
                (KeyCode::Backspace, _) => {
                    state.input.pop();
                }
                (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    state.input.push(ch);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Returns `true` if the REPL should exit.
async fn handle_line(
    layout: &DataLayout,
    session: &AccountSession,
    peers: &PeerConfig,
    state: &mut IrcState,
    line: &str,
) -> Result<bool> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(false);
    }

    if let Some(rest) = line.strip_prefix("/join ") {
        let channel = rest.trim().to_string();
        if let Err(err) = rymnet_chat::validate_channel(&channel) {
            state.status = Some(format!("{err:#}"));
            return Ok(false);
        }
        state.channel = Some(channel);
        state.messages.clear();
        state.seen_ids.clear();
        state.verified.clear();
        state.status = Some("Joining...".to_string());
        state.last_poll = Instant::now() - POLL_INTERVAL;
        return Ok(false);
    }

    if let Some(rest) = line.strip_prefix("/msg ") {
        let mut parts = rest.splitn(2, ' ');
        let (Some(to), Some(text)) = (parts.next(), parts.next()) else {
            state.status = Some("usage: /msg <user> <text>".to_string());
            return Ok(false);
        };
        state.status = Some(format!("Sending to {to}..."));
        match mail::send_quick(layout, session, peers, to, text).await {
            Ok((delivered, total)) => {
                state.status = Some(format!("DM to {to}: delivered to {delivered}/{total} peer(s)."));
            }
            Err(err) => {
                state.status = Some(format!("DM to {to} failed: {err:#}"));
            }
        }
        return Ok(false);
    }

    if line == "/quit" {
        return Ok(true);
    }

    if line == "/help" {
        state.status = Some(
            "/join #channel   /msg <user> <text>   /quit   Ctrl-C to leave -- \
             plain text sends to the active channel."
                .to_string(),
        );
        return Ok(false);
    }

    if line.starts_with('/') {
        state.status = Some(format!("unknown command: {line} (try /help)"));
        return Ok(false);
    }

    let Some(channel) = state.channel.clone() else {
        state.status = Some("not in a channel -- /join #channel first".to_string());
        return Ok(false);
    };

    match ChatMessage::create(&channel, &session.username, &session.identity, line) {
        Ok(message) => {
            let mut delivered = 0;
            for peer in &peers.peers {
                if send_request(
                    peer,
                    &session.identity,
                    PeerMessage::new("chat.channel.deliver", json!({ "message": &message })),
                )
                .await
                .is_ok_and(|response| {
                    response.payload.get("status").and_then(|v| v.as_str()) == Some("accepted")
                }) {
                    delivered += 1;
                }
            }
            // Echo locally right away rather than waiting for the next
            // poll to see our own message.
            if state.seen_ids.insert(message.id.clone()) {
                state.verified.insert(message.id.clone(), true);
                state.messages.push(message);
            }
            state.status = Some(format!("sent (delivered to {delivered}/{} peer(s))", peers.peers.len()));
        }
        Err(err) => {
            state.status = Some(format!("send failed: {err:#}"));
        }
    }
    Ok(false)
}

/// Local disk first, then every configured peer, dedup by message id --
/// the same ordering fix this project's mail/account-sync code already
/// needed twice: a two-node test where each side only peers with the
/// other never discovers content already sitting locally otherwise.
async fn poll_channel(layout: &DataLayout, session: &AccountSession, peers: &PeerConfig, state: &mut IrcState) {
    let Some(channel) = state.channel.clone() else {
        return;
    };
    let Ok(dir_name) = rymnet_chat::channel_dir_name(&channel) else {
        return;
    };

    let dir = layout.chat_channel_dir(&dir_name);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path())
                && let Ok(message) = serde_json::from_slice::<ChatMessage>(&bytes)
                && state.seen_ids.insert(message.id.clone())
            {
                state.messages.push(message);
            }
        }
    }

    for peer in &peers.peers {
        let since: Option<DateTime<Utc>> = state.messages.iter().map(|m| m.sent_at).max();
        let mut payload = json!({ "channel": channel });
        if let Some(since) = since {
            payload["since"] = json!(since.to_rfc3339());
        }
        if let Ok(response) = send_request(
            peer,
            &session.identity,
            PeerMessage::new("chat.channel.fetch", payload),
        )
        .await
            && let Some(values) = response.payload.get("messages").and_then(|v| v.as_array())
        {
            for value in values {
                if let Ok(message) = serde_json::from_value::<ChatMessage>(value.clone())
                    && state.seen_ids.insert(message.id.clone())
                {
                    state.messages.push(message);
                }
            }
        }
    }

    state.messages.sort_by_key(|m| m.sent_at);
    for message in &state.messages {
        if state.verified.contains_key(&message.id) {
            continue;
        }
        let signature_ok = message.verify_signature().unwrap_or(false);
        let sender_key = sender_owner_key(peers, &session.identity, &message.from, &mut state.claim_cache).await;
        let verified = signature_ok && sender_key.as_deref() == Some(message.from_public_key.as_str());
        state.verified.insert(message.id.clone(), verified);
    }
    state.status = None;
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

fn draw(out: &mut impl Write, state: &IrcState) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    queue!(out, terminal::Clear(terminal::ClearType::All))?;

    let header = match &state.channel {
        Some(channel) => format!("rymirc -- {channel}"),
        None => "rymirc -- no channel joined".to_string(),
    };
    queue!(out, cursor::MoveTo(0, 0), SetAttribute(Attribute::Reverse), Print(fit(&header, cols)), SetAttribute(Attribute::Reset))?;

    let scrollback_height = rows.saturating_sub(4) as usize;
    let start = state.messages.len().saturating_sub(scrollback_height);
    for (row, message) in state.messages[start..].iter().enumerate() {
        let verified = state.verified.get(&message.id).copied().unwrap_or(false);
        let mark = if verified { "" } else { " [unverified]" };
        let time = message.sent_at.format("%H:%M");
        let line = format!("[{time}] {}{}: {}", message.from, mark, message.body);
        queue!(out, cursor::MoveTo(0, row as u16 + 1), Print(fit(&line, cols)))?;
    }

    if let Some(status) = &state.status {
        queue!(out, cursor::MoveTo(0, rows.saturating_sub(2)), SetAttribute(Attribute::Reverse), Print(fit(status, cols)), SetAttribute(Attribute::Reset))?;
    }

    let prompt = format!("> {}", state.input);
    queue!(out, cursor::MoveTo(0, rows.saturating_sub(1)), Print(fit(&prompt, cols)))?;

    out.flush()?;
    Ok(())
}

fn fit(text: &str, width: u16) -> String {
    let width = width as usize;
    let mut out: String = text.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}
