use crate::session::{AccountState, Pending, WebSession};
use crate::vfs::VirtualComputer;
use crate::{account, browse, mail};
use rymnet_core::NodeIdentity;
use rymnet_storage::{AppConfig, DataLayout, PeerConfig};
use std::path::PathBuf;

pub struct AppState {
    pub layout: DataLayout,
    pub server_identity: NodeIdentity,
}

pub struct ExecResult {
    pub lines: Vec<String>,
    pub prompt: String,
}

pub async fn exec(state: &AppState, session_id: &str, session: &mut WebSession, line: &str) -> ExecResult {
    if let Some(pending) = session.pending.take() {
        let lines = continue_pending(state, session, pending, line).await;
        return ExecResult {
            lines,
            prompt: prompt_for(session),
        };
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ExecResult {
            lines: Vec::new(),
            prompt: prompt_for(session),
        };
    }

    let tokens = match shell_words::split(trimmed) {
        Ok(tokens) => tokens,
        Err(err) => {
            return ExecResult {
                lines: vec![format!("parse error: {err}")],
                prompt: prompt_for(session),
            };
        }
    };
    let command = tokens[0].clone();
    let args = &tokens[1..];

    let lines = match command.as_str() {
        "help" => help_text(),
        "login" => cmd_login(state, session_id, session, args).await,
        "logout" | "exit" | "quit" => cmd_logout(state, session).await,
        "whoami" => cmd_whoami(session),
        "df" => cmd_df(state, session),
        "status" => cmd_status(state, session).await,
        "clear" => vec!["\u{0}CLEAR\u{0}".to_string()],
        "ls" => wrap(session.vfs.ls(args.first().map(String::as_str))),
        "cd" => {
            let target = args.first().map(String::as_str).unwrap_or("/");
            match session.vfs.cd(target) {
                Ok(()) => Vec::new(),
                Err(err) => vec![format!("error: {err:#}")],
            }
        }
        "pwd" => vec![session.vfs.pwd()],
        "mkdir" => match require_arg(args, "mkdir <dir>") {
            Ok(a) => wrap_unit(session.vfs.mkdir(a)),
            Err(e) => vec![e],
        },
        "touch" => match require_arg(args, "touch <file>") {
            Ok(a) => wrap_unit(session.vfs.touch(a)),
            Err(e) => vec![e],
        },
        "rm" => cmd_rm(session, args),
        "cp" => match require_two_args(args, "cp <src> <dst>") {
            Ok([a, b]) => wrap_unit(session.vfs.cp(a, b)),
            Err(e) => vec![e],
        },
        "mv" => match require_two_args(args, "mv <src> <dst>") {
            Ok([a, b]) => wrap_unit(session.vfs.mv(a, b)),
            Err(e) => vec![e],
        },
        "cat" => match require_arg(args, "cat <file>") {
            Ok(a) => match session.vfs.cat(a) {
                Ok(text) => text.lines().map(str::to_string).collect(),
                Err(err) => vec![format!("error: {err:#}")],
            },
            Err(e) => vec![e],
        },
        "edit" | "nano" => cmd_edit_start(session, args),
        "sync" | "push" => cmd_sync(state, session).await,
        "pull" => cmd_pull(state, session).await,
        "lynx" | "browse" | "open" => cmd_lynx(state, session, args).await,
        "search" => cmd_search(state, session, args).await,
        "emailto" => cmd_emailto_start(session, args),
        "email" => cmd_email(state, session, args).await,
        other => vec![format!(
            "unknown command: {other}. Type `help` for a list of commands."
        )],
    };

    ExecResult {
        lines,
        prompt: prompt_for(session),
    }
}

fn prompt_for(session: &WebSession) -> String {
    if let Some(pending) = &session.pending {
        return match pending {
            Pending::ComposeCc { .. } => "Cc (comma-separated, optional): ".to_string(),
            Pending::ComposeBcc { .. } => "Bcc (comma-separated, optional): ".to_string(),
            Pending::ComposeSubject { .. } => "Subject: ".to_string(),
            Pending::ComposeBody { .. } => "> ".to_string(),
            Pending::EditBody { .. } => "> ".to_string(),
            Pending::ForwardTo { .. } => "Forward to: ".to_string(),
            Pending::ForwardNote { .. } => "> ".to_string(),
        };
    }
    match &session.account {
        Some(account) => format!("rymweb:{}@{}$ ", account.username, session.vfs.display_path()),
        None => format!("rymweb:{}$ ", session.vfs.display_path()),
    }
}

