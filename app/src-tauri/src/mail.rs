//! The Email view: reads mail straight from the user's IMAP server. Nothing is stored —
//! messages are fetched when shown and live only in the window's memory. Opening a message
//! marks it as read on the server. The password stays in the OS keychain on this device.

use std::sync::Arc;
use std::time::Duration;

use async_imap::types::{Fetch, Flag, NameAttribute};
use async_imap::{Client, Session};
use base64::Engine as _;
use futures::TryStreamExt;
use mail_parser::{Address, MessageParser, MimeHeaders};
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls;

use crate::client::{CmdError, CmdResult};
use crate::App;
use crate::secrets;

type Imap = Session<TlsStream<TcpStream>>;

const PAGE: u32 = 50;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const OP_TIMEOUT: Duration = Duration::from_secs(90);
/// Inline images larger than this are left out of the message view.
const INLINE_IMAGE_MAX: usize = 4 * 1024 * 1024;

/// The account as saved in the vault (everything but the password).
#[derive(Deserialize, Clone)]
pub struct Account {
    pub host: String,
    pub port: u16,
    /// "ssl" (implicit TLS, usually 993) or "starttls" (usually 143).
    pub security: String,
    pub username: String,
}

impl Account {
    fn key(&self) -> String {
        format!("email:{}@{}:{}", self.username, self.host.to_lowercase(), self.port)
    }
}

/// One open IMAP session, reused between calls.
#[derive(Default)]
pub struct Mail {
    conn: Arc<Mutex<Option<(String, Imap)>>>,
}

impl Mail {
    /// Logs out in the background (on lock, or when the account changes).
    pub fn disconnect(&self) {
        let conn = self.conn.clone();
        tauri::async_runtime::spawn(async move {
            if let Some((_, mut s)) = conn.lock().await.take() {
                let _ = tokio::time::timeout(Duration::from_secs(5), s.logout()).await;
            }
        });
    }
}

#[derive(Serialize)]
pub struct Folder {
    /// The server's name, used to open it.
    name: String,
    /// For display: decoded, with provider prefixes like "[Gmail]/" removed.
    label: String,
    /// inbox | sent | drafts | archive | junk | trash | all | flagged
    special: Option<&'static str>,
}

#[derive(Serialize)]
pub struct Summary {
    uid: u32,
    from_name: String,
    from_addr: String,
    to: String,
    subject: String,
    date: i64,
    seen: bool,
    flagged: bool,
    attachments: bool,
    size: u32,
}

#[derive(Serialize)]
pub struct Page {
    total: u32,
    messages: Vec<Summary>,
    /// Pass back as `cursor` for the next (older) page; none when there's nothing more.
    next: Option<u32>,
}

#[derive(Serialize)]
pub struct Attachment {
    name: String,
    mime: String,
    size: usize,
}

#[derive(Serialize)]
pub struct FullMessage {
    uid: u32,
    subject: String,
    from: String,
    to: String,
    cc: String,
    reply_to: String,
    date: i64,
    text: String,
    html: Option<String>,
    attachments: Vec<Attachment>,
    flagged: bool,
}

fn mail_err(e: impl std::fmt::Display) -> CmdError {
    CmdError::new("mail", e.to_string())
}

fn unlocked(st: &App) -> CmdResult<()> {
    if st.client.has_session() {
        Ok(())
    } else {
        Err(CmdError::new("locked", "Unlock RAD to read email"))
    }
}

fn tls_connector() -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("rustls protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
}

async fn login(acc: &Account, password: &str) -> CmdResult<Imap> {
    let host = acc.host.trim();
    let tcp = TcpStream::connect((host, acc.port))
        .await
        .map_err(|e| CmdError::new("mail", format!("Can't reach {host}:{}: {e}", acc.port)))?;
    let starttls = acc.security == "starttls";
    let tcp = if starttls {
        let mut c = Client::new(tcp);
        c.read_response().await.map_err(mail_err)?.ok_or_else(|| mail_err("The server closed the connection"))?;
        c.run_command_and_check_ok("STARTTLS", None)
            .await
            .map_err(|e| CmdError::new("mail", format!("The server doesn't support STARTTLS: {e}")))?;
        c.into_inner()
    } else {
        tcp
    };
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| mail_err("That server name isn't valid"))?;
    let tls = tls_connector()
        .connect(name, tcp)
        .await
        .map_err(|e| CmdError::new("mail", format!("Secure connection to {host} failed: {e}")))?;
    let mut client = Client::new(tls);
    if !starttls {
        client.read_response().await.map_err(mail_err)?.ok_or_else(|| mail_err("The server closed the connection"))?;
    }
    client.login(&acc.username, password).await.map_err(|(e, _)| {
        CmdError::new(
            "auth",
            format!("Sign-in failed ({e}). Check the username and password — Gmail, iCloud, Yahoo and Outlook need an app password, not your normal one."),
        )
    })
}

