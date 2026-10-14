//! Actions from chat: "open ssh to coursemat.au", "open servo.org", "open a terminal".
//!
//! A message that reads like such a request is turned into one [`AgentAction`] by the chat
//! model, which sees the user's long-term memories and the vault passages that match the
//! request — so connection details (user, port, key file) come from the user's own notes.
//! What it resolves is remembered, so the next request for the same host needs no search.

use std::sync::Arc;
use std::time::Duration;

use rad_api::{AgentAction, Citation, Memory, ModelRef, Settings};
use rad_llm::{ChatMsg, ChatRequest, ProviderConn};
use rad_vault::{repo, Vault};
use serde_json::Value;

use crate::rag::{self, strip_thinking, Passage};
use crate::state::{db_vault, Shared};

const VERBS: &[&str] = &[
    "open", "connect", "ssh", "log in", "login", "log into", "go to", "goto", "browse", "visit", "navigate", "launch", "start", "show me",
];

/// Cheap first check so ordinary questions never pay for a planning call.
pub fn looks_like_action(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.len() > 300 || t.contains('?') && !t.starts_with("can you") && !t.starts_with("could you") {
        return false;
    }
    let mut t = t.as_str();
    for p in ["please ", "can you ", "could you ", "would you ", "hey rad, ", "rad, ", "now "] {
        t = t.strip_prefix(p).unwrap_or(t);
    }
    VERBS.iter().any(|v| t.starts_with(v)) && (t.contains("ssh") || t.contains("terminal") || t.contains("shell") || t.contains("server") || has_host(t))
        || t.starts_with("ssh ")
}