fn wrap(result: anyhow::Result<Vec<String>>) -> Vec<String> {
    match result {
        Ok(lines) => lines,
        Err(err) => vec![format!("error: {err:#}")],
    }
}

fn wrap_unit(result: anyhow::Result<()>) -> Vec<String> {
    match result {
        Ok(()) => Vec::new(),
        Err(err) => vec![format!("error: {err:#}")],
    }
}

fn require_arg<'a>(args: &'a [String], usage: &str) -> Result<&'a str, String> {
    args.first()
        .map(String::as_str)
        .ok_or_else(|| format!("usage: {usage}"))
}

fn require_two_args<'a>(args: &'a [String], usage: &str) -> Result<[&'a str; 2], String> {
    if args.len() < 2 {
        return Err(format!("usage: {usage}"));
    }
    Ok([args[0].as_str(), args[1].as_str()])
}

fn cmd_rm(session: &WebSession, args: &[String]) -> Vec<String> {
    let recursive = args.iter().any(|a| a == "-r" || a == "-rf");
    match args.iter().find(|a| !a.starts_with('-')) {
        Some(target) => wrap_unit(session.vfs.rm(target, recursive)),
        None => vec!["usage: rm [-r] <path>".to_string()],
    }
}

async fn cmd_login(
    state: &AppState,
    session_id: &str,
    session: &mut WebSession,
    args: &[String],
) -> Vec<String> {
    if session.account.is_some() {
        return vec!["already logged in; run `logout` first".to_string()];
    }
    let [username, password] = match require_two_args(args, "login <username> <password>") {
        Ok(v) => v,
        Err(e) => return vec![e],
    };
    let cache_dir = account::cache_dir_for_session(&state.layout, session_id);
    match account::login(&state.layout, username, password, &cache_dir).await {
        Ok(result) => {
            let vfs = match VirtualComputer::open(cache_dir.clone()) {
                Ok(vfs) => vfs,
                Err(err) => return vec![format!("error opening virtual computer: {err:#}")],
            };
            session.vfs = vfs;
            let mut lines = vec![format!(
                "Logged in as {username} (peers reached: {})",
                result.peers_reached
            )];
            if result.fresh_account {
                lines.push("No existing virtual computer found on the network; starting fresh.".to_string());
            } else {
                lines.push(format!("Restored {} file(s) from the network.", result.files_restored));
            }
            session.account = Some(AccountState {
                username: username.to_string(),
                identity: result.identity,
                cipher_key: result.cipher_key,
                mailbox_secret: result.mailbox_secret,
                vfs_version: result.vfs_version,
                cache_dir,
            });
            lines
        }
        Err(err) => vec![format!("error: {err:#}")],
    }
}

async fn cmd_logout(state: &AppState, session: &mut WebSession) -> Vec<String> {
    let Some(account) = session.account.take() else {
        return vec!["not logged in".to_string()];
    };
    let mut lines = vec!["Syncing your virtual computer before logging out...".to_string()];
    match account::push(
        &state.layout,
        &account.username,
        &account.identity,
        &account.cipher_key,
        &account.cache_dir,
        account.vfs_version,
    )
    .await
    {
        Ok(summary) => lines.push(format!(
            "Pushed version {} ({} file(s)) to {}/{} peers.",
            summary.vfs_version, summary.files_pushed, summary.peers_accepted, summary.peers_total
        )),
        Err(err) => lines.push(format!("warning: could not sync before logout: {err:#}")),
    }
    if let Ok(vfs) = VirtualComputer::open(guest_dir(&state.layout, session)) {
        session.vfs = vfs;
    }
    lines
}

fn guest_dir(layout: &DataLayout, session: &WebSession) -> PathBuf {
    let _ = session;
    layout.root.join("web-sessions-guest").join(uuid::Uuid::new_v4().to_string())
}