async fn connect(acc: &Account, password: &str) -> CmdResult<Imap> {
    tokio::time::timeout(CONNECT_TIMEOUT, login(acc, password))
        .await
        .map_err(|_| CmdError::new("mail", format!("{} didn't answer in time", acc.host)))?
}

/// The cached session for `acc`, reconnecting if it's for another account or has gone stale.
async fn session<'a>(slot: &'a mut Option<(String, Imap)>, st: &App, acc: &Account) -> CmdResult<&'a mut Imap> {
    let key = acc.key();
    let alive = match slot {
        Some((k, s)) if *k == key => tokio::time::timeout(Duration::from_secs(10), s.noop()).await.map(|r| r.is_ok()).unwrap_or(false),
        _ => false,
    };
    if !alive {
        let password = secrets::get_named(&st.cfg_dir, &key).ok_or_else(|| CmdError::new("no_password", "No password saved for this email account on this device. Add it in Settings → Email."))?;
        *slot = Some((key, connect(acc, &password).await?));
    }
    let s = &mut slot.as_mut().unwrap().1;
    while s.unsolicited_responses.try_recv().is_ok() {}
    Ok(s)
}

/// Runs `$body` with `$s` bound to the session. On failure the connection is dropped so the
/// next call starts fresh.
macro_rules! with_session {
    ($mail:expr, $st:expr, $acc:expr, |$s:ident| $body:expr) => {{
        let mut guard = $mail.conn.lock().await;
        let r: CmdResult<_> = async {
            let $s = session(&mut *guard, &*$st, &$acc).await?;
            tokio::time::timeout(OP_TIMEOUT, async { $body }).await.map_err(|_| mail_err("The mail server took too long"))?
        }
        .await;
        if r.is_err() {
            *guard = None;
        }
        r
    }};
}

// ------------------------------------------------------------------ commands

/// Checks the settings by signing in, without saving anything.
#[tauri::command]
pub async fn mail_test(account: Account, password: String) -> CmdResult<usize> {
    let mut s = connect(&account, &password).await?;
    let n = s.list(Some(""), Some("*")).await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?.len();
    let _ = s.logout().await;
    Ok(n)
}

/// Saves (or with `None`, forgets) this device's password for the account.
#[tauri::command]
pub async fn mail_set_password(st: State<'_, App>, mail: State<'_, Mail>, account: Account, password: Option<String>) -> CmdResult<()> {
    match password.filter(|p| !p.is_empty()) {
        Some(p) => secrets::set_named(&st.cfg_dir, &account.key(), &p)?,
        None => secrets::delete_named(&st.cfg_dir, &account.key()),
    }
    mail.disconnect();
    Ok(())
}

#[tauri::command]
pub fn mail_has_password(st: State<'_, App>, account: Account) -> bool {
    secrets::get_named(&st.cfg_dir, &account.key()).is_some()
}

#[tauri::command]
pub async fn mail_folders(st: State<'_, App>, mail: State<'_, Mail>, account: Account) -> CmdResult<Vec<Folder>> {
    unlocked(&st)?;
    let names = with_session!(mail, st, account, |s| {
        let names = s.list(Some(""), Some("*")).await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?;
        Ok(names)
    })?;
    let mut out: Vec<Folder> = names
        .iter()
        .filter(|n| !n.attributes().iter().any(|a| matches!(a, NameAttribute::NoSelect)))
        .map(|n| {
            let special = if n.name().eq_ignore_ascii_case("INBOX") {
                Some("inbox")
            } else {
                n.attributes().iter().find_map(|a| match a {
                    NameAttribute::Sent => Some("sent"),
                    NameAttribute::Drafts => Some("drafts"),
                    NameAttribute::Archive => Some("archive"),
                    NameAttribute::Junk => Some("junk"),
                    NameAttribute::Trash => Some("trash"),
                    NameAttribute::All => Some("all"),
                    NameAttribute::Flagged => Some("flagged"),
                    _ => None,
                })
            };
            let mut label = decode_utf7(n.name());
            for prefix in ["[Gmail]", "[Google Mail]", "INBOX"] {
                if let Some(d) = n.delimiter() {
                    if let Some(rest) = label.strip_prefix(&format!("{prefix}{d}")) {
                        label = rest.to_string();
                    }
                }
            }
            if let Some(d) = n.delimiter() {
                label = label.replace(d, " / ");
            }
            if special == Some("inbox") {
                label = "Inbox".into();
            }
            Folder { name: n.name().to_string(), label, special }
        })
        .collect();
    let rank = |f: &Folder| match f.special {
        Some("inbox") => 0,
        Some("flagged") => 1,
        Some("drafts") => 2,
        Some("sent") => 3,
        Some("archive") => 4,
        Some("all") => 5,
        Some("junk") => 6,
        Some("trash") => 7,
        _ => 8,
    };
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase())));
    Ok(out)
}