fn has_host(t: &str) -> bool {
    t.split_whitespace().any(|w| host_ok(w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-')) && w.contains('.'))
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') && !h.starts_with('-')
}

fn user_ok(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && u.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && !u.starts_with('-')
}

pub struct Planned {
    pub action: AgentAction,
    pub reply: String,
    pub citations: Vec<Citation>,
    /// A memory to keep (new or replacing an older one about the same host).
    pub remember: Option<String>,
}

const SYSTEM: &str = "You turn the user's request into ONE action for the RAD desktop app.\n\
Actions:\n\
- ssh: open an SSH session in RAD's terminal. Fields: host (required), user, port, identity_file.\n\
- open_url: open a web page in RAD's browser. Field: url.\n\
- terminal: open a local terminal on this computer.\n\
- none: the message is not a request to do one of these.\n\
Fill connection details (user name, port, key file, the real host name behind a nickname) from the MEMORIES and the numbered SOURCES — the user's own notes. \
Never invent details that are not in the memories, sources or request; leave them out instead. Never output passwords. \
Reply with ONLY a JSON object, for example {\"kind\":\"ssh\",\"host\":\"example.org\",\"user\":\"deploy\",\"port\":2222,\"identity_file\":\"~/.ssh/id_ed25519\",\"sources\":[1]}.";

/// Decides what the message asks for. `None`: not an action — answer it as a normal chat message.
pub async fn plan(
    state: &Shared,
    vault: &Arc<Vault>,
    settings: &Settings,
    memories: &[Memory],
    model: &ModelRef,
    conn: &ProviderConn,
    request: &str,
) -> Option<Planned> {
    let passages = rag::retrieve(state, vault, settings, request, Vec::new(), 6, 6_000, None).await.unwrap_or_default();
    let mems: Vec<&str> = memories.iter().filter(|m| m.enabled).map(|m| m.text.as_str()).collect();
    let prompt = format!(
        "MEMORIES:\n{}\n\n{}REQUEST: {request}\n\nJSON:",
        if mems.is_empty() { "(none)".to_string() } else { mems.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n") },
        rag::sources_block(&passages),
    );
    let req = ChatRequest { system: Some(SYSTEM.into()), messages: vec![ChatMsg::new("user", prompt)], temperature: Some(0.0), max_tokens: 300 };
    let raw = match tokio::time::timeout(Duration::from_secs(60), state.llm.complete(conn, &model.model, &req)).await {
        Ok(Ok(r)) => Some(r),
        Ok(Err(e)) => {
            tracing::warn!("agent planning failed: {e}");
            None
        }
        Err(_) => None,
    };
    let v = raw.and_then(|r| parse_json(&strip_thinking(&r)));
    let (action, used) = match v {
        Some(v) => (to_action(&v)?, v["sources"].as_array().map(|a| a.iter().filter_map(|n| n.as_u64()).collect::<Vec<_>>()).unwrap_or_default()),
        // No model to ask: still handle the plain "ssh host" form.
        None => (fallback(request)?, Vec::new()),
    };
    let citations = cited(&passages, &used);
    Some(Planned { reply: reply(&action, &citations), remember: memory_for(&action), action, citations })
}

fn parse_json(s: &str) -> Option<Value> {
    let (a, b) = (s.find('{')?, s.rfind('}')?);
    serde_json::from_str(s.get(a..=b)?).ok()
}

fn to_action(v: &Value) -> Option<AgentAction> {
    let s = |k: &str| v[k].as_str().map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    match v["kind"].as_str()? {
        "ssh" => {
            let mut host = s("host")?;
            let mut user = s("user").filter(|u| user_ok(u));
            if let Some((u, h)) = host.clone().split_once('@') {
                user = user.or(Some(u.to_string()).filter(|u| user_ok(u)));
                host = h.to_string();
            }
            if !host_ok(&host) {
                return None;
            }
            let port = v["port"].as_u64().or_else(|| v["port"].as_str().and_then(|p| p.parse().ok())).filter(|p| (1..=65535).contains(p)).map(|p| p as u16);
            let identity_file = s("identity_file").filter(|f| (f.starts_with('~') || f.starts_with('/') || f.contains(":\\")) && !f.contains(['\n', '\r', '\0']));
            Some(AgentAction { kind: "ssh".into(), host: Some(host), user, port, identity_file, url: None })
        }
        "open_url" => {
            let u = s("url")?;
            let u = if u.starts_with("http://") || u.starts_with("https://") { u } else { format!("https://{u}") };
            url_ok(&u).then(|| AgentAction { kind: "open_url".into(), url: Some(u), ..Default::default() })
        }
        "terminal" => Some(AgentAction { kind: "terminal".into(), ..Default::default() }),
        _ => None,
    }
}

fn url_ok(u: &str) -> bool {
    let rest = u.split_once("://").map(|x| x.1).unwrap_or("");
    let host = rest.split(['/', '?', '#', ':']).next().unwrap_or("");
    host_ok(host) && host.contains('.') || host == "localhost"
}

/// "ssh deploy@example.org -p 2222" and friends, without a model.
fn fallback(request: &str) -> Option<AgentAction> {
    let words: Vec<&str> = request.split_whitespace().collect();
    let lower = request.to_lowercase();
    if lower.contains("ssh") {
        let target = words.iter().map(|w| w.trim_matches(|c: char| ",.;:!\"'".contains(c))).find(|w| w.contains('.') && (host_ok(w) || w.contains('@')))?;
        let (user, host) = match target.split_once('@') {
            Some((u, h)) => (Some(u.to_string()).filter(|u| user_ok(u)), h.to_string()),
            None => (None, target.to_string()),
        };
        let port = words.windows(2).find(|w| w[0] == "-p" || w[0] == "port").and_then(|w| w[1].parse().ok());
        return host_ok(&host).then(|| AgentAction { kind: "ssh".into(), host: Some(host), user, port, ..Default::default() });
    }
    if lower.contains("terminal") || lower.contains("shell") {
        return Some(AgentAction { kind: "terminal".into(), ..Default::default() });
    }
    let target = words.iter().map(|w| w.trim_matches(|c: char| ",;!\"'".contains(c))).find(|w| w.contains('.') && w.len() > 3)?;
    let u = if target.starts_with("http") { target.to_string() } else { format!("https://{}", target.trim_end_matches('.')) };
    url_ok(&u).then(|| AgentAction { kind: "open_url".into(), url: Some(u), ..Default::default() })
}

fn cited(passages: &[Passage], used: &[u64]) -> Vec<Citation> {
    passages.iter().filter(|p| used.contains(&(p.citation.n as u64))).map(|p| p.citation.clone()).collect()
}

pub fn ssh_target(a: &AgentAction) -> String {
    let host = a.host.as_deref().unwrap_or("");
    match &a.user {
        Some(u) => format!("{u}@{host}"),
        None => host.to_string(),
    }
}

fn reply(a: &AgentAction, cites: &[Citation]) -> String {
    let from = if cites.is_empty() { String::new() } else { format!(" Connection details from {}.", cites.iter().map(|c| format!("[{}]", c.n)).collect::<Vec<_>>().join(", ")) };
    match a.kind.as_str() {
        "ssh" => {
            let mut s = format!("Opening an SSH session to `{}`", ssh_target(a));
            if let Some(p) = a.port {
                s.push_str(&format!(" on port {p}"));
            }
            if let Some(k) = &a.identity_file {
                s.push_str(&format!(" with key `{k}`"));
            }
            s.push_str(" in **Terminal**.");
            s.push_str(&from);
            if a.user.is_none() {
                s.push_str(" I didn't find a user name for this host in your vault, so SSH uses your default; tell me “remember that …” to save one.");
            }
            s
        }
        "open_url" => format!("Opening {} in the **Web Browser**.", a.url.as_deref().unwrap_or("")),
        _ => "Opening a terminal on this computer.".into(),
    }
}

/// What to remember about an SSH host, once something beyond its name is known.
fn memory_for(a: &AgentAction) -> Option<String> {
    if a.kind != "ssh" || (a.user.is_none() && a.port.is_none() && a.identity_file.is_none()) {
        return None;
    }
    let mut s = format!("SSH to {}: connect as {}", a.host.as_deref()?, ssh_target(a));
    if let Some(p) = a.port {
        s.push_str(&format!(", port {p}"));
    }
    if let Some(k) = &a.identity_file {
        s.push_str(&format!(", key {k}"));
    }
    Some(s)
}

/// Saves `text`, replacing an earlier memory about the same host. Returns whether it changed.
pub async fn remember(state: &Shared, vault: &Arc<Vault>, host: String, text: String) -> bool {
    db_vault(vault.clone(), state, move |st, c| {
        let prefix = format!("SSH to {host}:");
        let old: Vec<Memory> = repo::memories(c)?.into_iter().filter(|m| m.text.starts_with(&prefix)).collect();
        if old.iter().any(|m| m.text == text) {
            return Ok(false);
        }
        for m in &old {
            repo::delete_memory(c, m.id)?;
        }
        let m = repo::create_memory(c, &text)?;
        st.changed(c, "memory", m.id, "create");
        Ok(true)
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_actions_but_not_questions() {
        for t in ["open ssh to coursemat.au", "Please connect to the coursemat server", "ssh deploy@example.org", "open servo.org", "open a terminal"] {
            assert!(looks_like_action(t), "{t}");
        }
        for t in ["what is the coursemat ip address?", "summarise my notes about ssh keys", "When should I plant the tomatoes?"] {
            assert!(!looks_like_action(t), "{t}");
        }
    }

    #[test]
    fn validates_model_output() {
        let a = to_action(&serde_json::json!({"kind":"ssh","host":"deploy@coursemat.au","port":"2222","identity_file":"~/.ssh/cm"})).unwrap();
        assert_eq!((a.user.as_deref(), a.host.as_deref(), a.port), (Some("deploy"), Some("coursemat.au"), Some(2222)));
        assert!(to_action(&serde_json::json!({"kind":"ssh","host":"evil.com; rm -rf ~"})).is_none());
        assert!(to_action(&serde_json::json!({"kind":"ssh","host":"-oProxyCommand=x"})).is_none());
        let a = to_action(&serde_json::json!({"kind":"ssh","host":"h.example","user":"$(id)"})).unwrap();
        assert_eq!(a.user, None);
        assert_eq!(to_action(&serde_json::json!({"kind":"open_url","url":"servo.org"})).unwrap().url.as_deref(), Some("https://servo.org"));
        assert!(to_action(&serde_json::json!({"kind":"none"})).is_none());
    }

    #[test]
    fn falls_back_without_a_model() {
        let a = fallback("ssh root@10.0.0.5 -p 2200").unwrap();
        assert_eq!((a.user.as_deref(), a.host.as_deref(), a.port), (Some("root"), Some("10.0.0.5"), Some(2200)));
        assert_eq!(fallback("open servo.org").unwrap().url.as_deref(), Some("https://servo.org"));
        assert_eq!(memory_for(&a).unwrap(), "SSH to 10.0.0.5: connect as root@10.0.0.5, port 2200");
    }
}