fn cmd_whoami(session: &WebSession) -> Vec<String> {
    match &session.account {
        Some(account) => vec![format!(
            "username: {}\npublic key: {}\nvfs version: {}",
            account.username, account.identity.public_key, account.vfs_version
        )],
        None => vec!["not logged in. Use: login <username> <password>".to_string()],
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn dir_size(path: &std::path::Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

fn cmd_df(state: &AppState, session: &WebSession) -> Vec<String> {
    let used = dir_size(&session.vfs.root);
    let mut lines = vec![format!(
        "{:<26} {:>10} {:>10} {:>10} {:>6}",
        "VIRTUAL COMPUTER", "SIZE", "USED", "AVAIL", "USE%"
    )];
    match &session.account {
        Some(account) => {
            let config = AppConfig::load(state.layout.config_path())
                .map(|c| c.account)
                .unwrap_or_default();
            if config.is_unlimited(&account.username) {
                lines.push(format!(
                    "{:<26} {:>10} {:>10} {:>10} {:>6}",
                    format!("rymnet:{}", account.username),
                    "unlimited",
                    human_bytes(used),
                    "unlimited",
                    "0.0%"
                ));
            } else {
                let quota_bytes = rymnet_account::quota_bytes(config.quota_mb);
                let avail = quota_bytes.saturating_sub(used);
                let pct = if quota_bytes == 0 {
                    0.0
                } else {
                    used as f64 / quota_bytes as f64 * 100.0
                };
                lines.push(format!(
                    "{:<26} {:>10} {:>10} {:>10} {:>5.1}%",
                    format!("rymnet:{}", account.username),
                    human_bytes(quota_bytes),
                    human_bytes(used),
                    human_bytes(avail),
                    pct
                ));
            }
        }
        None => {
            lines.push(format!(
                "{:<26} {:>10} {:>10} {:>10} {:>6}",
                "local (not logged in)",
                "--",
                human_bytes(used),
                "--",
                "--"
            ));
        }
    }
    lines
}

async fn cmd_status(state: &AppState, session: &WebSession) -> Vec<String> {
    let peers = PeerConfig::load(state.layout.peers_path()).unwrap_or_default();
    let mut lines = vec![
        format!("Server node: {}", state.server_identity.node_id().map(|id| id.to_string()).unwrap_or_default()),
        format!("Configured peers: {}", peers.peers.len()),
    ];
    match &session.account {
        Some(account) => lines.push(format!("Logged in as: {}", account.username)),
        None => lines.push("Logged in as: (none)".to_string()),
    }
    lines
}

fn cmd_edit_start(session: &mut WebSession, args: &[String]) -> Vec<String> {
    let Some(path) = args.first() else {
        return vec!["usage: edit <file>".to_string()];
    };
    let existing = session.vfs.cat(path).unwrap_or_default();
    let mut lines = vec![format!("Editing {path}. Current contents:"), "----".to_string()];
    lines.extend(existing.lines().map(str::to_string));
    lines.push("----".to_string());
    lines.push("Type new contents, blank line to save, Ctrl+C-equivalent: send just \".\" to cancel.".to_string());
    session.pending = Some(Pending::EditBody {
        path: path.clone(),
        lines: Vec::new(),
    });
    lines
}

async fn cmd_sync(state: &AppState, session: &mut WebSession) -> Vec<String> {
    let Some(account) = session.account.as_mut() else {
        return vec!["log in first".to_string()];
    };
    match account::push(
        &state.layout,
        &account.username,
        &account.identity,
        &account.cipher_key,
        &account.cache_dir,
        account.vfs_version,
    )
    .await
    {
        Ok(summary) => {
            account.vfs_version = summary.vfs_version;
            vec![format!(
                "Pushed version {} ({} file(s), {:.2} MB) - accepted by {}/{} peers.",
                summary.vfs_version,
                summary.files_pushed,
                summary.total_bytes as f64 / (1024.0 * 1024.0),
                summary.peers_accepted,
                summary.peers_total
            )]
        }
        Err(err) => vec![format!("error: {err:#}")],
    }
}

async fn cmd_pull(_state: &AppState, session: &mut WebSession) -> Vec<String> {
    if session.account.is_none() {
        return vec!["log in first".to_string()];
    }
    // Login already pulls the latest vfs; there's no separate pull-only
    // path, so point the user at the flow that actually re-fetches it.
    vec!["`pull` isn't wired up as its own step -- `logout` then `login` again to refresh from the network.".to_string()]
}

async fn cmd_lynx(state: &AppState, session: &WebSession, args: &[String]) -> Vec<String> {
    let Some(target) = args.first() else {
        return vec!["usage: lynx <domain.rym>[/path]".to_string()];
    };
    let identity = session.identity(&state.server_identity);
    render_page(state, identity, target).await
}

async fn render_page(state: &AppState, identity: &NodeIdentity, target: &str) -> Vec<String> {
    let trimmed = target.trim().trim_start_matches('/');
    let (domain, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
    let site_path = format!("/{rest}");
    if domain == rymnet_gateway::SEARCH_DOMAIN {
        return vec!["use `search <query>` to search rymnet from the web terminal".to_string()];
    }
    let peers = match PeerConfig::load(state.layout.peers_path()) {
        Ok(p) => p,
        Err(err) => return vec![format!("error: {err:#}")],
    };
    match browse::fetch_page(identity, &peers, domain, &site_path).await {
        Ok(bytes) => {
            let (text, links) = browse::render_text(&String::from_utf8_lossy(&bytes));
            let mut lines = vec![format!("=== {domain}{rest_display} ===", rest_display = if rest.is_empty() { "/".to_string() } else { format!("/{rest}") })];
            lines.extend(wrap_text(&text, 90));
            if !links.is_empty() {
                lines.push(String::new());
                lines.push("Links:".to_string());
                for link in &links {
                    lines.push(format!("  lynx {link}"));
                }
            }
            lines
        }
        Err(err) => vec![format!("error: {err:#}")],
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

async fn cmd_search(state: &AppState, session: &WebSession, args: &[String]) -> Vec<String> {
    if args.is_empty() {
        return vec!["usage: search <query>".to_string()];
    }
    let query = args.join(" ");
    let identity = session.identity(&state.server_identity);
    let peers = match PeerConfig::load(state.layout.peers_path()) {
        Ok(p) => p,
        Err(err) => return vec![format!("error: {err:#}")],
    };
    match browse::search(identity, &peers, &query).await {
        Ok(hits) if hits.is_empty() => vec![format!("Search: {query}"), "No results.".to_string()],
        Ok(hits) => {
            let mut lines = vec![format!("Search: {query}"), format!("{} result(s)", hits.len())];
            for (i, hit) in hits.iter().enumerate() {
                let suffix = if hit.path == "/index.html" { "/".to_string() } else { hit.path.clone() };
                lines.push(String::new());
                let title = if hit.title.is_empty() { hit.domain.clone() } else { hit.title.clone() };
                lines.push(format!("{}. {title}", i + 1));
                if !hit.description.is_empty() {
                    lines.push(hit.description.clone());
                }
                lines.push(format!("  lynx {}{suffix}", hit.domain));
            }
            lines
        }
        Err(err) => vec![format!("error: {err:#}")],
    }
}

fn cmd_emailto_start(session: &mut WebSession, args: &[String]) -> Vec<String> {
    if session.account.is_none() {
        return vec!["log in first".to_string()];
    }
    let Some(to) = args.first() else {
        return vec!["usage: emailto <username>".to_string()];
    };
    session.pending = Some(Pending::ComposeCc { to: to.clone() });
    vec![format!("Composing to {to}.")]
}

async fn cmd_email(state: &AppState, session: &mut WebSession, args: &[String]) -> Vec<String> {
    let Some(account) = &session.account else {
        return vec!["log in first".to_string()];
    };
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let messages = match mail::inbox(&state.layout, &account.username, &account.identity, &account.mailbox_secret).await {
                Ok(m) => m,
                Err(err) => return vec![format!("error: {err:#}")],
            };
            let mut lines = vec![format!("Inbox: {} message(s)", messages.len())];
            for (i, m) in messages.iter().enumerate() {
                let mark = if m.verified { "" } else { " [unverified]" };
                lines.push(format!(
                    "  {} {}  {}  {}{}",
                    i + 1,
                    m.envelope.from,
                    m.body.subject,
                    m.envelope.sent_at.format("%Y-%m-%d %H:%M"),
                    mark
                ));
            }
            lines.push(String::new());
            lines.push("Use `email read <n>`, `email reply <n>`, or `email forward <n>`.".to_string());
            session.last_inbox = messages;
            lines
        }
        Some("read") => {
            let Some(index) = args.get(1).and_then(|s| s.parse::<usize>().ok()) else {
                return vec!["usage: email read <n>".to_string()];
            };
            match session.last_inbox.get(index.wrapping_sub(1)) {
                Some(m) => {
                    let mut lines = vec![
                        format!("From:     {}", m.envelope.from),
                        format!("To:       {}", m.envelope.to),
                        format!("Subject:  {}", m.body.subject),
                        format!("Date:     {}", m.envelope.sent_at.format("%Y-%m-%d %H:%M")),
                        format!(
                            "Verified: {}",
                            if m.verified { "yes" } else { "no -- sender identity could not be confirmed" }
                        ),
                        String::new(),
                    ];
                    lines.extend(m.body.body.lines().map(str::to_string));
                    lines
                }
                None => vec![format!("no such message: {index} (run `email` first to list them)")],
            }
        }
        Some("reply") => {
            let Some(index) = args.get(1).and_then(|s| s.parse::<usize>().ok()) else {
                return vec!["usage: email reply <n>".to_string()];
            };
            let Some(m) = session.last_inbox.get(index.wrapping_sub(1)) else {
                return vec![format!("no such message: {index}")];
            };
            let to = m.envelope.from.clone();
            let subject = ensure_prefix(&m.body.subject, "Re: ");
            let in_reply_to = Some(m.envelope.id.clone());
            let thread_id = Some(m.body.thread_id.clone().unwrap_or_else(|| m.envelope.id.clone()));
            session.pending = Some(Pending::ComposeBody {
                to: to.clone(),
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: subject.clone(),
                lines: Vec::new(),
                in_reply_to,
                thread_id,
            });
            vec![
                format!("Replying to {to}. Subject: {subject}"),
                "Type your message, blank line to send.".to_string(),
            ]
        }
        Some("forward") => {
            let Some(index) = args.get(1).and_then(|s| s.parse::<usize>().ok()) else {
                return vec!["usage: email forward <n>".to_string()];
            };
            if session.last_inbox.get(index.wrapping_sub(1)).is_none() {
                return vec![format!("no such message: {index}")];
            }
            session.pending = Some(Pending::ForwardTo {
                original_index: index.wrapping_sub(1),
            });
            vec!["Forward to (username):".to_string()]
        }
        Some(other) => vec![format!("unknown email subcommand: {other}")],
    }
}

fn ensure_prefix(subject: &str, prefix: &str) -> String {
    if subject.to_ascii_lowercase().starts_with(&prefix.to_ascii_lowercase()) {
        subject.to_string()
    } else {
        format!("{prefix}{subject}")
    }
}

async fn continue_pending(state: &AppState, session: &mut WebSession, pending: Pending, line: &str) -> Vec<String> {
    let input = line.trim_end_matches('\r');
    match pending {
        Pending::ComposeCc { to } => {
            session.pending = Some(Pending::ComposeBcc {
                to,
                cc: split_addresses(input),
            });
            Vec::new()
        }
        Pending::ComposeBcc { to, cc } => {
            session.pending = Some(Pending::ComposeSubject {
                to,
                cc,
                bcc: split_addresses(input),
            });
            Vec::new()
        }
        Pending::ComposeSubject { to, cc, bcc } => {
            session.pending = Some(Pending::ComposeBody {
                to,
                cc,
                bcc,
                subject: input.to_string(),
                lines: Vec::new(),
                in_reply_to: None,
                thread_id: None,
            });
            vec!["Type your message, blank line to send.".to_string()]
        }
        Pending::ComposeBody {
            to,
            cc,
            bcc,
            subject,
            mut lines,
            in_reply_to,
            thread_id,
        } => {
            if input.is_empty() {
                let Some(account) = &session.account else {
                    return vec!["log in first".to_string()];
                };
                let body = lines.join("\n");
                match mail::send(
                    &state.layout,
                    &account.username,
                    &account.identity,
                    &to,
                    &subject,
                    cc,
                    bcc,
                    &body,
                    in_reply_to,
                    thread_id,
                )
                .await
                {
                    Ok((delivered, total)) => vec![format!("Sent to {to}: delivered to {delivered}/{total} peer(s).")],
                    Err(err) => vec![format!("error: {err:#}")],
                }
            } else {
                lines.push(input.to_string());
                session.pending = Some(Pending::ComposeBody {
                    to,
                    cc,
                    bcc,
                    subject,
                    lines,
                    in_reply_to,
                    thread_id,
                });
                Vec::new()
            }
        }
        Pending::EditBody { path, mut lines } => {
            if input == "." {
                return vec!["Cancelled.".to_string()];
            }
            if input.is_empty() {
                let contents = lines.join("\n");
                return wrap_unit(session.vfs.write(&path, &contents))
                    .into_iter()
                    .chain(std::iter::once(format!("Saved {path}.")))
                    .collect();
            }
            lines.push(input.to_string());
            session.pending = Some(Pending::EditBody { path, lines });
            Vec::new()
        }
        Pending::ForwardTo { original_index } => {
            if input.trim().is_empty() {
                return vec!["forward cancelled".to_string()];
            }
            session.pending = Some(Pending::ForwardNote {
                to: input.trim().to_string(),
                original_index,
                lines: Vec::new(),
            });
            vec!["Optional note (blank line to send):".to_string()]
        }
        Pending::ForwardNote {
            to,
            original_index,
            mut lines,
        } => {
            if input.is_empty() {
                let Some(account) = &session.account else {
                    return vec!["log in first".to_string()];
                };
                let Some(original) = session.last_inbox.get(original_index) else {
                    return vec!["original message no longer available".to_string()];
                };
                let subject = ensure_prefix(&original.body.subject, "Fwd: ");
                let mut body = lines.join("\n");
                body.push_str(&format!(
                    "\n\n---- Forwarded message ----\nFrom: {}\nSubject: {}\n\n{}",
                    original.envelope.from, original.body.subject, original.body.body
                ));
                match mail::send(
                    &state.layout,
                    &account.username,
                    &account.identity,
                    &to,
                    &subject,
                    Vec::new(),
                    Vec::new(),
                    &body,
                    None,
                    None,
                )
                .await
                {
                    Ok((delivered, total)) => vec![format!("Sent to {to}: delivered to {delivered}/{total} peer(s).")],
                    Err(err) => vec![format!("error: {err:#}")],
                }
            } else {
                lines.push(input.to_string());
                session.pending = Some(Pending::ForwardNote {
                    to,
                    original_index,
                    lines,
                });
                Vec::new()
            }
        }
    }
}

fn split_addresses(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn help_text() -> Vec<String> {
    r#"rymweb -- rymsh, in your browser

File commands (sandboxed to your own virtual computer):
  ls [path]   cd <path>   pwd   mkdir <dir>   touch <file>
  rm [-r] <path>   cp <src> <dst>   mv <src> <dst>   cat <file>
  edit <file>          multi-line editor: type content, blank line to save

Account (password-derived, synced over the peer network -- see docs for
what that means about server trust):
  login <user> <pass>   logout   sync | push   whoami   df

Browsing (no JavaScript ever runs; sites are HTML/CSS only):
  lynx <domain.rym>[/path]        search <query>

Email:
  emailto <user>        guided compose: Cc, Bcc, Subject, body
  email                 list inbox
  email read <n>        email reply <n>        email forward <n>

Not available in the web terminal (yet): `mc` (needs real local
filesystem access a browser can't give it) and the embedded Python REPL.
Use native `rymsh` for those.
"#
    .lines()
    .map(str::to_string)
    .collect()
}