/// The newest messages in `folder`, a page at a time; with `query`, the ones matching it.
/// `cursor` comes from the previous page's `next`.
#[tauri::command]
pub async fn mail_list(st: State<'_, App>, mail: State<'_, Mail>, account: Account, folder: String, query: Option<String>, cursor: Option<u32>) -> CmdResult<Page> {
    unlocked(&st)?;
    let query = query.map(|q| q.trim().to_string()).filter(|q| !q.is_empty());
    with_session!(mail, st, account, |s| {
        let mb = s.select(&folder).await.map_err(mail_err)?;
        const ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT DATE CONTENT-TYPE)])";
        let (fetches, total, next) = if let Some(q) = &query {
            // UIDs grow over time, so the newest matches are the largest.
            let quoted = format!("\"{}\"", q.replace('\\', "\\\\").replace('"', "\\\""));
            let cmd = if q.is_ascii() { format!("TEXT {quoted}") } else { format!("CHARSET UTF-8 TEXT {quoted}") };
            let mut uids: Vec<u32> = s.uid_search(cmd).await.map_err(mail_err)?.into_iter().collect();
            uids.sort_unstable_by(|a, b| b.cmp(a));
            let total = uids.len() as u32;
            let page: Vec<u32> = uids.into_iter().filter(|u| cursor.is_none_or(|c| *u < c)).take(PAGE as usize).collect();
            let next = if page.len() == PAGE as usize { page.last().copied() } else { None };
            let fetches = if page.is_empty() {
                Vec::new()
            } else {
                let set = page.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
                s.uid_fetch(set, ITEMS).await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?
            };
            (fetches, total, next)
        } else {
            // Browse by sequence number: the cursor is the lowest one already shown.
            let end = cursor.map(|c| c.saturating_sub(1)).unwrap_or(mb.exists).min(mb.exists);
            if end == 0 {
                (Vec::new(), mb.exists, None)
            } else {
                let start = end.saturating_sub(PAGE - 1).max(1);
                let fetches = s.fetch(format!("{start}:{end}"), ITEMS).await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?;
                (fetches, mb.exists, (start > 1).then_some(start))
            }
        };
        let mut messages: Vec<Summary> = fetches.iter().filter_map(summarize).collect();
        messages.sort_by(|a, b| b.uid.cmp(&a.uid));
        Ok(Page { total, messages, next })
    })
}

/// Opens one message. Fetching it marks it as read on the server.
#[tauri::command]
pub async fn mail_read(st: State<'_, App>, mail: State<'_, Mail>, account: Account, folder: String, uid: u32) -> CmdResult<FullMessage> {
    unlocked(&st)?;
    let (raw, flagged) = with_session!(mail, st, account, |s| {
        s.select(&folder).await.map_err(mail_err)?;
        let f = s.uid_fetch(uid.to_string(), "(UID FLAGS BODY[])").await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?;
        let f = f.into_iter().find(|f| f.uid == Some(uid)).ok_or_else(|| CmdError::new("mail", "That message is no longer on the server"))?;
        let flagged = f.flags().any(|x| matches!(x, Flag::Flagged));
        let seen = f.flags().any(|x| matches!(x, Flag::Seen));
        let raw = f.body().unwrap_or_default().to_vec();
        // BODY[] sets \Seen by itself; some servers only do so when asked.
        if !seen {
            s.uid_store(uid.to_string(), "+FLAGS.SILENT (\\Seen)").await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?;
        }
        Ok((raw, flagged))
    })?;
    let msg = MessageParser::default().parse(&raw).ok_or_else(|| mail_err("Couldn't read that message"))?;

    let mut html = msg.body_html(0).map(|h| h.into_owned());
    let mut attachments = Vec::new();
    for part in msg.attachments() {
        let mime = part.content_type().map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or(""))).unwrap_or_default();
        let bytes = part.contents();
        // Pictures shown inside the message (cid:) become data: URLs so they need no fetching.
        if let (Some(h), Some(cid)) = (html.as_mut(), part.content_id()) {
            let cid = cid.trim_matches(['<', '>']);
            let needle = format!("cid:{cid}");
            if mime.starts_with("image/") && bytes.len() <= INLINE_IMAGE_MAX && h.contains(&needle) {
                *h = h.replace(&needle, &format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
                continue;
            }
        }
        attachments.push(Attachment { name: part.attachment_name().unwrap_or("attachment").to_string(), mime, size: bytes.len() });
    }

    Ok(FullMessage {
        uid,
        subject: msg.subject().unwrap_or("(no subject)").to_string(),
        from: addresses(msg.from()),
        to: addresses(msg.to()),
        cc: addresses(msg.cc()),
        reply_to: addresses(msg.reply_to()),
        date: msg.date().map(|d| d.to_timestamp() * 1000).unwrap_or(0),
        text: msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default(),
        html,
        attachments,
        flagged,
    })
}

/// Marks a message read/unread or flagged/unflagged on the server.
#[tauri::command]
pub async fn mail_set_flags(st: State<'_, App>, mail: State<'_, Mail>, account: Account, folder: String, uid: u32, seen: Option<bool>, flagged: Option<bool>) -> CmdResult<()> {
    unlocked(&st)?;
    with_session!(mail, st, account, |s| {
        s.select(&folder).await.map_err(mail_err)?;
        for (on, flag) in [(seen, "\\Seen"), (flagged, "\\Flagged")] {
            if let Some(on) = on {
                let op = if on { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };
                s.uid_store(uid.to_string(), format!("{op} ({flag})")).await.map_err(mail_err)?.try_collect::<Vec<_>>().await.map_err(mail_err)?;
            }
        }
        Ok(())
    })
}

// ------------------------------------------------------------------ helpers

fn summarize(f: &Fetch) -> Option<Summary> {
    let uid = f.uid?;
    let header = f.header().unwrap_or_default();
    let msg = MessageParser::default().parse_headers(header);
    let from = msg.as_ref().and_then(|m| m.from()).and_then(|a| a.first());
    let header_date = msg.as_ref().and_then(|m| m.date()).map(|d| d.to_timestamp() * 1000);
    let multipart_mixed = msg
        .as_ref()
        .and_then(|m| m.content_type())
        .is_some_and(|c| c.ctype().eq_ignore_ascii_case("multipart") && c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("mixed")));
    Some(Summary {
        uid,
        from_name: from.and_then(|a| a.name.as_deref()).unwrap_or_default().to_string(),
        from_addr: from.and_then(|a| a.address.as_deref()).unwrap_or_default().to_string(),
        to: addresses(msg.as_ref().and_then(|m| m.to())),
        subject: msg.as_ref().and_then(|m| m.subject()).unwrap_or("(no subject)").to_string(),
        date: f.internal_date().map(|d| d.timestamp_millis()).or(header_date).unwrap_or(0),
        seen: f.flags().any(|x| matches!(x, Flag::Seen)),
        flagged: f.flags().any(|x| matches!(x, Flag::Flagged)),
        attachments: multipart_mixed,
        size: f.size.unwrap_or(0),
    })
}

fn addresses(a: Option<&Address>) -> String {
    let Some(a) = a else { return String::new() };
    a.iter()
        .map(|x| match (x.name.as_deref(), x.address.as_deref()) {
            (Some(n), Some(e)) if !n.is_empty() => format!("{n} <{e}>"),
            (_, Some(e)) => e.to_string(),
            (Some(n), None) => n.to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// IMAP folder names use "modified UTF-7" for anything outside ASCII (RFC 3501 §5.1.3).
fn decode_utf7(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let Some(j) = rest.find('-') else {
            out.push('&');
            break;
        };
        let enc = &rest[..j];
        rest = &rest[j + 1..];
        if enc.is_empty() {
            out.push('&');
            continue;
        }
        match base64::engine::general_purpose::STANDARD_NO_PAD.decode(enc.replace(',', "/")) {
            Ok(b) => out.push_str(&String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>())),
            Err(_) => {
                out.push('&');
                out.push_str(enc);
                out.push('-');
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{connect, decode_utf7, Account};

    /// Talks to real servers with a made-up account: TLS, STARTTLS and the greeting must work
    /// and the bad login must come back as a sign-in error. `cargo test -- --ignored live_`
    #[tokio::test]
    #[ignore]
    async fn live_login_is_rejected_cleanly() {
        for (host, port, security) in [("imap.gmail.com", 993, "ssl"), ("outlook.office365.com", 143, "starttls")] {
            let acc = Account { host: host.into(), port, security: security.into(), username: "rad-test-nobody@example.com".into() };
            let err = connect(&acc, "wrong").await.err().expect("login should fail");
            assert_eq!(err.code, "auth", "{host}: {}", err.message);
        }
    }

    #[test]
    fn utf7_folder_names() {
        assert_eq!(decode_utf7("INBOX"), "INBOX");
        assert_eq!(decode_utf7("Entw&APw-rfe"), "Entwürfe");
        assert_eq!(decode_utf7("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode_utf7("&ZeVnLIqe-"), "日本語");
    }
}
